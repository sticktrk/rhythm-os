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
            'apply_motion_target',
            'explain_room_output',
            'try_room_offset',
            'plan_absorb_offset'
          ])
            {
              'id': op,
              'tier': switch (op) {
                'apply_motion_target' => 'persistent_change',
                'try_room_offset' => 'temporary_light_output',
                _ => 'read',
              },
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
      'explain_room_output' => {
          'mode': 'day',
          'state': 'active',
          'profile_id': 'day',
          'canonical_brightness': 50,
          'canonical_kelvin': 3000,
        },
      'try_room_offset' => {'trial_id': 'trial-hall'},
      'plan_absorb_offset' => {
          'operation': 'apply_absorb_offset',
          'room_id': args['room_id'],
          'server_instance_id': 'hub1',
          'plan_id': 'review-offset',
          'profile_id': 'day',
          'affected_rooms': [],
          'cleared_offsets': [],
          'samples': [],
        },
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
      'Save this look uses the trial that ran after cancelling another preview',
      (tester) async {
    final hub = _Hub();
    await tester.pumpWidget(MaterialApp(
        home: AssistantChatScreen(
            api: () => hub,
            serverId: 'hub1',
            isCurrentTarget: () => true,
            targetChanges: ValueNotifier(true),
            openSetup: (_) async {})));
    await tester.pumpAndSettle();

    Future<void> send(String message) async {
      await tester.enterText(find.byType(TextField), message);
      await tester.testTextInput.receiveAction(TextInputAction.send);
      await tester.pumpAndSettle();
    }

    await send('Downstairs hall 1 hour earlier');
    await send('Try 60 minutes earlier');
    await send('Downstairs hall 30 minutes earlier');
    await send('Cancel');
    await send('Save this look');
    expect(hub.calls.where((c) => c.$1 == 'try_room_offset').single.$2,
        {'room_id': 'hall', 'offset_minutes': -60});
    expect(hub.calls.last.$1, 'plan_absorb_offset');
    expect(hub.calls.last.$2, {'room_id': 'hall', 'offset_minutes': -60});
  });

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
