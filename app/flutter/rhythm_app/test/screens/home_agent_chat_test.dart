import 'dart:typed_data';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart' show PlatformException;
import 'package:flutter_test/flutter_test.dart';
import 'package:image/image.dart' as image;
import 'package:image_picker/image_picker.dart';
import 'package:rhythm_app/screens/hubs/home_agent_chat.dart';
import 'package:rhythm_app/screens/hubs/lighting_chat_screen.dart';
import 'package:rhythm_app/services/customer_assistant_service.dart';
import 'package:rhythm_app/services/home_agent_catalog.dart';
import 'package:rhythm_app/services/home_agent_client.dart';
import 'package:rhythm_app/services/lighting_chat_service.dart';
import 'package:rhythm_app/widgets/assistant_chat_widgets.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class TestAgent extends HomeAgentClient {
  TestAgent()
      : super(Uri.https('fixture.invalid'), 'home', 'fixture', () => 'fixture',
            () => true) {
    session = 'session';
  }
  final calls = <(String, CustomerAction, Uint8List?)>[];
  CustomerAction? plan;
  String answer =
      'The two lamps have different shades. A photo of both could help.';
  @override
  Future<CustomerAction> call(String method,
      [CustomerAction body = const {}, Uint8List? photo]) async {
    calls.add((method, body, photo));
    if (method == 'turn' || method == 'photo') {
      return {'text': answer, 'plan': plan};
    }
    if (method == 'claim') {
      plan!['steps'][body['index']]['status'] = 'claimed';
      return {'claim': 'once', 'plan': plan};
    }
    if (method == 'receipt') {
      plan!['steps'][body['index']]['status'] = body['outcome'];
    }
    return {'plan': plan};
  }
}

void main() {
  const rooms = [
    RhythmTopologyNode(id: 'a', name: 'Kitchen', kind: RhythmNodeKind.room),
    RhythmTopologyNode(id: 'b', name: 'Dining', kind: RhythmNodeKind.room),
    RhythmTopologyNode(id: 'c', name: 'Hall', kind: RhythmNodeKind.room)
  ];
  late TestAgent client;
  late LightingChatHub hub;
  late List<(String, CustomerAction)> calls;
  late Uint8List pixels;
  setUp(() {
    client = TestAgent();
    calls = [];
    pixels = image.encodeJpg(image.Image(width: 8, height: 8));
    hub = LightingChatHub(rooms, {'adjust_room_brightness'}, (op, args) async {
      calls.add((op, args));
      return {
        'room_id': args['room_id'],
        'server_acknowledged': true,
        'canonical_brightness': 42,
        'light_readback': [
          {
            'light_id': args['room_id'],
            'control_scope': 'managed',
            'verification': 'fresh_device_readback',
            'observed': {'power': true}
          }
        ]
      };
    }, () {});
  });
  Future<void> open(WidgetTester t,
      {Future<Uint8List?> Function(ImageSource)? picker}) async {
    await t.pumpWidget(MaterialApp(
        home: HomeAgentChat(
            client: client,
            hub: hub,
            current: () => true,
            guided: () {},
            pickPhoto: picker ?? ((_) async => pixels))));
    await t.pumpAndSettle();
  }

  Future<void> send(WidgetTester t, String text) async {
    if (text.isNotEmpty) await t.enterText(find.byType(TextField), text);
    await t.pump();
    await t.tap(find.byTooltip('Send message'));
    await t.pumpAndSettle();
  }

  Future<void> pick(WidgetTester t, String label) async {
    await t.tap(find.byTooltip('Add a photo'));
    await t.pumpAndSettle();
    await t.tap(find.text(label));
    await t.pumpAndSettle();
  }

  testWidgets(
      'the app opens the home agent and sends the first turn through it',
      (t) async {
    final changes = ValueNotifier<bool>(true);
    addTearDown(changes.dispose);
    await t.pumpWidget(MaterialApp(
        home: LightingChatScreen(
      openHub: ({bool reconnect = false}) async => hub,
      openAgent: () async => client,
      account: CustomerAssistantService(
          (_) async => throw StateError('Unexpected legacy request'),
          () => true),
      current: () => true,
      changes: changes,
      local: true,
      guided: () {},
    )));
    await t.pumpAndSettle();
    expect(find.byTooltip('Add a photo'), findsOneWidget);
    await send(t, 'Why are my lights different?');
    expect(client.calls.where((call) => call.$1 == 'turn'), hasLength(1));
  });
  testWidgets(
      'a starter prompt opens the conversation and the plan arrives as one review card',
      (t) async {
    await open(t);
    expect(find.text('How should the light feel?'), findsOneWidget);
    client.plan = {
      'id': '1b9af27c-a109-4e92-94ed-bd6a7c6cc6b5',
      'expires':
          DateTime.now().add(const Duration(minutes: 5)).millisecondsSinceEpoch,
      'steps': [
        {
          'target': client.reference('r', 'a'),
          'operation': 'adjust_room_brightness',
          'arguments': {'delta_percent': -10},
          'status': 'review'
        }
      ]
    };
    await t.tap(find.text('A little brighter'));
    await t.pumpAndSettle();
    final turn = client.calls.singleWhere((c) => c.$1 == 'turn').$2;
    expect(turn['messages'].last['text'], 'Make the lights a little brighter.');
    expect(find.text('How should the light feel?'), findsNothing);
    final card = find.byType(AssistantReviewCard);
    expect(card, findsOneWidget);
    for (final text in [
      'Kitchen',
      '-10 brightness points',
      'Awaiting review',
      'Apply reviewed steps'
    ]) {
      expect(find.descendant(of: card, matching: find.text(text)),
          findsOneWidget);
    }
    expect(find.byType(ListTile), findsNothing);
  });
  testWidgets(
      'voluntary still can be discarded or sent alone; camera denial and skipping a suggestion keep chat usable',
      (t) async {
    await open(t,
        picker: (src) async => src == ImageSource.camera
            ? throw PlatformException(code: 'camera_access_denied')
            : pixels);
    await pick(t, 'Take photo');
    expect(find.byTooltip('Remove photo'), findsNothing);
    await send(t, 'Why do these lamps look different?');
    await send(t, 'I would rather describe them.');
    await pick(t, 'Choose photo');
    await t.tap(find.byTooltip('Remove photo'));
    await t.pumpAndSettle();
    expect(client.calls.where((c) => c.$1 == 'photo'), isEmpty);
    await pick(t, 'Choose photo');
    await send(t, '');
    final photo = client.calls.singleWhere((c) => c.$1 == 'photo');
    expect(photo.$3, pixels);
    expect(photo.$2['messages'].last['text'], '');
    expect(photo.$2['messages'].first['text'],
        'Why do these lamps look different?');
    expect(calls, isEmpty);
  });
  testWidgets(
      'two rooms are redacted and reviewed before one write per included room',
      (t) async {
    await open(t);
    client.plan = {
      'id': '1b9af27c-a109-4e92-94ed-bd6a7c6cc6b5',
      'expires':
          DateTime.now().add(const Duration(minutes: 5)).millisecondsSinceEpoch,
      'steps': [
        for (final id in ['a', 'b'])
          {
            'target': client.reference('r', id),
            'operation': 'adjust_room_brightness',
            'arguments': {'delta_percent': -10},
            'status': 'review'
          }
      ]
    };
    await send(t, 'Dim Kitchen and Dining except Hall');
    final turn = client.calls.singleWhere((c) => c.$1 == 'turn').$2;
    expect(turn['messages'].toString(), isNot(contains('Kitchen')));
    expect(
        (turn['targets'] as List).singleWhere(
            (v) => v['ref'] == client.reference('r', 'c'))['tools'],
        isEmpty);
    expect(calls.every((c) => c.$1 == 'explain_room_output'), isTrue);
    final button = find.text('Apply reviewed steps');
    await t.ensureVisible(button);
    await t.tap(button);
    await t.pumpAndSettle();
    expect(
        calls
            .where((c) => c.$1 == 'adjust_room_brightness')
            .map((c) => c.$2['room_id']),
        ['a', 'b']);
    expect(
        client.calls
            .where((c) => c.$1 == 'receipt')
            .map((c) => c.$2['outcome']),
        ['observed', 'observed']);
  });
  test(
      'duplicate names require a selected target, exclusions and schedule previews stay scoped',
      () async {
    final dup = LightingChatHub([
      rooms.first,
      const RhythmTopologyNode(
          id: 'd', name: 'Kitchen', kind: RhythmNodeKind.room),
      ...rooms.skip(1)
    ], hub.tools, hub.call, () {});
    final catalog = await HomeAgentCatalog.load(dup, client);
    expect(catalog.redact('Dim Kitchen'), contains('[ambiguous target]'));
    expect(catalog.redact('Dim Kitchen', client.reference('r', 'd')),
        contains(client.reference('r', 'd')));
    final context = await catalog.context(
        catalog.redact('All rooms except Hall and Dining'), null);
    expect(context.where((r) => (r['tools'] as List).isNotEmpty).length, 2);
    final def = {
      'transitions': [
        {
          'trigger': {'kind': 'scheduled', 'time': '00:10'}
        },
        {
          'trigger': {'kind': 'solar', 'offset_minutes': 5}
        },
        {
          'trigger_enabled': false,
          'trigger': {'kind': 'scheduled', 'time': '08:00'}
        }
      ]
    };
    final changed = shiftSchedule(def, -20)['transitions'];
    expect(changed[0]['trigger']['time'], '23:50');
    expect(changed[1]['trigger']['offset_minutes'], -15);
    expect(changed[2]['trigger']['time'], '08:00');
    expect(def.toString(), contains('00:10'));
  });
  test(
      'saved scene, schedule and motion repairs dispatch only the reviewed hub plan',
      () async {
    const sensor = RhythmTopologyNode(
        id: 'door-sensor',
        name: 'Door sensor',
        kind: RhythmNodeKind.motionSensor);
    final ops = {
      'read_automation_settings',
      'plan_home_scene_edit',
      'apply_home_scene_edit',
      'plan_light_schedule_edit',
      'apply_light_schedule_edit',
      'start_motion_walk',
      'plan_motion_target',
      'apply_motion_target'
    };
    final seen = <(String, CustomerAction)>[];
    final live = LightingChatHub(rooms, hub.tools, (op, args) async {
      seen.add((op, args));
      if (op == 'read_automation_settings') {
        return {
          'scenes': [
            {'id': 'evening', 'name': 'Evening'}
          ],
          'schedules': [
            {
              'id': 'work',
              'name': 'Work',
              'transitions': [
                {
                  'to_mode': 'day',
                  'trigger': {'kind': 'scheduled', 'time': '07:00'}
                },
                {
                  'to_mode': 'sleep',
                  'trigger': {'kind': 'solar', 'offset_minutes': 10}
                }
              ]
            }
          ]
        };
      }
      return {
        'plan_id': 'hub-plan',
        'server_instance_id': 'fixture',
        'before': {'value': 1},
        'affected_rooms': ['a', 'b']
      };
    }, () {}, catalog: ops, nodes: [sensor]);
    final catalog = await HomeAgentCatalog.load(live, client);
    final scene = await catalog.review({
      'target': client.reference('c', 'evening'),
      'operation': 'plan_home_scene_edit',
      'arguments': {}
    }, 'fixture', 'journey');
    expect(scene.description, contains('Kitchen, Dining'));
    expect(scene.arguments, {'plan_id': 'hub-plan', 'confirmed': true});
    final sched = await catalog.review({
      'target': client.reference('s', 'work'),
      'operation': 'plan_light_schedule_edit',
      'arguments': {'delta_minutes': -20, 'transition': 'sleep'}
    }, 'fixture', 'journey');
    expect(sched.operation, 'apply_light_schedule_edit');
    final times = seen.last.$2['definition']['transitions'];
    expect(times[0]['trigger']['time'], '07:00');
    expect(times[1]['trigger']['offset_minutes'], -10);
    final repair = await catalog.review({
      'target': client.reference('m', 'door-sensor'),
      'operation': 'plan_motion_target',
      'arguments': {'room': client.reference('r', 'c')}
    }, 'fixture', 'journey');
    expect(seen.last.$2, {
      'sensor_id': 'door-sensor',
      'room_id': 'c',
      'correlation_id': 'journey'
    });
    expect(repair.arguments, {
      'confirmed': true,
      'plan_id': 'hub-plan',
      'server_instance_id': 'fixture'
    });
    expect(seen.any((entry) => entry.$1.startsWith('apply_')), isFalse);
  });
  test(
      'the normalized JPEG contains pixels without source comments or GPS metadata',
      () {
    final source = image.Image(width: 12, height: 8);
    source.exif.imageIfd.imageDescription = 'private fixture label';
    source.exif.gpsIfd[1] = 'N';
    final output = normalizeLightingPhoto(image.encodeJpg(source));
    final result = image.decodeJpg(output)!;
    expect(result.width, 12);
    expect(result.height, 8);
    expect(result.exif.gpsIfd.isEmpty, isTrue);
    expect(
        String.fromCharCodes(output), isNot(contains('private fixture label')));
  });
}
