import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  // RhythmOtaApi creates its own Dio instances internally for each call, so
  // we cannot inject mocks. These tests verify construction and the public API
  // surface. The semver comparison logic (_isNewer) is private but is exercised
  // indirectly through checkForUpdate. Full HTTP coverage requires integration
  // tests or a local HTTP server.

  group('RhythmOtaApi', () {
    test('constructs without error', () {
      final api = RhythmOtaApi();
      expect(api, isNotNull);
    });

    test('checkForUpdate is callable and returns a Future', () {
      final api = RhythmOtaApi();

      // We just verify the method signature. The actual call will fail
      // without network, but that confirms it throws/completes rather
      // than being a compile-time issue.
      expect(api.checkForUpdate('1.0.0'), isA<Future<RhythmFirmwareRelease?>>());
    });

    test('startUpdate is callable and returns a Stream', () {
      final api = RhythmOtaApi();
      final release = RhythmFirmwareRelease(
        version: '2.0.0',
        url: 'https://example.com/firmware.bin',
      );

      final stream = api.startUpdate(release, deviceHost: '192.0.2.1');
      expect(stream, isA<Stream<RhythmOtaProgress>>());
    });
  });

  // -------------------------------------------------------------------------
  // RhythmFirmwareRelease model
  // -------------------------------------------------------------------------
  group('RhythmFirmwareRelease', () {
    test('fromJson parses all fields', () {
      final release = RhythmFirmwareRelease.fromJson({
        'version': '1.2.3',
        'url': 'firmware-1.2.3.bin',
        'size': 524288,
        'changelog': 'Bug fixes',
      });

      expect(release.version, '1.2.3');
      expect(release.url, 'https://dl.rhythm.lighting/esp32/firmware-1.2.3.bin');
      expect(release.size, 524288);
      expect(release.changelog, 'Bug fixes');
    });

    test('fromJson uses defaults for missing fields', () {
      final release = RhythmFirmwareRelease.fromJson({});

      expect(release.version, '0.0.0');
      expect(release.url, 'https://dl.rhythm.lighting/esp32/');
      expect(release.size, isNull);
      expect(release.changelog, isNull);
    });
  });

  // -------------------------------------------------------------------------
  // RhythmOtaProgress model
  // -------------------------------------------------------------------------
  group('RhythmOtaProgress', () {
    test('default progressPercent is 0', () {
      const progress = RhythmOtaProgress(state: RhythmOtaState.idle);
      expect(progress.progressPercent, 0);
      expect(progress.errorMessage, isNull);
    });

    test('error state includes message', () {
      const progress = RhythmOtaProgress(
        state: RhythmOtaState.error,
        errorMessage: 'Upload failed',
      );
      expect(progress.state, RhythmOtaState.error);
      expect(progress.errorMessage, 'Upload failed');
    });
  });
}
