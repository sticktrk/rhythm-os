import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../providers/server_sync_provider.dart';
import '../../services/analytics_service.dart';
import '../../widgets/solar_orbit.dart';
import '../../widgets/stage_timeline.dart';

class HueBlePairedDevice {
  const HueBlePairedDevice({
    required this.nativeDeviceId,
    required this.name,
    required this.deviceType,
    this.manufacturer,
    this.model,
  });

  final String nativeDeviceId;
  final String name;
  final String deviceType;
  final String? manufacturer;
  final String? model;
}

class HueBleDevicePairingResult {
  const HueBleDevicePairingResult({
    required this.devices,
    this.warnings = const [],
  });

  final List<HueBlePairedDevice> devices;
  final List<String> warnings;
}

class HueBleStaleBondCandidate {
  const HueBleStaleBondCandidate({
    required this.address,
    required this.name,
    this.nativeDeviceId,
    this.model,
  });

  final String address;
  final String name;
  final String? nativeDeviceId;
  final String? model;

  String get addressSuffix {
    final compact = address.replaceAll(':', '');
    return compact.length <= 6
        ? compact.toUpperCase()
        : compact.substring(compact.length - 6).toUpperCase();
  }

  String get label {
    final trimmedName = name.trim();
    final trimmedModel = model?.trim() ?? '';
    final friendlyName =
        trimmedName.isEmpty ? 'Hue Bluetooth bulb' : trimmedName;
    if (trimmedModel.isEmpty ||
        friendlyName.toLowerCase().contains(trimmedModel.toLowerCase())) {
      return friendlyName;
    }
    return '$friendlyName ($trimmedModel)';
  }
}

typedef HueBlePairingRequest = Future<Map<String, dynamic>?> Function({
  required String hubType,
  required Map<String, dynamic> params,
  required Duration receiveTimeout,
  required String sessionId,
});

/// Pairs eligible factory-reset Philips Hue bulbs directly with the Rhythm Box.
///
/// Discovery, BlueZ bonding, GATT inspection, and persistence all happen on
/// the appliance. The scan adds every eligible unpaired bulb it finds.
class HueBleDeviceAddScreen extends StatefulWidget {
  const HueBleDeviceAddScreen({
    super.key,
    this.analyticsSource = 'unknown',
    this.journeyId,
    this.inputMethod = 'nearby_scan',
    @visibleForTesting this.pairingRequest,
    @visibleForTesting this.progressEvents,
  });

  final String analyticsSource;
  final String? journeyId;
  final String inputMethod;
  final HueBlePairingRequest? pairingRequest;
  final Stream<RhythmPairingProgress>? progressEvents;

  static Future<HueBleDevicePairingResult?> show(
    BuildContext context, {
    String analyticsSource = 'unknown',
    String? journeyId,
    String inputMethod = 'nearby_scan',
  }) {
    return Navigator.of(context).push<HueBleDevicePairingResult>(
      MaterialPageRoute(
        builder: (_) => HueBleDeviceAddScreen(
          analyticsSource: analyticsSource,
          journeyId: journeyId,
          inputMethod: inputMethod,
        ),
      ),
    );
  }

  @override
  State<HueBleDeviceAddScreen> createState() => _HueBleDeviceAddScreenState();
}

class _HueBleDeviceAddScreenState extends State<HueBleDeviceAddScreen> {
  static const _hueGold = Color(0xFFFFB900);
  static const _danger = Color(0xFFEF5350);
  static const _pairingTimeout = Duration(minutes: 8);
  static const _stages = [
    StageTimelineItem(
      label: 'Find nearby Hue bulbs',
      icon: Icons.radar_rounded,
    ),
    StageTimelineItem(
      label: 'Secure Bluetooth connections',
      icon: Icons.bluetooth_connected_rounded,
    ),
    StageTimelineItem(
      label: 'Add lights to Rhythm',
      icon: Icons.lightbulb_rounded,
    ),
  ];

  late final String _journeyId;
  String _activeSessionId = '';
  StreamSubscription<RhythmPairingProgress>? _progressSubscription;
  RhythmPairingProgress? _latestProgress;
  bool _requestInFlight = false;
  bool _flowCompleted = false;
  bool _failed = false;
  String? _error;
  int _attemptNumber = 0;
  int _requestGeneration = 0;
  int _lastActiveStage = 0;
  late String _activeInputMethod;

  @override
  void initState() {
    super.initState();
    _journeyId = widget.journeyId ??
        'hue-ble-pair-${DateTime.now().microsecondsSinceEpoch.toRadixString(36)}';
    _activeInputMethod = widget.inputMethod;
    AnalyticsService().logScreenView('hue_ble_device_add');
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _startPairing();
    });
  }

  @override
  void dispose() {
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

  void _subscribeToProgress() {
    _progressSubscription?.cancel();
    final subscribedSessionId = _activeSessionId;
    _progressSubscription = _pairingProgressStream?.listen((event) {
      if (!mounted || event.hubType != 'hue_ble') return;
      if (event.sessionId != subscribedSessionId || _flowCompleted) return;

      if (event.stage == RhythmPairingStage.complete ||
          event.status == RhythmPairingStatus.complete) {
        final devices = _pairedDevicesFromProgress(event);
        if (devices.isNotEmpty) {
          _completeSuccessfully(devices, warnings: event.warnings);
        } else {
          _handleTerminalFailure(
            'The Rhythm Box completed pairing but did not identify any bulbs.',
            failureStage: 'terminal_event_missing_devices',
          );
        }
        return;
      }
      if (event.stage == RhythmPairingStage.failed ||
          event.status == RhythmPairingStatus.failed) {
        final error = event.error?.trim();
        final message = event.message.trim();
        _handleTerminalFailure(
          error?.isNotEmpty == true
              ? error!
              : message.isNotEmpty
                  ? message
                  : 'Bluetooth pairing failed.',
          failureStage: 'terminal_event',
        );
        return;
      }

      setState(() {
        _latestProgress = event;
        _lastActiveStage = _stageIndex(event.stage);
      });
    });
  }

  int _stageIndex(RhythmPairingStage stage) => switch (stage) {
        RhythmPairingStage.requested ||
        RhythmPairingStage.hubConnecting ||
        RhythmPairingStage.searching =>
          0,
        RhythmPairingStage.connecting || RhythmPairingStage.commissioning => 1,
        RhythmPairingStage.finalizing => 2,
        RhythmPairingStage.complete => _stages.length,
        RhythmPairingStage.failed => _lastActiveStage,
      };

  Future<Map<String, dynamic>?> _pair({
    required Map<String, dynamic> params,
    required String sessionId,
  }) {
    final injected = widget.pairingRequest;
    if (injected != null) {
      return injected(
        hubType: 'hue_ble',
        params: params,
        receiveTimeout: _pairingTimeout,
        sessionId: sessionId,
      );
    }
    return context.read<ServerSyncProvider>().api.pairDevice(
          hubType: 'hue_ble',
          params: params,
          receiveTimeout: _pairingTimeout,
          sessionId: sessionId,
        );
  }

  Future<void> _startPairing({
    bool replaceStaleBonds = false,
    String? candidateAddress,
  }) async {
    if (_requestInFlight || _flowCompleted) return;

    _requestInFlight = true;
    _attemptNumber += 1;
    _activeInputMethod =
        replaceStaleBonds ? 'stale_bond_recovery' : widget.inputMethod;
    final requestGeneration = ++_requestGeneration;
    _activeSessionId = _attemptNumber == 1
        ? _journeyId
        : '$_journeyId-attempt-$_attemptNumber';
    _subscribeToProgress();
    setState(() {
      _failed = false;
      _error = null;
      _latestProgress = null;
      _lastActiveStage = 0;
    });
    HapticFeedback.mediumImpact();
    AnalyticsService().logHueBlePairingAttempted(
      journeyId: _journeyId,
      source: widget.analyticsSource,
      inputMethod: _activeInputMethod,
      attemptNumber: _attemptNumber,
    );

    try {
      final response = await _pair(
        params: replaceStaleBonds
            ? <String, dynamic>{
                'replace_stale_bonds': true,
                if (candidateAddress != null)
                  'candidate_address': candidateAddress,
              }
            : const <String, dynamic>{},
        sessionId: _activeSessionId,
      );
      if (!mounted ||
          requestGeneration != _requestGeneration ||
          _flowCompleted) {
        return;
      }
      if (response == null) {
        _showError(
          'The Rhythm Box did not answer the pairing request.',
          failureStage: 'empty_response',
        );
        return;
      }

      final httpStatus = response['http_status'] as int?;
      if (httpStatus != null && httpStatus != 200) {
        _showError(
          _responseError(response) ??
              'The Rhythm Box rejected the pairing request.',
          failureStage: 'server_rejected',
        );
        return;
      }

      final status = response['status'] as String?;
      if (status == 'failed') {
        _showError(
          _responseError(response) ?? 'Bluetooth pairing failed.',
          failureStage: 'pairing',
        );
        return;
      }

      final devices = _pairedDevicesFromResponse(response);
      if (status != 'complete' || devices.isEmpty) {
        _showError(
          _responseError(response) ??
              'No eligible Hue Bluetooth bulbs were found.',
          failureStage: 'unexpected_response',
        );
        return;
      }

      _completeSuccessfully(
        devices,
        warnings: _responseWarnings(response),
      );
    } catch (error) {
      if (!mounted ||
          requestGeneration != _requestGeneration ||
          _flowCompleted) {
        return;
      }
      _showError(
        'Could not reach the Rhythm Box. Check its connection and try again.',
        failureStage: 'request_exception',
      );
    } finally {
      if (mounted &&
          requestGeneration == _requestGeneration &&
          !_flowCompleted) {
        setState(() => _requestInFlight = false);
      } else if (!mounted) {
        _requestInFlight = false;
      }
    }
  }

  void _completeSuccessfully(
    List<HueBlePairedDevice> devices, {
    List<String> warnings = const [],
  }) {
    if (!mounted || _flowCompleted) return;
    _flowCompleted = true;
    _requestInFlight = false;
    _requestGeneration += 1;
    AnalyticsService().logHueBlePairingCompleted(
      journeyId: _journeyId,
      source: widget.analyticsSource,
      inputMethod: _activeInputMethod,
      attemptNumber: _attemptNumber,
      outcome: 'succeeded',
    );
    HapticFeedback.heavyImpact();
    Navigator.of(context).pop(
      HueBleDevicePairingResult(devices: devices, warnings: warnings),
    );
  }

  void _handleTerminalFailure(
    String message, {
    required String failureStage,
  }) {
    if (!mounted || _flowCompleted || !_requestInFlight) return;
    _requestGeneration += 1;
    _requestInFlight = false;
    _showError(message, failureStage: failureStage);
  }

  void _showError(String message, {required String failureStage}) {
    AnalyticsService().logHueBlePairingCompleted(
      journeyId: _journeyId,
      source: widget.analyticsSource,
      inputMethod: _activeInputMethod,
      attemptNumber: _attemptNumber,
      outcome: 'failed',
      failureStage: failureStage,
    );
    HapticFeedback.lightImpact();
    if (!mounted) return;
    setState(() {
      _failed = true;
      _error = message;
    });
  }

  Future<void> _confirmStaleBondRecovery() async {
    if (_requestInFlight || _flowCompleted) return;

    final recoverySessionId =
        '$_journeyId-recovery-candidates-${DateTime.now().microsecondsSinceEpoch.toRadixString(36)}';
    setState(() => _requestInFlight = true);
    // The prior scan has already returned, but a delayed terminal SSE event
    // can still be queued. Change correlation before cancelling so that event
    // cannot complete or pop the auxiliary candidate-selection phase.
    _requestGeneration += 1;
    _activeSessionId = recoverySessionId;
    final previousProgressSubscription = _progressSubscription;
    _progressSubscription = null;
    if (previousProgressSubscription != null) {
      unawaited(previousProgressSubscription.cancel());
    }

    late List<HueBleStaleBondCandidate> candidates;
    try {
      final response = await _pair(
        params: const <String, dynamic>{
          'list_stale_bond_candidates': true,
        },
        sessionId: recoverySessionId,
      );
      if (!mounted || _flowCompleted) return;
      if (response == null) {
        _showError(
          'The Rhythm Box did not answer the recovery check.',
          failureStage: 'stale_bond_candidates_empty_response',
        );
        return;
      }
      final httpStatus = response['http_status'] as int?;
      if (httpStatus != null && httpStatus != 200) {
        _showError(
          _responseError(response) ??
              'The Rhythm Box could not inspect stale Bluetooth bonds.',
          failureStage: 'stale_bond_candidates_rejected',
        );
        return;
      }
      candidates = _staleBondCandidatesFromResponse(response);
      if (candidates.isEmpty) {
        _showError(
          'No recoverable stale Hue Bluetooth bond is stored on this Rhythm '
          'Box. Try a normal scan after factory-resetting the bulb.',
          failureStage: 'stale_bond_candidates_none',
        );
        return;
      }
    } catch (_) {
      if (!mounted || _flowCompleted) return;
      _showError(
        'Could not reach the Rhythm Box to inspect stale Bluetooth bonds.',
        failureStage: 'stale_bond_candidates_exception',
      );
      return;
    } finally {
      if (mounted && !_flowCompleted) {
        setState(() => _requestInFlight = false);
      } else if (!mounted) {
        _requestInFlight = false;
      }
    }

    HueBleStaleBondCandidate? selected;
    if (candidates.length == 1) {
      selected = candidates.single;
    } else {
      selected = await showDialog<HueBleStaleBondCandidate>(
        context: context,
        builder: (dialogContext) => SimpleDialog(
          title: const Text('Choose the reset Hue bulb'),
          children: [
            for (final candidate in candidates)
              SimpleDialogOption(
                key: ValueKey(
                  'hue-ble-stale-candidate-${candidate.address}',
                ),
                onPressed: () => Navigator.of(dialogContext).pop(candidate),
                child: ListTile(
                  contentPadding: EdgeInsets.zero,
                  leading: const Icon(Icons.lightbulb_outline_rounded),
                  title: Text(candidate.label),
                  subtitle: Text('Bluetooth ID …${candidate.addressSuffix}'),
                ),
              ),
          ],
        ),
      );
    }
    if (selected == null || !mounted) return;
    final selectedCandidate = selected;

    final confirmed = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: const Text('Confirm this bulb was reset'),
        content: Text(
          'Confirm that ${selectedCandidate.label} (Bluetooth ID '
          '…${selectedCandidate.addressSuffix}) has been physically '
          'factory-reset. Rhythm will remove only this bulb’s stale local '
          'Bluetooth key, then scan for it again.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            key: const ValueKey('hue-ble-confirm-stale-bond-recovery'),
            onPressed: () => Navigator.of(dialogContext).pop(true),
            child: const Text('Recover Bulb'),
          ),
        ],
      ),
    );

    if (confirmed == true && mounted) {
      await _startPairing(
        replaceStaleBonds: true,
        candidateAddress: selectedCandidate.address,
      );
    }
  }

  String? _responseError(Map<String, dynamic> response) {
    final error = response['error'];
    if (error is String && error.trim().isNotEmpty) return error.trim();
    if (error is Map) {
      final message = error['message'];
      if (message is String && message.trim().isNotEmpty) {
        return message.trim();
      }
    }
    return null;
  }

  List<String> _responseWarnings(Map<String, dynamic> response) {
    return (response['warnings'] as List? ?? const [])
        .whereType<String>()
        .map((warning) => warning.trim())
        .where((warning) => warning.isNotEmpty)
        .toList(growable: false);
  }

  Map<String, dynamic>? _jsonMap(Object? value) {
    if (value is Map<String, dynamic>) return value;
    if (value is Map) return value.cast<String, dynamic>();
    return null;
  }

  List<HueBleStaleBondCandidate> _staleBondCandidatesFromResponse(
    Map<String, dynamic> response,
  ) {
    final details = _jsonMap(response['details']);
    final rawCandidates = details?['recovery_candidates'];
    if (rawCandidates is! List) return const [];
    final candidatesByAddress = <String, HueBleStaleBondCandidate>{};
    for (final rawCandidate in rawCandidates) {
      final candidate = _jsonMap(rawCandidate);
      if (candidate == null) continue;
      final address = candidate['candidate_address']?.toString().trim() ?? '';
      if (address.isEmpty) continue;
      final name = candidate['name']?.toString().trim() ?? '';
      candidatesByAddress[address.toLowerCase()] = HueBleStaleBondCandidate(
        address: address,
        name: name.isEmpty ? 'Hue Bluetooth bulb' : name,
        nativeDeviceId: candidate['device_id']?.toString(),
        model: candidate['model']?.toString(),
      );
    }
    final candidates = candidatesByAddress.values.toList(growable: false);
    candidates.sort(
      (left, right) => left.label.toLowerCase().compareTo(
            right.label.toLowerCase(),
          ),
    );
    return candidates;
  }

  List<HueBlePairedDevice> _pairedDevicesFromResponse(
    Map<String, dynamic> response,
  ) {
    final responseDevices = response['devices'];
    final rawDevices = responseDevices is List
        ? responseDevices.cast<Object?>()
        : <Object?>[response['device']];
    final devicesById = <String, HueBlePairedDevice>{};
    for (final rawDevice in rawDevices) {
      final device = _jsonMap(rawDevice);
      if (device == null) continue;
      final nativeDeviceId =
          (device['device_id'] ?? device['native_id'] ?? device['id'])
                  ?.toString()
                  .trim() ??
              '';
      if (nativeDeviceId.isEmpty) continue;
      devicesById[nativeDeviceId] = HueBlePairedDevice(
        nativeDeviceId: nativeDeviceId,
        name: device['name'] as String? ?? 'Hue light',
        deviceType: device['device_type'] as String? ?? 'light',
        manufacturer: device['manufacturer'] as String?,
        model: device['model'] as String?,
      );
    }
    return devicesById.values.toList(growable: false);
  }

  List<HueBlePairedDevice> _pairedDevicesFromProgress(
    RhythmPairingProgress progress,
  ) {
    final devicesById = <String, HueBlePairedDevice>{};
    for (final device in progress.completedDevices) {
      final nativeDeviceId = device.deviceId.trim();
      if (nativeDeviceId.isEmpty) continue;
      devicesById[nativeDeviceId] = HueBlePairedDevice(
        nativeDeviceId: nativeDeviceId,
        name: device.name,
        deviceType: device.deviceType,
        manufacturer: device.manufacturer,
        model: device.model,
      );
    }
    return devicesById.values.toList(growable: false);
  }

  @override
  Widget build(BuildContext context) {
    final stage = _latestProgress == null
        ? _lastActiveStage
        : _stageIndex(_latestProgress!.stage);
    final message = _failed
        ? _error
        : (_latestProgress?.message.trim().isNotEmpty == true
            ? _latestProgress!.message
            : 'Keep the bulbs powered on and close to your Rhythm Box.');

    return PopScope(
      canPop: !_requestInFlight,
      onPopInvokedWithResult: (didPop, _) {
        if (didPop || !_requestInFlight) return;
        final messenger = ScaffoldMessenger.of(context);
        messenger
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
        key: const ValueKey('hue-ble-pairing-screen'),
        backgroundColor: CelestialColors.backgroundDark,
        appBar: AppBar(
          backgroundColor: CelestialColors.backgroundDark,
          foregroundColor: CelestialColors.textPrimary,
          title: const Text('Add Hue Bluetooth Bulbs'),
          elevation: 0,
        ),
        body: SafeArea(
          top: false,
          child: ListView(
            padding: const EdgeInsets.fromLTRB(24, 16, 24, 28),
            children: [
              Align(
                alignment: Alignment.centerLeft,
                child: Container(
                  width: 72,
                  height: 72,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: _hueGold.withValues(alpha: 0.14),
                    border: Border.all(color: _hueGold.withValues(alpha: 0.42)),
                  ),
                  child: const Icon(
                    Icons.lightbulb_rounded,
                    color: _hueGold,
                    size: 36,
                  ),
                ),
              ),
              const SizedBox(height: 20),
              const Text(
                'Adding nearby Hue Bluetooth bulbs',
                style: TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 25,
                  height: 1.15,
                  fontWeight: FontWeight.w700,
                  letterSpacing: -0.4,
                ),
              ),
              const SizedBox(height: 9),
              Text(
                'Power on each factory-reset bulb near your Rhythm Box. Rhythm '
                'will securely add every eligible unpaired Hue Bluetooth bulb '
                'it finds.',
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.84),
                  fontSize: 14,
                  height: 1.45,
                ),
              ),
              const SizedBox(height: 26),
              Container(
                padding: const EdgeInsets.all(18),
                decoration: BoxDecoration(
                  color: CelestialColors.backgroundCard,
                  borderRadius: BorderRadius.circular(18),
                  border: Border.all(
                    color:
                        (_failed ? _danger : _hueGold).withValues(alpha: 0.34),
                  ),
                ),
                child: StageTimeline(
                  stages: _stages,
                  activeIndex: stage,
                  activeMessage: message,
                  failed: _failed,
                  accent: _hueGold,
                ),
              ),
              if (_failed && _error != null) ...[
                const SizedBox(height: 12),
                Text(
                  _error!,
                  key: const ValueKey('hue-ble-pairing-error'),
                  style: const TextStyle(
                    color: _danger,
                    fontSize: 13,
                    height: 1.4,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ],
              if (_failed) ...[
                const SizedBox(height: 18),
                SizedBox(
                  height: 52,
                  child: FilledButton.icon(
                    key: const ValueKey('hue-ble-retry'),
                    onPressed: _requestInFlight ? null : _startPairing,
                    style: FilledButton.styleFrom(
                      backgroundColor: _hueGold,
                      foregroundColor: const Color(0xFF302500),
                    ),
                    icon: const Icon(Icons.refresh_rounded),
                    label: const Text(
                      'Try Again',
                      style: TextStyle(fontWeight: FontWeight.w700),
                    ),
                  ),
                ),
                const SizedBox(height: 10),
                SizedBox(
                  height: 48,
                  child: OutlinedButton.icon(
                    key: const ValueKey('hue-ble-stale-bond-recovery'),
                    onPressed:
                        _requestInFlight ? null : _confirmStaleBondRecovery,
                    style: OutlinedButton.styleFrom(
                      foregroundColor: CelestialColors.textPrimary,
                      side: BorderSide(
                        color: CelestialColors.textSecondary.withValues(
                          alpha: 0.42,
                        ),
                      ),
                    ),
                    icon: const Icon(Icons.settings_backup_restore_rounded),
                    label: const Text(
                      'Recover Physically Reset Bulb',
                      style: TextStyle(fontWeight: FontWeight.w600),
                    ),
                  ),
                ),
                const SizedBox(height: 8),
                Text(
                  'Recovery shows the stale bonds stored on this Rhythm Box '
                  'and removes only the bulb you select.',
                  textAlign: TextAlign.center,
                  style: TextStyle(
                    color:
                        CelestialColors.textSecondary.withValues(alpha: 0.72),
                    fontSize: 12,
                    height: 1.35,
                  ),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}
