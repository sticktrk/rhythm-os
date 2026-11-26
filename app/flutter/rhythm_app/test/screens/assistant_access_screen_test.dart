import 'dart:async';
import 'dart:io';

import 'package:dio/dio.dart';

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
  bool supportsConnections = true;
  bool waiting = false;
  bool registered = false;

  @override
  Future<RhythmAssistantClientRegistration> registerAssistantClient(
      {bool unattendedChanges = false}) async {
    registered = true;
    return const RhythmAssistantClientRegistration(
        'local-client', 'local-secret', 'legacy-code', Duration(minutes: 5));
  }

  @override
  Future<List<RhythmAssistantConnection>> listAssistantConnections() async {
    if (!supportsConnections) {
      throw DioException(
          requestOptions: RequestOptions(),
          response:
              Response(requestOptions: RequestOptions(), statusCode: 404));
    }
    return [
      if (waiting)
        RhythmAssistantConnection(
            id: 'waiting-1',
            destination: 'Home Assistant',
            kind: 'home_assistant',
            remote: false,
            expiresAt: DateTime.now().add(const Duration(minutes: 5)))
    ];
  }

  bool pending = false;
  bool? approved;
  @override
  Future<List<RhythmAssistantApproval>> listAssistantApprovals() async => [
        if (pending)
          const RhythmAssistantApproval(
              planId: 'plan-1',
              operation: 'plan_room_tuning',
              credentialId: 'client-1',
              review: {
                'before': 'Minimum brightness 15%',
                'after': 'Minimum brightness 20%'
              })
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
  for (final supported in [true, false]) {
    testWidgets(
        'Home Assistant setup uses the advertised connection flow ($supported)',
        (tester) async {
      tester.view.devicePixelRatio = 1;
      tester.view.physicalSize = const Size(412, 1000);
      addTearDown(tester.view.reset);
      final auth = _Auth()..supportsConnections = supported;
      final stream = StreamController<void>.broadcast();
      addTearDown(stream.close);
      await tester.pumpWidget(MaterialApp(
          home: AssistantAccessScreen(
              api: auth,
              endpoint: 'https://hub.example/api/assistant/mcp',
              isCurrentTarget: () => true,
              targetChanges: ValueNotifier(true),
              attentionEvents: stream.stream)));
      await tester.pumpAndSettle();
      auth.waiting = true;
      stream.add(null);
      await tester.pumpAndSettle();
      expect(find.text('WAITING TO CONNECT'),
          supported ? findsOneWidget : findsNothing);
      final connect = find.text('Connect Home Assistant Assist');
      await tester.ensureVisible(connect);
      await tester.tap(connect);
      await tester.pumpAndSettle();
      expect(auth.registered, !supported);
      expect(find.textContaining('type rhythm for both'),
          supported ? findsOneWidget : findsNothing);
      expect(find.text('Reveal'), supported ? findsNothing : findsOneWidget);
      if (!supported) {
        await tester.tap(find.text('Reveal'));
        await tester.pumpAndSettle();
        expect(find.text('legacy-code'), findsOneWidget);
      }
    });
  }

  for (final approve in [true, false]) {
    testWidgets(
        'owner reviews and ${approve ? "approves" : "declines"} a pending change',
        (tester) async {
      tester.view.devicePixelRatio = 1;
      tester.view.physicalSize = const Size(412, 860);
      addTearDown(tester.view.reset);
      final auth = _Auth()..pending = true;
      await tester.pumpWidget(MaterialApp(
          debugShowCheckedModeBanner: false,
          home: AssistantAccessScreen(
              api: auth,
              endpoint: 'https://hub.example/api/assistant/mcp',
              isCurrentTarget: () => true,
              targetChanges: ValueNotifier(true))));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Room tuning'));
      await tester.pumpAndSettle();
      expect(find.text('Minimum brightness 15%'), findsOneWidget);
      expect(find.text('Minimum brightness 20%'), findsOneWidget);
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
