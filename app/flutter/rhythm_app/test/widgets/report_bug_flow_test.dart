import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/widgets/report_bug_flow.dart';

void main() {
  group('report bug flow', () {
    test('fallback summary preserves user text and debug bundle failure', () {
      final summary = summaryWithDebugBundleFailureForTesting(
        summary: 'Lights are stuck',
        endpoint: 'http://192.168.5.123:54448/',
        detail: 'Failed to generate debug bundle (HTTP 500): log read failed',
      );

      expect(summary, startsWith('Lights are stuck'));
      expect(
        summary,
        contains('Server debug bundle download failed before upload.'),
      );
      expect(summary, contains('Endpoint: http://192.168.5.123:54448/'));
      expect(summary, contains('Failed to generate debug bundle'));
    });

    test('fallback summary works without user text', () {
      final summary = summaryWithDebugBundleFailureForTesting(
        summary: '  ',
        endpoint: 'https://server.rhythm.lighting:443/',
        detail: null,
      );

      expect(
        summary,
        startsWith('Server debug bundle download failed before upload.'),
      );
      expect(
          summary, contains('Endpoint: https://server.rhythm.lighting:443/'));
      expect(summary, isNot(contains('Error:')));
    });
  });
}
