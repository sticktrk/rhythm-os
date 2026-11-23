import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/hubs/lighting_chat_screen.dart';
import 'package:rhythm_app/services/customer_assistant_service.dart';
import 'package:rhythm_app/services/lighting_chat_service.dart';
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
      {Future<LightingChatHub?> Function(bool)? connect}) async {
    await tester.pumpWidget(MaterialApp(
        home: LightingChatScreen(
      openHub: connect ?? (_) async => hub,
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
      'free-form prompt reads MCP and sends only safe room state; one confirmed write reports readback',
      (tester) async {
    await open(tester);
    await tester.enterText(
        find.byType(TextField), 'A warmer glow for reading, please');
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
      'pairing is explicit and older offline hubs retain guided fallback',
      (tester) async {
    final enrollment = <bool>[];
    await open(tester, connect: (enroll) async {
      enrollment.add(enroll);
      if (enroll) throw UnsupportedError('older hub');
      return null;
    });
    expect(enrollment, [false]);
    await tester.tap(find.text('Connect lighting chat'));
    await tester.pumpAndSettle();
    expect(enrollment, [false, true]);
    expect(find.text('Open guided tools'), findsOneWidget);
    expect(turns, isEmpty);
  });
  testWidgets(
      'lost acknowledgement clears the proposal and a follow-up rereads the room',
      (tester) async {
    hub = LightingChatHub([room], tools, (op, args) async {
      calls.add((op, args));
      if (op == 'adjust_room_warmth')
        throw TimeoutException('lost acknowledgement');
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
