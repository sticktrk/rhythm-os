import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../providers/server_sync_provider.dart';
import '../../services/analytics_service.dart';
import '../../services/device_pairing_code.dart';
import '../../widgets/solar_orbit.dart';
import '../../widgets/stage_timeline.dart';

class HueBridgeLightAddResult {
  const HueBridgeLightAddResult({
    this.addedCount,
    this.devices = const [],
    this.warnings = const [],
  });

  final int? addedCount;
  final List<RhythmPairedDevice> devices;
  final List<String> warnings;
}

typedef HueBridgePairingRequest = Future<Map<String, dynamic>?> Function({
  required String hubType,
  required Map<String, dynamic> params,
  required Duration receiveTimeout,
  required String sessionId,
});

/// Searches a connected Hue Bridge for the Zigbee bulb matching [serial].
class HueBridgeLightAddScreen extends StatefulWidget {
  const HueBridgeLightAddScreen({
    super.key,
    required this.serial,
    this.analyticsSource = 'unknown',
    this.journeyId,
    this.hubAddress,
    @visibleForTesting this.pairingRequest,
    @visibleForTesting this.progressEvents,
  });

  final String serial;
  final String analyticsSource;
  final String? journeyId;
  final String? hubAddress;
  final HueBridgePairingRequest? pairingRequest;
  final Stream<RhythmPairingProgress>? progressEvents;

  static Future<HueBridgeLightAddResult?> show(
    BuildContext context, {
    required String serial,
    String analyticsSource = 'unknown',
    String? journeyId,
    String? hubAddress,
  }) {
    return Navigator.of(context).push<HueBridgeLightAddResult>(
      MaterialPageRoute(
        builder: (_) => HueBridgeLightAddScreen(
          serial: serial,
          analyticsSource: analyticsSource,
          journeyId: journeyId,
          hubAddress: hubAddress,
        ),
      ),
    );
  }

  @override
  State<HueBridgeLightAddScreen> createState() =>
      _HueBridgeLightAddScreenState();
}

class _HueBridgeLightAddScreenState extends State<HueBridgeLightAddScreen> {
  static const _hueGold = Color(0xFFFFB900);
  static const _danger = Color(0xFFEF5350);
  static const _timeout = Duration(minutes: 2);
  static const _stages = [
    StageTimelineItem(
      label: 'Ask the Hue Bridge to search',
      icon: Icons.hub_rounded,
    ),
    StageTimelineItem(
      label: 'Find the Zigbee bulb',
      icon: Icons.radar_rounded,
    ),
    StageTimelineItem(
      label: 'Sync the light to Rhythm',
      icon: Icons.lightbulb_rounded,
    ),
  ];

  late final String _serial;
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

  @override
  void initState() {
    super.initState();
    _serial = normalizeHueBridgeSerial(widget.serial) ?? '';
    _journeyId = widget.journeyId ??
        'hue-bridge-add-${DateTime.now().microsecondsSinceEpoch.toRadixString(36)}';
    AnalyticsService().logScreenView('hue_bridge_light_add');
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _startSearch();
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
    _progressSubscription = _pairingProgressStream?.listen((event) {
      if (!mounted || event.hubType != 'hue') return;
      if (event.sessionId != _activeSessionId || _flowCompleted) return;

      if (event.stage == RhythmPairingStage.complete ||
          event.status == RhythmPairingStatus.complete) {
        _completeSuccessfully(
          HueBridgeLightAddResult(
            addedCount: event.completedDevices.isEmpty
                ? null
                : event.completedDevices.length,
            devices: event.completedDevices,
            warnings: event.warnings,
          ),
        );
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
                  : 'The Hue Bridge did not find a bulb with that serial.',
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
        RhythmPairingStage.requested || RhythmPairingStage.hubConnecting => 0,
        RhythmPairingStage.searching ||
        RhythmPairingStage.connecting ||
        RhythmPairingStage.commissioning =>
          1,
        RhythmPairingStage.finalizing => 2,
        RhythmPairingStage.complete => _stages.length,
        RhythmPairingStage.failed => _lastActiveStage,
      };

  Future<Map<String, dynamic>?> _pair() {
    final bridgeAddress = widget.hubAddress?.trim();
    final params = <String, dynamic>{
      'serial': _serial,
      if (bridgeAddress != null && bridgeAddress.isNotEmpty)
        'hub_address': bridgeAddress,
    };
    final injected = widget.pairingRequest;
    if (injected != null) {
      return injected(
        hubType: 'hue',
        params: params,
        receiveTimeout: _timeout,
        sessionId: _activeSessionId,
      );
    }
    return context.read<ServerSyncProvider>().api.pairDevice(
          hubType: 'hue',
          params: params,
          receiveTimeout: _timeout,
          sessionId: _activeSessionId,
        );
  }

  Future<void> _startSearch() async {
    if (_requestInFlight || _flowCompleted) return;
    if (_serial.isEmpty) {
      _showError('The Hue bulb serial is invalid.');
      return;
    }

    _requestInFlight = true;
    _attemptNumber += 1;
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

    try {
      final response = await _pair();
      if (!mounted ||
          requestGeneration != _requestGeneration ||
          _flowCompleted) {
        return;
      }
      if (response == null) {
        _showError('The Rhythm Box did not answer the Hue Bridge search.');
        return;
      }
      final httpStatus = response['http_status'] as int?;
      if (httpStatus != null && httpStatus != 200) {
        _showError(
          _responseError(response) ??
              'The Rhythm Box rejected the Hue Bridge search.',
        );
        return;
      }
      if (response['status'] != 'complete') {
        _showError(
          _responseError(response) ??
              'The Hue Bridge did not find a bulb with that serial.',
        );
        return;
      }

      _completeSuccessfully(
        HueBridgeLightAddResult(
          addedCount: _addedCount(response),
          devices: _pairedDevicesFromResponse(response),
          warnings: _responseWarnings(response),
        ),
      );
    } catch (_) {
      if (!mounted ||
          requestGeneration != _requestGeneration ||
          _flowCompleted) {
        return;
      }
      _showError(
        'Could not reach the Hue Bridge. Check its connection and try again.',
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

  void _completeSuccessfully(HueBridgeLightAddResult result) {
    if (!mounted || _flowCompleted) return;
    _flowCompleted = true;
    _requestInFlight = false;
    _requestGeneration += 1;
    HapticFeedback.heavyImpact();
    Navigator.of(context).pop(result);
  }

  void _handleTerminalFailure(String message) {
    if (!mounted || _flowCompleted || !_requestInFlight) return;
    _requestGeneration += 1;
    _requestInFlight = false;
    _showError(message);
  }

  int? _addedCount(Map<String, dynamic> response) {
    final explicit = response['added_count'];
    if (explicit is int) return explicit;
    final devices = response['devices'];
    if (devices is List && devices.isNotEmpty) return devices.length;
    return response['device'] is Map ? 1 : null;
  }

  List<RhythmPairedDevice> _pairedDevicesFromResponse(
    Map<String, dynamic> response,
  ) {
    final responseDevices = response['devices'];
    final rawDevices = responseDevices is List
        ? responseDevices.cast<Object?>()
        : <Object?>[response['device']];
    final devicesById = <String, RhythmPairedDevice>{};
    for (final rawDevice in rawDevices) {
      if (rawDevice is! Map) continue;
      final json = rawDevice.cast<String, dynamic>();
      final device = RhythmPairedDevice.fromJson(json);
      if (device.deviceId.trim().isEmpty) continue;
      devicesById[device.deviceId] = device;
    }
    return devicesById.values.toList(growable: false);
  }

  List<String> _responseWarnings(Map<String, dynamic> response) {
    return (response['warnings'] as List? ?? const [])
        .whereType<String>()
        .map((warning) => warning.trim())
        .where((warning) => warning.isNotEmpty)
        .toList(growable: false);
  }

  void _showError(String message) {
    HapticFeedback.lightImpact();
    if (!mounted) return;
    setState(() {
      _failed = true;
      _error = message;
    });
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

  @override
  Widget build(BuildContext context) {
    final stage = _latestProgress == null
        ? _lastActiveStage
        : _stageIndex(_latestProgress!.stage);
    final message = _failed
        ? _error
        : (_latestProgress?.message.trim().isNotEmpty == true
            ? _latestProgress!.message
            : 'Keep the bulb powered on while the Hue Bridge searches.');

    return PopScope(
      canPop: !_requestInFlight,
      onPopInvokedWithResult: (didPop, _) {
        if (didPop || !_requestInFlight) return;
        ScaffoldMessenger.of(context)
          ..hideCurrentSnackBar()
          ..showSnackBar(
            const SnackBar(
              content: Text(
                'The Hue Bridge is still searching. Keep this screen open '
                'until it finishes.',
              ),
            ),
          );
      },
      child: Scaffold(
        key: const ValueKey('hue-bridge-light-add-screen'),
        backgroundColor: CelestialColors.backgroundDark,
        appBar: AppBar(
          backgroundColor: CelestialColors.backgroundDark,
          foregroundColor: CelestialColors.textPrimary,
          title: const Text('Search with Hue Bridge'),
          elevation: 0,
        ),
        body: SafeArea(
          top: false,
          child: ListView(
            padding: const EdgeInsets.fromLTRB(24, 16, 24, 28),
            children: [
              const Icon(
                Icons.hub_rounded,
                color: _hueGold,
                size: 58,
              ),
              const SizedBox(height: 18),
              const Text(
                'Adding this bulb through your Hue Bridge',
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 24,
                  height: 1.15,
                  fontWeight: FontWeight.w700,
                ),
              ),
              const SizedBox(height: 8),
              Text(
                'The six-character label serial tells the bridge which Zigbee '
                'bulb to search for. It is not used for direct Bluetooth.',
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.84),
                  fontSize: 14,
                  height: 1.45,
                ),
              ),
              const SizedBox(height: 18),
              Container(
                padding:
                    const EdgeInsets.symmetric(horizontal: 14, vertical: 11),
                decoration: BoxDecoration(
                  color: CelestialColors.backgroundCard,
                  borderRadius: BorderRadius.circular(12),
                  border: Border.all(
                    color: CelestialColors.orbitRing.withValues(alpha: 0.55),
                  ),
                ),
                child: Row(
                  children: [
                    Text(
                      'HUE SERIAL',
                      style: TextStyle(
                        color: CelestialColors.textSecondary.withValues(
                          alpha: 0.62,
                        ),
                        fontSize: 10,
                        fontWeight: FontWeight.w700,
                        letterSpacing: 1.2,
                      ),
                    ),
                    const Spacer(),
                    Text(
                      _serial,
                      key: const ValueKey('hue-bridge-serial'),
                      style: const TextStyle(
                        color: _hueGold,
                        fontSize: 16,
                        fontFamily: 'monospace',
                        fontWeight: FontWeight.w700,
                        letterSpacing: 2,
                      ),
                    ),
                  ],
                ),
              ),
              const SizedBox(height: 24),
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
                  key: const ValueKey('hue-bridge-add-error'),
                  style: const TextStyle(
                    color: _danger,
                    fontSize: 13,
                    height: 1.4,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                const SizedBox(height: 18),
                SizedBox(
                  height: 52,
                  child: FilledButton.icon(
                    key: const ValueKey('hue-bridge-add-retry'),
                    onPressed: _requestInFlight ? null : _startSearch,
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
              ],
            ],
          ),
        ),
      ),
    );
  }
}
