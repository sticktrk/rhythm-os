import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/apple_health_service.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final calls = <MethodCall>[];
  Object? result;
  setUp(() {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    calls.clear();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(AppleHealthService.channel, (call) async {
      calls.add(call);
      return result;
    });
  });
  tearDown(() {
    debugDefaultTargetPlatformOverride = null;
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(AppleHealthService.channel, null);
  });

  test('request completion still decodes denied write permission', () async {
    result = 'denied';
    expect(await AppleHealthService().requestAuthorization(),
        AppleHealthAuthorization.denied);
    expect(calls.single.method, 'requestAuthorization');
    result = 'authorized';
    expect(await AppleHealthService().authorization(),
        AppleHealthAuthorization.authorized);
    result = 'future_status';
    await expectLater(
        AppleHealthService().authorization(), throwsFormatException);
  });

  test(
      'exact write contract requires acknowledgement and never reads Health samples',
      () async {
    result = true;
    final batch = [
      {'sync_id': 'opaque', 'start_epoch_ms': 1, 'end_epoch_ms': 2}
    ];
    await AppleHealthService().saveIntervals(batch);
    expect(calls.single.method, 'saveIntervals');
    expect(calls.single.arguments, batch);
    result = false;
    await expectLater(
        AppleHealthService().saveIntervals(batch), throwsStateError);
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    expect(AppleHealthService.supportedPlatform, isFalse);
    expect(await AppleHealthService().authorization(),
        AppleHealthAuthorization.unavailable);
    expect(calls, hasLength(2));
  });
}
