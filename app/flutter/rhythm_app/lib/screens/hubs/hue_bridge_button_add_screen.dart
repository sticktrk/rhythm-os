import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:uuid/uuid.dart';

import '../../providers/server_sync_provider.dart';
import '../../services/analytics_service.dart';
import '../../widgets/solar_orbit.dart';
import '../../widgets/stage_timeline.dart';
import 'hue_bridge_light_add_screen.dart' show HueBridgePairingRequest;

class HueBridgeButtonAddResult {
  const HueBridgeButtonAddResult({required this.devices});

  final List<RhythmPairedDevice> devices;
}

/// Runs the connected Hue Bridge's native Zigbee accessory search and accepts
/// only exact V2 button-device projections returned by the appliance.
class HueBridgeButtonAddScreen extends StatefulWidget {
  const HueBridgeButtonAddScreen({
    super.key,
    required this.hubAddress,
    this.analyticsSource = 'hue_bridge_hub_detail',
    this.journeyId,
    @visibleForTesting this.pairingRequest,
    @visibleForTesting this.progressEvents,
  });

  final String hubAddress;
  final String analyticsSource;
  final String? journeyId;
  final HueBridgePairingRequest? pairingRequest;
  final Stream<RhythmPairingProgress>? progressEvents;

  static Future<HueBridgeButtonAddResult?> show(
    BuildContext context, {
    required String hubAddress,
    String analyticsSource = 'hue_bridge_hub_detail',
  }) {
    return Navigator.of(context).push<HueBridgeButtonAddResult>(
      MaterialPageRoute(
        builder: (_) => HueBridgeButtonAddScreen(
          hubAddress: hubAddress,
          analyticsSource: analyticsSource,
        ),
      ),
    );
  }

  @override
  State<HueBridgeButtonAddScreen> createState() =>
      _HueBridgeButtonAddScreenState();
}

class _HueBridgeButtonAddScreenState extends State<HueBridgeButtonAddScreen> {
  static const _hueGold = Color(0xFFFFB900);
  static const _danger = Color(0xFFEF5350);
  static const _timeout = Duration(minutes: 2);
  static const _stages = [
    StageTimelineItem(
      label: 'Ask the Hue Bridge to search',
      icon: Icons.hub_rounded,
    ),
    StageTimelineItem(
      label: 'Find the button or switch',
      icon: Icons.sensors_rounded,
    ),
    StageTimelineItem(
      label: 'Sync the input to Rhythm',
      icon: Icons.toggle_on_rounded,
    ),
  ];

  late final String _journeyId;
  StreamSubscription<RhythmPairingProgress>? _progressSubscription;
  RhythmPairingProgress? _latestProgress;
  bool _started = false;
  bool _requestInFlight = false;
  bool _flowCompleted = false;
  bool _failed = false;
  String? _error;
  String _activeSessionId = '';
  int _attemptNumber = 0;
  int _requestGeneration = 0;
  int _lastActiveStage = 0;

  @override
  void initState() {
    super.initState();
    _journeyId = widget.journeyId ?? 'hue-bridge-button-${const Uuid().v4()}';
    unawaited(AnalyticsService().logScreenView('hue_bridge_button_add'));
  }

  @override
  void dispose() {
    _progressSubscription?.cancel();
    super.dispose();
  }

  Stream<RhythmPairingProgress>? get _pairingProgressStream {
    return widget.progressEvents ??
        context.read<ServerSyncProvider?>()?.connection.pairingProgressEvents;
  }

  void _subscribeToProgress() {
    _progressSubscription?.cancel();
    _progressSubscription = _pairingProgressStream?.listen((event) {
      if (!mounted || _flowCompleted || event.hubType != 'hue') return;
      if (event.sessionId != _activeSessionId) return;
      if (event.status == RhythmPairingStatus.complete ||
          event.stage == RhythmPairingStage.complete) {
        final devices = event.completedDevices
            .where((device) => device.deviceType == 'button')
            .toList(growable: false);
        if (devices.isEmpty) {
          _failAttempt(
            'The Hue Bridge finished searching, but no button or switch was confirmed.',
            failureStage: 'canonical_projection',
          );
        } else {
          _completeSuccessfully(devices);
        }
        return;
      }
      if (event.status == RhythmPairingStatus.failed ||
          event.stage == RhythmPairingStage.failed) {
        _failAttempt(
          event.error?.trim().isNotEmpty == true
              ? event.error!.trim()
              : event.message,
          failureStage: event.failureStage ?? 'bridge_search',
        );
        return;
      }
      setState(() {
        _latestProgress = event;
        _lastActiveStage = _stageIndex(event.stage);
      });
    });
  }

  Future<Map<String, dynamic>?> _pair() {
    final params = <String, dynamic>{
      'device_kind': 'button',
      'correlation_id': _journeyId,
      if (widget.hubAddress.trim().isNotEmpty)
        'hub_address': widget.hubAddress.trim(),
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
    _attemptNumber += 1;
    final generation = ++_requestGeneration;
    _activeSessionId = '$_journeyId-attempt-$_attemptNumber';
    _subscribeToProgress();
    setState(() {
      _started = true;
      _requestInFlight = true;
      _failed = false;
      _error = null;
      _latestProgress = null;
      _lastActiveStage = 0;
    });
    HapticFeedback.mediumImpact();
    unawaited(
      AnalyticsService().logHueBridgeButtonPairingAttempted(
        journeyId: _journeyId,
        source: widget.analyticsSource,
        attemptNumber: _attemptNumber,
      ),
    );

    try {
      final response = await _pair();
      if (!mounted || generation != _requestGeneration || _flowCompleted) {
        return;
      }
      if (response == null) {
        _failAttempt(
          'The Rhythm Box did not answer the Hue Bridge search.',
          failureStage: 'server_unavailable',
        );
        return;
      }
      if (response['status'] != 'complete') {
        _failAttempt(
          _responseError(response) ??
              'The Hue Bridge did not find a new button or switch.',
          failureStage:
              response['failure_stage']?.toString() ?? 'bridge_search',
        );
        return;
      }
      final devices = _pairedButtonsFromResponse(response);
      if (devices.isEmpty) {
        _failAttempt(
          'The Hue Bridge finished searching, but no button or switch was confirmed.',
          failureStage: 'canonical_projection',
        );
        return;
      }
      _completeSuccessfully(devices);
    } catch (_) {
      if (!mounted || generation != _requestGeneration || _flowCompleted) {
        return;
      }
      _failAttempt(
        'Could not reach the Hue Bridge. Check its connection and try again.',
        failureStage: 'bridge_unreachable',
      );
    } finally {
      if (mounted && generation == _requestGeneration && !_flowCompleted) {
        setState(() => _requestInFlight = false);
      }
    }
  }

  List<RhythmPairedDevice> _pairedButtonsFromResponse(
    Map<String, dynamic> response,
  ) {
    final rawDevices = response['devices'] is List
        ? response['devices'] as List
        : <Object?>[response['device']];
    final byId = <String, RhythmPairedDevice>{};
    for (final raw in rawDevices) {
      if (raw is! Map) continue;
      final device = RhythmPairedDevice.fromJson(raw.cast<String, dynamic>());
      if (device.deviceId.trim().isEmpty || device.deviceType != 'button') {
        continue;
      }
      byId[device.deviceId] = device;
    }
    return byId.values.toList(growable: false);
  }

  void _completeSuccessfully(List<RhythmPairedDevice> devices) {
    if (!mounted || _flowCompleted) return;
    _flowCompleted = true;
    _requestGeneration += 1;
    _requestInFlight = false;
    unawaited(
      AnalyticsService().logHueBridgeButtonPairingCompleted(
        journeyId: _journeyId,
        source: widget.analyticsSource,
        attemptNumber: _attemptNumber,
        outcome: 'succeeded',
      ),
    );
    HapticFeedback.heavyImpact();
    Navigator.of(context).pop(HueBridgeButtonAddResult(devices: devices));
  }

  void _failAttempt(String message, {required String failureStage}) {
    if (!mounted || _flowCompleted || !_requestInFlight) return;
    _requestGeneration += 1;
    _requestInFlight = false;
    unawaited(
      AnalyticsService().logHueBridgeButtonPairingCompleted(
        journeyId: _journeyId,
        source: widget.analyticsSource,
        attemptNumber: _attemptNumber,
        outcome: 'failed',
        failureStage: failureStage,
      ),
    );
    HapticFeedback.lightImpact();
    setState(() {
      _failed = true;
      _error = message;
    });
  }

  String? _responseError(Map<String, dynamic> response) {
    final error = response['error'];
    if (error is String && error.trim().isNotEmpty) return error.trim();
    if (error is Map && error['message'] is String) {
      final message = (error['message'] as String).trim();
      if (message.isNotEmpty) return message;
    }
    return null;
  }

  int _stageIndex(RhythmPairingStage stage) => switch (stage) {
        RhythmPairingStage.requested || RhythmPairingStage.hubConnecting => 0,
        RhythmPairingStage.searching || RhythmPairingStage.connecting => 1,
        RhythmPairingStage.commissioning || RhythmPairingStage.finalizing => 2,
        RhythmPairingStage.complete => _stages.length,
        RhythmPairingStage.failed => _lastActiveStage,
      };

  @override
  Widget build(BuildContext context) {
    final stage = _latestProgress == null
        ? _lastActiveStage
        : _stageIndex(_latestProgress!.stage);
    final message = _failed
        ? _error
        : (_latestProgress?.message.trim().isNotEmpty == true
            ? _latestProgress!.message
            : 'Keep the accessory in pairing mode while the Bridge searches.');
    return PopScope(
      canPop: !_requestInFlight,
      child: Scaffold(
        key: const ValueKey('hue-bridge-button-add-screen'),
        backgroundColor: CelestialColors.backgroundDark,
        appBar: AppBar(
          backgroundColor: CelestialColors.backgroundDark,
          foregroundColor: CelestialColors.textPrimary,
          title: const Text('Pair Hue Button or Switch'),
          elevation: 0,
        ),
        body: SafeArea(
          top: false,
          child: ListView(
            padding: const EdgeInsets.fromLTRB(24, 16, 24, 28),
            children: [
              const Icon(Icons.toggle_on_rounded, color: _hueGold, size: 62),
              const SizedBox(height: 16),
              const Text(
                'Put the accessory in pairing mode',
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 24,
                  height: 1.15,
                  fontWeight: FontWeight.w700,
                ),
              ),
              const SizedBox(height: 10),
              Text(
                'Use the setup button or pairing sequence from the accessory '
                'instructions, then start the Bridge search. Keep the button, '
                'remote, or wall switch near the Hue Bridge until it finishes.',
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.84),
                  fontSize: 14,
                  height: 1.45,
                ),
              ),
              const SizedBox(height: 24),
              if (_started)
                Container(
                  padding: const EdgeInsets.all(18),
                  decoration: BoxDecoration(
                    color: CelestialColors.backgroundCard,
                    borderRadius: BorderRadius.circular(18),
                    border: Border.all(
                      color: (_failed ? _danger : _hueGold)
                          .withValues(alpha: 0.34),
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
                  key: const ValueKey('hue-bridge-button-add-error'),
                  style: const TextStyle(
                    color: _danger,
                    fontSize: 13,
                    height: 1.4,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ],
              const SizedBox(height: 20),
              SizedBox(
                height: 52,
                child: FilledButton.icon(
                  key: const ValueKey('hue-bridge-button-search'),
                  onPressed: _requestInFlight ? null : _startSearch,
                  style: FilledButton.styleFrom(
                    backgroundColor: _hueGold,
                    foregroundColor: const Color(0xFF302500),
                  ),
                  icon: Icon(
                    _failed ? Icons.refresh_rounded : Icons.sensors_rounded,
                  ),
                  label: Text(
                    _failed ? 'Try Again' : 'Start Bridge Search',
                    style: const TextStyle(fontWeight: FontWeight.w700),
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
