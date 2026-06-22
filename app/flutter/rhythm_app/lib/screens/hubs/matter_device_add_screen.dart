import 'dart:async';
import 'dart:math' as math;

import 'package:dio/dio.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart' show HubEndpoint;
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../providers/server_sync_provider.dart';
import '../../services/demo_server_api.dart';
import '../../services/hue/hue_service_locator.dart';
import '../../services/matter_setup_payload.dart';
import '../../widgets/bulb_pairing_instructions.dart';
import '../../widgets/solar_orbit.dart';
import '../../widgets/stage_timeline.dart';
import 'matter_add_method.dart';
import 'matter_qr_scanner_screen.dart';

class MatterDevicePairingResult {
  const MatterDevicePairingResult({
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

enum _PairingPhase { input, pairing, failed }

/// Full-screen modal for pairing a Matter device through Rhythm's backend.
///
/// Design language: "Signal Acquisition Console" — engineering blueprint dark,
/// editorial monospace eyebrows, a QR viewfinder treated as a capture surface,
/// a terminal-style payload console, and a radar-lock visualization while the
/// server is commissioning.
class MatterDeviceAddScreen extends StatefulWidget {
  const MatterDeviceAddScreen({
    super.key,
    required this.endpoint,
    required this.addMethod,
    this.authToken,
    this.dio,
  });

  final HubEndpoint endpoint;
  final MatterAddMethod addMethod;
  final String? authToken;
  final Dio? dio;

  static Future<MatterDevicePairingResult?> show(
    BuildContext context, {
    required HubEndpoint endpoint,
    required MatterAddMethod addMethod,
    String? authToken,
    Dio? dio,
  }) {
    return Navigator.of(context).push(
      PageRouteBuilder(
        opaque: false,
        barrierColor: Colors.black54,
        pageBuilder: (context, animation, secondaryAnimation) {
          return MatterDeviceAddScreen(
            endpoint: endpoint,
            addMethod: addMethod,
            authToken: authToken,
            dio: dio,
          );
        },
        transitionsBuilder: (context, animation, secondaryAnimation, child) {
          final curve = CurvedAnimation(
            parent: animation,
            curve: Curves.easeOutCubic,
            reverseCurve: Curves.easeInCubic,
          );
          return SlideTransition(
            position: Tween<Offset>(
              begin: const Offset(0, 1),
              end: Offset.zero,
            ).animate(curve),
            child: child,
          );
        },
        transitionDuration: const Duration(milliseconds: 350),
        reverseTransitionDuration: const Duration(milliseconds: 300),
      ),
    );
  }

  @override
  State<MatterDeviceAddScreen> createState() => _MatterDeviceAddScreenState();
}

class _MatterDeviceAddScreenState extends State<MatterDeviceAddScreen>
    with TickerProviderStateMixin {
  static const _teal = Color(0xFF00BCD4);
  static const _tealDeep = Color(0xFF00838F);
  static const _amber = CelestialColors.sunWarm;
  static const _danger = Color(0xFFEF5350);

  final _setupPayloadController = TextEditingController();
  late final AnimationController _pulseController;
  late final AnimationController _sweepController;
  late final RhythmMatterApi _pairingApi;
  late final String _sessionId;

  _PairingPhase _phase = _PairingPhase.input;
  String? _errorText;
  bool _hasFailedOnce = false;
  StreamSubscription<RhythmPairingProgress>? _progressSub;
  RhythmPairingProgress? _latestProgress;

  @override
  void initState() {
    super.initState();
    _pairingApi = RhythmMatterApi(
      baseUrl: widget.endpoint.baseUrl,
      dio: widget.dio,
      authToken: widget.authToken,
    );
    _sessionId =
        'matter-pair-${DateTime.now().microsecondsSinceEpoch.toRadixString(36)}';
    _pulseController = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 2000),
    )..repeat(reverse: true);
    _sweepController = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 2400),
    )..repeat();
    _setupPayloadController.addListener(_handlePayloadChanged);
  }

  @override
  void dispose() {
    _setupPayloadController.removeListener(_handlePayloadChanged);
    _setupPayloadController.dispose();
    _pulseController.dispose();
    _sweepController.dispose();
    _progressSub?.cancel();
    super.dispose();
  }

  void _subscribeToPairingProgress() {
    _progressSub?.cancel();
    final connection = context.read<ServerSyncProvider?>()?.connection;
    if (connection == null) return;
    _progressSub = connection.pairingProgressEvents.listen((event) {
      if (!mounted) return;
      if (event.sessionId != null && event.sessionId != _sessionId) return;
      if (event.hubType != 'matter') return;
      setState(() {
        _latestProgress = event;
      });
    });
  }

  void _clearPairingProgress() {
    _progressSub?.cancel();
    _progressSub = null;
    _latestProgress = null;
  }

  bool get _supportsQrScan {
    if (kIsWeb) return false;
    return defaultTargetPlatform == TargetPlatform.android ||
        defaultTargetPlatform == TargetPlatform.iOS ||
        defaultTargetPlatform == TargetPlatform.macOS;
  }

  String get _setupPayload =>
      normalizeMatterSetupPayload(_setupPayloadController.text);

  bool get _hasSetupPayload => _setupPayload.isNotEmpty;

  bool get _looksLikeMatterPayload =>
      isLikelyMatterSetupPayload(_setupPayloadController.text);

  bool get _usesWifiCommissioningPreflight =>
      widget.addMethod.requiresWifiCommissioningPreflight;

  String get _sessionShortId {
    final tail = _sessionId.split('-').last;
    return tail.length > 8 ? tail.substring(tail.length - 8) : tail;
  }

  String get _phaseTag => switch (_phase) {
        _PairingPhase.input => '01 / CAPTURE',
        _PairingPhase.pairing => '02 / TRANSMIT',
        _PairingPhase.failed => '03 / ABORT',
      };

  Color get _phaseAccent => switch (_phase) {
        _PairingPhase.input => _teal,
        _PairingPhase.pairing => _amber,
        _PairingPhase.failed => _danger,
      };

  void _handlePayloadChanged() {
    if (mounted) {
      setState(() {});
    }
  }

  Future<void> _scanQrCode() async {
    final payload = await MatterQrScannerScreen.show(context);
    if (!mounted || payload == null) return;

    _setupPayloadController.text = payload;
    _setupPayloadController.selection = TextSelection.collapsed(
      offset: payload.length,
    );

    await _startPairing();
  }

  Future<void> _startPairing() async {
    final setupPayload = _setupPayload;
    if (setupPayload.isEmpty) return;

    FocusScope.of(context).unfocus();
    HapticFeedback.mediumImpact();
    _subscribeToPairingProgress();
    setState(() {
      _phase = _PairingPhase.pairing;
      _errorText = null;
      _latestProgress = null;
    });

    if (HueServiceLocator.isDemoMode) {
      await _simulateDemoPairing();
      return;
    }

    if (_usesWifiCommissioningPreflight) {
      final wifiStatus = await _pairingApi.getWifiStatus();
      if (!mounted) return;

      if (wifiStatus != null && !wifiStatus.readyForMatterPairing) {
        final details = <String>[
          if (wifiStatus.configPresent != true)
            'Wi-Fi configuration is not present on the server appliance.',
          if (wifiStatus.connected != true)
            'The server appliance is not currently connected to Wi-Fi.',
          'Connect the appliance to Wi-Fi first, then try again.',
          'You do not need to enter network credentials here.',
        ];
        _showPairingError(
          'This Rhythm appliance is not ready to add a new device.',
          detail: details.join(' '),
        );
        return;
      }
    }

    final result = await _pairingApi.pairDevice(
      setupPayload: setupPayload,
      rendezvous: 'auto',
      network: 'wifi',
      receiveTimeout: const Duration(seconds: 45),
      sessionId: _sessionId,
    );

    if (!mounted) return;

    if (result.httpStatus != null && result.httpStatus != 200) {
      _showPairingError(
        'The server rejected the pairing request.',
        detail: result.error,
      );
      return;
    }

    if (result.status == 'failed') {
      _showPairingError('Pairing failed.', detail: result.error);
      return;
    }

    final device = result.device;
    if (result.status == 'complete' && device != null) {
      final nativeDeviceId = device['device_id'] as String? ?? '';
      if (nativeDeviceId.isEmpty) {
        _showPairingError(
          'Pairing completed, but the server returned no device ID.',
        );
        return;
      }

      HapticFeedback.heavyImpact();
      Navigator.of(context).pop(
        MatterDevicePairingResult(
          nativeDeviceId: nativeDeviceId,
          name: device['name'] as String? ?? 'Device',
          deviceType: device['device_type'] as String? ?? 'light',
          manufacturer: device['manufacturer'] as String?,
          model: device['model'] as String?,
        ),
      );
      return;
    }

    _showPairingError(
      'Pairing did not complete.',
      detail:
          result.error ?? 'Unexpected status: ${result.status ?? 'unknown'}',
    );
  }

  Future<void> _simulateDemoPairing() async {
    await Future.delayed(const Duration(milliseconds: 1500));
    if (!mounted) return;

    final nativeId = 'matter-demo-${DateTime.now().millisecondsSinceEpoch}';
    const name = 'Matter Bulb';
    DemoServerApi.instance.addDemoMatterDevice(
      nativeId: nativeId,
      name: name,
    );

    HapticFeedback.heavyImpact();
    Navigator.of(context).pop(
      MatterDevicePairingResult(
        nativeDeviceId: nativeId,
        name: name,
        deviceType: 'light',
        manufacturer: 'Demo Lighting',
        model: 'Matter Bulb',
      ),
    );
  }

  void _showPairingError(String message, {String? detail}) {
    _progressSub?.cancel();
    _progressSub = null;
    setState(() {
      _phase = _PairingPhase.failed;
      _hasFailedOnce = true;
      _errorText =
          detail == null || detail.isEmpty ? message : '$message\n\n$detail';
    });
  }

  void _resetToInput() {
    _clearPairingProgress();
    setState(() {
      _phase = _PairingPhase.input;
      _errorText = null;
    });
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _buildHeader(),
            Expanded(
              child: SingleChildScrollView(
                padding: const EdgeInsets.symmetric(horizontal: 20),
                child: AnimatedSwitcher(
                  duration: const Duration(milliseconds: 280),
                  transitionBuilder: (child, anim) => FadeTransition(
                    opacity: anim,
                    child: child,
                  ),
                  child: KeyedSubtree(
                    key: ValueKey(_phase),
                    child: switch (_phase) {
                      _PairingPhase.input => _buildInputPhase(),
                      _PairingPhase.pairing => _buildPairingPhase(),
                      _PairingPhase.failed => _buildFailurePhase(),
                    },
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  // ──────────────────────────────────────────────────────────────────────────
  // Header
  // ──────────────────────────────────────────────────────────────────────────

  Widget _buildHeader() {
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 10, 20, 16),
      child: Column(
        children: [
          Row(
            crossAxisAlignment: CrossAxisAlignment.center,
            children: [
              _CloseButton(onTap: () => Navigator.of(context).pop()),
              const Spacer(),
              _PhaseTag(text: _phaseTag, accent: _phaseAccent),
            ],
          ),
          const SizedBox(height: 18),
          Row(
            children: [
              _BreathingPip(
                controller: _pulseController,
                color: _phaseAccent,
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Text(
                  widget.addMethod.actionLabel.toUpperCase(),
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 12,
                    fontWeight: FontWeight.w700,
                    letterSpacing: 2.6,
                  ),
                ),
              ),
              Text(
                _sessionShortId,
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.55),
                  fontSize: 10.5,
                  fontFamily: 'monospace',
                  fontWeight: FontWeight.w600,
                  letterSpacing: 1.6,
                ),
              ),
            ],
          ),
          const SizedBox(height: 12),
          const _HairlineRule(),
        ],
      ),
    );
  }

  // ──────────────────────────────────────────────────────────────────────────
  // Phase 1 — input
  // ──────────────────────────────────────────────────────────────────────────

  Widget _buildInputPhase() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const SizedBox(height: 14),
        BulbPairingInstructions(showResetSection: _hasFailedOnce),
        const SizedBox(height: 30),
        const _SectionKicker(
          accent: _teal,
          label: 'SIGNAL CAPTURE',
          counter: '02 / 02',
        ),
        const SizedBox(height: 18),
        if (_supportsQrScan) ...[
          _QrScanButton(
            accent: _teal,
            accentDeep: _tealDeep,
            onTap: _scanQrCode,
          ),
          const SizedBox(height: 18),
          const _OrDivider(label: 'OR ENTER MANUALLY'),
          const SizedBox(height: 18),
        ],
        _PayloadConsole(
          controller: _setupPayloadController,
          accent: _teal,
          warning: _amber,
          helperText: _setupPayloadController.text.isEmpty
              ? 'Paste the setup payload or the code printed on the device.'
              : (_looksLikeMatterPayload
                  ? 'Ready to send to the server.'
                  : 'This does not look like a typical setup code, but you '
                      'can still try adding the device.'),
          isValidLooking: _looksLikeMatterPayload,
        ),
        const SizedBox(height: 28),
        _PrimaryTransmitButton(
          label: widget.addMethod.actionLabel,
          enabled: _hasSetupPayload,
          onTap: _startPairing,
        ),
        const SizedBox(height: 40),
      ],
    );
  }

  // ──────────────────────────────────────────────────────────────────────────
  // Phase 2 — pairing
  // ──────────────────────────────────────────────────────────────────────────

  Widget _buildPairingPhase() {
    final progress = _latestProgress;
    final activeIndex = _matterPairingActiveIndex(progress);
    final activeMessage = _matterPairingActiveMessage(progress);

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const SizedBox(height: 28),
        Center(
          child: _SignalLockHero(
            pulse: _pulseController,
            sweep: _sweepController,
            accent: _teal,
            accentDeep: _tealDeep,
            accentSecondary: _amber,
          ),
        ),
        const SizedBox(height: 32),
        _SectionKicker(
          accent: _amber,
          label: 'TRANSMITTING',
          counter: 'SESSION $_sessionShortId',
        ),
        const SizedBox(height: 14),
        Text(
          'Acquiring device signal',
          style: const TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 20,
            fontWeight: FontWeight.w600,
            letterSpacing: 0.3,
          ),
        ),
        const SizedBox(height: 6),
        Text(
          activeMessage ??
              switch (widget.addMethod) {
                MatterAddMethod.automatic =>
                  'Keep this screen open while Rhythm adds the device.',
                MatterAddMethod.onNetworkSetupCode =>
                  'Keep this screen open while Rhythm finds the device and '
                      'adds it.',
                MatterAddMethod.bleWifiCommissioning =>
                  'Keep this screen open while Rhythm completes setup and '
                      'adds the device.',
              },
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.75),
            fontSize: 14,
            height: 1.5,
          ),
        ),
        const SizedBox(height: 22),
        Container(
          padding: const EdgeInsets.fromLTRB(20, 22, 20, 22),
          decoration: BoxDecoration(
            color: CelestialColors.backgroundCard.withValues(alpha: 0.55),
            borderRadius: BorderRadius.circular(18),
            border: Border.all(
              color: _teal.withValues(alpha: 0.12),
              width: 1,
            ),
          ),
          child: StageTimeline(
            stages: const [
              StageTimelineItem(
                label: 'Sending request',
                icon: Icons.outbox_outlined,
              ),
              StageTimelineItem(
                label: 'Searching for device',
                icon: Icons.radar_outlined,
              ),
              StageTimelineItem(
                label: 'Commissioning',
                icon: Icons.verified_user_outlined,
              ),
              StageTimelineItem(
                label: 'Finalizing',
                icon: Icons.check_circle_outline,
              ),
            ],
            activeIndex: activeIndex,
            activeMessage: progress?.message,
            failed: progress?.stage == RhythmPairingStage.failed,
            accent: _teal,
          ),
        ),
        const SizedBox(height: 20),
        _PayloadEcho(payload: _setupPayload),
        const SizedBox(height: 32),
      ],
    );
  }

  /// Map the server's `PairingStage` onto our 4-step UI timeline.
  ///
  /// 0 = Sending request, 1 = Searching, 2 = Commissioning, 3 = Finalizing.
  /// Returns 4 (== stages.length) when complete.
  int _matterPairingActiveIndex(RhythmPairingProgress? progress) {
    if (progress == null) return 0;
    return switch (progress.stage) {
      RhythmPairingStage.requested => 0,
      RhythmPairingStage.hubConnecting => 1,
      RhythmPairingStage.searching => 1,
      RhythmPairingStage.connecting => 1,
      RhythmPairingStage.commissioning => 2,
      RhythmPairingStage.finalizing => 3,
      RhythmPairingStage.complete => 4,
      RhythmPairingStage.failed => _matterFailureIndex(progress),
    };
  }

  int _matterFailureIndex(RhythmPairingProgress progress) {
    // We don't know which specific stage failed, so attribute the failure to
    // the most likely stage based on whether a device was found.
    return progress.device != null ? 3 : 2;
  }

  String? _matterPairingActiveMessage(RhythmPairingProgress? progress) {
    if (progress == null) return null;
    final trimmed = progress.message.trim();
    if (trimmed.isEmpty) return null;
    return trimmed;
  }

  // ──────────────────────────────────────────────────────────────────────────
  // Phase 3 — failure
  // ──────────────────────────────────────────────────────────────────────────

  Widget _buildFailurePhase() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const SizedBox(height: 36),
        Center(
          child: _FailureGlyph(pulse: _pulseController),
        ),
        const SizedBox(height: 24),
        Center(
          child: Container(
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
            decoration: BoxDecoration(
              color: _danger.withValues(alpha: 0.10),
              borderRadius: BorderRadius.circular(6),
              border: Border.all(
                color: _danger.withValues(alpha: 0.35),
                width: 1,
              ),
            ),
            child: Text(
              'TRANSMISSION FAILED · RETRY AVAILABLE',
              style: TextStyle(
                color: _danger.withValues(alpha: 0.95),
                fontSize: 10.5,
                fontFamily: 'monospace',
                fontWeight: FontWeight.w700,
                letterSpacing: 1.6,
              ),
            ),
          ),
        ),
        const SizedBox(height: 20),
        Container(
          padding: const EdgeInsets.fromLTRB(18, 16, 18, 18),
          decoration: BoxDecoration(
            color: CelestialColors.backgroundCard.withValues(alpha: 0.55),
            borderRadius: BorderRadius.circular(14),
            border: Border.all(
              color: _danger.withValues(alpha: 0.18),
              width: 1,
            ),
          ),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Container(
                margin: const EdgeInsets.only(top: 5, right: 12),
                width: 6,
                height: 6,
                decoration: BoxDecoration(
                  color: _danger.withValues(alpha: 0.85),
                  shape: BoxShape.circle,
                  boxShadow: [
                    BoxShadow(
                      color: _danger.withValues(alpha: 0.5),
                      blurRadius: 6,
                      spreadRadius: 0.5,
                    ),
                  ],
                ),
              ),
              Expanded(
                child: Text(
                  _errorText ?? 'Pairing failed.',
                  style: TextStyle(
                    color: CelestialColors.textPrimary.withValues(alpha: 0.88),
                    fontSize: 13.5,
                    height: 1.55,
                  ),
                ),
              ),
            ],
          ),
        ),
        const SizedBox(height: 28),
        _PrimaryTransmitButton(
          label: 'Try Again',
          enabled: true,
          onTap: _resetToInput,
        ),
        const SizedBox(height: 12),
        _SecondaryGhostButton(
          label: 'Close',
          onTap: () => Navigator.of(context).pop(),
        ),
        const SizedBox(height: 40),
      ],
    );
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Header chrome
// ─────────────────────────────────────────────────────────────────────────────

class _CloseButton extends StatelessWidget {
  const _CloseButton({required this.onTap});
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onTap,
      behavior: HitTestBehavior.opaque,
      child: Container(
        width: 38,
        height: 38,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          color: CelestialColors.backgroundCard.withValues(alpha: 0.7),
          border: Border.all(
            color: CelestialColors.orbitRing.withValues(alpha: 0.75),
            width: 1,
          ),
        ),
        child: Icon(
          Icons.close_rounded,
          color: CelestialColors.textPrimary.withValues(alpha: 0.85),
          size: 18,
        ),
      ),
    );
  }
}

class _PhaseTag extends StatelessWidget {
  const _PhaseTag({required this.text, required this.accent});
  final String text;
  final Color accent;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
      decoration: BoxDecoration(
        color: accent.withValues(alpha: 0.10),
        borderRadius: BorderRadius.circular(6),
        border: Border.all(
          color: accent.withValues(alpha: 0.30),
          width: 1,
        ),
      ),
      child: Text(
        text,
        style: TextStyle(
          color: accent.withValues(alpha: 0.95),
          fontSize: 10.5,
          fontFamily: 'monospace',
          fontWeight: FontWeight.w700,
          letterSpacing: 1.6,
        ),
      ),
    );
  }
}

class _BreathingPip extends StatelessWidget {
  const _BreathingPip({required this.controller, required this.color});
  final AnimationController controller;
  final Color color;

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: controller,
      builder: (context, _) {
        final t = Curves.easeInOut.transform(controller.value);
        return Container(
          width: 8,
          height: 8,
          decoration: BoxDecoration(
            color: color,
            shape: BoxShape.circle,
            boxShadow: [
              BoxShadow(
                color: color.withValues(alpha: 0.4 + t * 0.4),
                blurRadius: 6 + t * 6,
                spreadRadius: t * 1.5,
              ),
            ],
          ),
        );
      },
    );
  }
}

class _HairlineRule extends StatelessWidget {
  const _HairlineRule();

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        Container(
          width: 16,
          height: 1,
          color: CelestialColors.textSecondary.withValues(alpha: 0.35),
        ),
        const SizedBox(width: 6),
        Container(
          width: 3,
          height: 3,
          decoration: BoxDecoration(
            color: CelestialColors.textSecondary.withValues(alpha: 0.45),
            shape: BoxShape.circle,
          ),
        ),
        const SizedBox(width: 6),
        Expanded(
          child: Container(
            height: 1,
            color: CelestialColors.textSecondary.withValues(alpha: 0.18),
          ),
        ),
      ],
    );
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Section kicker (matches the BulbPairingInstructions editorial header)
// ─────────────────────────────────────────────────────────────────────────────

class _SectionKicker extends StatelessWidget {
  const _SectionKicker({
    required this.accent,
    required this.label,
    required this.counter,
  });
  final Color accent;
  final String label;
  final String counter;

  @override
  Widget build(BuildContext context) {
    return Row(
      crossAxisAlignment: CrossAxisAlignment.center,
      children: [
        Container(
          width: 6,
          height: 6,
          decoration: BoxDecoration(
            color: accent.withValues(alpha: 0.95),
            shape: BoxShape.circle,
            boxShadow: [
              BoxShadow(
                color: accent.withValues(alpha: 0.55),
                blurRadius: 8,
                spreadRadius: 1,
              ),
            ],
          ),
        ),
        const SizedBox(width: 10),
        Expanded(
          child: Text(
            label,
            style: TextStyle(
              color: CelestialColors.textPrimary.withValues(alpha: 0.95),
              fontSize: 11.5,
              fontWeight: FontWeight.w700,
              letterSpacing: 2.4,
            ),
          ),
        ),
        Text(
          counter,
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.7),
            fontSize: 11,
            fontFamily: 'monospace',
            fontWeight: FontWeight.w600,
            letterSpacing: 1.4,
          ),
        ),
      ],
    );
  }
}

class _OrDivider extends StatelessWidget {
  const _OrDivider({required this.label});
  final String label;

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        Expanded(
          child: Container(
            height: 1,
            color: CelestialColors.textSecondary.withValues(alpha: 0.18),
          ),
        ),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 14),
          child: Text(
            label,
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.55),
              fontSize: 10,
              fontFamily: 'monospace',
              fontWeight: FontWeight.w700,
              letterSpacing: 1.6,
            ),
          ),
        ),
        Expanded(
          child: Container(
            height: 1,
            color: CelestialColors.textSecondary.withValues(alpha: 0.18),
          ),
        ),
      ],
    );
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// QR scan button — primary scan affordance.
//
// Deliberately styled like a tactile button (icon tile + label + chevron),
// not a camera viewfinder, so it reads as "tap to launch the scanner"
// rather than "the camera is already live."
// ─────────────────────────────────────────────────────────────────────────────

class _QrScanButton extends StatefulWidget {
  const _QrScanButton({
    required this.accent,
    required this.accentDeep,
    required this.onTap,
  });

  final Color accent;
  final Color accentDeep;
  final VoidCallback onTap;

  @override
  State<_QrScanButton> createState() => _QrScanButtonState();
}

class _QrScanButtonState extends State<_QrScanButton> {
  bool _pressed = false;

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: widget.onTap,
      onTapDown: (_) => setState(() => _pressed = true),
      onTapCancel: () => setState(() => _pressed = false),
      onTapUp: (_) => setState(() => _pressed = false),
      behavior: HitTestBehavior.opaque,
      child: AnimatedScale(
        scale: _pressed ? 0.985 : 1.0,
        duration: const Duration(milliseconds: 120),
        curve: Curves.easeOut,
        child: Container(
          padding: const EdgeInsets.fromLTRB(14, 14, 14, 14),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(16),
            gradient: LinearGradient(
              begin: Alignment.topLeft,
              end: Alignment.bottomRight,
              colors: [
                widget.accent.withValues(alpha: 0.12),
                widget.accent.withValues(alpha: 0.04),
              ],
            ),
            border: Border.all(
              color: widget.accent.withValues(alpha: 0.55),
              width: 1.2,
            ),
            boxShadow: [
              BoxShadow(
                color: widget.accent.withValues(alpha: _pressed ? 0.10 : 0.22),
                blurRadius: _pressed ? 8 : 18,
                offset: Offset(0, _pressed ? 2 : 8),
              ),
            ],
          ),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.center,
            children: [
              // Tactile icon tile.
              Container(
                width: 56,
                height: 56,
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(13),
                  gradient: LinearGradient(
                    begin: Alignment.topLeft,
                    end: Alignment.bottomRight,
                    colors: [widget.accent, widget.accentDeep],
                  ),
                  boxShadow: [
                    BoxShadow(
                      color: widget.accent.withValues(alpha: 0.45),
                      blurRadius: 14,
                      offset: const Offset(0, 6),
                    ),
                  ],
                ),
                child: Stack(
                  children: [
                    // Subtle top highlight.
                    Positioned(
                      top: 0,
                      left: 10,
                      right: 10,
                      child: Container(
                        height: 1,
                        decoration: BoxDecoration(
                          color: Colors.white.withValues(alpha: 0.28),
                          borderRadius: BorderRadius.circular(1),
                        ),
                      ),
                    ),
                    const Center(
                      child: Icon(
                        Icons.qr_code_scanner_rounded,
                        color: Colors.white,
                        size: 28,
                      ),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      'Scan QR Code',
                      style: TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 15,
                        fontWeight: FontWeight.w600,
                        letterSpacing: 0.2,
                      ),
                    ),
                    const SizedBox(height: 3),
                    Text(
                      'Opens the camera to read the QR on the device.',
                      style: TextStyle(
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.7),
                        fontSize: 12,
                        height: 1.35,
                      ),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 10),
              Container(
                width: 34,
                height: 34,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: widget.accent.withValues(alpha: 0.16),
                  border: Border.all(
                    color: widget.accent.withValues(alpha: 0.5),
                    width: 1,
                  ),
                ),
                child: Icon(
                  Icons.arrow_forward_rounded,
                  color: widget.accent,
                  size: 16,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Payload console — terminal-style text field
// ─────────────────────────────────────────────────────────────────────────────

class _PayloadConsole extends StatelessWidget {
  const _PayloadConsole({
    required this.controller,
    required this.accent,
    required this.warning,
    required this.helperText,
    required this.isValidLooking,
  });

  final TextEditingController controller;
  final Color accent;
  final Color warning;
  final String helperText;
  final bool isValidLooking;

  @override
  Widget build(BuildContext context) {
    final empty = controller.text.isEmpty;
    final statusColor = empty
        ? CelestialColors.textSecondary.withValues(alpha: 0.55)
        : (isValidLooking ? accent : warning);
    final statusLabel =
        empty ? 'AWAITING INPUT' : (isValidLooking ? 'READY' : 'UNVERIFIED');

    return Container(
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard.withValues(alpha: 0.7),
        borderRadius: BorderRadius.circular(14),
        border: Border.all(
          color: empty
              ? CelestialColors.orbitRing.withValues(alpha: 0.7)
              : statusColor.withValues(alpha: 0.45),
          width: 1,
        ),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // Top metadata strip.
          Container(
            padding: const EdgeInsets.fromLTRB(14, 10, 12, 8),
            decoration: BoxDecoration(
              border: Border(
                bottom: BorderSide(
                  color: CelestialColors.orbitRing.withValues(alpha: 0.35),
                  width: 1,
                ),
              ),
            ),
            child: Row(
              children: [
                Container(
                  width: 5,
                  height: 5,
                  decoration: BoxDecoration(
                    color: statusColor,
                    shape: BoxShape.circle,
                  ),
                ),
                const SizedBox(width: 8),
                Text(
                  'PAYLOAD · MT://',
                  style: TextStyle(
                    color:
                        CelestialColors.textSecondary.withValues(alpha: 0.75),
                    fontSize: 10.5,
                    fontFamily: 'monospace',
                    fontWeight: FontWeight.w700,
                    letterSpacing: 1.5,
                  ),
                ),
                const Spacer(),
                Text(
                  statusLabel,
                  style: TextStyle(
                    color: statusColor,
                    fontSize: 10,
                    fontFamily: 'monospace',
                    fontWeight: FontWeight.w700,
                    letterSpacing: 1.5,
                  ),
                ),
              ],
            ),
          ),
          // Text field.
          Padding(
            padding: const EdgeInsets.fromLTRB(14, 14, 14, 6),
            child: TextField(
              controller: controller,
              keyboardType: TextInputType.visiblePassword,
              textInputAction: TextInputAction.done,
              autocorrect: false,
              enableSuggestions: false,
              cursorColor: accent,
              cursorWidth: 1.5,
              style: const TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 15,
                fontFamily: 'monospace',
                letterSpacing: 0.4,
                fontWeight: FontWeight.w500,
              ),
              minLines: 1,
              maxLines: 3,
              decoration: InputDecoration(
                isCollapsed: true,
                border: InputBorder.none,
                hintText: 'MT:Y.K908OC16750648G00   ·   3497-123-4567',
                hintStyle: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.35),
                  fontSize: 13,
                  fontFamily: 'monospace',
                  letterSpacing: 0.4,
                ),
              ),
            ),
          ),
          // Helper text.
          Padding(
            padding: const EdgeInsets.fromLTRB(14, 6, 14, 12),
            child: Text(
              helperText,
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.65),
                fontSize: 11.5,
                height: 1.45,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Buttons
// ─────────────────────────────────────────────────────────────────────────────

class _PrimaryTransmitButton extends StatelessWidget {
  const _PrimaryTransmitButton({
    required this.label,
    required this.enabled,
    required this.onTap,
  });
  final String label;
  final bool enabled;
  final VoidCallback onTap;

  static const _teal = Color(0xFF00BCD4);
  static const _tealDeep = Color(0xFF00838F);

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: enabled ? onTap : null,
      behavior: HitTestBehavior.opaque,
      child: Container(
        height: 58,
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(14),
          gradient: enabled
              ? const LinearGradient(
                  begin: Alignment.topLeft,
                  end: Alignment.bottomRight,
                  colors: [_teal, _tealDeep],
                )
              : null,
          color: enabled
              ? null
              : CelestialColors.backgroundCard.withValues(alpha: 0.55),
          border: Border.all(
            color: enabled
                ? _teal.withValues(alpha: 0.55)
                : CelestialColors.orbitRing.withValues(alpha: 0.5),
            width: 1,
          ),
          boxShadow: enabled
              ? [
                  BoxShadow(
                    color: _teal.withValues(alpha: 0.32),
                    blurRadius: 20,
                    offset: const Offset(0, 8),
                  ),
                ]
              : null,
        ),
        child: Stack(
          children: [
            // Inner top highlight line.
            if (enabled)
              Positioned(
                top: 0,
                left: 14,
                right: 14,
                child: Container(
                  height: 1,
                  color: Colors.white.withValues(alpha: 0.22),
                ),
              ),
            Positioned(
              left: 18,
              top: 0,
              bottom: 0,
              child: Center(
                child: Icon(
                  Icons.bolt_rounded,
                  size: 18,
                  color:
                      (enabled ? Colors.white : CelestialColors.textSecondary)
                          .withValues(alpha: enabled ? 0.9 : 0.4),
                ),
              ),
            ),
            Center(
              child: Text(
                label,
                style: TextStyle(
                  color: enabled
                      ? Colors.white
                      : CelestialColors.textSecondary.withValues(alpha: 0.45),
                  fontSize: 15,
                  fontWeight: FontWeight.w700,
                  letterSpacing: 0.5,
                ),
              ),
            ),
            Positioned(
              right: 18,
              top: 0,
              bottom: 0,
              child: Center(
                child: Icon(
                  Icons.arrow_forward_rounded,
                  size: 18,
                  color:
                      (enabled ? Colors.white : CelestialColors.textSecondary)
                          .withValues(alpha: enabled ? 0.85 : 0.35),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _SecondaryGhostButton extends StatelessWidget {
  const _SecondaryGhostButton({required this.label, required this.onTap});
  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onTap,
      behavior: HitTestBehavior.opaque,
      child: Container(
        height: 50,
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(14),
          border: Border.all(
            color: CelestialColors.orbitRing.withValues(alpha: 0.7),
            width: 1,
          ),
        ),
        child: Center(
          child: Text(
            label,
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.95),
              fontSize: 14,
              fontWeight: FontWeight.w600,
              letterSpacing: 0.4,
            ),
          ),
        ),
      ),
    );
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pairing hero — radar / signal lock
// ─────────────────────────────────────────────────────────────────────────────

class _SignalLockHero extends StatelessWidget {
  const _SignalLockHero({
    required this.pulse,
    required this.sweep,
    required this.accent,
    required this.accentDeep,
    required this.accentSecondary,
  });

  final Animation<double> pulse;
  final Animation<double> sweep;
  final Color accent;
  final Color accentDeep;
  final Color accentSecondary;

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: 220,
      height: 220,
      child: AnimatedBuilder(
        animation: Listenable.merge([pulse, sweep]),
        builder: (context, _) {
          return Stack(
            alignment: Alignment.center,
            children: [
              Positioned.fill(
                child: CustomPaint(
                  painter: _SignalLockPainter(
                    pulse: pulse.value,
                    sweep: sweep.value,
                    accent: accent,
                    accentSecondary: accentSecondary,
                  ),
                ),
              ),
              Container(
                width: 64,
                height: 64,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  gradient: LinearGradient(
                    begin: Alignment.topLeft,
                    end: Alignment.bottomRight,
                    colors: [accent, accentDeep],
                  ),
                  boxShadow: [
                    BoxShadow(
                      color: accent.withValues(
                        alpha: 0.35 + pulse.value * 0.25,
                      ),
                      blurRadius: 24 + pulse.value * 12,
                      spreadRadius: 1 + pulse.value * 2,
                    ),
                  ],
                ),
                child: const Icon(
                  Icons.memory_outlined,
                  color: Colors.white,
                  size: 26,
                ),
              ),
            ],
          );
        },
      ),
    );
  }
}

class _SignalLockPainter extends CustomPainter {
  _SignalLockPainter({
    required this.pulse,
    required this.sweep,
    required this.accent,
    required this.accentSecondary,
  });

  final double pulse;
  final double sweep;
  final Color accent;
  final Color accentSecondary;

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);
    final maxR = size.shortestSide / 2;

    // Three concentric expanding rings, phase-shifted.
    for (var i = 0; i < 3; i++) {
      final phase = (pulse + i / 3) % 1.0;
      final radius = maxR * (0.30 + phase * 0.68);
      final opacity = (1.0 - phase) * 0.32;
      canvas.drawCircle(
        center,
        radius,
        Paint()
          ..color = accent.withValues(alpha: opacity)
          ..strokeWidth = 1.1
          ..style = PaintingStyle.stroke,
      );
    }

    // Outer dotted ring — 60 tiny dots around the circumference.
    final dotPaint = Paint()..color = accent.withValues(alpha: 0.30);
    const dotCount = 60;
    for (var i = 0; i < dotCount; i++) {
      final angle = (i / dotCount) * math.pi * 2;
      final r = maxR * 0.96;
      canvas.drawCircle(
        center.translate(math.cos(angle) * r, math.sin(angle) * r),
        i % 5 == 0 ? 1.4 : 0.8,
        dotPaint,
      );
    }

    // Radar sweep cone — a soft sector that rotates.
    final sweepAngle = sweep * math.pi * 2;
    const coneWidth = 0.7;
    final cone = Path()
      ..moveTo(center.dx, center.dy)
      ..arcTo(
        Rect.fromCircle(center: center, radius: maxR * 0.9),
        sweepAngle - coneWidth,
        coneWidth,
        false,
      )
      ..close();
    canvas.drawPath(
      cone,
      Paint()
        ..shader = SweepGradient(
          startAngle: sweepAngle - coneWidth,
          endAngle: sweepAngle,
          colors: [
            accent.withValues(alpha: 0.0),
            accent.withValues(alpha: 0.20),
          ],
          transform: const GradientRotation(0),
        ).createShader(Rect.fromCircle(center: center, radius: maxR * 0.9)),
    );

    // Leading edge of the sweep, a bright radial line.
    canvas.drawLine(
      center,
      Offset(
        center.dx + math.cos(sweepAngle) * maxR * 0.9,
        center.dy + math.sin(sweepAngle) * maxR * 0.9,
      ),
      Paint()
        ..shader = LinearGradient(
          colors: [
            accent.withValues(alpha: 0.0),
            accent.withValues(alpha: 0.85),
            accentSecondary.withValues(alpha: 0.9),
          ],
          stops: const [0.0, 0.7, 1.0],
        ).createShader(
          Rect.fromPoints(
            center,
            Offset(
              center.dx + math.cos(sweepAngle) * maxR * 0.9,
              center.dy + math.sin(sweepAngle) * maxR * 0.9,
            ),
          ),
        )
        ..strokeWidth = 1.5
        ..strokeCap = StrokeCap.round,
    );

    // Crosshair reticle — 4 ticks just outside the central chip.
    final reticle = Paint()
      ..color = accent.withValues(alpha: 0.55)
      ..strokeWidth = 1
      ..strokeCap = StrokeCap.round;
    const tickLen = 8.0;
    final tickRadius = maxR * 0.42;
    canvas.drawLine(
      center.translate(-tickRadius - tickLen, 0),
      center.translate(-tickRadius, 0),
      reticle,
    );
    canvas.drawLine(
      center.translate(tickRadius, 0),
      center.translate(tickRadius + tickLen, 0),
      reticle,
    );
    canvas.drawLine(
      center.translate(0, -tickRadius - tickLen),
      center.translate(0, -tickRadius),
      reticle,
    );
    canvas.drawLine(
      center.translate(0, tickRadius),
      center.translate(0, tickRadius + tickLen),
      reticle,
    );
  }

  @override
  bool shouldRepaint(covariant _SignalLockPainter old) =>
      old.pulse != pulse || old.sweep != sweep;
}

class _PayloadEcho extends StatelessWidget {
  const _PayloadEcho({required this.payload});
  final String payload;

  @override
  Widget build(BuildContext context) {
    if (payload.isEmpty) return const SizedBox.shrink();
    return Center(
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 7),
        decoration: BoxDecoration(
          color: CelestialColors.backgroundCard.withValues(alpha: 0.5),
          borderRadius: BorderRadius.circular(6),
          border: Border.all(
            color: CelestialColors.orbitRing.withValues(alpha: 0.5),
            width: 1,
          ),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              'MT://  ',
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.45),
                fontSize: 11,
                fontFamily: 'monospace',
                fontWeight: FontWeight.w700,
                letterSpacing: 1.0,
              ),
            ),
            Flexible(
              child: Text(
                payload,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.7),
                  fontSize: 11.5,
                  fontFamily: 'monospace',
                  letterSpacing: 0.4,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Failure glyph — a red ringed cross with a slow breathing halo.
// ─────────────────────────────────────────────────────────────────────────────

class _FailureGlyph extends StatelessWidget {
  const _FailureGlyph({required this.pulse});
  final Animation<double> pulse;

  static const _danger = Color(0xFFEF5350);

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: pulse,
      builder: (context, _) {
        final t = pulse.value;
        return SizedBox(
          width: 110,
          height: 110,
          child: Stack(
            alignment: Alignment.center,
            children: [
              Container(
                width: 110,
                height: 110,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  border: Border.all(
                    color: _danger.withValues(alpha: 0.10 + t * 0.10),
                    width: 1,
                  ),
                ),
              ),
              Container(
                width: 82,
                height: 82,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  border: Border.all(
                    color: _danger.withValues(alpha: 0.20 + t * 0.15),
                    width: 1,
                  ),
                ),
              ),
              Container(
                width: 56,
                height: 56,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: _danger.withValues(alpha: 0.18),
                  border: Border.all(
                    color: _danger.withValues(alpha: 0.55),
                    width: 1.5,
                  ),
                  boxShadow: [
                    BoxShadow(
                      color: _danger.withValues(alpha: 0.25 + t * 0.20),
                      blurRadius: 18 + t * 8,
                      spreadRadius: t * 2,
                    ),
                  ],
                ),
                child: const Icon(
                  Icons.close_rounded,
                  color: Colors.white,
                  size: 28,
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}
