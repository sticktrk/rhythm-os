import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/hubs/assistant_chat_screen.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class _Hub extends RhythmServerApi {
  _Hub() : super(Dio());
  final calls = <(String, Map<String, dynamic>)>[];
  bool linked = false, missing = false;
  @override
  Future<RhythmAssistantContract?> getAssistantContract() async =>
      RhythmAssistantContract.fromJson({
        'operations': [
          for (final op in [
            'start_motion_walk',
            'inspect_motion_walk',
            'plan_motion_target',
            'apply_motion_target'
          ])
            {
              'id': op,
              'tier':
                  op == 'apply_motion_target' ? 'persistent_change' : 'read',
              'invocation_path': '/api/assistant/operations/$op'
            }
        ]
      });
  @override
  Future<RhythmAssistantTopologySnapshot>
      getAssistantTopologySnapshot() async =>
          RhythmAssistantTopologySnapshot.fromJson({
            'server_instance_id': 'hub1',
            'nodes': [
              {'id': 'hall', 'name': 'Downstairs hall', 'kind': 'room'},
              {'id': 'entry', 'name': 'Entry', 'kind': 'room'},
              if (!missing)
                {'id': 'pir', 'name': 'Landing PIR', 'kind': 'motion_sensor'},
              {'id': 'bulb', 'name': 'Hall lamp', 'kind': 'light_device'},
            ]
          });
  @override
  Future<Map<String, dynamic>> callAssistantOperation(
      RhythmAssistantContract contract,
      String id,
      Map<String, dynamic> args) async {
    calls.add((id, args));
    return switch (id) {
      'start_motion_walk' when args['sensor_id'] == '' => {
          'status': 'handoff',
          'server_instance_id': 'hub1',
          'handoff': {
            'actor': 'resident',
            'surface': 'rhythm_app',
            'instruction': 'Open Add Device.'
          }
        },
      'start_motion_walk' => {
          'server_instance_id': 'hub1',
          'walk_id': 'w${calls.length}',
          'window_seconds': 60
        },
      'inspect_motion_walk' => {
          'status': 'walk_active',
          'stage': linked ? 'command_acknowledged' : 'detected',
          'reason': linked ? 'check_each_light' : 'missing_target',
          'next_check': 'Inspect the intended room.',
          'lights': [
            {
              'device_id': 'bulb',
              'readback': 'no_fresh_readback',
              'availability': 'unavailable',
              'power_on': null
            }
          ]
        },
      'plan_motion_target' => {
          'status': 'ready',
          'server_instance_id': 'hub1',
          'plan_id': 'review1',
          'room_id': 'hall',
          'sensor_id': 'pir',
          'physical_room_id': 'entry',
          'before_targets': ['entry'],
          'after_targets': ['entry', 'hall']
        },
      'apply_motion_target' => {
          'server_instance_id': 'hub1',
          'canonical_readback_verified': linked = true
        },
      _ => throw UnsupportedError(id),
    };
  }
}

void main() {
  testWidgets(
      'resident resumes after setup on the same hub, reviews, confirms and retests',
      (tester) async {
    final hub = _Hub()..missing = true;
    final active = ValueNotifier(true);
    await tester.pumpWidget(MaterialApp(
        home: AssistantChatScreen(
            api: () => hub,
            serverId: 'hub1',
            isCurrentTarget: () => active.value,
            targetChanges: active,
            openSetup: (_) async {
              hub.missing = false;
            })));
    await tester.pumpAndSettle();
    Future<void> choose(String label) async {
      await tester.tap(find.widgetWithText(ActionChip, label));
      await tester.pumpAndSettle();
    }

    await tester.enterText(find.byType(TextField), 'Hook up this sensor');
    await tester.testTextInput.receiveAction(TextInputAction.send);
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'the downstairs hall');
    await tester.testTextInput.receiveAction(TextInputAction.send);
    await tester.pumpAndSettle();
    await choose('My sensor is missing');
    expect(find.textContaining('Open Add Device.'), findsOneWidget);
    await choose('Open device setup');
    await choose('Landing PIR');
    await choose('Check walk');
    expect(find.textContaining('missing target'), findsOneWidget);
    await choose('Review room link');
    expect(find.textContaining('Physical room: Entry'), findsOneWidget);
    expect(hub.linked, false);
    active.value = false;
    await tester.pumpAndSettle();
    expect(find.widgetWithText(ActionChip, 'Confirm room link'), findsNothing);
    active.value = true;
    await tester.pumpAndSettle();
    await choose('Confirm room link');
    expect(
        hub.calls.where((call) => call.$1 == 'apply_motion_target').single.$2, {
      'plan_id': 'review1',
      'server_instance_id': 'hub1',
      'confirmed': true
    });
    expect(hub.calls.last.$1, 'start_motion_walk');
    await choose('Check walk');
    expect(find.textContaining('command acknowledged'), findsOneWidget);
    expect(
        find.textContaining(
            'Hall lamp · unavailable · no fresh device readback'),
        findsWidgets);
  });
}
