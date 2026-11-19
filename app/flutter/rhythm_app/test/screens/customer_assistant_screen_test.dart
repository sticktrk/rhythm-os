import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:rhythm_app/screens/hubs/assistant_chat_screen.dart';
import 'package:rhythm_app/screens/hubs/customer_assistant_screen.dart';
import 'package:rhythm_app/services/customer_assistant_service.dart';

class _Hub extends Fake implements RhythmServerApi {
  @override
  Future<RhythmAssistantContract?> getAssistantContract() async => null;
}

class _Phone extends Fake implements ResidentAssistantDevice {
  @override
  String? get connectedHomeId => null;
  CustomerAction? selected;
  int dispatches = 0;
  @override
  Future<void> select(CustomerAction handoff) async {
    selected = handoff;
  }

  @override
  Future<void> check(CustomerAction claim) async {}
  @override
  Future<bool> execute(CustomerAction claim) async {
    dispatches++;
    return true;
  }
}

void main() {
  testWidgets('hub-first conversation ends when the signed-in account changes',
      (tester) async {
    final changes = StreamController<void>.broadcast();
    addTearDown(changes.close);
    var sameAccount = true;
    final account = CustomerAssistantService((_) async => {}, () => sameAccount,
        accountChanges: changes.stream);
    final transcript = <AssistantMessage>[];
    await tester.pumpWidget(MaterialApp(
        home: AssistantChatScreen(
            account: account,
            api: () => _Hub(),
            serverId: 'box',
            isCurrentTarget: () => true,
            targetChanges: ValueNotifier(true),
            history: transcript,
            openSetup: (_) async {},
            openAccountTools: (context, messages) async {
              messages.add((text: 'Sample Home membership', resident: false));
            })));
    await tester.pumpAndSettle();
    await tester.tap(find.byTooltip('Account and Home tools'));
    await tester.enterText(find.byType(TextField), 'Private draft');
    sameAccount = false;
    changes.add(null);
    await tester.pumpAndSettle();
    expect(transcript, isEmpty);
    expect(find.text('Private draft'), findsNothing);
    expect(
        tester
            .widget<IconButton>(
                find.widgetWithIcon(IconButton, Icons.home_outlined))
            .onPressed,
        isNull);

    sameAccount = true;
    changes.add(null);
    await tester.pumpAndSettle();
    expect(tester.widget<TextField>(find.byType(TextField)).enabled, isFalse);
    expect(find.text('Account changed. Open a new conversation to continue.'),
        findsOneWidget);
  });

  testWidgets(
      'resident selects an account Box and carries the same conversation into hub tools',
      (tester) async {
    final phone = _Phone();
    final transcript = <AssistantMessage>[];
    const handoff = {
      'status': 'handoff',
      'actor': 'resident_app',
      'home_id': 'h',
      'hub_id': 'b',
      'server_instance_id': 's'
    };
    final service = CustomerAssistantService((body) async {
      if (body['contract'] == true) {
        return {
          'operations': [
            for (final id in ['list_homes', 'select_hub', 'list_home_hubs'])
              {'id': id, 'tier': 'read'}
          ]
        };
      }
      return switch (body['operation']) {
        'list_homes' => {
            'homes': [
              {'home_id': 'h', 'display_name': 'North home'}
            ]
          },
        'list_home_hubs' => {
            'hubs': [
              {'hub_id': 'b', 'display_name': 'Kitchen Box'}
            ]
          },
        'select_hub' => handoff,
        _ => throw StateError('Unknown operation')
      };
    }, () => true);
    await tester.pumpWidget(MaterialApp(
        home: CustomerAssistantScreen(
            service: service,
            device: phone,
            continueOnHub: (_, history) async {
              transcript.addAll(history);
            })));
    await tester.pumpAndSettle();
    for (final label in [
      'Homes',
      'North home',
      'Kitchen Box',
      'Lighting tools'
    ]) {
      final chip = find.widgetWithText(ActionChip, label);
      await tester.ensureVisible(chip);
      await tester.tap(chip);
      await tester.pumpAndSettle();
    }
    expect(phone.selected, handoff);
    expect(
        transcript.map((m) => m.text),
        containsAllInOrder([
          'North home',
          'Kitchen Box',
          'Selected Kitchen Box.',
          'Lighting tools'
        ]));
  });

  testWidgets(
      'resumed external report shows summary and attachment choice before device execution',
      (tester) async {
    final phone = _Phone();
    final calls = <CustomerAction>[];
    const review = {
      'status': 'awaiting_device',
      'plan_id': 'p',
      'change': 'submit_report',
      'home_id': 'h',
      'hub_id': 'b',
      'server_instance_id': 's',
      'report_summary': 'motion stopped at five',
      'include_bundle': false
    };
    final service = CustomerAssistantService((body) async {
      calls.add(body);
      if (body['contract'] == true) {
        return {
          'operations': [
            {'id': 'list_pending_actions', 'tier': 'read'}
          ]
        };
      }
      if (body['operation'] == 'list_pending_actions') {
        return {
          'actions': [review]
        };
      }
      if (body['device_claim'] == 'p') return {...review, 'status': 'claimed'};
      if (body['device_complete'] == 'p') {
        return {'status': 'acknowledged', 'plan_id': 'p'};
      }
      throw StateError('Unknown request');
    }, () => true);
    await tester.pumpWidget(MaterialApp(
        home: CustomerAssistantScreen(
            service: service, device: phone, continueOnHub: (_, __) async {})));
    await tester.pumpAndSettle();
    Future<void> tap(String label) async {
      final chip = find.widgetWithText(ActionChip, label);
      await tester.ensureVisible(chip);
      await tester.tap(chip);
      await tester.pumpAndSettle();
    }

    await tap('Pending actions');
    await tap('submit report · awaiting_device');
    expect(find.textContaining('motion stopped at five'), findsOneWidget);
    expect(find.textContaining('without log attachments'), findsOneWidget);
    expect(phone.dispatches, 0);
    await tap('Cancel');
    expect(calls.any((c) => c.containsKey('device_claim')), isFalse);
    await tap('Pending actions');
    await tap('submit report · awaiting_device');
    await tap('Confirm reviewed change');
    expect(phone.dispatches, 1);
    expect(calls.last, {'device_complete': 'p', 'outcome': 'acknowledged'});
  });
}
