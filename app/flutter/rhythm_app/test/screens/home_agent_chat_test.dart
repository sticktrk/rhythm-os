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
  final history = <CustomerAction>[];
  CustomerAction? plan;
  String? roomQuery;
  CustomerAction Function(CustomerAction)? proposalForTurn;
  String answer =
      'The two lamps have different shades. A photo of both could help.';
  @override
  Future<CustomerAction> call(String method,
      [CustomerAction body = const {}, Uint8List? photo]) async {
    calls.add((method, body, photo));
    if (method == 'turn' || method == 'photo') {
      if (body['room_lookup'] == true && roomQuery != null) {
        return {
          'text': '',
          'proposed': false,
          'room_query': roomQuery,
          'plan': plan,
          'history': history
        };
      }
      if (proposalForTurn != null) {
        if (plan != null) history.add(plan!);
        plan = proposalForTurn!(body);
      }
      return {
        'text': answer,
        'plan': plan,
        'history': history,
        'proposed': plan != null
      };
    }
    if (method == 'claim') {
      plan!['steps'][body['index']]['status'] = 'claimed';
      return {'claim': 'once', 'plan': plan, 'history': history};
    }
    if (method == 'receipt') {
      final target = [if (plan != null) plan!, ...history]
          .singleWhere((p) => p['id'] == body['plan']);
      final step = target['steps'][body['index']];
      step['status'] = body['outcome'];
      if (body.containsKey('receipt')) step['receipt'] = body['receipt'];
    }
    return {'plan': plan, 'history': history};
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
      {String? initialRoomId,
      Future<Uint8List?> Function(ImageSource)? picker}) async {
    await t.pumpWidget(MaterialApp(
        home: HomeAgentChat(
            client: client,
            hub: hub,
            current: () => true,
            guided: () {},
            initialRoomId: initialRoomId,
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
    expect(find.textContaining('OpenAI processes'), findsOneWidget);
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
      expect(
          find.descendant(of: card, matching: find.text(text)), findsOneWidget);
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
    expect(find.text('Photo ready to send to OpenAI'), findsOneWidget);
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
  group('conversational room detection', () {
    List<CustomerAction> turns() =>
        client.calls.where((c) => c.$1 == 'turn').map((c) => c.$2).toList();
    List<String> offered(CustomerAction turn) => [
          for (final target in turn['targets'])
            if ((target['tools'] as List).isNotEmpty) target['ref'] as String
        ];

    void enableLookup({String? query}) {
      client.supportsRoomLookup = true;
      client.roomQuery = query;
    }

    void proposeBrightness() {
      client.proposalForTurn = (turn) => {
            'id': 'd553eca1-eaf4-4c99-aa93-5c705343d245',
            'expires': DateTime.now()
                .add(const Duration(minutes: 5))
                .millisecondsSinceEpoch,
            'steps': [
              {
                'target': offered(turn).single,
                'operation': 'adjust_room_brightness',
                'arguments': {'delta_percent': -10},
                'status': 'review',
              }
            ],
          };
    }

    testWidgets(
        'an indirect description resolves to one local room before review',
        (t) async {
      enableLookup(query: 'kitchen');
      proposeBrightness();
      await open(t);
      expect(find.byType(ChoiceChip), findsNothing);
      const prompt = 'Dim the lights where I cook.';
      await send(t, prompt);
      final requests = turns();
      expect(requests, hasLength(2));
      expect(requests.first['room_lookup'], true);
      expect(requests.first['sole_room'], isNot(true));
      expect(requests.first['selected'], isNull);
      expect(offered(requests.first), isEmpty);
      expect(requests.last['turn'], isNot(requests.first['turn']));
      expect(requests.last['room_lookup'], false);
      expect(requests.last['selected'], client.reference('r', 'a'));
      expect(offered(requests.last), [client.reference('r', 'a')]);
      expect(requests.last['messages'], [
        {'role': 'user', 'text': prompt}
      ]);
      expect(requests.toString(), isNot(contains('Kitchen')));
      expect(calls, isNotEmpty);
      expect(
          calls.every(
              (c) => c.$1 == 'explain_room_output' && c.$2['room_id'] == 'a'),
          isTrue);
      final review = find.byType(AssistantReviewCard);
      expect(find.descendant(of: review, matching: find.text('Kitchen')),
          findsOneWidget);
      expect(find.byType(ChoiceChip), findsNothing);
      final apply = find.text('Apply reviewed steps');
      await t.ensureVisible(apply);
      await t.tap(apply);
      await t.pumpAndSettle();
      final writes =
          calls.where((c) => c.$1 == 'adjust_room_brightness').toList();
      expect(writes.single.$2, {'room_id': 'a', 'delta_percent': -10});
    });

    testWidgets('a one-room home flags its sole room only during lookup',
        (t) async {
      hub = LightingChatHub([rooms.first], hub.tools, hub.call, () {});
      enableLookup(query: 'kitchen');
      proposeBrightness();
      await open(t);
      await send(t, 'Dim the lights where I cook.');
      final requests = turns();
      expect(requests, hasLength(2));
      expect(requests.first['room_lookup'], true);
      expect(requests.first['sole_room'], true);
      expect(requests.last['room_lookup'], false);
      expect(requests.last.containsKey('sole_room'), isFalse);
      expect(offered(requests.last), [client.reference('r', 'a')]);
    });

    testWidgets(
        'a typed room answer grounds the original request without a picker',
        (t) async {
      enableLookup(query: '');
      proposeBrightness();
      await open(t);
      const prompt = 'Dim the lights by ten percent.';
      await send(t, prompt);
      expect(turns(), hasLength(1));
      expect(calls, isEmpty);
      expect(find.textContaining('Which room'), findsOneWidget);
      expect(find.textContaining('1. Dining'), findsOneWidget);
      expect(find.byType(ChoiceChip), findsNothing);
      client.roomQuery = 'unmapped place';
      await send(t, 'The loft');
      expect(turns().last['messages'], [
        {'role': 'user', 'text': prompt},
        {'role': 'assistant', 'text': 'Which room did you mean?'},
        {'role': 'user', 'text': 'The loft'}
      ]);
      expect(calls, isEmpty);
      await send(t, 'The first one');
      final resolved = turns().last;
      expect(resolved['room_lookup'], false);
      expect(offered(resolved), [client.reference('r', 'b')]);
      expect(resolved['messages'], [
        {'role': 'user', 'text': prompt}
      ]);
      expect(
          calls.every(
              (c) => c.$1 == 'explain_room_output' && c.$2['room_id'] == 'b'),
          isTrue);
      expect(
          find.descendant(
              of: find.byType(AssistantReviewCard),
              matching: find.text('Dining')),
          findsOneWidget);
    });

    testWidgets(
        'a duplicate room name needs a typed answer even with a previous room',
        (t) async {
      final original = hub;
      const duplicate = RhythmTopologyNode(
          id: 'z', name: 'Kitchen', kind: RhythmNodeKind.room);
      hub = LightingChatHub(
          [...rooms, duplicate], original.tools, original.call, () {});
      enableLookup();
      proposeBrightness();
      await open(t, initialRoomId: 'a');
      await send(t, 'Dim the Kitchen.');
      expect(turns(), isEmpty);
      expect(calls, isEmpty);
      expect(find.textContaining('Kitchen (2)'), findsOneWidget);
      expect(find.byType(ChoiceChip), findsNothing);
      await send(t, 'In the Kitchen, please');
      expect(turns(), isEmpty);
      expect(calls, isEmpty);
      await send(t, 'Kitchen (2)');
      final resolved = turns().single;
      final kitchen = client.reference('r', duplicate.id);
      expect(resolved['room_lookup'], false);
      expect(offered(resolved), [kitchen]);
      expect(resolved['messages'], [
        {'role': 'user', 'text': 'Dim the $kitchen.'}
      ]);
      expect(
          calls.every((c) =>
              c.$1 == 'explain_room_output' && c.$2['room_id'] == duplicate.id),
          isTrue);
      expect(
          find.descendant(
              of: find.byType(AssistantReviewCard),
              matching: find.text('Kitchen (2)')),
          findsOneWidget);
    });

    testWidgets(
        'a named room carries through a follow-up and a whole-home request clears it',
        (t) async {
      enableLookup();
      await open(t, initialRoomId: 'a');
      await send(t, 'How bright is the Hall?');
      final hall = client.reference('r', 'c');
      expect(turns().last['selected'], hall);
      expect(offered(turns().last), [hall]);
      await send(t, 'A little brighter there, please.');
      expect(turns().last['selected'], hall);
      expect(turns().last['room_lookup'], true);
      expect(offered(turns().last), [hall]);
      await send(t, 'Check all rooms.');
      expect(turns().last['selected'], isNull);
      expect(offered(turns().last), hasLength(3));
      await send(t, 'Make it warmer.');
      expect(turns().last['room_lookup'], true);
      expect(offered(turns().last), isEmpty);
    });
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
  group('whole-home scene design', () {
    const creation = 'plan_generated_home_scene';
    const draftPlan = '1b9af27c-a109-4e92-94ed-bd6a7c6cc6b5';
    const sceneId = 'chat-$draftPlan';
    const applyPlan = '26028af8-83e4-42bf-b518-d76190e540cb';
    const saveHandle = '1c809342-aaad-4f41-8623-9c3d0fdae01a';
    const homeHandle = '7c2baa7f-16f1-4af2-b369-013e9f62c304';
    const ops = {
      'read_automation_settings',
      'plan_scene_edit',
      'apply_scene_edit',
      'plan_home_scene_edit',
      'apply_home_scene_edit',
      'read_plan_outcome',
    };
    const xmas = 'Set a whole home Xmas scene!';
    const holidayColors = ['#cf193a', '#11804c', '#f1b94e'];
    const aurora = 'Create a whole-home Northern Lights scene.';
    const auroraColors = ['#0b8457', '#5f4b8b'];
    late CustomerAction draft;
    CustomerAction? saved;
    bool ownerApproved = false;

    String? homeDecision;
    void sceneHub(
        {Set<String> catalog = ops,
        bool loseReply = false,
        bool reviewHome = false}) {
      saved = null;
      ownerApproved = false;
      homeDecision = null;
      hub =
          LightingChatHub(rooms, {'adjust_room_brightness'}, (op, args) async {
        calls.add((op, args));
        switch (op) {
          case 'explain_room_output':
            return {'room_id': args['room_id'], 'canonical_brightness': 50};
          case 'read_automation_settings':
            return {
              'scenes': [if (saved != null) saved],
              'schedules': [],
            };
          case 'plan_scene_edit':
            draft = CustomerAction.from(args['definition']);
            return {
              'plan_id': saveHandle,
              'operation': 'apply_scene_edit',
              'server_instance_id': 'fixture',
              'before': {
                'current': null,
                'room_ids': ['a', 'b'],
                'bound_nodes': {}
              },
              'proposal': {
                'scene': {'id': sceneId, 'definition': draft}
              },
              'affected_rooms': ['a', 'b'],
            };
          case 'apply_scene_edit':
            if (loseReply) {
              saved = draft;
              throw StateError('Connection lost after saving the scene');
            }
            return {'status': 'awaiting_owner_approval', 'plan_id': saveHandle};
          case 'read_plan_outcome':
            if (args['plan_id'] == homeHandle) {
              if (homeDecision == null || homeDecision == 'declined') {
                return {
                  'plan_id': homeHandle,
                  'status': homeDecision ?? 'awaiting_owner_approval'
                };
              }
              return {
                'plan_id': homeHandle,
                'status': homeDecision,
                'server_instance_id': 'fixture',
                'operation': 'apply_home_scene_edit',
                'after': saved,
                'dispatch': {'queued': true},
                'physical_verification': 'not_observed',
              };
            }
            if (!ownerApproved) {
              return {
                'status': 'awaiting_owner_approval',
                'plan_id': saveHandle
              };
            }
            return {
              'status': 'applied',
              'operation': 'apply_scene_edit',
              'plan_id': saveHandle,
              'server_instance_id': 'fixture',
              'after': draft,
              'physical_verification': 'not_observed',
            };
          case 'plan_home_scene_edit':
            return {
              'plan_id': homeHandle,
              'operation': 'apply_home_scene_edit',
              'server_instance_id': 'fixture',
              'before': {
                'current': saved,
                'room_ids': ['a', 'b'],
                'bound_nodes': {}
              },
              'proposal': {
                'home_scene': {'id': sceneId, 'target_mode': 'rooms'}
              },
              'affected_rooms': ['a', 'b'],
            };
          case 'apply_home_scene_edit':
            if (reviewHome) {
              return {
                'status': 'awaiting_owner_approval',
                'plan_id': homeHandle
              };
            }
            return {
              'status': 'queued',
              'operation': 'apply_home_scene_edit',
              'plan_id': homeHandle,
              'server_instance_id': 'fixture',
              'dispatch': {'queued': true},
              'physical_verification': 'not_observed',
            };
          default:
            throw StateError('Unexpected hub call: $op');
        }
      }, () {}, catalog: catalog);
    }

    void agentPalette(List<String> colors) {
      client.supportsSceneDrafts = true;
      client.answer = '';
      client.proposalForTurn = (turn) {
        final target = (turn['targets'] as List)
            .singleWhere((t) => (t['tools'] as List).isNotEmpty);
        final op = (target['tools'] as List).single;
        return {
          'id': op == creation ? draftPlan : applyPlan,
          'expires': DateTime.now()
              .add(const Duration(minutes: 5))
              .millisecondsSinceEpoch,
          'steps': [
            {
              'target': target['ref'],
              'operation': op,
              'arguments': op == creation
                  ? {
                      for (final (i, color) in colors.indexed)
                        'color_${i + 1}': color,
                      'brightness_percent': 65,
                    }
                  : {},
              'status': 'review',
            },
          ],
        };
      };
    }

    Future<void> tap(WidgetTester t, String label) async {
      final button = find.text(label);
      expect(button, findsOneWidget);
      await t.ensureVisible(button);
      await t.tap(button);
      await t.pumpAndSettle();
    }

    Future<void> requestAndSave(
        WidgetTester t, String prompt, List<String> colors,
        {bool loseReply = false,
        bool reviewHome = false,
        bool startFromChip = false}) async {
      sceneHub(loseReply: loseReply, reviewHome: reviewHome);
      agentPalette(colors);
      await open(t, initialRoomId: 'a');
      if (startFromChip) {
        await tap(t, 'Make a scene');
        expect(find.text('What colors or mood would you like for the scene?'),
            findsOneWidget);
      }
      await send(t, prompt);
      final turn = client.calls.singleWhere((c) => c.$1 == 'turn').$2;
      expect(turn['messages'].last['text'], prompt);
      expect(turn['selected'], isNull);
      final offered = turn['targets'] as List;
      expect(offered, hasLength(1));
      expect(offered.single['ref'], client.reference('d', 'new-scene'));
      expect(offered.single['tools'], [creation]);
      expect(calls.where((c) => c.$1 == 'explain_room_output'), isEmpty);
      expect(calls.where((c) => c.$1.startsWith('apply_')), isEmpty);
      final proposal = calls.singleWhere((c) => c.$1 == 'plan_scene_edit').$2;
      expect(proposal['scene_id'], sceneId);
      expect(draft['id'], sceneId);
      expect(draft['extensions']['whole_home'], true);
      final palette = draft['light']['palette'] as List;
      expect(palette, hasLength(colors.length));
      for (final (i, color) in colors.indexed) {
        final rgb = int.parse(color.substring(1), radix: 16);
        expect(palette[i]['color'], {
          'kind': 'rgb',
          'rgb': {'r': rgb >> 16, 'g': (rgb >> 8) & 255, 'b': rgb & 255},
        });
        expect(palette[i]['brightness'], 65);
      }
      expect(find.textContaining('65% brightness'), findsOneWidget);
      await tap(t, 'Save scene');
      expect(calls.singleWhere((c) => c.$1 == 'apply_scene_edit').$2,
          {'confirmed': true, 'plan_id': saveHandle});
      expect(calls.where((c) => c.$1 == 'plan_home_scene_edit'), isEmpty);
      expect(find.text('Use across the home'), findsNothing);
    }

    testWidgets(
        'a whole-home clarification creates a scene for the original mood',
        (t) async {
      sceneHub();
      client.supportsRoomLookup = true;
      client.roomQuery = '';
      agentPalette(holidayColors);
      await open(t);

      await send(t, 'Set a Christmas vibe');
      final first = client.calls.singleWhere((c) => c.$1 == 'turn').$2;
      expect(first['room_lookup'], true);
      expect(find.textContaining('Which room'), findsOneWidget);
      expect(calls.where((c) => c.$1 == 'plan_scene_edit'), isEmpty);

      await send(t, 'the whole home');
      final turns = client.calls.where((c) => c.$1 == 'turn').toList();
      expect(turns, hasLength(2));
      final design = turns.last.$2;
      expect(design['turn'], isNot(first['turn']));
      expect(design['room_lookup'], isNot(true));
      expect(design['selected'], isNull);
      final messages = (design['messages'] as List)
          .where((m) => m['role'] == 'user')
          .map((m) => m['text'])
          .join(' ')
          .toLowerCase();
      expect(messages, contains('christmas vibe'));
      expect(messages, matches(RegExp(r'whole[- ]home')));
      expect(design['targets'], [
        {
          'ref': client.reference('d', 'new-scene'),
          'included': true,
          'tools': [creation]
        }
      ]);
      expect(find.text('Save scene'), findsOneWidget);
      expect(calls.where((c) => c.$1 == 'plan_scene_edit'), hasLength(1));
      expect(
          calls.every((c) =>
              c.$1 == 'read_automation_settings' || c.$1 == 'plan_scene_edit'),
          isTrue);
    });

    for (final (prompt, colors, decision, result, statusLabel) in [
      (xmas, holidayColors, null, 'queued', 'Hub queued the change'),
      (aurora, auroraColors, 'queued', 'queued', 'Hub queued the change'),
      (
        'Soft violet and teal at twilight',
        auroraColors,
        'partially_applied',
        'partial',
        'Configuration changed · some lights unresolved'
      ),
      (
        'Use my firework palette',
        holidayColors,
        'declined',
        'rejected',
        'Hub rejected'
      ),
    ]) {
      testWidgets(
          '$prompt saves the generated palette before a separate home review ($result)',
          (t) async {
        await requestAndSave(t, prompt, colors,
            reviewHome: decision != null,
            startFromChip: prompt != xmas && prompt != aurora);
        await tap(t, 'Check outcome');
        expect(find.text('Use across the home'), findsNothing);
        ownerApproved = true;
        saved = draft;
        await tap(t, 'Check outcome');
        expect(calls.lastWhere((c) => c.$1 == 'read_plan_outcome').$2,
            {'plan_id': saveHandle});
        expect(client.calls.lastWhere((c) => c.$1 == 'receipt').$2, {
          'plan': draftPlan,
          'index': 0,
          'receipt': saveHandle,
          'outcome': 'verified',
        });
        await tap(t, 'Use across the home');
        final turn = client.calls.lastWhere((c) => c.$1 == 'turn').$2;
        final sceneRef = client.reference('c', sceneId);
        expect(turn['messages'].last['text'],
            'Apply $sceneRef to the whole home.');
        expect(calls.singleWhere((c) => c.$1 == 'plan_home_scene_edit').$2,
            {'scene_id': sceneId, 'target_mode': 'rooms'});
        expect(calls.where((c) => c.$1 == 'apply_home_scene_edit'), isEmpty);
        await tap(t, 'Apply reviewed steps');
        expect(calls.singleWhere((c) => c.$1 == 'apply_home_scene_edit').$2,
            {'plan_id': homeHandle, 'confirmed': true});
        if (decision != null) {
          expect(find.text('Awaiting hub owner'), findsOneWidget);
          await tap(t, 'Check outcome');
          expect(find.text('Awaiting hub owner'), findsOneWidget);
          homeDecision = decision;
          await tap(t, 'Check outcome');
          expect(calls.lastWhere((c) => c.$1 == 'read_plan_outcome').$2,
              {'plan_id': homeHandle});
          expect(client.calls.lastWhere((c) => c.$1 == 'receipt').$2, {
            'plan': applyPlan,
            'index': 0,
            'receipt': homeHandle,
            'outcome': result,
          });
          expect(calls.where((c) => c.$1 == 'apply_home_scene_edit'),
              hasLength(1));
        }
        expect(
            client.calls
                .where((c) => c.$1 == 'receipt')
                .map((c) => c.$2['outcome']),
            [
              'awaiting_owner',
              'verified',
              if (decision != null) 'awaiting_owner',
              result
            ]);
        expect(
            find.descendant(
                of: find.byType(AssistantReviewCard),
                matching: find.text(statusLabel)),
            findsOneWidget);
        expect(calls.where((c) => c.$1 == 'adjust_room_brightness'), isEmpty);
      });
    }

    testWidgets(
        'a scene request naming a room keeps the room as its only target',
        (t) async {
      sceneHub();
      client.supportsSceneDrafts = true;
      await open(t, initialRoomId: 'a');
      await send(t, 'Make a sunset scene in Dining');
      final turn = client.calls.singleWhere((c) => c.$1 == 'turn').$2;
      final offered = (turn['targets'] as List)
          .where((t) => (t['tools'] as List).isNotEmpty);
      final dining = client.reference('r', 'b');
      expect(turn['selected'], dining);
      expect(offered.single['ref'], dining);
      expect(offered.single['tools'], ['adjust_room_brightness']);
      expect(calls.where((c) => c.$1 == 'plan_scene_edit'), isEmpty);
    });

    testWidgets(
        'an owner save receipt cannot apply a palette subsequently edited on the hub',
        (t) async {
      await requestAndSave(t, aurora, auroraColors);
      ownerApproved = true;
      saved = {
        ...draft,
        'light': {'palette': []}
      };
      await tap(t, 'Check outcome');
      expect(find.text('Use across the home'), findsNothing);
      expect(calls.where((c) => c.$1 == 'plan_home_scene_edit'), isEmpty);
    });

    testWidgets(
        'a lost save reply recovers the saved scene without repeating the write',
        (t) async {
      await requestAndSave(t, aurora, auroraColors, loseReply: true);
      expect(client.calls.singleWhere((c) => c.$1 == 'receipt').$2['outcome'],
          'uncertain');
      await tap(t, 'Check outcome');
      expect(find.text('Use across the home'), findsOneWidget);
      expect(calls.where((c) => c.$1 == 'apply_scene_edit'), hasLength(1));
      expect(calls.where((c) => c.$1 == 'apply_home_scene_edit'), isEmpty);
    });

    for (final oldHub in [true, false]) {
      testWidgets(
          'a whole-home theme with an older ${oldHub ? 'hub' : 'agent'} keeps the selected room untouched',
          (t) async {
        sceneHub(catalog: oldHub ? ops.difference({'apply_scene_edit'}) : ops);
        client.supportsSceneDrafts = oldHub;
        await open(t, initialRoomId: 'a');
        await send(t, xmas);
        expect(client.calls.where((c) => c.$1 == 'turn'), isEmpty);
        expect(find.textContaining('Scene creation is unavailable'),
            findsOneWidget);
        expect(find.text('Save scene'), findsNothing);
        expect(
            calls.where((c) =>
                c.$1 == 'explain_room_output' || c.$1.startsWith('apply_')),
            isEmpty);
      });
    }
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
