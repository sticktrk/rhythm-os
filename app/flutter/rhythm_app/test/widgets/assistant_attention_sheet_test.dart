import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/assistant_attention_controller.dart';
import 'package:rhythm_app/widgets/assistant_attention_sheet.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class _Api extends RhythmAuthApi {
  _Api() : super(baseUrl: 'http://hub.example');
  String? code;
  bool? unattended;
  var result =
      const RhythmConnectionDecisionResult(RhythmConnectionDecision.approved);
  @override
  Future<RhythmConnectionDecisionResult> decideAssistantConnection(String id,
      {required bool approve,
      String matchCode = '',
      bool unattendedChanges = false}) async {
    expect(id, 'request-1');
    code = matchCode;
    unattended = unattendedChanges;
    return result;
  }
}

void main() {
  late _Api api;
  late ValueNotifier<bool> selected;
  Future<void> open(WidgetTester tester) async {
    api = _Api();
    selected = ValueNotifier(true);
    await tester.pumpWidget(MaterialApp(
        home: Scaffold(
            body: Builder(
                builder: (context) => TextButton(
                    onPressed: () => showAssistantAttentionSheet(context,
                        request: ConnectionAttention(RhythmAssistantConnection(
                            id: 'request-1',
                            destination: 'Home Assistant',
                            kind: 'home_assistant',
                            remote: true,
                            expiresAt: DateTime.now()
                                .add(const Duration(minutes: 5)))),
                        api: api,
                        changes: selected,
                        isCurrentTarget: () => selected.value),
                    child: const Text('Open'))))));
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
  }

  Future<void> approve(WidgetTester tester, String code) async {
    await tester.enterText(find.byType(TextField), code);
    await tester.pump();
    await tester.tap(find.text('Approve'));
    await tester.pumpAndSettle();
  }

  testWidgets(
      'mismatch retains the sheet and shows attempts; approval sends the typed digits',
      (tester) async {
    await open(tester);
    expect(find.textContaining('remote access'), findsOneWidget);
    expect(find.text('Allow unattended changes'), findsNothing);
    api.result = const RhythmConnectionDecisionResult(
        RhythmConnectionDecision.codeMismatch,
        attemptsLeft: 2);
    await approve(tester, '42');
    expect(find.textContaining('2 attempts left'), findsOneWidget);
    api.result =
        const RhythmConnectionDecisionResult(RhythmConnectionDecision.approved);
    await approve(tester, '47');
    expect(api.code, '47');
    expect(api.unattended, false);
    expect(find.byType(TextField), findsNothing);
    expect(find.text('Connection approved.'), findsOneWidget);
  });
  testWidgets('expired receipt closes the sheet with a message',
      (tester) async {
    await open(tester);
    api.result =
        const RhythmConnectionDecisionResult(RhythmConnectionDecision.expired);
    await approve(tester, '47');
    expect(find.byType(TextField), findsNothing);
    expect(find.text('This request expired.'), findsOneWidget);
  });
  testWidgets('switching hubs closes the sheet without sending a decision',
      (tester) async {
    await open(tester);
    selected.value = false;
    await tester.pumpAndSettle();
    expect(find.byType(TextField), findsNothing);
    expect(api.code, isNull);
  });
}
