import 'dart:async';

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../providers/server_sync_provider.dart';
import '../screens/hubs/device_pairing_flow.dart';
import '../services/analytics_service.dart';
import '../services/server_identity.dart';
import '../services/settings_service.dart';
import 'solar_orbit.dart';

typedef PendingLocalBlePairingsLoader = Future<List<PendingLocalBlePairing>>
    Function();
typedef PendingLocalBlePairingClearer = Future<bool> Function({
  required String serverScope,
  required String sessionId,
});
typedef PendingLocalBlePairingUpdater = Future<bool> Function(
  PendingLocalBlePairing pairing,
);
typedef PendingLocalBlePairingStatusRequest = Future<RhythmPairingResultStatus?>
    Function(String sessionId);
typedef PendingLocalBlePairingAcknowledger = Future<bool> Function(
  String sessionId,
);
typedef RecoveredLocalBlePairingHandler = Future<void> Function(
  BuildContext context,
  RhythmPairedDevice device,
);

/// App-shell recovery surface for a local-BLE operation that outlived its
/// original route or app process.
///
/// Only the active Home/appliance scope is queried. An unreachable or pending
/// operation remains durable. Every terminal outcome is cached locally before
/// server acknowledgement and remains visible until server DELETE and local
/// pointer clearing both succeed.
class AppPendingLocalBlePairingRecovery extends StatelessWidget {
  const AppPendingLocalBlePairingRecovery({super.key});

  @override
  Widget build(BuildContext context) {
    final connected = context
        .select<ServerSyncProvider, ({Hub? hub, String? serverInstanceId})>(
      (sync) => (
        hub: sync.connectedServerHub,
        serverInstanceId: sync.connectedServerInstanceId,
      ),
    );
    final hub = connected.hub;
    if (hub == null ||
        serverIdentityKind(connected.serverInstanceId) !=
            ServerIdentityKind.durable) {
      return const SizedBox.shrink();
    }

    final serverScope = localBlePairingConnectedServerScope(
      hub,
      connected.serverInstanceId,
    );
    if (serverScope == null) return const SizedBox.shrink();
    final sync = context.read<ServerSyncProvider>();
    final settings = SettingsService.instance;

    bool activeConnectionStillMatchesScope() {
      final currentHub = sync.connectedServerHub;
      final currentInstanceId = sync.connectedServerInstanceId;
      if (currentHub == null ||
          serverIdentityKind(currentInstanceId) != ServerIdentityKind.durable) {
        return false;
      }
      return localBlePairingConnectedServerScope(
            currentHub,
            currentInstanceId,
          ) ==
          serverScope;
    }

    return ValueListenableBuilder<Set<String>>(
      valueListenable: LocalBlePairingRouteOwnership.ownedKeys,
      builder: (context, ownedPairingKeys, _) =>
          PendingLocalBlePairingRecoveryBanner(
        key: ValueKey('pending-local-ble-pairing-$serverScope'),
        serverScope: serverScope,
        loadPendingPairings: settings.loadPendingLocalBlePairings,
        clearPendingPairing: ({
          required String serverScope,
          required String sessionId,
        }) =>
            settings.clearPendingLocalBlePairing(
          serverScope: serverScope,
          sessionId: sessionId,
        ),
        updatePendingPairing: (pairing) => pairing.serverScope == serverScope
            ? settings.savePendingLocalBlePairing(pairing)
            : Future.value(false),
        requestStatus: (sessionId) => activeConnectionStillMatchesScope()
            ? sync.api.getPairingResult(sessionId)
            : Future.value(null),
        acknowledgeServerResult: (sessionId) =>
            activeConnectionStillMatchesScope()
                ? sync.api.acknowledgePairingResult(sessionId)
                : Future.value(false),
        onRecovered: continueRecoveredLocalBlePairingFlow,
        ownedPairingKeys: ownedPairingKeys,
      ),
    );
  }
}

enum _RecoveryState { hidden, pending, unavailable, succeeded, failed }

/// Testable recovery coordinator/banner. Production uses
/// [AppPendingLocalBlePairingRecovery] to bind it to the active server.
class PendingLocalBlePairingRecoveryBanner extends StatefulWidget {
  const PendingLocalBlePairingRecoveryBanner({
    super.key,
    required this.serverScope,
    required this.loadPendingPairings,
    required this.clearPendingPairing,
    required this.updatePendingPairing,
    required this.requestStatus,
    required this.acknowledgeServerResult,
    required this.onRecovered,
    this.ownedPairingKeys = const {},
    this.pollInterval = const Duration(seconds: 3),
  });

  final String serverScope;
  final PendingLocalBlePairingsLoader loadPendingPairings;
  final PendingLocalBlePairingClearer clearPendingPairing;
  final PendingLocalBlePairingUpdater updatePendingPairing;
  final PendingLocalBlePairingStatusRequest requestStatus;
  final PendingLocalBlePairingAcknowledger acknowledgeServerResult;
  final RecoveredLocalBlePairingHandler onRecovered;
  final Set<String> ownedPairingKeys;
  final Duration pollInterval;

  @override
  State<PendingLocalBlePairingRecoveryBanner> createState() =>
      _PendingLocalBlePairingRecoveryBannerState();
}

class _PendingLocalBlePairingRecoveryBannerState
    extends State<PendingLocalBlePairingRecoveryBanner>
    with WidgetsBindingObserver {
  static const _accent = Color(0xFF26A69A);

  Timer? _pollTimer;
  PendingLocalBlePairing? _pending;
  RhythmPairedDevice? _device;
  List<String> _warnings = const [];
  _RecoveryState _state = _RecoveryState.hidden;
  String? _message;
  bool _refreshing = false;
  bool _checking = false;
  bool _acknowledging = false;
  String? _loggedTerminalSession;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _startPolling();
    unawaited(_tick());
  }

  @override
  void didUpdateWidget(PendingLocalBlePairingRecoveryBanner oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.serverScope != widget.serverScope ||
        oldWidget.pollInterval != widget.pollInterval) {
      _reset();
      _startPolling();
      unawaited(_tick());
    } else if (oldWidget.ownedPairingKeys != widget.ownedPairingKeys) {
      unawaited(_tick());
    }
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed) unawaited(_tick());
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _pollTimer?.cancel();
    super.dispose();
  }

  void _startPolling() {
    _pollTimer?.cancel();
    _pollTimer = Timer.periodic(
      widget.pollInterval,
      (_) => unawaited(_tick()),
    );
  }

  void _reset() {
    _pollTimer?.cancel();
    _pending = null;
    _device = null;
    _warnings = const [];
    _state = _RecoveryState.hidden;
    _message = null;
    _refreshing = false;
    _checking = false;
    _acknowledging = false;
    _loggedTerminalSession = null;
  }

  Future<void> _tick() async {
    if (!mounted || _acknowledging) return;
    final hasPending = await _refreshPointer();
    if (hasPending) await _checkStatus();
  }

  Future<bool> _refreshPointer() async {
    if (_refreshing || !mounted) {
      return _pending != null && _pending?.terminalResult == null;
    }
    _refreshing = true;
    try {
      final records = await widget.loadPendingPairings();
      if (!mounted) return false;
      PendingLocalBlePairing? match;
      for (final record in records) {
        if (record.serverScope == widget.serverScope) {
          match = record;
          break;
        }
      }
      if (match == null) {
        if (_pending != null || _state != _RecoveryState.hidden) {
          setState(() {
            _pending = null;
            _device = null;
            _warnings = const [];
            _state = _RecoveryState.hidden;
            _message = null;
          });
        }
        return false;
      }
      final pairingOwned = widget.ownedPairingKeys.contains(
        LocalBlePairingRouteOwnership.key(
          widget.serverScope,
          match.sessionId,
        ),
      );
      if (pairingOwned) {
        if (_pending != null || _state != _RecoveryState.hidden) {
          setState(() {
            _pending = null;
            _device = null;
            _warnings = const [];
            _state = _RecoveryState.hidden;
            _message = null;
          });
        }
        return false;
      }

      // Do not let a stale loader snapshot downgrade a terminal result that
      // this coordinator just durably cached.
      if (_pending?.sessionId == match.sessionId &&
          _pending?.terminalResult != null &&
          match.terminalResult == null) {
        match = _pending;
      }

      if (match!.terminalResult != null) {
        _surfaceCachedTerminal(match);
        return false;
      }

      if (_pending?.sessionId != match.sessionId) {
        setState(() {
          _pending = match;
          _device = null;
          _warnings = const [];
          _state = _RecoveryState.pending;
          _message = 'Checking the saved Bluetooth pairing session…';
          _loggedTerminalSession = null;
        });
      }
      return true;
    } catch (_) {
      final cached = _pending;
      if (mounted && cached?.terminalResult != null) {
        _surfaceCachedTerminal(cached!);
        return false;
      }
      if (mounted && _pending != null) {
        setState(() {
          _state = _RecoveryState.unavailable;
          _message = 'Rhythm cannot reach the Box to confirm this pairing. '
              'The saved session is safe and will be checked again.';
        });
      }
      return _pending != null;
    } finally {
      _refreshing = false;
    }
  }

  Future<void> _checkStatus({bool manual = false}) async {
    final pending = _pending;
    if (_checking ||
        pending == null ||
        pending.terminalResult != null ||
        !mounted) {
      return;
    }
    _checking = true;
    if (manual) setState(() {});
    try {
      final status = await widget.requestStatus(pending.sessionId);
      if (!mounted || _pending?.sessionId != pending.sessionId) return;
      if (status == null || status.sessionId != pending.sessionId) {
        _surfaceUnavailable();
        return;
      }
      if (status.state != RhythmPairingResultState.notFound &&
          status.hubType != 'local_ble') {
        _surfaceUnavailable();
        return;
      }

      switch (status.state) {
        case RhythmPairingResultState.pending:
          setState(() {
            _state = _RecoveryState.pending;
            _message = 'Your Rhythm Box is still finishing Bluetooth '
                'pairing. You can keep using the app while it completes.';
          });
          return;
        case RhythmPairingResultState.notFound:
          await _cacheAndSurfaceFailure(
            pending,
            'The Rhythm Box no longer has this pairing request. You can '
            'start a new Bluetooth pairing safely.',
            failureStage: 'terminal_status_not_found',
            terminalStatus: PendingLocalBleTerminalResult.notFound,
          );
          return;
        case RhythmPairingResultState.terminal:
          final result = status.result;
          if (result == null || result.hubType != 'local_ble') {
            _surfaceUnavailable();
            return;
          }
          if (result.status == RhythmPairingStatus.failed) {
            await _cacheAndSurfaceFailure(
              pending,
              result.error?.trim().isNotEmpty == true
                  ? result.error!.trim()
                  : 'Bluetooth pairing failed. You can start a new pairing.',
              failureStage: localBleFailureStageOrFallback(
                result.failureStage,
                fallback: 'terminal_status',
              ),
              terminalStatus: PendingLocalBleTerminalResult.failed,
            );
            return;
          }
          final devices = result.completedDevices;
          if (devices.isEmpty || devices.first.deviceId.trim().isEmpty) {
            _surfaceUnavailable();
            return;
          }
          await _cacheAndSurfaceSuccess(
            pending,
            devices.first,
            result.warnings,
          );
          return;
      }
    } catch (_) {
      if (mounted && _pending?.sessionId == pending.sessionId) {
        _surfaceUnavailable();
      }
    } finally {
      _checking = false;
      if (mounted && manual) setState(() {});
    }
  }

  void _surfaceUnavailable() {
    if (!mounted) return;
    setState(() {
      _state = _RecoveryState.unavailable;
      _message = 'Rhythm cannot reach the Box to confirm this pairing. '
          'The saved session is safe and will be checked again.';
    });
  }

  Future<void> _cacheAndSurfaceFailure(
    PendingLocalBlePairing pending,
    String message, {
    required String failureStage,
    required String terminalStatus,
  }) async {
    final safeMessage = _sanitizeTerminalMessage(message);
    final updated = pending.withTerminalResult(
      PendingLocalBleTerminalResult(
        status: terminalStatus,
        error: safeMessage,
        failureStage: failureStage,
      ),
    );
    final saved = await _saveTerminalPointer(updated);
    if (!mounted || _pending?.sessionId != pending.sessionId) return;
    if (!saved) {
      setState(() {
        _state = _RecoveryState.unavailable;
        _message = 'Pairing finished, but Rhythm could not save the result '
            'safely. The saved session will be checked again before another '
            'pairing can start.';
      });
      return;
    }
    _pending = updated;
    _surfaceCachedTerminal(updated, failureStage: failureStage);
  }

  Future<void> _cacheAndSurfaceSuccess(
    PendingLocalBlePairing pending,
    RhythmPairedDevice device,
    List<String> warnings,
  ) async {
    final sanitizedWarnings = warnings
        .map((warning) => warning.trim())
        .where((warning) => warning.isNotEmpty)
        .take(32)
        .map((warning) =>
            warning.length <= 512 ? warning : warning.substring(0, 512))
        .toList(growable: false);
    final terminal = PendingLocalBleTerminalResult(
      status: PendingLocalBleTerminalResult.complete,
      deviceId: device.deviceId,
      deviceName: device.name,
      deviceType: device.deviceType,
      manufacturer: device.manufacturer,
      model: device.model,
      warnings: sanitizedWarnings,
    );
    // Validate the bounded cache representation before attempting storage.
    if (PendingLocalBleTerminalResult.fromJson(terminal.toJson()) == null) {
      _surfaceUnavailable();
      return;
    }
    final updated = pending.withTerminalResult(terminal);
    final saved = await _saveTerminalPointer(updated);
    if (!mounted || _pending?.sessionId != pending.sessionId) return;
    if (!saved) {
      setState(() {
        _state = _RecoveryState.unavailable;
        _message = 'The device was added, but Rhythm could not save the '
            'result safely. The saved session will be checked again.';
      });
      return;
    }
    _pending = updated;
    _surfaceCachedTerminal(updated);
  }

  Future<bool> _saveTerminalPointer(PendingLocalBlePairing pairing) async {
    try {
      return await widget.updatePendingPairing(pairing);
    } catch (_) {
      return false;
    }
  }

  String _sanitizeTerminalMessage(String message) {
    final trimmed = message.trim();
    if (trimmed.isEmpty) return 'Bluetooth pairing failed.';
    return trimmed.length <= 512 ? trimmed : trimmed.substring(0, 512);
  }

  void _surfaceCachedTerminal(
    PendingLocalBlePairing pending, {
    String? failureStage,
  }) {
    final terminal = pending.terminalResult;
    if (!mounted || terminal == null) return;
    if (terminal.status == PendingLocalBleTerminalResult.complete) {
      final device = RhythmPairedDevice(
        deviceId: terminal.deviceId!,
        name: terminal.deviceName!,
        deviceType: terminal.deviceType!,
        manufacturer: terminal.manufacturer,
        model: terminal.model,
      );
      _logTerminal(pending, outcome: 'succeeded');
      setState(() {
        _pending = pending;
        _device = device;
        _warnings = terminal.warnings;
        _state = _RecoveryState.succeeded;
        _message = null;
      });
      return;
    }
    final notFound = terminal.status == PendingLocalBleTerminalResult.notFound;
    _logTerminal(
      pending,
      outcome: 'failed',
      failureStage: failureStage ??
          terminal.failureStage ??
          (notFound ? 'terminal_status_not_found' : 'terminal_status'),
    );
    setState(() {
      _pending = pending;
      _device = null;
      _warnings = const [];
      _state = _RecoveryState.failed;
      _message = terminal.error?.trim().isNotEmpty == true
          ? terminal.error!.trim()
          : notFound
              ? 'The Rhythm Box no longer has this pairing request. You can '
                  'start a new Bluetooth pairing safely.'
              : 'Bluetooth pairing failed. You can start a new pairing.';
    });
  }

  void _logTerminal(
    PendingLocalBlePairing pending, {
    required String outcome,
    String? failureStage,
  }) {
    if (_loggedTerminalSession == pending.sessionId) return;
    _loggedTerminalSession = pending.sessionId;
    unawaited(
      AnalyticsService().logLocalBlePairingCompleted(
        journeyId: pending.journeyId,
        profileId: pending.profileId,
        source: 'app_shell_recovery',
        inputMethod: 'persisted_session',
        attemptNumber: pending.attemptNumber,
        outcome: outcome,
        failureStage: failureStage,
        deduplicationId: 'local_ble_pairing_completed:${pending.sessionId}',
      ),
    );
  }

  Future<void> _acknowledgeSuccess() async {
    final pending = _pending;
    final device = _device;
    if (_acknowledging || pending == null || device == null) return;
    setState(() => _acknowledging = true);
    try {
      final serverAcknowledged =
          await widget.acknowledgeServerResult(pending.sessionId);
      if (!mounted || _pending?.sessionId != pending.sessionId) return;
      if (!serverAcknowledged) {
        setState(() {
          _state = _RecoveryState.succeeded;
          _message = 'The device was added, but the Rhythm Box could not save '
              'your acknowledgement. The result is safe; tap Continue to '
              'try again.';
        });
        return;
      }
      final cleared = await widget.clearPendingPairing(
        serverScope: pending.serverScope,
        sessionId: pending.sessionId,
      );
      if (!mounted || _pending?.sessionId != pending.sessionId) return;
      if (!cleared) {
        setState(() {
          _state = _RecoveryState.succeeded;
          _message = 'The device was added and the Rhythm Box saved your '
              'acknowledgement, but the app could not. The result is still '
              'safe; tap Continue to try again.';
        });
        return;
      }
      setState(() {
        _pending = null;
        _device = null;
        _warnings = const [];
        _state = _RecoveryState.hidden;
        _message = null;
      });
      await widget.onRecovered(context, device);
      if (!mounted) return;
    } catch (_) {
      if (!mounted) return;
      setState(() {
        if (_pending?.sessionId == pending.sessionId) {
          _state = _RecoveryState.succeeded;
          _message = 'The device was added, but Rhythm could not save your '
              'acknowledgement. The result is safe; tap Continue to try '
              'again.';
        } else {
          _state = _RecoveryState.failed;
          _message = '${device.name} was added. Rhythm could not open room '
              'assignment, so you can assign it later from Devices.';
        }
      });
    } finally {
      _acknowledging = false;
      if (mounted) setState(() {});
    }
  }

  Future<void> _acknowledgeFailure() async {
    final pending = _pending;
    if (_acknowledging || pending?.terminalResult == null) return;
    setState(() => _acknowledging = true);
    try {
      final serverAcknowledged =
          await widget.acknowledgeServerResult(pending!.sessionId);
      if (!mounted || _pending?.sessionId != pending.sessionId) return;
      if (!serverAcknowledged) {
        setState(() {
          _state = _RecoveryState.failed;
          _message = 'The Rhythm Box could not save your acknowledgement. '
              'The final result is safe; tap Continue to try again.';
        });
        return;
      }
      final cleared = await widget.clearPendingPairing(
        serverScope: pending.serverScope,
        sessionId: pending.sessionId,
      );
      if (!mounted || _pending?.sessionId != pending.sessionId) return;
      if (!cleared) {
        setState(() {
          _state = _RecoveryState.failed;
          _message = 'The Rhythm Box saved your acknowledgement, but the app '
              'could not. The final result is safe; tap Continue to try '
              'again.';
        });
        return;
      }
      setState(() {
        _pending = null;
        _device = null;
        _warnings = const [];
        _state = _RecoveryState.hidden;
        _message = null;
      });
    } catch (_) {
      if (!mounted || _pending?.sessionId != pending!.sessionId) return;
      setState(() {
        _state = _RecoveryState.failed;
        _message = 'Rhythm could not save your acknowledgement. The final '
            'result is safe; tap Continue to try again.';
      });
    } finally {
      _acknowledging = false;
      if (mounted) setState(() {});
    }
  }

  @override
  Widget build(BuildContext context) {
    if (_state == _RecoveryState.hidden) return const SizedBox.shrink();

    final succeeded = _state == _RecoveryState.succeeded;
    final failed = _state == _RecoveryState.failed;
    final title = succeeded
        ? 'Bluetooth device added'
        : failed
            ? 'Bluetooth pairing finished'
            : 'Finishing Bluetooth pairing';
    final body = succeeded
        ? _message ?? '${_device!.name} was added while you were away.'
        : _message ?? 'Checking the saved Bluetooth pairing session…';

    return SafeArea(
      bottom: false,
      minimum: const EdgeInsets.fromLTRB(14, 8, 14, 8),
      child: Material(
        key: const ValueKey('pending-local-ble-pairing-recovery'),
        color: const Color(0xFF202535),
        elevation: 8,
        borderRadius: BorderRadius.circular(14),
        child: Padding(
          padding: const EdgeInsets.fromLTRB(14, 12, 12, 12),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Padding(
                padding: const EdgeInsets.only(top: 2),
                child: Icon(
                  succeeded
                      ? Icons.check_circle_rounded
                      : failed
                          ? Icons.info_outline_rounded
                          : Icons.bluetooth_searching_rounded,
                  color: succeeded ? _accent : const Color(0xFFFFC857),
                ),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(
                      title,
                      style: const TextStyle(
                        color: CelestialColors.textPrimary,
                        fontWeight: FontWeight.w700,
                      ),
                    ),
                    const SizedBox(height: 4),
                    Text(
                      body,
                      style: const TextStyle(
                        color: CelestialColors.textSecondary,
                        height: 1.35,
                      ),
                    ),
                    if (succeeded && _warnings.isNotEmpty) ...[
                      const SizedBox(height: 6),
                      ConstrainedBox(
                        constraints: const BoxConstraints(maxHeight: 120),
                        child: SingleChildScrollView(
                          key: const ValueKey(
                            'recovered-local-ble-pairing-warnings',
                          ),
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: [
                              for (final warning in _warnings)
                                Text(
                                  '• $warning',
                                  style: const TextStyle(
                                    color: Color(0xFFFFD98A),
                                    height: 1.35,
                                  ),
                                ),
                            ],
                          ),
                        ),
                      ),
                    ],
                  ],
                ),
              ),
              const SizedBox(width: 8),
              TextButton(
                key: ValueKey(
                  succeeded
                      ? 'acknowledge-recovered-local-ble-pairing'
                      : failed
                          ? 'acknowledge-failed-local-ble-recovery'
                          : 'check-recovered-local-ble-pairing',
                ),
                onPressed: _checking || _acknowledging
                    ? null
                    : succeeded
                        ? _acknowledgeSuccess
                        : failed
                            ? _acknowledgeFailure
                            : () => unawaited(_checkStatus(manual: true)),
                child: _checking || _acknowledging
                    ? const SizedBox(
                        width: 18,
                        height: 18,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      )
                    : Text(succeeded
                        ? 'Continue'
                        : failed
                            ? 'Continue'
                            : 'Check status'),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
