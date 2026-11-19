import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/hubs/assistant_chat_screen.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class _Hub extends RhythmServerApi {
  _Hub() : super(Dio());
  final calls = <(String, Map<String, dynamic>)>[];
  bool linked = false, missing = false, legacy = false;
  bool? sawPower;
  Map<String, dynamic> current = {
    'room_id': 'hall',
    'state': 'active',
    'scheduled_default_state': 'active',
    'mode': 'day',
    'profile_id': 'default',
    'canonical_brightness': 36,
    'canonical_kelvin': 2700,
    'time_offset_minutes': 0,
    'brightness_offset': 0,
    'adaptation_enabled': true
  };
  @override
  Future<RhythmAssistantContract?> getAssistantContract() async =>
      RhythmAssistantContract.fromJson({
        'operations': [
          for (final op in legacy
              ? ['get_topology_snapshot']
              : [
                  'explain_room_output',
                  'adjust_room_brightness',
                  'adjust_room_warmth',
                  'return_room_to_schedule',
                  'plan_absorb_offset',
                  'try_room_offset',
                  'undo_room_trial',
                  'identify_device',
                  'start_motion_walk',
                  'inspect_motion_walk',
                  'plan_motion_target',
                  'apply_motion_target'
                ])
            {
              'id': op,
              'tier': op == 'apply_motion_target'
                  ? 'persistent_change'
                  : {
                      'adjust_room_brightness',
                      'adjust_room_warmth',
                      'return_room_to_schedule',
                      'try_room_offset',
                      'undo_room_trial',
                      'identify_device'
                    }.contains(op)
                      ? 'temporary_light_output'
                      : 'read',
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
    if (id == 'inspect_motion_walk' &&
        args.containsKey('resident_observations')) {
      sawPower = args['resident_observations'][0]['power_on'] as bool;
    }
    return switch (id) {
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
      'explain_room_output' => current,
      'adjust_room_brightness' ||
      'adjust_room_warmth' ||
      'return_room_to_schedule' ||
      'try_room_offset' ||
      'undo_room_trial' =>
        {
          ...current,
          if (id == 'try_room_offset') 'trial_id': 'trial',
          'server_acknowledged': true,
          'light_readback': [
            {
              'light_id': 'bulb',
              'availability': 'available',
              'verification': 'fresh_device_readback',
              'observed': {'on': true, 'brightness': 41, 'kelvin': 2700}
            }
          ]
        },
      'identify_device' => {
          'server_acknowledged': true,
          'physical_verification': 'ask_resident'
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
              'power_on': null,
              'resident_power_on': sawPower
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
    await choose('I saw Hall lamp off');
    expect(hub.sawPower, false);
    expect(find.textContaining('resident saw it off'), findsOneWidget);
    await choose('Identify Hall lamp');
    expect(hub.calls[hub.calls.length - 2].$1, 'identify_device');
    expect(hub.calls[hub.calls.length - 2].$2, {'device_id': 'bulb'});
    expect(hub.calls.last.$1, 'start_motion_walk');
  });

  testWidgets(
      'guided lighting reads the room, limits the adjustment and restores the right behavior',
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
    Future<void> request(String value) async {
      await tester.enterText(find.byType(TextField), value);
      await tester.testTextInput.receiveAction(TextInputAction.send);
      await tester.pumpAndSettle();
    }

    Future<void> choose(String label) async {
      final chip = find.widgetWithText(ActionChip, label);
      await tester.ensureVisible(chip);
      await tester.tap(chip);
      await tester.pumpAndSettle();
    }

    await request('A little brighter in the downstairs hall');
    expect(hub.calls.last.$1, 'explain_room_output');
    await choose('Adjust brightness by +5 points');
    expect(hub.calls.last.$1, 'adjust_room_brightness');
    expect(hub.calls.last.$2, {'room_id': 'hall', 'delta_percent': 5});
    expect(find.textContaining('41% brightness'), findsOneWidget);
    await request('Warmer, but keep this brightness');
    await choose('Try 2500 K at this brightness');
    expect(hub.calls.last.$1, 'adjust_room_warmth');
    expect(hub.calls.last.$2, {'room_id': 'hall', 'kelvin': 2500});
    await request('Show me the downstairs hall an hour earlier');
    await choose('Try 60 minutes earlier');
    await request('Go back to normal');
    await choose('End this trial');
    expect(hub.calls.last.$1, 'undo_room_trial');
    expect(hub.calls.last.$2, {'room_id': 'hall', 'trial_id': 'trial'});
    hub.current['scheduled_default_state'] = 'hard_off';
    await request('Go back to normal');
    expect(find.textContaining('turns the room off'), findsOneWidget);
    await choose('Restore scheduled default');
    expect(hub.calls.last.$1, 'return_room_to_schedule');
    expect(hub.calls.last.$2,
        {'room_id': 'hall', 'behavior': 'scheduled_default'});
  });

  testWidgets(
      'previous hub explains unavailable diagnostics before offering a workflow',
      (tester) async {
    final hub = _Hub()..legacy = true;
    await tester.pumpWidget(MaterialApp(
        home: AssistantChatScreen(
            api: () => hub,
            serverId: 'hub1',
            isCurrentTarget: () => true,
            targetChanges: ValueNotifier(true),
            openSetup: (_) async {})));
    await tester.pumpAndSettle();
    expect(
        find.textContaining(
            'does not advertise guided lighting or motion tests'),
        findsOneWidget);
    expect(hub.calls, isEmpty);
  });
}
