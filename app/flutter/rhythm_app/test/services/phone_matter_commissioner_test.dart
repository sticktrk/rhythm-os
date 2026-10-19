import 'dart:async';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/phone_matter_commissioner.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:rhythm_core/rhythm_core.dart' show DirectHubAccess;

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel('test/phone_matter');
  const commissioner = PhoneMatterCommissioner(channel: channel);
  final requests = <Map<Object?, Object?>>[];
  setUp(() {
    requests.clear();
    DirectHubAccess.select(scope: 'test', allowed: true);
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (call) async {
      requests.add(call.arguments as Map<Object?, Object?>);
      return {
        'http_status': 200,
        'body': {'status': 'completed'}
      };
    });
  });
  tearDown(() => TestDefaultBinaryMessengerBinding
      .instance.defaultBinaryMessenger
      .setMockMethodCallHandler(channel, null));
  test('default rpiz handoff contract is unchanged', () async {
    await commissioner.commission(
        baseUrl: 'http://box.invalid',
        originalSetupPayload: 'MT:OWNER',
        sessionId: 'attempt');
    expect(requests.single, {
      'base_url': 'http://box.invalid',
      'setup_payload': 'MT:OWNER',
      'session_id': 'attempt'
    });
  });
  test('HA ownership blocks old direct native path', () async {
    DirectHubAccess.select(scope: 'ha-addon', allowed: false);
    await expectLater(
        commissioner.commission(
            baseUrl: 'http://box.invalid',
            originalSetupPayload: 'MT:OWNER',
            sessionId: 'attempt'),
        throwsStateError);
    expect(requests, isEmpty);
  });

  for (final backend in PhoneMatterBackend.values) {
    test('scope change cancels only the active ${backend.name} native session',
        () async {
      final response = Completer<Map<String, Object?>>();
      final cancellations = <Object?>[];
      var targetCurrent = true;
      if (backend == PhoneMatterBackend.haAddon) {
        DirectHubAccess.select(scope: 'ha-start', allowed: false);
      }
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(channel, (call) async {
        if (call.method == 'cancel') {
          cancellations.add(call.arguments);
          return null;
        }
        return response.future;
      });
      final operation = commissioner.commission(
          baseUrl: 'http://selected.invalid',
          originalSetupPayload: 'MT:OWNER',
          sessionId: 'active-attempt',
          backend: backend,
          isCurrentTarget: () => targetCurrent);
      await Future<void>.delayed(Duration.zero);
      // HA provider metadata may still report the old instance during the gate event.
      if (backend == PhoneMatterBackend.rhythm) targetCurrent = false;
      DirectHubAccess.select(scope: 'different-home', allowed: false);
      await Future<void>.delayed(Duration.zero);
      expect(cancellations, [
        {'session_id': 'active-attempt'}
      ]);
      final rejected = expectLater(
          operation, throwsA(isA<PhoneMatterCommissioningException>()));
      response.complete({
        'http_status': 200,
        'body': {'status': 'completed'}
      });
      await rejected;
    });
  }

  test('HA handoff selects whitelisted backend and explicit provenance',
      () async {
    await commissioner.commission(
        baseUrl: 'http://addon.invalid',
        originalSetupPayload: 'MT:SHARED',
        sessionId: 'attempt',
        backend: PhoneMatterBackend.haAddon,
        codeSource: HaMatterCodeSource.sharing);
    expect(requests.single['backend'], 'ha_addon');
    expect(requests.single['code_source'], 'sharing');
    expect(requests.single['setup_payload'], 'MT:SHARED');
  });
}
