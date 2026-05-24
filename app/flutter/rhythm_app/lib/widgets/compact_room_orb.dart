import 'dart:async';
import 'dart:math' as math;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' show RoomModeState;
import '../providers/server_sync_provider.dart';
import '../providers/home_provider.dart';
import '../providers/room_provider.dart';
import 'solar_orbit.dart';
import 'room_settings_sheet.dart';

/// Container widget for a single room in the grid.
///
/// Combines CompactSolarOrbit with room name label.
/// Manages per-room state via RoomProvider (Rust brain).
class CompactRoomOrb extends StatefulWidget {
  final String roomId;
  final CurveConfigDto globalConfig;
  final CurveData? curveData;
  final bool isSettingsMode;

  const CompactRoomOrb({
    super.key,
    required this.roomId,
    required this.globalConfig,
    this.curveData,
    this.isSettingsMode = false,
  });

  @override
  State<CompactRoomOrb> createState() => _CompactRoomOrbState();
}

class _CompactRoomOrbState extends State<CompactRoomOrb> {
  int? _manualBrightness;
  double? _dragHour; // non-null only during active drag
  int _lastResetGen = 0;
  Timer? _nowTimer;

  @override
  void initState() {
    super.initState();

    // Update display periodically when rhythm is active
    _nowTimer = Timer.periodic(const Duration(seconds: 30), (_) {
      if (mounted) setState(() {});
    });
  }

  @override
  void dispose() {
    _nowTimer?.cancel();
    super.dispose();
  }

  /// Derive the displayed hour from Rust state (or drag override).
  double _displayedHour(RoomDto room) {
    if (_dragHour != null) return _dragHour!;

    if (!room.rhythmEnabled) {
      // Paused: show the pinned position (now + offset)
      final now = DateTime.now();
      final nowHour = now.hour + now.minute / 60.0;
      return ((nowHour + room.timeOffsetMinutes / 60.0) % 24.0 + 24.0) % 24.0;
    }

    // Following: show current time + offset
    final now = DateTime.now();
    final nowHour = now.hour + now.minute / 60.0;
    return ((nowHour + room.timeOffsetMinutes / 60.0) % 24.0 + 24.0) % 24.0;
  }

  void _onHourChanged(double hour) {
    setState(() {
      _dragHour = hour;
      _manualBrightness = null;
    });
  }

  void _onHourChangeEnd() {
    if (_dragHour == null) return;
    final draggedHour = _dragHour!;

    final roomProvider = context.read<RoomProvider>();
    final room = roomProvider.getRoom(widget.roomId);
    if (room == null) return;

    // Compute offset: how far the dragged position is from real time
    final now = DateTime.now();
    final nowHour = now.hour + now.minute / 60.0;
    var offset = (draggedHour - nowHour) * 60.0; // in minutes
    // Normalize to ±12h range
    if (offset > 720) offset -= 1440;
    if (offset < -720) offset += 1440;

    // Set offset locally — rhythm continues following real time (shifted)
    roomProvider.setRoomTimeOffset(widget.roomId, offset);

    setState(() {
      _dragHour = null;
    });

    // Push time offset to server — it will apply the values to lights
    // Server treats this as a reset action at the new offset
    final serverSync = context.read<ServerSyncProvider>();
    serverSync.dispatchNodeAction(widget.roomId, 'reset');
  }

  Future<void> _toggleLight() async {
    HapticFeedback.mediumImpact();

    final roomProvider = context.read<RoomProvider>();
    final room = roomProvider.getRoom(widget.roomId);
    if (room == null) return;

    final newOn = !room.lightsOn;
    roomProvider.setRoomLightsOnLocal(widget.roomId, newOn);

    // Turn on → auto-enable rhythm and clear any stale hard-off semantics.
    if (newOn) {
      roomProvider.setRoomRhythmEnabled(widget.roomId, true);
      roomProvider.setRoomStateLocal(widget.roomId, RoomModeState.active);
    }

    final serverSync = context.read<ServerSyncProvider>();
    serverSync.dispatchNodeAction(widget.roomId, newOn ? 'on' : 'off');
  }

  void _onPlayPauseTap(RoomDto room) {
    HapticFeedback.lightImpact();

    final newEnabled = !room.rhythmEnabled;
    final roomProvider = context.read<RoomProvider>();
    roomProvider.setRoomRhythmEnabled(widget.roomId, newEnabled);

    final serverSync = context.read<ServerSyncProvider>();
    serverSync.pushNodePreferences(widget.roomId, rhythmEnabled: newEnabled);
  }

  Future<void> _resetToNow() async {
    HapticFeedback.lightImpact();

    final roomProvider = context.read<RoomProvider>();
    roomProvider.setRoomTimeOffset(widget.roomId, 0);

    setState(() {
      _manualBrightness = null;
      _dragHour = null;
    });

    final serverSync = context.read<ServerSyncProvider>();
    serverSync.dispatchNodeAction(widget.roomId, 'reset');
  }

  void _onBrightnessChanged(int brightness) {
    setState(() {
      _manualBrightness = brightness;
    });
  }

  void _onBrightnessChangeEnd() {
    if (_manualBrightness == null) return;
    final serverSync = context.read<ServerSyncProvider>();
    serverSync.dispatchNodeBrightness(widget.roomId, _manualBrightness!);
  }

  int _getBrightnessAtHour(double hour, CurveData? curveData) {
    if (curveData == null) return 50;
    if (curveData.hours.isEmpty) return 50;

    int idx = 0;
    double minDiff = double.infinity;
    for (int i = 0; i < curveData.hours.length; i++) {
      final diff = (curveData.hours[i] - hour).abs();
      if (diff < minDiff) {
        minDiff = diff;
        idx = i;
      }
    }
    return curveData.brightness[idx];
  }

  @override
  Widget build(BuildContext context) {
    return Selector<RoomProvider, (RoomDto?, int, MotionTimerInfo?, int?)>(
      selector: (_, p) => (
        p.getRoom(widget.roomId),
        p.resetGeneration,
        p.getMotionTimer(widget.roomId),
        p.getBrightness(widget.roomId),
      ),
      builder: (context, data, child) {
        final (room, resetGen, motionTimer, serverBrightness) = data;
        if (room == null) return const SizedBox.shrink();

        // Reset actions bump the generation counter. Drop local overrides so
        // the orb displays curve-computed values.
        if (resetGen != _lastResetGen) {
          _lastResetGen = resetGen;
          _manualBrightness = null;
          _dragHour = null;
        }

        final selectedHour = _displayedHour(room);
        final isLightOn = room.lightsOn;

        // Use server-provided brightness, fallback to curve data interpolation
        final curveBrightness = serverBrightness ??
            _getBrightnessAtHour(selectedHour, widget.curveData);

        final brightness = _manualBrightness ?? curveBrightness;

        return LayoutBuilder(
          builder: (context, outerConstraints) {
            final labelFontSize =
                (outerConstraints.maxHeight * 0.07).clamp(10.0, 13.0);

            return Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                // Room name label — constellation designation style
                Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 4),
                  child: Text(
                    room.name.toUpperCase(),
                    textAlign: TextAlign.center,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: isLightOn
                          ? CelestialColors.textPrimary.withValues(alpha: 0.9)
                          : CelestialColors.textSecondary
                              .withValues(alpha: 0.6),
                      fontSize: labelFontSize,
                      fontWeight: FontWeight.w500,
                      letterSpacing: 1.8,
                    ),
                  ),
                ),
                Expanded(
                  child: LayoutBuilder(
                    builder: (context, constraints) {
                      final orbSize =
                          math.min(constraints.maxWidth, constraints.maxHeight);
                      final moreButtonSize = (orbSize * 0.22).clamp(26.0, 38.0);
                      final hitSize = math.max(moreButtonSize + 10, 44.0);
                      final ringMidRadius = orbSize * 0.38;
                      const moreAngle = math.pi / 4;
                      final cx = constraints.maxWidth / 2 +
                          ringMidRadius * math.cos(moreAngle);
                      final cy = constraints.maxHeight / 2 +
                          ringMidRadius * math.sin(moreAngle);
                      const targetAngle = -math.pi / 4; // top-right
                      final cxTarget = constraints.maxWidth / 2 +
                          ringMidRadius * math.cos(targetAngle);
                      final cyTarget = constraints.maxHeight / 2 +
                          ringMidRadius * math.sin(targetAngle);
                      const settingsAngle = 3 * math.pi / 4; // bottom-left
                      final cxSettings = constraints.maxWidth / 2 +
                          ringMidRadius * math.cos(settingsAngle);
                      final cySettings = constraints.maxHeight / 2 +
                          ringMidRadius * math.sin(settingsAngle);
                      const motionAngle = -3 * math.pi / 4; // top-left
                      final cxMotion = constraints.maxWidth / 2 +
                          ringMidRadius * math.cos(motionAngle);
                      final cyMotion = constraints.maxHeight / 2 +
                          ringMidRadius * math.sin(motionAngle);
                      return Stack(
                        clipBehavior: Clip.none,
                        children: [
                          Container(
                            decoration: BoxDecoration(
                              shape: BoxShape.circle,
                              boxShadow: isLightOn
                                  ? [
                                      BoxShadow(
                                        color: CelestialColors.sunWarm
                                            .withValues(alpha: 0.15),
                                        blurRadius: 30,
                                        spreadRadius: 5,
                                      ),
                                    ]
                                  : [],
                            ),
                            child: SolarOrbit(
                              curveData: widget.curveData,
                              selectedHour: selectedHour,
                              isTuneMode: false,
                              isLightOn: isLightOn,
                              isRhythmMode: room.rhythmEnabled,
                              brightness: brightness,
                              showTimeLabels: false,
                              eagerGestures: true,
                              onHourChanged: _onHourChanged,
                              onHourChangeEnd: _onHourChangeEnd,
                              onSunTap: _toggleLight,
                              onBrightnessChanged: _onBrightnessChanged,
                              onBrightnessChangeEnd: _onBrightnessChangeEnd,
                            ),
                          ),
                          // Rhythm pause/play satellite — visible when light is on and RhythmServer paired
                          if (isLightOn &&
                              context
                                      .read<HomeProvider>()
                                      .getFirstHubOfType(HubType.server) !=
                                  null)
                            Positioned(
                              left: cx - hitSize / 2,
                              top: cy - hitSize / 2,
                              child: SizedBox(
                                width: hitSize,
                                height: hitSize,
                                child: GestureDetector(
                                  behavior: HitTestBehavior.opaque,
                                  onTap: () => _onPlayPauseTap(room),
                                  child: Center(
                                    child: Container(
                                      width: moreButtonSize,
                                      height: moreButtonSize,
                                      decoration: BoxDecoration(
                                        shape: BoxShape.circle,
                                        color: const Color(0xFF2A2F38),
                                        boxShadow: [
                                          BoxShadow(
                                            color: Colors.black
                                                .withValues(alpha: 0.5),
                                            blurRadius: 4,
                                            offset: const Offset(1.5, 2),
                                          ),
                                          BoxShadow(
                                            color: const Color(0xFF444D5A)
                                                .withValues(alpha: 0.4),
                                            blurRadius: 3,
                                            offset: const Offset(-1, -1),
                                          ),
                                          BoxShadow(
                                            color: CelestialColors.sunWarm
                                                .withValues(alpha: 0.1),
                                            blurRadius: 6,
                                            spreadRadius: 1,
                                          ),
                                        ],
                                      ),
                                      child: AnimatedSwitcher(
                                        duration:
                                            const Duration(milliseconds: 150),
                                        switchInCurve: Curves.easeOut,
                                        switchOutCurve: Curves.easeIn,
                                        child: room.rhythmEnabled
                                            // Rhythm active: pause icon
                                            ? Row(
                                                key: const ValueKey('pause'),
                                                mainAxisAlignment:
                                                    MainAxisAlignment.center,
                                                children: [
                                                  Container(
                                                    width:
                                                        moreButtonSize * 0.14,
                                                    height:
                                                        moreButtonSize * 0.42,
                                                    decoration: BoxDecoration(
                                                      borderRadius:
                                                          BorderRadius.circular(
                                                              1.5),
                                                      color: CelestialColors
                                                          .sunWarm
                                                          .withValues(
                                                              alpha: 0.9),
                                                    ),
                                                  ),
                                                  SizedBox(
                                                      width: moreButtonSize *
                                                          0.14),
                                                  Container(
                                                    width:
                                                        moreButtonSize * 0.14,
                                                    height:
                                                        moreButtonSize * 0.42,
                                                    decoration: BoxDecoration(
                                                      borderRadius:
                                                          BorderRadius.circular(
                                                              1.5),
                                                      color: CelestialColors
                                                          .sunWarm
                                                          .withValues(
                                                              alpha: 0.9),
                                                    ),
                                                  ),
                                                ],
                                              )
                                            // Rhythm paused: play icon
                                            : Icon(
                                                Icons.play_arrow_rounded,
                                                key: const ValueKey('play'),
                                                size: moreButtonSize * 0.55,
                                                color: CelestialColors.sunWarm
                                                    .withValues(alpha: 0.9),
                                              ),
                                      ),
                                    ),
                                  ),
                                ),
                              ),
                            ),
                          // Target satellite — visible when dragged away from "now"
                          if (isLightOn &&
                              (_manualBrightness != null ||
                                  room.timeOffsetMinutes != 0.0))
                            Positioned(
                              left: cxTarget - hitSize / 2,
                              top: cyTarget - hitSize / 2,
                              child: SizedBox(
                                width: hitSize,
                                height: hitSize,
                                child: GestureDetector(
                                  behavior: HitTestBehavior.opaque,
                                  onTap: _resetToNow,
                                  child: Center(
                                    child: Container(
                                      width: moreButtonSize,
                                      height: moreButtonSize,
                                      decoration: BoxDecoration(
                                        shape: BoxShape.circle,
                                        color: const Color(0xFF2A2F38),
                                        boxShadow: [
                                          BoxShadow(
                                            color: Colors.black
                                                .withValues(alpha: 0.5),
                                            blurRadius: 4,
                                            offset: const Offset(1.5, 2),
                                          ),
                                          BoxShadow(
                                            color: const Color(0xFF444D5A)
                                                .withValues(alpha: 0.4),
                                            blurRadius: 3,
                                            offset: const Offset(-1, -1),
                                          ),
                                          BoxShadow(
                                            color: CelestialColors.accentBlue
                                                .withValues(alpha: 0.1),
                                            blurRadius: 6,
                                            spreadRadius: 1,
                                          ),
                                        ],
                                      ),
                                      child: Icon(
                                        Icons.my_location_rounded,
                                        size: moreButtonSize * 0.5,
                                        color: CelestialColors.accentBlue
                                            .withValues(alpha: 0.9),
                                      ),
                                    ),
                                  ),
                                ),
                              ),
                            ),
                          // Settings ellipsis satellite — visible in settings mode
                          Positioned(
                            left: cxSettings - hitSize / 2,
                            top: cySettings - hitSize / 2,
                            child: AnimatedScale(
                              scale: widget.isSettingsMode ? 1.0 : 0.0,
                              duration: const Duration(milliseconds: 300),
                              curve: widget.isSettingsMode
                                  ? Curves.elasticOut
                                  : Curves.easeIn,
                              child: AnimatedOpacity(
                                opacity: widget.isSettingsMode ? 1.0 : 0.0,
                                duration: const Duration(milliseconds: 200),
                                child: SizedBox(
                                  width: hitSize,
                                  height: hitSize,
                                  child: GestureDetector(
                                    behavior: HitTestBehavior.opaque,
                                    onTap: () =>
                                        RoomSettingsSheet.show(context, room),
                                    child: Center(
                                      child: Container(
                                        width: moreButtonSize,
                                        height: moreButtonSize,
                                        decoration: BoxDecoration(
                                          shape: BoxShape.circle,
                                          color: CelestialColors.sunWarm
                                              .withValues(alpha: 0.15),
                                          border: Border.all(
                                            color: CelestialColors.sunWarm
                                                .withValues(alpha: 0.5),
                                            width: 1,
                                          ),
                                          boxShadow: [
                                            BoxShadow(
                                              color: Colors.black
                                                  .withValues(alpha: 0.5),
                                              blurRadius: 4,
                                              offset: const Offset(1.5, 2),
                                            ),
                                            BoxShadow(
                                              color: CelestialColors.sunWarm
                                                  .withValues(alpha: 0.15),
                                              blurRadius: 6,
                                              spreadRadius: 1,
                                            ),
                                          ],
                                        ),
                                        child: Icon(
                                          Icons.more_horiz,
                                          size: moreButtonSize * 0.55,
                                          color: CelestialColors.sunWarm
                                              .withValues(alpha: 0.9),
                                        ),
                                      ),
                                    ),
                                  ),
                                ),
                              ),
                            ),
                          ),
                          // Motion timer satellite — visible when motion tracking active
                          if (motionTimer != null)
                            Positioned(
                              left: cxMotion - hitSize / 2,
                              top: cyMotion - hitSize / 2,
                              child: _MotionTimerSatellite(
                                info: motionTimer,
                                size: moreButtonSize,
                              ),
                            ),
                        ],
                      );
                    },
                  ),
                ),
              ],
            );
          },
        );
      },
    );
  }
}

// =============================================================================
// Motion Timer Satellite
// =============================================================================

/// Satellite widget showing motion timer state on a room orb.
///
/// When motion is active: shows a walk icon with pulse animation.
/// When counting down: shows a depleting ring with time text.
/// Smooth countdown interpolates locally between 15s Rhythm bridge polls.
class _MotionTimerSatellite extends StatefulWidget {
  final MotionTimerInfo info;
  final double size;

  const _MotionTimerSatellite({
    required this.info,
    required this.size,
  });

  @override
  State<_MotionTimerSatellite> createState() => _MotionTimerSatelliteState();
}

class _MotionTimerSatelliteState extends State<_MotionTimerSatellite>
    with SingleTickerProviderStateMixin {
  Timer? _countdownTimer;
  int _interpolatedRemaining = 0;
  late AnimationController _pulseController;

  @override
  void initState() {
    super.initState();
    _pulseController = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 1500),
    );
    _syncFromInfo();
  }

  @override
  void didUpdateWidget(_MotionTimerSatellite oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.info != widget.info) {
      _syncFromInfo();
    }
  }

  void _syncFromInfo() {
    if (widget.info.motionActive) {
      // Active motion — show walk icon with pulse
      _countdownTimer?.cancel();
      _countdownTimer = null;
      if (!_pulseController.isAnimating) {
        _pulseController.repeat(reverse: true);
      }
    } else if (widget.info.remainingSecs != null) {
      // Counting down — start local interpolation
      _pulseController.stop();
      _pulseController.value = 0;
      _interpolatedRemaining = widget.info.remainingSecs!;
      _startCountdown();
    }
  }

  void _startCountdown() {
    _countdownTimer?.cancel();
    _countdownTimer = Timer.periodic(const Duration(seconds: 1), (_) {
      if (!mounted) return;
      final elapsed =
          DateTime.now().difference(widget.info.receivedAt).inSeconds;
      final remaining = (widget.info.remainingSecs ?? 0) - elapsed;
      setState(() {
        _interpolatedRemaining = remaining.clamp(0, widget.info.timeoutSecs);
      });
    });
  }

  @override
  void dispose() {
    _countdownTimer?.cancel();
    _pulseController.dispose();
    super.dispose();
  }

  String _formatTime(int secs) {
    if (secs >= 60) {
      return '${(secs / 60).ceil()}m';
    }
    return '${secs}s';
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedScale(
      scale: 1.0,
      duration: const Duration(milliseconds: 300),
      curve: Curves.elasticOut,
      child: AnimatedOpacity(
        opacity: 1.0,
        duration: const Duration(milliseconds: 200),
        child: SizedBox(
          width: widget.size,
          height: widget.size,
          child: widget.info.motionActive
              ? _buildActiveIcon()
              : _buildCountdownRing(),
        ),
      ),
    );
  }

  Widget _buildActiveIcon() {
    return AnimatedBuilder(
      animation: _pulseController,
      builder: (context, child) {
        final scale = 1.0 + _pulseController.value * 0.08;
        return Transform.scale(
          scale: scale,
          child: child,
        );
      },
      child: Container(
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          color: const Color(0xFF2A2F38),
          boxShadow: [
            BoxShadow(
              color: Colors.black.withValues(alpha: 0.5),
              blurRadius: 4,
              offset: const Offset(1.5, 2),
            ),
            BoxShadow(
              color: const Color(0xFF444D5A).withValues(alpha: 0.4),
              blurRadius: 3,
              offset: const Offset(-1, -1),
            ),
            BoxShadow(
              color: CelestialColors.accentBlue.withValues(alpha: 0.15),
              blurRadius: 6,
              spreadRadius: 1,
            ),
          ],
        ),
        child: Icon(
          Icons.directions_walk_rounded,
          size: widget.size * 0.5,
          color: CelestialColors.accentBlue.withValues(alpha: 0.9),
        ),
      ),
    );
  }

  Widget _buildCountdownRing() {
    final progress = widget.info.timeoutSecs > 0
        ? _interpolatedRemaining.toDouble() / widget.info.timeoutSecs.toDouble()
        : 0.0;

    return Container(
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        boxShadow: [
          BoxShadow(
            color: Colors.black.withValues(alpha: 0.5),
            blurRadius: 4,
            offset: const Offset(1.5, 2),
          ),
          BoxShadow(
            color: const Color(0xFF444D5A).withValues(alpha: 0.4),
            blurRadius: 3,
            offset: const Offset(-1, -1),
          ),
        ],
      ),
      child: CustomPaint(
        painter: _CountdownRingPainter(
          progress: progress,
          ringColor: CelestialColors.accentBlue,
          backgroundColor: const Color(0xFF2A2F38),
        ),
        child: Center(
          child: Text(
            _formatTime(_interpolatedRemaining),
            style: TextStyle(
              color: CelestialColors.accentBlue.withValues(alpha: 0.9),
              fontSize: widget.size * 0.28,
              fontWeight: FontWeight.w600,
              height: 1,
            ),
          ),
        ),
      ),
    );
  }
}

/// Custom painter for the countdown ring.
class _CountdownRingPainter extends CustomPainter {
  final double progress;
  final Color ringColor;
  final Color backgroundColor;

  _CountdownRingPainter({
    required this.progress,
    required this.ringColor,
    required this.backgroundColor,
  });

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);
    final radius = size.width / 2;
    final strokeWidth = size.width * 0.08;

    // Background circle
    canvas.drawCircle(
      center,
      radius,
      Paint()..color = backgroundColor,
    );

    // Ring track
    canvas.drawCircle(
      center,
      radius - strokeWidth / 2,
      Paint()
        ..color = ringColor.withValues(alpha: 0.15)
        ..style = PaintingStyle.stroke
        ..strokeWidth = strokeWidth,
    );

    // Progress arc (depleting clockwise from top)
    if (progress > 0) {
      final rect = Rect.fromCircle(
        center: center,
        radius: radius - strokeWidth / 2,
      );
      canvas.drawArc(
        rect,
        -math.pi / 2, // start from top
        2 * math.pi * progress, // sweep
        false,
        Paint()
          ..color = ringColor.withValues(alpha: 0.8)
          ..style = PaintingStyle.stroke
          ..strokeWidth = strokeWidth
          ..strokeCap = StrokeCap.round,
      );
    }
  }

  @override
  bool shouldRepaint(_CountdownRingPainter oldDelegate) =>
      oldDelegate.progress != progress;
}
