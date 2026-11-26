import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/hubs/lighting_chat_screen.dart';
import 'package:rhythm_app/services/customer_assistant_service.dart';
import 'package:rhythm_app/services/lighting_chat_service.dart';
import 'package:rhythm_app/widgets/assistant_chat_widgets.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

void main() {
  const room = RhythmTopologyNode(
      id: 'target-room', name: 'Private kitchen', kind: RhythmNodeKind.room);
  final tools = {'adjust_room_warmth', 'set_room_color'};
  const plan = {
    'operation': 'adjust_room_warmth',
    'arguments': {'kelvin': 2800}
  };
  late List<(String, CustomerAction)> calls;
  late List<CustomerAction> turns;
  late ValueNotifier<bool> account;
  late Future<CustomerAction> Function() reply;
  late LightingChatHub hub;
  setUp(() {
    calls = [];
    turns = [];
    account = ValueNotifier(true);
    reply = () async => plan;
    hub = LightingChatHub([room], tools, (op, args) async {
      calls.add((op, args));
      return {
        'room_id': room.id,
        'canonical_brightness': 42,
        'canonical_kelvin': 3000,
        'display_name': room.name,
        'token': 'private-token',
        'server_acknowledged': true,
        'light_readback': [
          {
            'verification': 'fresh_device_readback',
            'observed': {'on': true}
          }
        ]
      };
    }, () {});
  });
  Future<void> open(WidgetTester tester,
      {Future<LightingChatHub> Function({bool reconnect})? connect,
      String? initialRoomId}) async {
    await tester.pumpWidget(MaterialApp(
        home: LightingChatScreen(
      openHub: connect ?? ({bool reconnect = false}) async => hub,
      initialRoomId: initialRoomId,
      current: () => account.value,
      changes: account,
      local: true,
      guided: () {},
      account: CustomerAssistantService((body) async {
        if (body['method'] == 'ping') return {'id': 1, 'result': {}};
        turns.add(body['lighting_chat'] as CustomerAction);
        return reply();
      }, () => account.value),
    )));
    await tester.pumpAndSettle();
  }

  testWidgets(
      'room-only chat keeps a whole-home scene request out of the selected room',
      (tester) async {
    const office = RhythmTopologyNode(
        id: 'office-id', name: 'Office', kind: RhythmNodeKind.room);
    hub = LightingChatHub([room, office], tools, (op, args) async {
      calls.add((op, args));
      return {'room_id': args['room_id']};
    }, () {});
    await open(tester, initialRoomId: room.id);
    Future<void> send(String prompt) async {
      await tester.enterText(find.byType(TextField), prompt);
      await tester.testTextInput.receiveAction(TextInputAction.send);
      await tester.pumpAndSettle();
    }

    const scene = 'Create a whole-home scene inspired by the northern lights';
    await send(scene);
    expect(find.textContaining('requires the whole-home lighting assistant'),
        findsOneWidget);
    expect(find.text('Apply to room'), findsNothing);
    expect(calls, isEmpty);
    expect(turns, isEmpty);

    await send('Make it warmer');
    expect(calls.single.$1, 'explain_room_output');
    expect(calls.single.$2, {'room_id': room.id});
    expect((turns.single['messages'] as List).single['text'], 'Make it warmer');
    expect(find.text('Apply to room'), findsOneWidget);
  });

  testWidgets(
      'free-form prompt reads MCP and sends only safe room state; one confirmed write reports readback',
      (tester) async {
    await open(tester);
    await tester.enterText(
        find.byType(TextField), 'A warmer glow for reading, please');
    await tester.pump();
    await tester.tap(find.byTooltip('Send message'));
    await tester.pumpAndSettle();
    expect(calls.map((c) => c.$1), ['explain_room_output']);
    expect(jsonEncode(turns.single), contains('A warmer glow for reading'));
    expect(
        jsonEncode(turns.single),
        isNot(matches(
            'target-room|Private kitchen|private-token|light_readback')));
    expect(find.text('2800 K'), findsOneWidget);
    final apply = find.text('Apply to room');
    await tester.ensureVisible(apply);
    await tester.tap(apply);
    await tester.tap(apply, warnIfMissed: false);
    await tester.pumpAndSettle();
    expect(calls.length, 2);
    expect(calls.last.$1, 'adjust_room_warmth');
    expect(calls.last.$2, {'room_id': room.id, 'kelvin': 2800});
    expect(find.textContaining('Fresh readings from 1 of 1'), findsOneWidget);
  });
  testWidgets(
      'skip and a changed account during inference cannot send a write or reveal late replies',
      (tester) async {
    await open(tester);
    await tester.tap(find.text('Warm it up'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Skip'));
    await tester.pumpAndSettle();
    final pending = Completer<CustomerAction>();
    reply = () => pending.future;
    await tester.enterText(find.byType(TextField), 'Softer for a movie');
    await tester.pump();
    await tester.tap(find.byTooltip('Send message'));
    await tester.pump();
    account.value = false;
    await tester.pump();
    pending.complete(plan);
    await tester.pumpAndSettle();
    expect(calls.every((c) => c.$1 == 'explain_room_output'), isTrue);
    expect(find.textContaining('Open a new chat'), findsOneWidget);
    expect(find.text('Softer for a movie'), findsNothing);
    expect(find.text('Apply to room'), findsNothing);
  });
  testWidgets(
      'revoked access needs an intentional reconnect, then the composer opens',
      (tester) async {
    final attempts = <bool>[];
    await open(tester, connect: ({bool reconnect = false}) async {
      attempts.add(reconnect);
      if (!reconnect) throw LightingChatAccessIssue.revoked;
      return hub;
    });
    expect(attempts, [false]);
    await tester.tap(find.text('Reconnect chat'));
    await tester.pumpAndSettle();
    expect(attempts, [false, true]);
    expect(find.byType(TextField), findsOneWidget);
    expect(turns, isEmpty);
  });
  testWidgets(
      'lost acknowledgement clears the proposal and a follow-up rereads the room',
      (tester) async {
    hub = LightingChatHub([room], tools, (op, args) async {
      calls.add((op, args));
      if (op == 'adjust_room_warmth') {
        throw TimeoutException('lost acknowledgement');
      }
      return {'room_id': room.id, 'canonical_kelvin': 2800};
    }, () {});
    await open(tester);
    await tester.tap(find.text('Warm it up'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Apply to room'));
    await tester.pumpAndSettle();
    expect(find.textContaining('result is uncertain'), findsOneWidget);
    expect(find.text('Apply to room'), findsNothing);
    reply = () async => {'text': 'The hub target is 2800 K.'};
    await tester.enterText(find.byType(TextField), 'What is it set to now?');
    await tester.pump();
    await tester.tap(find.byTooltip('Send message'));
    await tester.pumpAndSettle();
    expect(calls.map((c) => c.$1),
        ['explain_room_output', 'adjust_room_warmth', 'explain_room_output']);
    expect(find.text('The hub target is 2800 K.'), findsOneWidget);
  });

  testWidgets(
      'color swatch can be tried and undone using the exact local trial receipt',
      (tester) async {
    reply = () async => {
          'operation': 'set_room_color',
          'arguments': {'red': 255, 'green': 160, 'blue': 0}
        };
    hub = LightingChatHub([room], tools, (op, args) async {
      calls.add((op, args));
      return {
        'room_id': room.id,
        'server_acknowledged': true,
        if (op == 'set_room_color') 'trial_id': 'local-trial'
      };
    }, () {}, canUndo: true);
    await open(tester);
    await tester.tap(find.text('Try a color'));
    await tester.pumpAndSettle();
    expect(find.text('Try color #FFA000'), findsOneWidget);
    await tester.tap(find.text('Apply to room'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Undo preview'));
    await tester.pumpAndSettle();
    expect(calls.last.$1, 'undo_room_trial');
    expect(calls.last.$2, {'room_id': room.id, 'trial_id': 'local-trial'});
    expect(jsonEncode(turns), isNot(contains('local-trial')));
  });

  testWidgets(
      'multi-room chat opens on a small phone, answers without a room and carries a named target across follow-ups',
      (tester) async {
    tester.view.physicalSize = const Size(375, 667);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    const office = RhythmTopologyNode(
        id: 'office-id', name: 'Office', kind: RhythmNodeKind.room);
    hub = LightingChatHub([room, office], tools, (op, args) async {
      calls.add((op, args));
      return {'room_id': args['room_id'], 'canonical_kelvin': 3200};
    }, () {});
    await open(tester);
    Future<void> send(String text) async {
      await tester.enterText(find.byType(TextField), text);
      await tester.testTextInput.receiveAction(TextInputAction.send);
      await tester.pumpAndSettle();
    }

    expect(find.text('Choose a room'), findsNothing);
    reply = () async => {'text': 'Warmer lighting has a lower Kelvin target.'};
    await send('How does warmth work?');
    expect(turns.single['scope'], 'unresolved');
    expect(turns.single['tools'], isEmpty);
    expect(calls, isEmpty);
    reply = () async => {'rooms': 'clarify'};
    await send('Make it warmer');
    expect(find.widgetWithText(ActionChip, 'Office'), findsNothing);
    expect(find.textContaining('1. Office'), findsOneWidget);
    reply = () async => plan;
    await send('Office');
    expect(turns.last['scope'], 'clarified');
    expect((turns.last['messages'] as List).last['text'], 'Make it warmer');
    expect(jsonEncode(turns.last), isNot(contains('Office')));
    expect(calls.last.$2['room_id'], 'office-id');
    await tester.ensureVisible(find.text('Skip'));
    await tester.tap(find.text('Skip'));
    await tester.pumpAndSettle();
    await send('Make the private kitchen warmer');
    expect(turns.last['scope'], 'mentioned');
    expect(calls.last.$2['room_id'], room.id);
    await tester.ensureVisible(find.text('Skip'));
    await tester.tap(find.text('Skip'));
    await tester.pumpAndSettle();
    reply = () async => {'text': 'The target is 3200 K.'};
    await send('What is it set to now?');
    expect(turns.last['scope'], 'continuing');
    expect(calls.last.$2['room_id'], room.id);
    await tester.drag(find.byType(ListView), const Offset(0, 1800));
    await tester.pumpAndSettle();
    expect(find.text('How does warmth work?'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
      'typing a room answer resumes the original request and an unresolved turn cannot write',
      (tester) async {
    const kitchen =
        RhythmTopologyNode(id: 'k', name: 'Kitchen', kind: RhythmNodeKind.room);
    const kitchenette = RhythmTopologyNode(
        id: 'kn', name: 'Kitchen Nook', kind: RhythmNodeKind.room);
    hub = LightingChatHub([kitchen, kitchenette], tools, (op, args) async {
      calls.add((op, args));
      return {'room_id': args['room_id']};
    }, () {});
    expect(hub.mentionedRooms('Warm the KITCHEN nook.'), [kitchenette]);
    expect(hub.mentionedRooms('Compare kitchen nook and kitchen.'),
        [kitchen, kitchenette]);
    await open(tester);
    Future<void> send(String prompt) async {
      await tester.enterText(find.byType(TextField), prompt);
      await tester.testTextInput.receiveAction(TextInputAction.send);
      await tester.pumpAndSettle();
    }

    await send('Try a warmer light');
    expect(find.text('Apply to room'), findsNothing);
    expect(calls, isEmpty);
    reply = () async => {'rooms': 'clarify'};
    await send('Set my lights to 2800 K');
    reply = () async => plan;
    await send('The kitchen nook, please');
    expect(turns.last['scope'], 'clarified');
    expect((turns.last['messages'] as List).last['text'],
        'Set my lights to 2800 K');
    expect(calls.single.$2['room_id'], kitchenette.id);
    expect(find.text('Apply to room'), findsOneWidget);
  });

  testWidgets(
      'duplicate room names require clarification before reading or proposing a change',
      (tester) async {
    const first = RhythmTopologyNode(
        id: 'east-id', name: 'Office', kind: RhythmNodeKind.room);
    const second = RhythmTopologyNode(
        id: 'west-id', name: 'Office', kind: RhythmNodeKind.room);
    hub = LightingChatHub([room, second, first], tools, (op, args) async {
      calls.add((op, args));
      return {'room_id': args['room_id'], 'server_acknowledged': true};
    }, () {});
    await open(tester, initialRoomId: room.id);
    Future<void> send(String prompt) async {
      await tester.enterText(find.byType(TextField), prompt);
      await tester.testTextInput.receiveAction(TextInputAction.send);
      await tester.pumpAndSettle();
    }

    reply = () async => {'rooms': 'clarify'};
    await send('Dim the Office');
    expect(calls, isEmpty);
    expect(turns.single['tools'], isEmpty);
    expect(find.textContaining('2. Office (2)'), findsOneWidget);
    expect(find.byType(ActionChip), findsNothing);
    reply = () async => plan;
    await send('Office');
    await send('Option 8, please');
    expect(turns, hasLength(1));
    expect(calls, isEmpty);
    await send('Number 2, please');
    expect(calls.single.$2['room_id'], second.id);
    expect(turns.last['scope'], 'clarified');
    expect(turns.last['messages'], [
      {'role': 'user', 'text': 'Dim the Office'}
    ]);
    expect(
        jsonEncode(turns), isNot(matches('east-id|west-id|Option 8|Number 2')));
    await tester.ensureVisible(find.text('Apply to room'));
    await tester.tap(find.text('Apply to room'));
    await tester.pumpAndSettle();
    expect(calls.last.$1, 'adjust_room_warmth');
    expect(calls.last.$2, {'room_id': second.id, 'kelvin': 2800});
    await send('Warm up Office (1)');
    expect(turns.last['scope'], 'mentioned');
    expect(calls.last.$1, 'explain_room_output');
    expect(calls.last.$2, {'room_id': first.id});
    expect(
        find.widgetWithText(AssistantReviewCard, 'Office (1)'), findsOneWidget);
  });

  test('model cannot supply a room ID, unknown tool or invalid Kelvin', () {
    for (final invalid in [
      {
        'operation': 'apply_customer_plan',
        'arguments': {'confirmed': true}
      },
      {
        'operation': 'adjust_room_warmth',
        'arguments': {'kelvin': 0}
      },
      {
        'operation': 'adjust_room_warmth',
        'arguments': {'kelvin': 3000, 'room_id': 'other'}
      },
    ]) {
      expect(
          () => LightingProposal.parse(invalid, tools), throwsFormatException);
    }
  });
}
