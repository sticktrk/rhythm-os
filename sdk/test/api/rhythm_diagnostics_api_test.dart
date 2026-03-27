import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  // RhythmDiagnosticsApi creates its own Dio internally, so we cannot inject
  // a mock. These tests verify construction and method signatures. Full HTTP
  // coverage requires integration tests against a running device.

  group('RhythmDiagnosticsApi', () {
    test('constructs with host and default port', () {
      final api = RhythmDiagnosticsApi(host: '192.168.1.42');

      // The object should instantiate without error.
      expect(api, isNotNull);
    });

    test('constructs with custom port', () {
      final api = RhythmDiagnosticsApi(host: '10.0.0.5', port: 8080);

      expect(api, isNotNull);
    });

    test('healthCheck returns false when device is unreachable', () async {
      // Use a non-routable address to guarantee a connection failure.
      final api = RhythmDiagnosticsApi(host: '192.0.2.1', port: 1);

      final result = await api.healthCheck();
      expect(result, isFalse);
    });

    test('getDiagVitals returns null when device is unreachable', () async {
      final api = RhythmDiagnosticsApi(host: '192.0.2.1', port: 1);

      final result = await api.getDiagVitals();
      expect(result, isNull);
    });

    test('getDiagLogs returns null when device is unreachable', () async {
      final api = RhythmDiagnosticsApi(host: '192.0.2.1', port: 1);

      final result = await api.getDiagLogs();
      expect(result, isNull);
    });

    test('clearCrashInfo returns false when device is unreachable', () async {
      final api = RhythmDiagnosticsApi(host: '192.0.2.1', port: 1);

      final result = await api.clearCrashInfo();
      expect(result, isFalse);
    });

    test('resetWifi returns false when device is unreachable', () async {
      final api = RhythmDiagnosticsApi(host: '192.0.2.1', port: 1);

      final result = await api.resetWifi();
      expect(result, isFalse);
    });

    test('reboot returns false when device is unreachable', () async {
      final api = RhythmDiagnosticsApi(host: '192.0.2.1', port: 1);

      final result = await api.reboot();
      expect(result, isFalse);
    });
  });
}
