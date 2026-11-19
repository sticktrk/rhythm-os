import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/hubs/assistant_access_screen.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class _Auth extends RhythmAuthApi {
  _Auth() : super(baseUrl: 'http://test.invalid');
  bool issued = false, revoked = false;
  @override
  Future<({String id, String token})> createAgentCredential(
      {bool unattendedChanges = false}) async {
    issued = true;
    return (id: '1234', token: 'one-time-secret');
  }

  @override
  Future<List<RhythmAgentCredential>> listAgentCredentials() async =>
      [if (issued && !revoked) const RhythmAgentCredential('1234', 'bearer')];
  bool pending = false;
  bool? approved;
  @override
  Future<List<RhythmAssistantApproval>> listAssistantApprovals() async => [
        if (pending)
          const RhythmAssistantApproval(
              planId: 'plan-1',
              operation: 'plan_room_tuning',
              credentialId: 'client-1',
              review: {'before': 'low', 'after': 'medium'})
      ];
  @override
  Future<Map<String, dynamic>> decideAssistantApproval(String planId,
      {required bool approve}) async {
    expect(planId, 'plan-1');
    pending = false;
    approved = approve;
    return {'status': approve ? 'applied' : 'declined'};
  }

  @override
  Future<void> revokeAgentCredential(String id) async {
    expect(id, '1234');
    revoked = true;
  }
}

void main() {
  for (final approve in [true, false]) {
    testWidgets(
        'owner reviews and ${approve ? "approves" : "declines"} a pending change',
        (tester) async {
      final auth = _Auth()..pending = true;
      await tester.pumpWidget(MaterialApp(
          home: AssistantAccessScreen(
              api: auth,
              endpoint: 'fixture',
              isCurrentTarget: () => true,
              targetChanges: ValueNotifier(true))));
      await tester.pumpAndSettle();
      await tester.tap(find.text('plan room tuning'));
      await tester.pumpAndSettle();
      expect(find.text('low'), findsOneWidget);
      expect(find.text('medium'), findsOneWidget);
      final screenshot =
          Platform.environment['RHYTHM_AGENT_APPROVAL_SCREENSHOT'];
      if (approve && screenshot != null) {
        await expectLater(find.byType(Overlay), matchesGoldenFile(screenshot));
      }
      final decision = find.text(approve ? 'Approve' : 'Decline');
      await tester.ensureVisible(decision);
      await tester.tap(decision);
      await tester.pumpAndSettle();
      expect(auth.approved, approve);
      expect(find.text('Pending approvals'), findsNothing);
    });
  }
  testWidgets('owner reveals a new secret once and can revoke the registration',
      (tester) async {
    final auth = _Auth();
    final active = ValueNotifier(true);
    await tester.pumpWidget(MaterialApp(
        home: AssistantAccessScreen(
            api: auth,
            endpoint: 'http://test.invalid/api/assistant/mcp',
            isCurrentTarget: () => active.value,
            targetChanges: active)));
    await tester.pumpAndSettle();
    Future<void> tap(String label) async {
      await tester.tap(find.text(label).last);
      await tester.pumpAndSettle();
    }

    await tap('Create agent token');
    expect(find.text('one-time-secret'), findsNothing);
    await tap('Reveal');
    expect(find.text('one-time-secret'), findsOneWidget);
    active.value = false;
    await tester.pumpAndSettle();
    active.value = true;
    await tester.pumpAndSettle();
    expect(find.text('one-time-secret'), findsNothing);
    expect(find.text('Reveal'), findsNothing);
    await tap('Revoke');
    await tap('Cancel');
    expect(auth.revoked, false);
    await tap('Revoke');
    await tap('Revoke');
    expect(auth.revoked, true);
  });
}
