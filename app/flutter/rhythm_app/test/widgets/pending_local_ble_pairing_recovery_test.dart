import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/services/analytics_service.dart';
import 'package:rhythm_app/services/settings_service.dart';
import 'package:rhythm_app/widgets/pending_local_ble_pairing_recovery.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../helpers/capturing_analytics_backend.dart';

const _scope = 'home-1:server-1';
const _pending = PendingLocalBlePairing(
  sessionId: 'pairing-session-1',
  journeyId: 'pairing-journey-1',
  attemptNumber: 2,
  profileId: 'orein.oc02001_button.v1',
  serverScope: _scope,
  startedAtEpochMs: 1000,
);
const _device = RhythmPairedDevice(
  deviceId: 'local-ble:button-1',
  name: 'Bedside Button',
  deviceType: 'button',
);

Widget _harness({
  required Future<List<PendingLocalBlePairing>> Function() loader,
  required Future<RhythmPairingResultStatus?> Function(String) status,
  required Future<bool> Function({
    required String serverScope,
    required String sessionId,
  }) clearer,
  required Future<void> Function(BuildContext, RhythmPairedDevice) recovered,
  Future<bool> Function(PendingLocalBlePairing)? updater,
  Future<bool> Function(String)? acknowledger,
  String scope = _scope,
  Set<String> ownedPairingKeys = const {},
}) {
  return MaterialApp(
    home: Scaffold(
      body: PendingLocalBlePairingRecoveryBanner(
        serverScope: scope,
        loadPendingPairings: loader,
        clearPendingPairing: clearer,
        updatePendingPairing: updater ?? (_) async => true,
        requestStatus: status,
        acknowledgeServerResult: acknowledger ?? (_) async => true,
        onRecovered: recovered,
        ownedPairingKeys: ownedPairingKeys,
        pollInterval: const Duration(days: 1),
      ),
    ),
  );
}

Future<void> _settleRecovery(WidgetTester tester) async {
  await tester.pump();
  await tester.pump();
  await tester.pump();
}

void main() {
  late CapturingAnalyticsBackend analytics;

  setUp(() async {
    analytics = CapturingAnalyticsBackend();
    await analytics.initialize();
    BackendProvider.setInstanceForTesting(
      auth: OfflineAuthBackend(),
      analytics: analytics,
    );
    AnalyticsService().resetForTesting();
    await AnalyticsService().initialize();
  });

  tearDown(() {
    AnalyticsService().resetForTesting();
    BackendProvider.resetForTesting();
  });

  testWidgets('loads and polls an app-start pending session for active server',
      (tester) async {
    var statusRequests = 0;
    var clearRequests = 0;
    await tester.pumpWidget(
      _harness(
        loader: () async => const [
          PendingLocalBlePairing(
            sessionId: 'another-server-session',
            journeyId: 'another-server-journey',
            attemptNumber: 1,
            profileId: 'other.profile',
            serverScope: 'home-2:server-2',
            startedAtEpochMs: 900,
          ),
          _pending,
        ],
        status: (sessionId) async {
          statusRequests += 1;
          expect(sessionId, _pending.sessionId);
          return const RhythmPairingResultStatus(
            sessionId: 'pairing-session-1',
            state: RhythmPairingResultState.pending,
            hubType: 'local_ble',
          );
        },
        clearer: ({required serverScope, required sessionId}) async {
          clearRequests += 1;
          return true;
        },
        recovered: (_, __) async {},
      ),
    );
    await _settleRecovery(tester);

    expect(
      find.byKey(const ValueKey('pending-local-ble-pairing-recovery')),
      findsOneWidget,
    );
    expect(find.text('Finishing Bluetooth pairing'), findsOneWidget);
    expect(statusRequests, 1);
    expect(clearRequests, 0);
  });

  testWidgets('keeps the pointer when status is unreachable', (tester) async {
    var clearRequests = 0;
    await tester.pumpWidget(
      _harness(
        loader: () async => const [_pending],
        status: (_) async => throw Exception('offline'),
        clearer: ({required serverScope, required sessionId}) async {
          clearRequests += 1;
          return true;
        },
        recovered: (_, __) async {},
      ),
    );
    await _settleRecovery(tester);

    expect(find.textContaining('saved session is safe'), findsOneWidget);
    expect(clearRequests, 0);
  });

  testWidgets('malformed storage never polls, acknowledges, clears, or updates',
      (tester) async {
    var statusRequests = 0;
    var acknowledgementRequests = 0;
    var clearRequests = 0;
    var updateRequests = 0;
    await tester.pumpWidget(
      _harness(
        loader: () async => throw const FormatException('corrupt pointer'),
        status: (_) async {
          statusRequests += 1;
          return null;
        },
        acknowledger: (_) async {
          acknowledgementRequests += 1;
          return true;
        },
        clearer: ({required serverScope, required sessionId}) async {
          clearRequests += 1;
          return true;
        },
        updater: (_) async {
          updateRequests += 1;
          return true;
        },
        recovered: (_, __) async {},
      ),
    );
    await _settleRecovery(tester);

    expect(statusRequests, 0);
    expect(acknowledgementRequests, 0);
    expect(clearRequests, 0);
    expect(updateRequests, 0);
    expect(
      find.byKey(const ValueKey('pending-local-ble-pairing-recovery')),
      findsNothing,
    );
  });

  testWidgets('fallback pointer is ignored after server B authenticates',
      (tester) async {
    var statusRequests = 0;
    var acknowledgementRequests = 0;
    var clearRequests = 0;
    const fallbackScope = 'home-1:hub-record-1';
    final fallback = PendingLocalBlePairing(
      sessionId: _pending.sessionId,
      journeyId: _pending.journeyId,
      attemptNumber: _pending.attemptNumber,
      profileId: _pending.profileId,
      serverScope: fallbackScope,
      startedAtEpochMs: _pending.startedAtEpochMs,
    );
    await tester.pumpWidget(
      _harness(
        loader: () async => [fallback],
        status: (_) async {
          statusRequests += 1;
          return null;
        },
        acknowledger: (_) async {
          acknowledgementRequests += 1;
          return true;
        },
        clearer: ({required serverScope, required sessionId}) async {
          clearRequests += 1;
          return true;
        },
        recovered: (_, __) async {},
        scope: 'home-1:server-b',
      ),
    );
    await _settleRecovery(tester);

    expect(statusRequests, 0);
    expect(acknowledgementRequests, 0);
    expect(clearRequests, 0);
    expect(
      find.byKey(const ValueKey('pending-local-ble-pairing-recovery')),
      findsNothing,
    );
  });

  testWidgets('canonical server A pointer is ignored by server B',
      (tester) async {
    var statusRequests = 0;
    var acknowledgementRequests = 0;
    var clearRequests = 0;
    await tester.pumpWidget(
      _harness(
        loader: () async => const [_pending],
        status: (_) async {
          statusRequests += 1;
          return null;
        },
        acknowledger: (_) async {
          acknowledgementRequests += 1;
          return true;
        },
        clearer: ({required serverScope, required sessionId}) async {
          clearRequests += 1;
          return true;
        },
        recovered: (_, __) async {},
        scope: 'home-1:server-b',
      ),
    );
    await _settleRecovery(tester);

    expect(statusRequests, 0);
    expect(acknowledgementRequests, 0);
    expect(clearRequests, 0);
    expect(
      find.byKey(const ValueKey('pending-local-ble-pairing-recovery')),
      findsNothing,
    );
  });

  testWidgets('typed not-found is cached before acknowledgement and clearing',
      (tester) async {
    final cleared = <String>[];
    final ordering = <String>[];
    await tester.pumpWidget(
      _harness(
        loader: () async => const [_pending],
        status: (_) async => const RhythmPairingResultStatus(
          sessionId: 'pairing-session-1',
          state: RhythmPairingResultState.notFound,
        ),
        clearer: ({required serverScope, required sessionId}) async {
          ordering.add('local-clear');
          cleared.add('$serverScope/$sessionId');
          return true;
        },
        updater: (pairing) async {
          expect(pairing.terminalResult?.status,
              PendingLocalBleTerminalResult.notFound);
          ordering.add('terminal-cache');
          return true;
        },
        acknowledger: (_) async {
          ordering.add('server-ack');
          return true;
        },
        recovered: (_, __) async {},
      ),
    );
    await _settleRecovery(tester);

    expect(cleared, isEmpty);
    expect(find.textContaining('start a new Bluetooth pairing safely'),
        findsOneWidget);
    expect(
        find.byKey(
          const ValueKey('acknowledge-failed-local-ble-recovery'),
        ),
        findsOneWidget);

    await tester.tap(
      find.byKey(
        const ValueKey('acknowledge-failed-local-ble-recovery'),
      ),
    );
    await _settleRecovery(tester);
    expect(cleared, ['$_scope/${_pending.sessionId}']);
    expect(ordering, ['terminal-cache', 'server-ack', 'local-clear']);
  });

  testWidgets('retains successful result until warnings are acknowledged',
      (tester) async {
    final cleared = <String>[];
    final ordering = <String>[];
    RhythmPairedDevice? recoveredDevice;
    await tester.pumpWidget(
      _harness(
        loader: () async => const [_pending],
        status: (_) async => const RhythmPairingResultStatus(
          sessionId: 'pairing-session-1',
          state: RhythmPairingResultState.terminal,
          hubType: 'local_ble',
          result: RhythmPairingSessionResult(
            hubType: 'local_ble',
            status: RhythmPairingStatus.complete,
            devices: [_device],
            warnings: ['Button events begin after the next sync.'],
          ),
        ),
        clearer: ({required serverScope, required sessionId}) async {
          ordering.add('local-clear');
          cleared.add('$serverScope/$sessionId');
          return true;
        },
        updater: (pairing) async {
          expect(pairing.terminalResult?.status,
              PendingLocalBleTerminalResult.complete);
          ordering.add('terminal-cache');
          return true;
        },
        acknowledger: (_) async {
          ordering.add('server-ack');
          return true;
        },
        recovered: (_, device) async => recoveredDevice = device,
      ),
    );
    await _settleRecovery(tester);

    expect(find.text('Bluetooth device added'), findsOneWidget);
    expect(find.textContaining('Bedside Button'), findsOneWidget);
    expect(
      find.text('• Button events begin after the next sync.'),
      findsOneWidget,
    );
    expect(cleared, isEmpty);
    expect(recoveredDevice, isNull);

    await tester.tap(
      find.byKey(
        const ValueKey('acknowledge-recovered-local-ble-pairing'),
      ),
    );
    await _settleRecovery(tester);

    expect(cleared, ['$_scope/${_pending.sessionId}']);
    expect(ordering, ['terminal-cache', 'server-ack', 'local-clear']);
    expect(recoveredDevice?.deviceId, _device.deviceId);
    expect(
      find.byKey(const ValueKey('pending-local-ble-pairing-recovery')),
      findsNothing,
    );
    final completion = analytics.events.singleWhere(
      (event) => event.name == 'local_ble_pairing_completed',
    );
    expect(completion.properties['journey_id'], _pending.journeyId);
    expect(completion.properties['attempt_number'], _pending.attemptNumber);
    expect(
      completion.properties['\$insert_id'],
      'local_ble_pairing_completed:${_pending.sessionId}',
    );
  });

  testWidgets('active pairing route owns pointer and suppresses shell UI',
      (tester) async {
    var statusRequests = 0;
    await tester.pumpWidget(
      _harness(
        loader: () async => const [_pending],
        status: (_) async {
          statusRequests += 1;
          return const RhythmPairingResultStatus(
            sessionId: 'pairing-session-1',
            state: RhythmPairingResultState.pending,
            hubType: 'local_ble',
          );
        },
        clearer: ({required serverScope, required sessionId}) async => true,
        recovered: (_, __) async {},
        ownedPairingKeys: {
          LocalBlePairingRouteOwnership.key(_scope, _pending.sessionId),
        },
      ),
    );
    await _settleRecovery(tester);

    expect(statusRequests, 0);
    expect(
      find.byKey(const ValueKey('pending-local-ble-pairing-recovery')),
      findsNothing,
    );

    await tester.pumpWidget(
      _harness(
        loader: () async => const [_pending],
        status: (_) async {
          statusRequests += 1;
          return const RhythmPairingResultStatus(
            sessionId: 'pairing-session-1',
            state: RhythmPairingResultState.pending,
            hubType: 'local_ble',
          );
        },
        clearer: ({required serverScope, required sessionId}) async => true,
        recovered: (_, __) async {},
      ),
    );
    await _settleRecovery(tester);
    expect(statusRequests, 1);
    expect(
      find.byKey(const ValueKey('pending-local-ble-pairing-recovery')),
      findsOneWidget,
    );
  });

  testWidgets('failed local acknowledgement keeps successful recovery visible',
      (tester) async {
    var recoveredCalls = 0;
    await tester.pumpWidget(
      _harness(
        loader: () async => const [_pending],
        status: (_) async => const RhythmPairingResultStatus(
          sessionId: 'pairing-session-1',
          state: RhythmPairingResultState.terminal,
          hubType: 'local_ble',
          result: RhythmPairingSessionResult(
            hubType: 'local_ble',
            status: RhythmPairingStatus.complete,
            devices: [_device],
          ),
        ),
        clearer: ({required serverScope, required sessionId}) async => false,
        recovered: (_, __) async => recoveredCalls += 1,
      ),
    );
    await _settleRecovery(tester);
    await tester.tap(
      find.byKey(
        const ValueKey('acknowledge-recovered-local-ble-pairing'),
      ),
    );
    await _settleRecovery(tester);

    expect(recoveredCalls, 0);
    expect(find.textContaining('result is still safe'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('pending-local-ble-pairing-recovery')),
      findsOneWidget,
    );
  });

  testWidgets('restart reuses deterministic completion deduplication ID',
      (tester) async {
    Widget buildRecovery() => _harness(
          loader: () async => const [_pending],
          status: (_) async => const RhythmPairingResultStatus(
            sessionId: 'pairing-session-1',
            state: RhythmPairingResultState.terminal,
            hubType: 'local_ble',
            result: RhythmPairingSessionResult(
              hubType: 'local_ble',
              status: RhythmPairingStatus.complete,
              devices: [_device],
            ),
          ),
          clearer: ({required serverScope, required sessionId}) async => true,
          recovered: (_, __) async {},
        );

    await tester.pumpWidget(buildRecovery());
    await _settleRecovery(tester);
    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pump();
    await tester.pumpWidget(buildRecovery());
    await _settleRecovery(tester);

    final completionEvents = analytics.events
        .where((event) => event.name == 'local_ble_pairing_completed')
        .toList();
    expect(completionEvents, hasLength(2));
    expect(
      completionEvents.map((event) => event.properties['\$insert_id']).toSet(),
      {'local_ble_pairing_completed:${_pending.sessionId}'},
    );
  });

  testWidgets('cached success survives server acknowledgement failure',
      (tester) async {
    var stored = _pending;
    var statusRequests = 0;
    var acknowledgementRequests = 0;
    var clearRequests = 0;

    Widget buildRecovery() => _harness(
          loader: () async => [stored],
          updater: (pairing) async {
            stored = pairing;
            return true;
          },
          status: (_) async {
            statusRequests += 1;
            return const RhythmPairingResultStatus(
              sessionId: 'pairing-session-1',
              state: RhythmPairingResultState.terminal,
              hubType: 'local_ble',
              result: RhythmPairingSessionResult(
                hubType: 'local_ble',
                status: RhythmPairingStatus.complete,
                devices: [_device],
              ),
            );
          },
          acknowledger: (_) async {
            acknowledgementRequests += 1;
            return false;
          },
          clearer: ({required serverScope, required sessionId}) async {
            clearRequests += 1;
            return true;
          },
          recovered: (_, __) async {},
        );

    await tester.pumpWidget(buildRecovery());
    await _settleRecovery(tester);
    expect(
        stored.terminalResult?.status, PendingLocalBleTerminalResult.complete);
    await tester.tap(
      find.byKey(
        const ValueKey('acknowledge-recovered-local-ble-pairing'),
      ),
    );
    await _settleRecovery(tester);
    expect(acknowledgementRequests, 1);
    expect(clearRequests, 0);
    expect(find.textContaining('result is safe'), findsOneWidget);

    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pump();
    await tester.pumpWidget(buildRecovery());
    await _settleRecovery(tester);

    expect(statusRequests, 1,
        reason: 'cached terminal skips GET after restart');
    expect(find.text('Bluetooth device added'), findsOneWidget);
  });

  testWidgets('cached success survives local clear failure after server ACK',
      (tester) async {
    var stored = _pending;
    var statusRequests = 0;
    var acknowledgementRequests = 0;
    var clearRequests = 0;
    var clearSucceeds = false;
    var recoveredCalls = 0;

    Widget buildRecovery() => _harness(
          loader: () async => [stored],
          updater: (pairing) async {
            stored = pairing;
            return true;
          },
          status: (_) async {
            statusRequests += 1;
            return const RhythmPairingResultStatus(
              sessionId: 'pairing-session-1',
              state: RhythmPairingResultState.terminal,
              hubType: 'local_ble',
              result: RhythmPairingSessionResult(
                hubType: 'local_ble',
                status: RhythmPairingStatus.complete,
                devices: [_device],
              ),
            );
          },
          acknowledger: (_) async {
            acknowledgementRequests += 1;
            return true;
          },
          clearer: ({required serverScope, required sessionId}) async {
            clearRequests += 1;
            if (clearSucceeds) stored = _pending;
            return clearSucceeds;
          },
          recovered: (_, __) async => recoveredCalls += 1,
        );

    await tester.pumpWidget(buildRecovery());
    await _settleRecovery(tester);
    await tester.tap(
      find.byKey(
        const ValueKey('acknowledge-recovered-local-ble-pairing'),
      ),
    );
    await _settleRecovery(tester);
    expect(acknowledgementRequests, 1);
    expect(clearRequests, 1);
    expect(recoveredCalls, 0);
    expect(find.textContaining('result is still safe'), findsOneWidget);

    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pump();
    await tester.pumpWidget(buildRecovery());
    await _settleRecovery(tester);
    expect(statusRequests, 1,
        reason: 'cached terminal skips GET after restart');

    clearSucceeds = true;
    await tester.tap(
      find.byKey(
        const ValueKey('acknowledge-recovered-local-ble-pairing'),
      ),
    );
    await _settleRecovery(tester);
    expect(acknowledgementRequests, 2,
        reason: 'DELETE is safe to retry against a server tombstone');
    expect(clearRequests, 2);
    expect(recoveredCalls, 1);
  });

  testWidgets('cached terminal failure survives acknowledgement failure',
      (tester) async {
    var stored = _pending;
    var statusRequests = 0;
    var acknowledgementRequests = 0;
    var clearRequests = 0;

    Widget buildRecovery() => _harness(
          loader: () async => [stored],
          updater: (pairing) async {
            stored = pairing;
            return true;
          },
          status: (_) async {
            statusRequests += 1;
            return const RhythmPairingResultStatus(
              sessionId: 'pairing-session-1',
              state: RhythmPairingResultState.terminal,
              hubType: 'local_ble',
              result: RhythmPairingSessionResult(
                hubType: 'local_ble',
                status: RhythmPairingStatus.failed,
                error: 'Device stopped advertising.',
                failureStage: 'candidate_connect',
              ),
            );
          },
          acknowledger: (_) async {
            acknowledgementRequests += 1;
            return false;
          },
          clearer: ({required serverScope, required sessionId}) async {
            clearRequests += 1;
            return true;
          },
          recovered: (_, __) async {},
        );

    await tester.pumpWidget(buildRecovery());
    await _settleRecovery(tester);
    expect(stored.terminalResult?.status, PendingLocalBleTerminalResult.failed);
    expect(stored.terminalResult?.failureStage, 'candidate_connect');
    await tester.tap(
      find.byKey(
        const ValueKey('acknowledge-failed-local-ble-recovery'),
      ),
    );
    await _settleRecovery(tester);
    expect(acknowledgementRequests, 1);
    expect(clearRequests, 0);

    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pump();
    await tester.pumpWidget(buildRecovery());
    await _settleRecovery(tester);
    expect(statusRequests, 1,
        reason: 'cached terminal skips GET after restart');
    expect(find.text('Device stopped advertising.'), findsOneWidget);
    final completions = analytics.events
        .where((event) => event.name == 'local_ble_pairing_completed')
        .toList(growable: false);
    expect(completions, hasLength(2));
    expect(
      completions.map((event) => event.properties['failure_stage']).toSet(),
      {'candidate_connect'},
    );
  });
}
