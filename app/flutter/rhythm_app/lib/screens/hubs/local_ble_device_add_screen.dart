import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../providers/server_sync_provider.dart';
import '../../services/analytics_service.dart';
import '../../services/device_pairing_code.dart';
import '../../widgets/solar_orbit.dart';

typedef LocalBlePairingRequest = Future<Map<String, dynamic>?> Function({
  required String hubType,
  required Map<String, dynamic> params,
  required Duration receiveTimeout,
  required String sessionId,
});

/// Pairs one device through a server-advertised local-BLE profile.
///
/// The parsed setup fields are sent instead of the original QR payload. Once
/// pairing starts, this route stays open until a correlated HTTP or SSE
/// terminal result arrives so a successful appliance-side association cannot
/// become an invisible "ghost" pairing.
class LocalBleDeviceAddScreen extends StatefulWidget {
  const LocalBleDeviceAddScreen({
    super.key,
    required this.setup,
    this.pairingProfileId,
    required this.inputMethod,
    this.analyticsSource = 'unknown',
    required this.journeyId,
    @visibleForTesting this.pairingRequest,
    @visibleForTesting this.progressEvents,
    // Rust owns a 120-second absolute completion budget. Keep 15 seconds for
    // the correlated HTTP/SSE terminal result to reach and reconcile here.
    @visibleForTesting this.pairingDeadline = const Duration(seconds: 135),
  });

  final LocalBleSetup setup;

  /// Server-advertised current ID resolved from [setup]'s parser ID.
  /// Tests that exercise the screen in isolation may omit it when both match.
  final String? pairingProfileId;
  final String inputMethod;
  final String analyticsSource;
  final String journeyId;
  final LocalBlePairingRequest? pairingRequest;
  final Stream<RhythmPairingProgress>? progressEvents;
  final Duration pairingDeadline;

  static Future<RhythmPairedDevice?> show(
    BuildContext context, {
    required LocalBleSetup setup,
    required String pairingProfileId,
    required String inputMethod,
    required String analyticsSource,
    required String journeyId,
  }) {
    return Navigator.of(context).push<RhythmPairedDevice>(
      MaterialPageRoute(
        builder: (_) => LocalBleDeviceAddScreen(
          setup: setup,
          pairingProfileId: pairingProfileId,
          inputMethod: inputMethod,
          analyticsSource: analyticsSource,
          journeyId: journeyId,
        ),
      ),
    );
  }

  @override
  State<LocalBleDeviceAddScreen> createState() =>
      _LocalBleDeviceAddScreenState();
}

class _LocalBleDeviceAddScreenState extends State<LocalBleDeviceAddScreen> {
  static const _accent = Color(0xFF26A69A);

  bool _pairing = false;
  bool _completed = false;
  String? _error;
  String? _progressMessage;
  int _attemptNumber = 0;
  int _requestGeneration = 0;
  String _activeSessionId = '';
  StreamSubscription<RhythmPairingProgress>? _progressSubscription;
  Timer? _deadlineTimer;

  String get _pairingProfileId =>
      widget.pairingProfileId ?? widget.setup.profileId;

  Map<String, dynamic> get _pairingParams => {
        ...widget.setup.pairingParams,
        'profile_id': _pairingProfileId,
      };

  @override
  void initState() {
    super.initState();
    AnalyticsService().logScreenView('local_ble_device_add');
  }

  @override
  void dispose() {
    _deadlineTimer?.cancel();
    _progressSubscription?.cancel();
    super.dispose();
  }

  Stream<RhythmPairingProgress>? get _pairingProgressStream {
    final injected = widget.progressEvents;
    if (injected != null) return injected;
    return context
        .read<ServerSyncProvider?>()
        ?.connection
        .pairingProgressEvents;
  }

  Future<Map<String, dynamic>?> _pair(String sessionId) {
    final injected = widget.pairingRequest;
    if (injected != null) {
      return injected(
        hubType: 'local_ble',
        params: _pairingParams,
        receiveTimeout: widget.pairingDeadline,
        sessionId: sessionId,
      );
    }
    return context.read<ServerSyncProvider>().api.pairDevice(
          hubType: 'local_ble',
          params: _pairingParams,
          receiveTimeout: widget.pairingDeadline,
          sessionId: sessionId,
        );
  }

  void _subscribeToProgress() {
    _progressSubscription?.cancel();
    final sessionId = _activeSessionId;
    _progressSubscription = _pairingProgressStream?.listen((event) {
      if (!mounted || _completed || !_pairing || event.hubType != 'local_ble') {
        return;
      }
      if (event.sessionId != sessionId) return;

      if (event.status == RhythmPairingStatus.complete ||
          event.stage == RhythmPairingStage.complete) {
        final devices = event.completedDevices;
        if (devices.isNotEmpty) {
          _completeSuccessfully(devices.first);
        } else {
          _fail(
            'The Rhythm Box completed pairing but did not return a device.',
            failureStage: 'terminal_event_missing_device',
          );
        }
        return;
      }
      if (event.status == RhythmPairingStatus.failed ||
          event.stage == RhythmPairingStage.failed) {
        _fail(
          event.error?.trim().isNotEmpty == true
              ? event.error!.trim()
              : event.message.trim().isNotEmpty
                  ? event.message.trim()
                  : 'Bluetooth pairing failed.',
          failureStage: 'terminal_event',
        );
        return;
      }

      final message = event.message.trim();
      if (message.isNotEmpty) {
        setState(() => _progressMessage = message);
      }
    });
  }

  Future<void> _startPairing() async {
    if (_pairing || _completed) return;
    final attempt = ++_attemptNumber;
    final generation = ++_requestGeneration;
    _activeSessionId = attempt == 1
        ? widget.journeyId
        : '${widget.journeyId}-attempt-$attempt';
    setState(() {
      _pairing = true;
      _error = null;
      _progressMessage = 'Looking for the device near your Rhythm Box…';
    });
    _deadlineTimer?.cancel();
    _deadlineTimer = Timer(widget.pairingDeadline, () {
      if (!mounted ||
          _completed ||
          !_pairing ||
          generation != _requestGeneration) {
        return;
      }
      _fail(
        'The Rhythm Box did not confirm whether pairing finished. Put the '
        'device back in pairing mode and try again.',
        failureStage: 'reconciliation_deadline',
      );
    });
    _subscribeToProgress();
    HapticFeedback.mediumImpact();
    AnalyticsService().logLocalBlePairingAttempted(
      journeyId: widget.journeyId,
      profileId: _pairingProfileId,
      source: widget.analyticsSource,
      inputMethod: widget.inputMethod,
      attemptNumber: attempt,
    );

    try {
      final response = await _pair(_activeSessionId);
      if (!mounted || _completed || generation != _requestGeneration) return;
      final status = response?['status']?.toString().toLowerCase();
      final httpStatus = _httpStatus(response?['http_status']);
      if (status == 'failed') {
        _fail(
          'The Rhythm Box could not pair this device. Put it back in pairing '
          'mode and try again.',
          failureStage: 'pairing',
        );
        return;
      }
      if (_isDefinitiveHttpRejection(httpStatus)) {
        _fail(
          'The Rhythm Box rejected the pairing request. Put the device back '
          'in pairing mode and try again.',
          failureStage: 'server_rejected',
        );
        return;
      }
      final rawDevice = response?['device'];
      if (status == 'complete') {
        if (rawDevice is! Map) {
          _fail(
            'The Rhythm Box completed pairing but did not return a device.',
            failureStage: 'terminal_response_missing_device',
          );
          return;
        }
        try {
          _completeSuccessfully(
            RhythmPairedDevice.fromJson(
              Map<String, dynamic>.from(rawDevice),
            ),
          );
        } catch (_) {
          _fail(
            'The Rhythm Box completed pairing but returned an invalid device.',
            failureStage: 'invalid_response',
          );
        }
        return;
      }
      _waitForTerminalConfirmation(generation);
    } catch (_) {
      if (!mounted || _completed || generation != _requestGeneration) return;
      _waitForTerminalConfirmation(generation);
    }
  }

  int? _httpStatus(Object? value) => switch (value) {
        int status => status,
        num status => status.toInt(),
        String status => int.tryParse(status),
        _ => null,
      };

  bool _isDefinitiveHttpRejection(int? status) {
    // 408 and proxy-style 499 describe the request channel ending; neither
    // proves the appliance stopped work that it may already have accepted.
    if (status == null || status == 408 || status == 499) return false;
    return status >= 400 && status < 500;
  }

  void _waitForTerminalConfirmation(int generation) {
    if (!mounted ||
        _completed ||
        !_pairing ||
        generation != _requestGeneration) {
      return;
    }
    setState(() {
      _progressMessage =
          'Request sent. Waiting for final confirmation from your Rhythm Box…';
    });
  }

  void _completeSuccessfully(RhythmPairedDevice device) {
    if (!mounted || _completed || !_pairing) return;
    if (device.deviceId.trim().isEmpty) {
      _fail(
        'The Rhythm Box completed pairing but returned no device ID.',
        failureStage: 'invalid_response',
      );
      return;
    }
    _completed = true;
    _pairing = false;
    _requestGeneration += 1;
    _stopActiveReconciliation();
    AnalyticsService().logLocalBlePairingCompleted(
      journeyId: widget.journeyId,
      profileId: _pairingProfileId,
      source: widget.analyticsSource,
      inputMethod: widget.inputMethod,
      attemptNumber: _attemptNumber,
      outcome: 'succeeded',
    );
    HapticFeedback.heavyImpact();
    Navigator.of(context).pop(device);
  }

  void _fail(String message, {required String failureStage}) {
    if (!mounted || _completed || !_pairing) return;
    _requestGeneration += 1;
    _pairing = false;
    _stopActiveReconciliation();
    AnalyticsService().logLocalBlePairingCompleted(
      journeyId: widget.journeyId,
      profileId: _pairingProfileId,
      source: widget.analyticsSource,
      inputMethod: widget.inputMethod,
      attemptNumber: _attemptNumber,
      outcome: 'failed',
      failureStage: failureStage,
    );
    setState(() {
      _error = message;
      _progressMessage = null;
    });
  }

  void _stopActiveReconciliation() {
    _deadlineTimer?.cancel();
    _deadlineTimer = null;
    _progressSubscription?.cancel();
    _progressSubscription = null;
  }

  @override
  Widget build(BuildContext context) {
    return PopScope(
      canPop: !_pairing,
      onPopInvokedWithResult: (didPop, _) {
        if (didPop || !_pairing) return;
        ScaffoldMessenger.of(context)
          ..hideCurrentSnackBar()
          ..showSnackBar(
            const SnackBar(
              content: Text(
                'Pairing is still in progress. Keep this screen open until it '
                'finishes.',
              ),
            ),
          );
      },
      child: Scaffold(
        key: const ValueKey('local-ble-pairing-screen'),
        backgroundColor: CelestialColors.backgroundDark,
        appBar: AppBar(
          backgroundColor: CelestialColors.backgroundDark,
          foregroundColor: CelestialColors.textPrimary,
          title: const Text('Add Bluetooth Device'),
        ),
        body: SafeArea(
          top: false,
          child: ListView(
            padding: const EdgeInsets.fromLTRB(22, 20, 22, 32),
            children: [
              const Icon(Icons.bluetooth_rounded, color: _accent, size: 58),
              const SizedBox(height: 20),
              const Text(
                'Put the device in pairing mode',
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 24,
                  fontWeight: FontWeight.w700,
                ),
              ),
              const SizedBox(height: 12),
              Text(
                'Follow the device’s pairing-mode instructions, then keep it '
                'close to the Rhythm Box. Rhythm will verify that it matches '
                'the setup code you scanned.',
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.86),
                  fontSize: 15,
                  height: 1.45,
                ),
              ),
              if (_progressMessage != null) ...[
                const SizedBox(height: 24),
                Text(
                  _progressMessage!,
                  key: const ValueKey('local-ble-pairing-progress'),
                  textAlign: TextAlign.center,
                  style: const TextStyle(color: CelestialColors.textSecondary),
                ),
              ],
              if (_error != null) ...[
                const SizedBox(height: 24),
                Container(
                  key: const ValueKey('local-ble-pairing-error'),
                  padding: const EdgeInsets.all(14),
                  decoration: BoxDecoration(
                    color: Colors.red.withValues(alpha: 0.1),
                    borderRadius: BorderRadius.circular(12),
                    border: Border.all(
                      color: Colors.red.withValues(alpha: 0.35),
                    ),
                  ),
                  child: Text(
                    _error!,
                    style: const TextStyle(color: CelestialColors.textPrimary),
                  ),
                ),
              ],
              const SizedBox(height: 30),
              SizedBox(
                height: 54,
                child: FilledButton.icon(
                  key: const ValueKey('find-local-ble-device'),
                  onPressed: _pairing ? null : _startPairing,
                  style: FilledButton.styleFrom(backgroundColor: _accent),
                  icon: _pairing
                      ? const SizedBox(
                          width: 20,
                          height: 20,
                          child: CircularProgressIndicator(
                            strokeWidth: 2,
                            color: Colors.white,
                          ),
                        )
                      : const Icon(Icons.bluetooth_searching_rounded),
                  label: Text(
                    _pairing ? 'Looking for Device…' : 'Find Device',
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
