import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  group('RhythmFirmwareRelease', () {
    group('fromJson', () {
      test('constructs full URL from relative url', () {
        final release = RhythmFirmwareRelease.fromJson({
          'url': 'firmware/v1.2.3/rhythm.bin',
          'version': '1.2.3',
          'size': 524288,
          'changelog': 'Bug fixes and improvements',
        });
        expect(release.url,
            'https://dl.rhythm.lighting/esp32/firmware/v1.2.3/rhythm.bin');
        expect(release.version, '1.2.3');
        expect(release.size, 524288);
        expect(release.changelog, 'Bug fixes and improvements');
      });

      test('version defaults to 0.0.0 when missing', () {
        final release = RhythmFirmwareRelease.fromJson({
          'url': 'firmware/latest.bin',
        });
        expect(release.version, '0.0.0');
      });

      test('url defaults to base URL with empty relative path when missing',
          () {
        final release = RhythmFirmwareRelease.fromJson({});
        expect(release.url, 'https://dl.rhythm.lighting/esp32/');
      });

      test('size is optional and null when missing', () {
        final release = RhythmFirmwareRelease.fromJson({
          'url': 'firmware/v1.0.0/rhythm.bin',
          'version': '1.0.0',
        });
        expect(release.size, isNull);
      });

      test('changelog is optional and null when missing', () {
        final release = RhythmFirmwareRelease.fromJson({
          'url': 'firmware/v1.0.0/rhythm.bin',
          'version': '1.0.0',
        });
        expect(release.changelog, isNull);
      });

      test('handles all fields present', () {
        final release = RhythmFirmwareRelease.fromJson({
          'url': 'v2.0.0/firmware.bin',
          'version': '2.0.0',
          'size': 1048576,
          'changelog': 'Major update with new features',
        });
        expect(
            release.url, 'https://dl.rhythm.lighting/esp32/v2.0.0/firmware.bin');
        expect(release.version, '2.0.0');
        expect(release.size, 1048576);
        expect(release.changelog, 'Major update with new features');
      });

      test('handles empty relative URL', () {
        final release = RhythmFirmwareRelease.fromJson({
          'url': '',
          'version': '1.0.0',
        });
        expect(release.url, 'https://dl.rhythm.lighting/esp32/');
      });
    });
  });

  group('RhythmOtaProgress', () {
    test('constructor with required state and default progressPercent', () {
      const progress = RhythmOtaProgress(state: RhythmOtaState.idle);
      expect(progress.state, RhythmOtaState.idle);
      expect(progress.progressPercent, 0);
      expect(progress.errorMessage, isNull);
    });

    test('constructor with all fields specified', () {
      const progress = RhythmOtaProgress(
        state: RhythmOtaState.downloading,
        progressPercent: 45,
        errorMessage: null,
      );
      expect(progress.state, RhythmOtaState.downloading);
      expect(progress.progressPercent, 45);
      expect(progress.errorMessage, isNull);
    });

    test('constructor with error state and error message', () {
      const progress = RhythmOtaProgress(
        state: RhythmOtaState.error,
        progressPercent: 0,
        errorMessage: 'Download failed: timeout',
      );
      expect(progress.state, RhythmOtaState.error);
      expect(progress.progressPercent, 0);
      expect(progress.errorMessage, 'Download failed: timeout');
    });

    test('progressPercent defaults to 0', () {
      const progress = RhythmOtaProgress(state: RhythmOtaState.checking);
      expect(progress.progressPercent, 0);
    });

    test('errorMessage is optional', () {
      const progress = RhythmOtaProgress(state: RhythmOtaState.complete);
      expect(progress.errorMessage, isNull);
    });

    test('supports all OTA states', () {
      for (final state in RhythmOtaState.values) {
        final progress = RhythmOtaProgress(state: state);
        expect(progress.state, state);
      }
    });
  });

  group('RhythmOtaState', () {
    test('has all expected values', () {
      expect(RhythmOtaState.values, containsAll([
        RhythmOtaState.idle,
        RhythmOtaState.checking,
        RhythmOtaState.available,
        RhythmOtaState.upToDate,
        RhythmOtaState.downloading,
        RhythmOtaState.uploading,
        RhythmOtaState.flashing,
        RhythmOtaState.rebooting,
        RhythmOtaState.complete,
        RhythmOtaState.error,
      ]));
    });

    test('has exactly 10 states', () {
      expect(RhythmOtaState.values.length, 10);
    });
  });
}
