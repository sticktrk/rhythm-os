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
        'hub_type': 'matter',
        'status': 'failed',
        'stage': 'failed',
        'message': 'Pairing failed',
        'error': 'Device unreachable',
      });

      expect(progress.status, RhythmPairingStatus.failed);
      expect(progress.stage, RhythmPairingStage.failed);
      expect(progress.error, 'Device unreachable');
      expect(progress.isTerminal, isTrue);
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

    test('falls back to default when status/stage strings are unrecognized', () {
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
}
