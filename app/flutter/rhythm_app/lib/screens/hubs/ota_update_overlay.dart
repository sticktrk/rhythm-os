import 'dart:async';

import 'package:flutter/material.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show
        RhythmConnection,
        RhythmConnectionState,
        RhythmOtaUpdateProgress,
        RhythmOtaUpdateStage;

import '../../services/ota_service.dart';
import '../../widgets/solar_orbit.dart' show CelestialColors;
import '../../widgets/stage_timeline.dart';

String _formatOtaVersionLabel(String version) {
  return version.startsWith('v') || version.startsWith('V')
      ? version
      : 'v$version';
}

// =============================================================================
// OTA Update Overlay (full-screen blocking during firmware update)
//
// Reused by both the server settings screen and BLE onboarding. When
// `connection` is null the overlay drives its staged progress from the coarse
// `OtaService` state and treats the server as ready as soon as the service
// reports `OtaState.complete`.
// =============================================================================

class OtaUpdateOverlay extends StatefulWidget {
  final OtaService otaService;
  final RhythmConnection? connection;

  const OtaUpdateOverlay({
    super.key,
    required this.otaService,
    this.connection,
  });

  static Future<void> show(
    BuildContext context, {
    required OtaService otaService,
    RhythmConnection? connection,
  }) {
    return Navigator.of(context, rootNavigator: true).push(
      PageRouteBuilder(
        opaque: true,
        pageBuilder: (_, __, ___) => OtaUpdateOverlay(
          otaService: otaService,
          connection: connection,
        ),
        transitionsBuilder: (_, animation, __, child) {
          return FadeTransition(opacity: animation, child: child);
        },
        transitionDuration: const Duration(milliseconds: 300),
      ),
    );
  }

  @override
  State<OtaUpdateOverlay> createState() => _OtaUpdateOverlayState();
}

class _OtaUpdateOverlayState extends State<OtaUpdateOverlay>
    with TickerProviderStateMixin {
  late AnimationController _pulseController;
  late Animation<double> _pulseAnimation;
  late AnimationController _spinController;
  String? _targetVersionLabel;
  bool _isBundleRepair = false;
  StreamSubscription<RhythmOtaUpdateProgress>? _otaProgressSub;
  StreamSubscription<RhythmConnectionState>? _connectionStateSub;
  RhythmOtaUpdateProgress? _latestProgress;
  RhythmConnectionState? _connectionState;
  bool _awaitingPostOtaConnection = false;
  bool _sawPostOtaConnected = false;

  static const _teal = Color(0xFF00BCD4);

  @override
  void initState() {
    super.initState();
    widget.otaService.addListener(_onStateChanged);
    _syncOverlayMetadata();
    _syncPostOtaConnectionGate();

    _pulseController = AnimationController(
      duration: const Duration(milliseconds: 2000),
      vsync: this,
    )..repeat(reverse: true);

    _pulseAnimation = Tween<double>(begin: 0.3, end: 0.8).animate(
      CurvedAnimation(parent: _pulseController, curve: Curves.easeInOut),
    );

    // Continuous clockwise spin for the circular-arrow hero icon
    // (Restarting / reconnecting stages).
    _spinController = AnimationController(
      duration: const Duration(milliseconds: 1100),
      vsync: this,
    )..repeat();

    _otaProgressSub =
        widget.connection?.otaUpdateProgressEvents.listen((event) {
      if (!mounted) return;
      setState(() {
        _latestProgress = event;
      });
    });

    _connectionState = widget.connection?.connectionState;
    _connectionStateSub =
        widget.connection?.connectionStateStream.listen((state) {
      if (!mounted) return;
      setState(() {
        _connectionState = state;
        if (_awaitingPostOtaConnection &&
            state == RhythmConnectionState.connected) {
          _sawPostOtaConnected = true;
        }
      });
    });
  }

  @override
  void dispose() {
    widget.otaService.removeListener(_onStateChanged);
    _otaProgressSub?.cancel();
    _connectionStateSub?.cancel();
    _pulseController.dispose();
    _spinController.dispose();
    super.dispose();
  }

  void _onStateChanged() {
    _syncOverlayMetadata();
    _syncPostOtaConnectionGate();
    if (mounted) setState(() {});
  }

  void _syncOverlayMetadata() {
    _isBundleRepair = _isBundleRepair || widget.otaService.isBundleRepair;
    _targetVersionLabel ??= _resolveTargetVersionLabel();
  }

  String? _resolveTargetVersionLabel() {
    final releaseVersion = widget.otaService.availableRelease?.version;
    if (releaseVersion != null && releaseVersion.isNotEmpty) {
      return _formatOtaVersionLabel(releaseVersion);
    }

    final latestVersion = widget.otaService.latestVersion;
    if (latestVersion != null && latestVersion.isNotEmpty) {
      return _formatOtaVersionLabel(latestVersion);
    }

    return null;
  }

  void _syncPostOtaConnectionGate() {
    if (widget.connection == null) {
      _sawPostOtaConnected = true;
      return;
    }
    if (widget.otaService.state == OtaState.complete &&
        !_awaitingPostOtaConnection) {
      _awaitingPostOtaConnection = true;
      _sawPostOtaConnected = false;
    }
  }

  String _buildProgressMessage({required bool isDownloading}) {
    final targetVersion = _targetVersionLabel;
    if (targetVersion == null) {
      return isDownloading ? 'Downloading update...' : 'Installing update...';
    }

    if (_isBundleRepair) {
      return isDownloading
          ? 'Downloading repair for $targetVersion...'
          : 'Installing repair for $targetVersion...';
    }

    return isDownloading
        ? 'Downloading update to $targetVersion...'
        : 'Installing update to $targetVersion...';
  }

  String _buildRebootingMessage() {
    final targetVersion = _targetVersionLabel;
    if (targetVersion == null) {
      return 'Restarting your device...';
    }

    if (_isBundleRepair) {
      return 'Restarting after repair on $targetVersion...';
    }

    return 'Restarting into $targetVersion...';
  }

  String _buildCompletionMessage() {
    final statusMessage = widget.otaService.statusMessage?.trim();
    if (statusMessage != null && statusMessage.isNotEmpty) {
      return statusMessage;
    }

    final targetVersion = _targetVersionLabel;
    if (targetVersion == null) {
      return 'Your device is up to date.';
    }

    return _isBundleRepair
        ? 'Repair completed on $targetVersion.'
        : 'Updated to $targetVersion.';
  }

  bool get _isFinished {
    final state = widget.otaService.state;
    return state == OtaState.error ||
        (state == OtaState.complete && _serverReady);
  }

  bool get _serverReady {
    final connection = widget.connection;
    if (connection == null) return true;
    if (widget.otaService.state == OtaState.complete) {
      return _sawPostOtaConnected;
    }
    return (_connectionState ?? connection.connectionState) ==
        RhythmConnectionState.connected;
  }

  bool get _waitingForServerReady =>
      widget.otaService.state == OtaState.complete && !_serverReady;

  void _dismiss() {
    Navigator.of(context).pop();
  }

  @override
  Widget build(BuildContext context) {
    return PopScope(
      canPop: false,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop && _isFinished) {
          _dismiss();
        }
      },
      child: Scaffold(
        backgroundColor: CelestialColors.backgroundDark,
        body: SafeArea(
          child: Center(
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 40),
              child: _buildContent(),
            ),
          ),
        ),
      ),
    );
  }

  Widget _buildContent() {
    switch (widget.otaService.state) {
      case OtaState.complete:
        if (_waitingForServerReady) {
          return _buildStagedProgress(waitingForServerReady: true);
        }
        return _buildComplete();
      case OtaState.error:
        return _buildError();
      default:
        return _buildStagedProgress();
    }
  }

  /// 5-stage timeline for the server self-update flow:
  ///   0 = Checking, 1 = Downloading (with %), 2 = Verifying,
  ///   3 = Installing, 4 = Restarting.
  int _otaActiveStageIndex() {
    final progress = _latestProgress;
    if (progress != null) {
      final mapped = switch (progress.stage) {
        RhythmOtaUpdateStage.checking => 0,
        RhythmOtaUpdateStage.updateAvailable => 0,
        RhythmOtaUpdateStage.upToDate => 0,
        RhythmOtaUpdateStage.downloading => 1,
        RhythmOtaUpdateStage.verifying => 2,
        RhythmOtaUpdateStage.staging => 3,
        RhythmOtaUpdateStage.installing => 3,
        RhythmOtaUpdateStage.finalizing => 3,
        RhythmOtaUpdateStage.restarting => 4,
        // Server doesn't echo which stage failed, so attribute it to whatever
        // we last knew about — that gives the timeline a sensible failed dot.
        RhythmOtaUpdateStage.failed => _lastObservedActiveIndex,
      };
      _lastObservedActiveIndex = mapped;
      return mapped;
    }
    // Fall back to OtaService coarse state.
    final fromCoarse = switch (widget.otaService.state) {
      OtaState.checking => 0,
      OtaState.downloading => 1,
      OtaState.uploading => 3,
      OtaState.flashing => 3,
      OtaState.rebooting => 4,
      _ => 0,
    };
    _lastObservedActiveIndex = fromCoarse;
    return fromCoarse;
  }

  int _lastObservedActiveIndex = 0;

  String? _otaActiveMessage(int activeIndex) {
    final progress = _latestProgress;
    if (progress != null && progress.message.trim().isNotEmpty) {
      return progress.message.trim();
    }
    // Sensible fallbacks per stage when no SSE event has arrived yet.
    return switch (activeIndex) {
      0 => 'Checking for updates',
      1 => _buildProgressMessage(isDownloading: true),
      2 => 'Verifying download integrity',
      3 => _buildProgressMessage(isDownloading: false),
      4 => _buildRebootingMessage(),
      _ => null,
    };
  }

  String _otaTitle(int activeIndex) {
    if (activeIndex == 4) return 'Restarting Server';
    return 'Updating Server';
  }

  IconData _otaHeroIcon(int activeIndex) {
    return switch (activeIndex) {
      0 => Icons.search_rounded,
      1 => Icons.cloud_download_outlined,
      2 => Icons.verified_outlined,
      3 => Icons.system_update,
      4 => Icons.restart_alt_rounded,
      _ => Icons.system_update,
    };
  }

  /// Format `total / downloaded` bytes as e.g. `45.2 / 62.8 MB`.
  String? _bytesLabel() {
    final p = _latestProgress;
    if (p == null) return null;
    final downloaded = p.downloadedBytes;
    final total = p.totalBytes;
    if (downloaded == null) return null;
    if (total == null || total <= 0) {
      return _formatBytes(downloaded);
    }
    final d = _formatBytesValue(downloaded, total);
    final t = _formatBytes(total);
    return '$d / $t';
  }

  static const _kb = 1024;
  static const _mb = 1024 * 1024;
  static const _gb = 1024 * 1024 * 1024;

  static String _formatBytes(int bytes) {
    if (bytes >= _gb) return '${(bytes / _gb).toStringAsFixed(2)} GB';
    if (bytes >= _mb) return '${(bytes / _mb).toStringAsFixed(1)} MB';
    if (bytes >= _kb) return '${(bytes / _kb).toStringAsFixed(0)} KB';
    return '$bytes B';
  }

  static String _formatBytesValue(int value, int total) {
    if (total >= _gb) return (value / _gb).toStringAsFixed(2);
    if (total >= _mb) return (value / _mb).toStringAsFixed(1);
    if (total >= _kb) return (value / _kb).toStringAsFixed(0);
    return value.toString();
  }

  Widget _buildStagedProgress({bool waitingForServerReady = false}) {
    final activeIndex = waitingForServerReady ? 4 : _otaActiveStageIndex();
    final activeMessage = waitingForServerReady
        ? 'Waiting for the server connection to finish restoring'
        : _otaActiveMessage(activeIndex);
    final title =
        waitingForServerReady ? 'Reconnecting Server' : _otaTitle(activeIndex);
    final heroIcon =
        waitingForServerReady ? Icons.sync_rounded : _otaHeroIcon(activeIndex);
    // The Restarting / reconnecting stages use a circular-arrow icon — spin it.
    final spinHero = waitingForServerReady || activeIndex == 4;
    final percent = !waitingForServerReady && activeIndex == 1
        ? _latestProgress?.percent
        : null;
    final bytesLabel =
        !waitingForServerReady && activeIndex == 1 ? _bytesLabel() : null;

    return SingleChildScrollView(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Center(
            child: AnimatedBuilder(
              animation: _pulseAnimation,
              builder: (context, _) {
                return Container(
                  width: 92,
                  height: 92,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    gradient: RadialGradient(
                      colors: [
                        _teal.withValues(alpha: 0.18),
                        _teal.withValues(alpha: 0.04),
                      ],
                    ),
                    boxShadow: [
                      BoxShadow(
                        color: _teal.withValues(
                            alpha: _pulseAnimation.value * 0.28),
                        blurRadius: 32,
                        spreadRadius: 4,
                      ),
                    ],
                  ),
                  child: spinHero
                      ? RotationTransition(
                          turns: _spinController,
                          child: Icon(heroIcon, color: _teal, size: 38),
                        )
                      : Icon(heroIcon, color: _teal, size: 38),
                );
              },
            ),
          ),
          const SizedBox(height: 28),
          Text(
            title,
            textAlign: TextAlign.center,
            style: const TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 22,
              fontWeight: FontWeight.w600,
              letterSpacing: 0.2,
            ),
          ),
          if (_targetVersionLabel != null) ...[
            const SizedBox(height: 4),
            Text(
              _isBundleRepair
                  ? 'Repairing $_targetVersionLabel'
                  : 'Installing $_targetVersionLabel',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: _teal.withValues(alpha: 0.85),
                fontSize: 13,
                fontWeight: FontWeight.w500,
                fontFamily: 'monospace',
                letterSpacing: 0.4,
              ),
            ),
          ],
          const SizedBox(height: 28),
          Container(
            padding: const EdgeInsets.fromLTRB(20, 22, 20, 22),
            decoration: BoxDecoration(
              color: CelestialColors.backgroundCard.withValues(alpha: 0.6),
              borderRadius: BorderRadius.circular(18),
              border: Border.all(
                color: _teal.withValues(alpha: 0.12),
                width: 1,
              ),
            ),
            child: StageTimeline(
              stages: const [
                StageTimelineItem(
                  label: 'Checking for updates',
                  icon: Icons.search_rounded,
                ),
                StageTimelineItem(
                  label: 'Downloading',
                  icon: Icons.cloud_download_outlined,
                ),
                StageTimelineItem(
                  label: 'Verifying',
                  icon: Icons.fingerprint_rounded,
                ),
                StageTimelineItem(
                  label: 'Installing',
                  icon: Icons.settings_suggest_outlined,
                ),
                StageTimelineItem(
                  label: 'Restarting',
                  icon: Icons.restart_alt_rounded,
                ),
              ],
              activeIndex: activeIndex,
              activeMessage: activeMessage,
              activePercent: percent,
              activeBytesLabel: bytesLabel,
              failed: !waitingForServerReady &&
                  _latestProgress?.stage == RhythmOtaUpdateStage.failed,
              accent: _teal,
            ),
          ),
          const SizedBox(height: 24),
          Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              Icon(
                Icons.info_outline_rounded,
                color: CelestialColors.textSecondary.withValues(alpha: 0.4),
                size: 16,
              ),
              const SizedBox(width: 8),
              Text(
                'Do not close the app',
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.4),
                  fontSize: 13,
                ),
              ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _buildComplete() {
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Container(
          width: 80,
          height: 80,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: const Color(0xFF22C55E).withValues(alpha: 0.15),
            boxShadow: [
              BoxShadow(
                color: const Color(0xFF22C55E).withValues(alpha: 0.2),
                blurRadius: 24,
                spreadRadius: 4,
              ),
            ],
          ),
          child: const Icon(
            Icons.check_rounded,
            color: Color(0xFF22C55E),
            size: 40,
          ),
        ),
        const SizedBox(height: 32),
        const Text(
          'Update Complete',
          style: TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 22,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 8),
        Text(
          _buildCompletionMessage(),
          style: const TextStyle(
            color: Color(0xFF22C55E),
            fontSize: 15,
            fontWeight: FontWeight.w500,
          ),
        ),
        const SizedBox(height: 40),
        GestureDetector(
          onTap: _dismiss,
          child: Container(
            width: double.infinity,
            padding: const EdgeInsets.symmetric(vertical: 14),
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(14),
              color: _teal.withValues(alpha: 0.1),
              border: Border.all(
                color: _teal.withValues(alpha: 0.3),
              ),
            ),
            child: const Text(
              'Done',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: _teal,
                fontSize: 16,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
        ),
      ],
    );
  }

  Widget _buildError() {
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Container(
          width: 80,
          height: 80,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: Colors.red.shade400.withValues(alpha: 0.15),
          ),
          child: Icon(
            Icons.error_outline_rounded,
            color: Colors.red.shade400,
            size: 40,
          ),
        ),
        const SizedBox(height: 32),
        const Text(
          'Update Failed',
          style: TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 22,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 8),
        Text(
          'The update did not finish. Please try again.',
          textAlign: TextAlign.center,
          style: TextStyle(
            color: Colors.red.shade400,
            fontSize: 14,
          ),
        ),
        const SizedBox(height: 40),
        GestureDetector(
          onTap: _dismiss,
          child: Container(
            width: double.infinity,
            padding: const EdgeInsets.symmetric(vertical: 14),
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(14),
              color: CelestialColors.textSecondary.withValues(alpha: 0.08),
              border: Border.all(
                color: CelestialColors.textSecondary.withValues(alpha: 0.2),
              ),
            ),
            child: const Text(
              'Close',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: CelestialColors.textSecondary,
                fontSize: 16,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
        ),
      ],
    );
  }
}
