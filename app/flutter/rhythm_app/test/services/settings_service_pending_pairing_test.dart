import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/settings_service.dart';
import 'package:rhythm_core/rhythm_core.dart';

PendingLocalBlePairing _pairing(int index, {String? scope, String? session}) =>
    PendingLocalBlePairing(
      sessionId: session ?? 'session-$index',
      journeyId: 'journey-$index',
      attemptNumber: 1,
      profileId: 'profile-$index',
      serverScope: scope ?? 'home-$index:server-$index',
      startedAtEpochMs: index,
    );

void main() {
  test('provisional endpoint identity exposes only a live route anchor', () {
    final provisional = Hub.create(
      id: 'hub-record-1',
      homeId: 'home-1',
      type: HubType.server,
      name: 'Rhythm Box',
      endpoint: const HubEndpoint(host: '192.0.2.10', port: 54448),
      serverInstanceId: 'endpoint:https://192.0.2.10:54448',
    );
    final durable = Hub.create(
      id: 'hub-record-1',
      homeId: 'home-1',
      type: HubType.server,
      name: 'Rhythm Box',
      endpoint: const HubEndpoint(host: '192.0.2.10', port: 54448),
      serverInstanceId: 'BOX-INSTANCE-1',
    );

    expect(
      localBlePairingFallbackServerScope(provisional),
      'home-1:hub-record-1',
    );
    expect(
      localBlePairingConnectedServerScope(provisional, null),
      isNull,
    );
    expect(
      localBlePairingConnectedServerScope(durable, null),
      'home-1:box-instance-1',
    );
    expect(
      localBlePairingConnectedServerScope(provisional, 'BOX-INSTANCE-1'),
      'home-1:box-instance-1',
    );
    expect(
      localBlePairingConnectedServerScope(durable, 'box-instance-2'),
      isNull,
    );
  });

  test('same appliance cannot overwrite another unresolved session', () {
    final existing = [_pairing(1, scope: 'home:server')];
    final conflicting = _pairing(
      2,
      scope: 'home:server',
      session: 'different-session',
    );

    expect(
      SettingsService.mergePendingLocalBlePairingForSave(
        existing,
        conflicting,
      ),
      isNull,
    );
    expect(existing.single.sessionId, 'session-1');
  });

  test('capacity rejects a new scope instead of evicting unresolved state', () {
    final existing = [for (var index = 0; index < 8; index++) _pairing(index)];

    expect(
      SettingsService.mergePendingLocalBlePairingForSave(
        existing,
        _pairing(9),
      ),
      isNull,
    );
    expect(existing, hasLength(8));
  });

  test('same session can idempotently refresh its own pointer', () {
    final original = _pairing(1, scope: 'home:server');
    final refreshed = PendingLocalBlePairing(
      sessionId: original.sessionId,
      journeyId: original.journeyId,
      attemptNumber: original.attemptNumber,
      profileId: original.profileId,
      serverScope: original.serverScope,
      startedAtEpochMs: 99,
    );

    final merged = SettingsService.mergePendingLocalBlePairingForSave(
      [original],
      refreshed,
    );
    expect(merged, hasLength(1));
    expect(merged!.single.startedAtEpochMs, 99);
  });

  test('strict decoder rejects storage beyond the unresolved scope limit', () {
    final records = [for (var index = 0; index < 9; index++) _pairing(index)];
    expect(
      () => SettingsService.decodePendingLocalBlePairings(
        records.map((record) => record.toJson()).toList(),
      ),
      throwsFormatException,
    );
    expect(
      () => SettingsService.encodePendingLocalBlePairings(records),
      throwsFormatException,
    );
  });

  test('strict decoder rejects oversized encoded storage before parsing', () {
    expect(
      () => SettingsService.decodePendingLocalBlePairings(
        List.filled(256 * 1024 + 1, 'x').join(),
      ),
      throwsFormatException,
    );
  });

  test('strict decoder rejects malformed storage and invalid records', () {
    expect(
      () => SettingsService.decodePendingLocalBlePairings('{not-json'),
      throwsFormatException,
    );
    expect(
      () => SettingsService.decodePendingLocalBlePairings([
        _pairing(1).toJson(),
        {'session_id': 'truncated'},
      ]),
      throwsFormatException,
    );
  });

  test('strict decoder rejects fractional integer fields', () {
    final fractionalAttempt = _pairing(1).toJson()..['attempt_number'] = 1.5;
    final fractionalStart = _pairing(2).toJson()..['started_at_epoch_ms'] = 2.5;

    expect(
      () => SettingsService.decodePendingLocalBlePairings([
        fractionalAttempt,
      ]),
      throwsFormatException,
    );
    expect(
      () => SettingsService.decodePendingLocalBlePairings([
        fractionalStart,
      ]),
      throwsFormatException,
    );
  });

  test('strict decoder rejects ambiguous duplicate appliance scopes', () {
    expect(
      () => SettingsService.decodePendingLocalBlePairings([
        _pairing(1, scope: 'home:server').toJson(),
        _pairing(2, scope: 'home:server').toJson(),
      ]),
      throwsFormatException,
    );
  });

  test('round-trips a bounded terminal recovery snapshot', () {
    final terminal = _pairing(1).withTerminalResult(
      const PendingLocalBleTerminalResult(
        status: PendingLocalBleTerminalResult.complete,
        deviceId: 'local-ble:button-1',
        deviceName: 'Bedside Button',
        deviceType: 'button',
        manufacturer: 'Orein',
        model: 'OC02',
        warnings: ['Button events begin after the next sync.'],
      ),
    );

    final decoded = SettingsService.decodePendingLocalBlePairings(
      SettingsService.encodePendingLocalBlePairings([terminal]),
    );

    expect(decoded, hasLength(1));
    expect(decoded.single.terminalResult?.status,
        PendingLocalBleTerminalResult.complete);
    expect(decoded.single.terminalResult?.deviceId, 'local-ble:button-1');
    expect(decoded.single.terminalResult?.warnings,
        ['Button events begin after the next sync.']);
  });

  test('round-trips a privacy-safe local BLE failure stage', () {
    final terminal = _pairing(1).withTerminalResult(
      const PendingLocalBleTerminalResult(
        status: PendingLocalBleTerminalResult.failed,
        error: 'Local Bluetooth pairing failed.',
        failureStage: 'candidate_service_discovery',
      ),
    );

    final decoded = SettingsService.decodePendingLocalBlePairings(
      SettingsService.encodePendingLocalBlePairings([terminal]),
    );

    expect(decoded.single.terminalResult?.failureStage,
        'candidate_service_discovery');
  });

  test('unknown server failure stage uses the bounded caller fallback', () {
    expect(
      localBleFailureStageOrFallback(
        'candidate_connect',
        fallback: 'terminal_status',
      ),
      'candidate_connect',
    );
    expect(
      localBleFailureStageOrFallback(
        'future_radio_proof',
        fallback: 'terminal_status',
      ),
      'terminal_status',
    );
    expect(
      localBleFailureStageOrFallback(null, fallback: 'terminal_event'),
      'terminal_event',
    );
  });

  test('rejects malformed or unbounded terminal snapshots', () {
    final raw = _pairing(1).toJson();
    raw['terminal_result'] = {
      'status': PendingLocalBleTerminalResult.complete,
      'device_id': 'device-1',
      'device_name': 'Button',
      'device_type': 'button',
      'error': 'complete results cannot also fail',
    };
    expect(PendingLocalBlePairing.fromJson(raw), isNull);

    raw['terminal_result'] = {
      'status': PendingLocalBleTerminalResult.failed,
      'error': List.filled(513, 'x').join(),
    };
    expect(PendingLocalBlePairing.fromJson(raw), isNull);

    raw['terminal_result'] = {
      'status': PendingLocalBleTerminalResult.failed,
      'error': 'Local Bluetooth pairing failed.',
      'failure_stage': 'future_radio_proof',
    };
    expect(PendingLocalBlePairing.fromJson(raw), isNull);
  });

  test('terminal snapshot cannot be erased by a stale pending save', () {
    final pending = _pairing(1, scope: 'home:server');
    final terminal = pending.withTerminalResult(
      const PendingLocalBleTerminalResult(
        status: PendingLocalBleTerminalResult.failed,
        error: 'Device stopped advertising.',
      ),
    );

    expect(
      SettingsService.mergePendingLocalBlePairingForSave(
        [terminal],
        pending,
      ),
      isNull,
    );
  });

  test('terminal snapshot cannot be replaced by a conflicting outcome', () {
    final pending = _pairing(1, scope: 'home:server');
    final failed = pending.withTerminalResult(
      const PendingLocalBleTerminalResult(
        status: PendingLocalBleTerminalResult.failed,
        error: 'Device stopped advertising.',
      ),
    );
    final succeeded = pending.withTerminalResult(
      const PendingLocalBleTerminalResult(
        status: PendingLocalBleTerminalResult.complete,
        deviceId: 'device-1',
        deviceName: 'Button',
        deviceType: 'button',
      ),
    );

    expect(
      SettingsService.mergePendingLocalBlePairingForSave(
        [failed],
        succeeded,
      ),
      isNull,
    );
  });

  test('exact terminal snapshot retry is idempotent', () {
    final pending = _pairing(1, scope: 'home:server');
    final terminal = pending.withTerminalResult(
      const PendingLocalBleTerminalResult(
        status: PendingLocalBleTerminalResult.complete,
        deviceId: 'device-1',
        deviceName: 'Button',
        deviceType: 'button',
        warnings: ['Sync pending.'],
      ),
    );
    final retry = PendingLocalBlePairing.fromJson(terminal.toJson())!;

    final merged = SettingsService.mergePendingLocalBlePairingForSave(
      [terminal],
      retry,
    );
    expect(merged, hasLength(1));
    expect(merged!.single.terminalResult?.deviceId, 'device-1');
  });
}
