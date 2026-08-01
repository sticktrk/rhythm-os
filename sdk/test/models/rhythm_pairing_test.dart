import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  group('RhythmPairingProgress.fromJson', () {
    test('parses a typical commissioning event', () {
      final progress = RhythmPairingProgress.fromJson({
        'hub_type': 'matter',
        'session_id': 'pair-1',
        'status': 'commissioning',
        'stage': 'commissioning',
        'message': 'Commissioning Matter device',
      });

      expect(progress.hubType, 'matter');
      expect(progress.sessionId, 'pair-1');
      expect(progress.status, RhythmPairingStatus.commissioning);
      expect(progress.stage, RhythmPairingStage.commissioning);
      expect(progress.message, 'Commissioning Matter device');
      expect(progress.device, isNull);
      expect(progress.error, isNull);
      expect(progress.failureStage, isNull);
      expect(progress.isTerminal, isFalse);
    });

    test('parses a complete event with device payload', () {
      final progress = RhythmPairingProgress.fromJson({
        'hub_type': 'matter',
        'status': 'complete',
        'stage': 'complete',
        'message': 'Pairing complete',
        'device': {
          'device_id': 'matter-100',
          'name': 'Test Bulb',
          'device_type': 'light',
          'manufacturer': 'Acme',
          'model': 'A19',
        },
      });

      expect(progress.stage, RhythmPairingStage.complete);
      expect(progress.isTerminal, isTrue);
      expect(progress.device, isNotNull);
      expect(progress.device!.deviceId, 'matter-100');
      expect(progress.device!.name, 'Test Bulb');
      expect(progress.device!.deviceType, 'light');
      expect(progress.device!.manufacturer, 'Acme');
      expect(progress.device!.model, 'A19');
    });

    test('parses a failed event with error string', () {
      final progress = RhythmPairingProgress.fromJson({
        'hub_type': 'local_ble',
        'status': 'failed',
        'stage': 'failed',
        'message': 'Pairing failed',
        'error': 'Device unreachable',
        'failure_stage': 'candidate_connect',
      });

      expect(progress.status, RhythmPairingStatus.failed);
      expect(progress.stage, RhythmPairingStage.failed);
      expect(progress.error, 'Device unreachable');
      expect(progress.failureStage, 'candidate_connect');
      expect(progress.isTerminal, isTrue);
    });

    test('preserves unknown bounded failure stages for server skew', () {
      final progress = RhythmPairingProgress.fromJson({
        'status': 'failed',
        'failure_stage': 'future_radio_proof',
      });

      expect(progress.failureStage, 'future_radio_proof');
    });

    test('degrades malformed or oversized failure stages to null', () {
      for (final failureStage in <Object>[42, '', 'x' * 129]) {
        final progress = RhythmPairingProgress.fromJson({
          'status': 'failed',
          'failure_stage': failureStage,
        });

        expect(progress.failureStage, isNull, reason: '$failureStage');
      }
    });

    test('handles missing optional fields without throwing', () {
      final progress = RhythmPairingProgress.fromJson({
        'status': 'searching',
      });

      expect(progress.hubType, 'unknown');
      expect(progress.sessionId, isNull);
      expect(progress.status, RhythmPairingStatus.searching);
      expect(progress.message, isEmpty);
    });

    test('falls back to default when status/stage strings are unrecognized',
        () {
      final progress = RhythmPairingProgress.fromJson({
        'hub_type': 'matter',
        'status': 'wibble',
        'stage': 'wobble',
        'message': '',
      });

      expect(progress.status, RhythmPairingStatus.searching);
      expect(progress.stage, RhythmPairingStage.requested);
    });

    test('parses every wire stage correctly', () {
      const cases = {
        'requested': RhythmPairingStage.requested,
        'hub_connecting': RhythmPairingStage.hubConnecting,
        'searching': RhythmPairingStage.searching,
        'connecting': RhythmPairingStage.connecting,
        'commissioning': RhythmPairingStage.commissioning,
        'finalizing': RhythmPairingStage.finalizing,
        'complete': RhythmPairingStage.complete,
        'failed': RhythmPairingStage.failed,
      };

      for (final entry in cases.entries) {
        final progress = RhythmPairingProgress.fromJson({
          'hub_type': 'matter',
          'status': 'searching',
          'stage': entry.key,
          'message': '',
        });
        expect(progress.stage, entry.value, reason: entry.key);
      }
    });
  });

  group('RhythmPairingSessionResult.fromJson', () {
    Map<String, dynamic> failedResult([Object? failureStage]) => {
          'hub_type': 'local_ble',
          'status': 'failed',
          'error': 'Local Bluetooth pairing failed',
          if (failureStage != null) 'failure_stage': failureStage,
        };

    test('accepts previous-server terminal results without failure_stage', () {
      final result = RhythmPairingSessionResult.fromJson(failedResult());

      expect(result.status, RhythmPairingStatus.failed);
      expect(result.failureStage, isNull);
    });

    test('parses known and future bounded failure stages', () {
      final known = RhythmPairingSessionResult.fromJson(
        failedResult('candidate_service_discovery'),
      );
      final future = RhythmPairingSessionResult.fromJson(
        failedResult('future_radio_proof'),
      );

      expect(known.failureStage, 'candidate_service_discovery');
      expect(future.failureStage, 'future_radio_proof');
    });

    test('rejects malformed or oversized failure stages', () {
      for (final failureStage in <Object>[42, '', 'x' * 129]) {
        expect(
          () => RhythmPairingSessionResult.fromJson(
            failedResult(failureStage),
          ),
          throwsFormatException,
          reason: '$failureStage',
        );
      }
    });

    test('rejects a failure stage on a completed result', () {
      expect(
        () => RhythmPairingSessionResult.fromJson({
          'hub_type': 'local_ble',
          'status': 'complete',
          'device': {
            'device_id': 'local-1',
            'name': 'Button',
            'device_type': 'button',
          },
          'failure_stage': 'candidate_connect',
        }),
        throwsFormatException,
      );
    });
  });
}
