import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

enum AppleHealthAuthorization { authorized, denied, notDetermined, unavailable }

abstract class AppleHealthWriter {
  Future<AppleHealthAuthorization> authorization();
  Future<AppleHealthAuthorization> requestAuthorization();
  Future<void> saveIntervals(List<Map<String, Object>> intervals);
}

/// Write-only native boundary. No Health values are read into the app.
class AppleHealthService implements AppleHealthWriter {
  static bool get supportedPlatform =>
      !kIsWeb && defaultTargetPlatform == TargetPlatform.iOS;
  static const channel = MethodChannel('lighting.rhythm.app/apple_health');

  @override
  Future<AppleHealthAuthorization> authorization() async {
    if (!supportedPlatform) return AppleHealthAuthorization.unavailable;
    return _decode(await channel.invokeMethod<String>('authorization'));
  }

  @override
  Future<AppleHealthAuthorization> requestAuthorization() async {
    if (!supportedPlatform) return AppleHealthAuthorization.unavailable;
    return _decode(await channel.invokeMethod<String>('requestAuthorization'));
  }

  AppleHealthAuthorization _decode(String? value) => switch (value) {
        'authorized' => AppleHealthAuthorization.authorized,
        'denied' => AppleHealthAuthorization.denied,
        'not_determined' => AppleHealthAuthorization.notDetermined,
        'unavailable' => AppleHealthAuthorization.unavailable,
        _ =>
          throw const FormatException('Invalid Health authorization response'),
      };

  @override
  Future<void> saveIntervals(List<Map<String, Object>> intervals) async {
    if (!supportedPlatform) throw StateError('Apple Health is unavailable');
    final saved = await channel.invokeMethod<bool>('saveIntervals', intervals);
    if (saved != true) throw StateError('Health did not acknowledge the write');
  }
}
