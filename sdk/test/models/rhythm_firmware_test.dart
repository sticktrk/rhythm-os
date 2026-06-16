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

  group('RhythmOtaUpdateProgress.fromJson', () {
    test('parses a typical downloading event with byte counts', () {
      final progress = RhythmOtaUpdateProgress.fromJson({
        'stage': 'downloading',
        'message': 'Downloading update bundle',
        'current_version': '0.4.192-beta',
        'target_version': '0.4.193-beta',
        'update_available': true,
        'downloaded_bytes': 50,
        'total_bytes': 100,
        'percent': 50,
      });

      expect(progress.stage, RhythmOtaUpdateStage.downloading);
      expect(progress.message, 'Downloading update bundle');
      expect(progress.currentVersion, '0.4.192-beta');
      expect(progress.targetVersion, '0.4.193-beta');
      expect(progress.updateAvailable, isTrue);
      expect(progress.downloadedBytes, 50);
      expect(progress.totalBytes, 100);
      expect(progress.percent, 50);
      expect(progress.checksumVerified, isNull);
      expect(progress.installedTargets, isEmpty);
      expect(progress.error, isNull);
      expect(progress.isTerminal, isFalse);
    });

    test('parses a verifying event with checksum_verified true', () {
      final progress = RhythmOtaUpdateProgress.fromJson({
        'stage': 'verifying',
        'message': 'Verifying checksum',
        'checksum_verified': true,
      });

      expect(progress.stage, RhythmOtaUpdateStage.verifying);
      expect(progress.checksumVerified, isTrue);
    });

    test('parses an installing event with installed_targets list', () {
      final progress = RhythmOtaUpdateProgress.fromJson({
        'stage': 'installing',
        'message': 'Installing update bundle',
        'installed_targets': ['rhythm-server', 'rhythm-cli'],
      });

      expect(progress.stage, RhythmOtaUpdateStage.installing);
      expect(progress.installedTargets, ['rhythm-server', 'rhythm-cli']);
    });

    test('parses a failed event with error message', () {
      final progress = RhythmOtaUpdateProgress.fromJson({
        'stage': 'failed',
        'message': 'Update failed',
        'error': 'Checksum mismatch',
      });

      expect(progress.stage, RhythmOtaUpdateStage.failed);
      expect(progress.error, 'Checksum mismatch');
      expect(progress.isTerminal, isTrue);
    });

    test('isTerminal is true for restarting and up_to_date', () {
      final restarting = RhythmOtaUpdateProgress.fromJson({
        'stage': 'restarting',
        'message': '',
      });
      final upToDate = RhythmOtaUpdateProgress.fromJson({
        'stage': 'up_to_date',
        'message': '',
      });
      expect(restarting.isTerminal, isTrue);
      expect(upToDate.isTerminal, isTrue);
    });

    test('handles missing optional fields without throwing', () {
      final progress = RhythmOtaUpdateProgress.fromJson({
        'stage': 'checking',
        'message': 'Checking',
      });

      expect(progress.stage, RhythmOtaUpdateStage.checking);
      expect(progress.message, 'Checking');
      expect(progress.currentVersion, isNull);
      expect(progress.targetVersion, isNull);
      expect(progress.updateAvailable, isNull);
      expect(progress.downloadedBytes, isNull);
      expect(progress.totalBytes, isNull);
      expect(progress.percent, isNull);
      expect(progress.installedTargets, isEmpty);
    });

    test('falls back to default stage when wire string is unrecognized', () {
      final progress = RhythmOtaUpdateProgress.fromJson({
        'stage': 'unfamiliar_future_stage',
        'message': '',
      });
      expect(progress.stage, RhythmOtaUpdateStage.checking);
    });

    test('parses every wire stage correctly', () {
      const cases = {
        'checking': RhythmOtaUpdateStage.checking,
        'update_available': RhythmOtaUpdateStage.updateAvailable,
        'up_to_date': RhythmOtaUpdateStage.upToDate,
        'downloading': RhythmOtaUpdateStage.downloading,
        'verifying': RhythmOtaUpdateStage.verifying,
        'staging': RhythmOtaUpdateStage.staging,
        'installing': RhythmOtaUpdateStage.installing,
        'finalizing': RhythmOtaUpdateStage.finalizing,
        'restarting': RhythmOtaUpdateStage.restarting,
        'failed': RhythmOtaUpdateStage.failed,
      };
      for (final entry in cases.entries) {
        final progress = RhythmOtaUpdateProgress.fromJson({
          'stage': entry.key,
          'message': '',
        });
        expect(progress.stage, entry.value, reason: entry.key);
      }
    });

    test('coerces string-ish numeric byte fields', () {
      final progress = RhythmOtaUpdateProgress.fromJson({
        'stage': 'downloading',
        'message': '',
        'downloaded_bytes': '4096',
        'total_bytes': '8192',
        'percent': '50',
      });

      expect(progress.downloadedBytes, 4096);
      expect(progress.totalBytes, 8192);
      expect(progress.percent, 50);
    });
  });

  group('RhythmOtaUpdateStage', () {
    test('has all expected values', () {
      expect(RhythmOtaUpdateStage.values, containsAll([
        RhythmOtaUpdateStage.checking,
        RhythmOtaUpdateStage.updateAvailable,
        RhythmOtaUpdateStage.upToDate,
        RhythmOtaUpdateStage.downloading,
        RhythmOtaUpdateStage.verifying,
        RhythmOtaUpdateStage.staging,
        RhythmOtaUpdateStage.installing,
        RhythmOtaUpdateStage.finalizing,
        RhythmOtaUpdateStage.restarting,
        RhythmOtaUpdateStage.failed,
      ]));
    });

    test('has exactly 10 stages', () {
      expect(RhythmOtaUpdateStage.values.length, 10);
    });
  });
}
