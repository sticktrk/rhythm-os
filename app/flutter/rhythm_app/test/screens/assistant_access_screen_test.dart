import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/hubs/assistant_access_screen.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class _Auth extends RhythmAuthApi {
  _Auth() : super(baseUrl: 'http://test.invalid');
  bool issued = false, revoked = false;
  @override
  Future<({String id, String token})> createAgentCredential() async {
    issued = true;
    return (id: '1234', token: 'one-time-secret');
  }

  @override
  Future<List<RhythmAgentCredential>> listAgentCredentials() async =>
      [if (issued && !revoked) const RhythmAgentCredential('1234', 'bearer')];
  @override
  Future<void> revokeAgentCredential(String id) async {
    expect(id, '1234');
    revoked = true;
  }
}

void main() {
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
