import 'dart:async';
import 'dart:math' as math;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import '../providers/server_sync_provider.dart';
import '../providers/room_provider.dart';
import 'room_settings_sheet.dart';
import 'solar_orbit.dart'; // For CelestialColors

/// Light mode for a room card.
enum RoomMode { on, idle, off }

/// Default idle brightness percentage — very dim nightlight level.
const _kDefaultIdleBrightness = 1;

/// Hue-style room card with CCT-tinted background, three-state celestial
/// toggle (on / idle / off), rhythm controls, and brightness slider.
class RoomCard extends StatefulWidget {
  final String roomId;
  final CurveConfigDto globalConfig;
  final CurveData? curveData;
  final bool powerSave;
  final int softOffBrightness;

  const RoomCard({
    super.key,
    required this.roomId,
    required this.globalConfig,
    this.curveData,
    this.powerSave = false,
    this.softOffBrightness = _kDefaultIdleBrightness,
  });

  @override
  State<RoomCard> createState() => _RoomCardState();
}

class _RoomCardState extends State<RoomCard> {
  /// Non-null when the user is dragging the slider (local override).
  int? _sliderBrightness;
  int _lastResetGen = 0;

  /// Handle three-state mode transitions.
  ///
  /// Tap: ON ↔ IDLE (the common path)
  /// Long-press: → OFF (deliberate action)
  /// Tap from OFF: → ON
  void _onModeChanged(RoomMode newMode) {
    final roomProvider = context.read<RoomProvider>();
    final room = roomProvider.getRoom(widget.roomId);
    if (room == null) return;

    final serverSync = context.read<ServerSyncProvider>();

    // Capture previous state before optimistic updates
    final wasIdle = roomProvider.isRoomIdle(widget.roomId);
    final wasOn = room.lightsOn;

    // Optimistic local state update
    switch (newMode) {
      case RoomMode.on:
        HapticFeedback.mediumImpact();
        roomProvider.setRoomLightsOnLocal(widget.roomId, true);
        roomProvider.setRoomRhythmEnabled(widget.roomId, true);
        roomProvider.setRoomIdle(widget.roomId, false);
      case RoomMode.idle:
        HapticFeedback.lightImpact();
        roomProvider.setRoomRhythmEnabled(widget.roomId, true);
        roomProvider.setRoomIdle(widget.roomId, true);
      case RoomMode.off:
        HapticFeedback.heavyImpact();
        roomProvider.setRoomLightsOnLocal(widget.roomId, false);
        roomProvider.setRoomIdle(widget.roomId, false);
    }

    switch (newMode) {
      case RoomMode.on:
        // If was idle (soft-off), clear soft_off via preferences — the
        // server will immediately turn on at full adaptive values.
        // Otherwise dispatch a normal 'on' action.
        if (wasOn && wasIdle) {
          serverSync.pushRoomPreferences(widget.roomId,
              rhythmEnabled: true, softOff: false);
        } else {
          serverSync.dispatchAction(widget.roomId, 'on');
        }
      case RoomMode.idle:
        // Push soft_off preference — server immediately applies
        // soft-off brightness via soft_off_tick.
        serverSync.pushRoomPreferences(widget.roomId,
            rhythmEnabled: true, softOff: true);
      case RoomMode.off:
        serverSync.dispatchAction(widget.roomId, 'lights_off');
    }

    setState(() {
      _sliderBrightness = null;
    });
  }

  void _onBrightnessSliderEnd() {
    if (_sliderBrightness == null) return;
    final serverSync = context.read<ServerSyncProvider>();
    serverSync.dispatchBrightness(widget.roomId, _sliderBrightness!);
  }

  @override
  Widget build(BuildContext context) {
    return Selector<RoomProvider, (RoomDto?, int, MotionTimerInfo?, bool, int?, int?, bool)>(
      selector: (_, p) => (
        p.getRoom(widget.roomId),
        p.resetGeneration,
        p.getMotionTimer(widget.roomId),
        p.isRoomIdle(widget.roomId),
        p.getBrightness(widget.roomId),
        p.getKelvin(widget.roomId),
        p.hasMotionSensor(widget.roomId),
      ),
      builder: (context, data, _) {
        final (room, resetGen, motionTimer, isIdle, serverBrightness, serverKelvin, hasSensor) = data;
        if (room == null) return const SizedBox.shrink();

        // External reset bumps the generation counter — drop local overrides
        if (resetGen != _lastResetGen) {
          _lastResetGen = resetGen;
          _sliderBrightness = null;
        }

        // Determine room mode (power save skips idle)
        final mode = (!widget.powerSave && isIdle)
            ? RoomMode.idle
            : !room.lightsOn
                ? RoomMode.off
                : RoomMode.on;

        // Use server-provided brightness and kelvin
        final brightness = serverBrightness ?? 50;
        final kelvin = serverKelvin ?? 3000;

        // Display brightness depends on mode
        final displayBrightness = switch (mode) {
          RoomMode.on => _sliderBrightness ?? brightness,
          RoomMode.idle => widget.softOffBrightness,
          RoomMode.off => _sliderBrightness ?? brightness,
        };

        final cctColor = ColorUtils.cctToColor(kelvin);

        // Use CCT color directly, lightly softened with white
        final softCct = Color.lerp(
          Colors.white,
          cctColor,
          0.55,
        )!;
        // Dim cards darken the CCT color itself instead of blending toward
        // the dark card background — keeps hue visible even at low brightness.
        final dimT = 0.35 + displayBrightness / 100.0 * 0.65; // 0.35–1.0
        final dimmedCct = Color.lerp(
          const Color(0xFF1A1410), // very dark warm neutral (not pure black)
          softCct,
          dimT,
        )!;

        // Card background per mode
        final bgColor = switch (mode) {
          RoomMode.on => dimmedCct,
          // Idle: warm nightlight glow on dark card
          RoomMode.idle =>
            Color.lerp(CelestialColors.backgroundCard, cctColor, 0.18)!,
          RoomMode.off => CelestialColors.backgroundCard,
        };

        // Text and icon colors per mode — use luminance-based contrast
        // (same approach as _RhythmPill) so names stay readable at any brightness.
        final onLight = mode == RoomMode.on && bgColor.computeLuminance() > 0.4;
        final textColor = switch (mode) {
          RoomMode.on => onLight
              ? (kelvin < 4000
                  ? const Color(0xFF3A2A1A) // warm dark brown
                  : const Color(0xFF2A2C30)) // cool dark grey
              : Colors.white,
          RoomMode.idle => const Color(0xFFB8A890), // warm muted
          RoomMode.off => CelestialColors.textSecondary,
        };
        final iconColor = switch (mode) {
          RoomMode.on => onLight
              ? (kelvin < 4000
                  ? const Color(0xFF4A3828) // warm brown
                  : const Color(0xFF3A3C42)) // cool grey
              : Colors.white.withValues(alpha: 0.85),
          RoomMode.idle => const Color(0xFFA08E78),
          RoomMode.off => CelestialColors.textSecondary,
        };

        final sliderActive = mode == RoomMode.on;
        final rhythmGlowActive = mode == RoomMode.on && room.rhythmEnabled;
        // Warm amber on dark cards, contrast-aware dark tone on light cards
        final glowColor = onLight ? iconColor : CelestialColors.sunWarm;

        return GestureDetector(
          onTap: () => RoomSettingsSheet.show(context, room),
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 400),
            curve: Curves.easeInOut,
            clipBehavior: Clip.antiAlias,
            decoration: BoxDecoration(
              color: bgColor,
              borderRadius: BorderRadius.circular(20),
            ),
            child: Stack(
              children: [
                // Subtle radial glow for idle mode — like moonlight
                if (mode == RoomMode.idle)
                  Positioned.fill(
                    child: IgnorePointer(
                      child: DecoratedBox(
                        decoration: BoxDecoration(
                          borderRadius: BorderRadius.circular(20),
                          gradient: RadialGradient(
                            center: const Alignment(0.65, -0.4),
                            radius: 0.9,
                            colors: [
                              cctColor.withValues(alpha: 0.12),
                              Colors.transparent,
                            ],
                          ),
                        ),
                      ),
                    ),
                  ),
                // Main content
                Column(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    // Top row: [motion+name]  [pill]  [toggle]
                    Padding(
                      padding: const EdgeInsets.fromLTRB(12, 14, 8, 0),
                      child: Row(
                        children: [
                          // Left: motion + name
                          Expanded(
                            child: Row(
                              children: [
                                if (motionTimer != null)
                                  Padding(
                                    padding: const EdgeInsets.only(right: 8),
                                    child: _MotionIndicator(
                                      info: motionTimer,
                                      color: iconColor,
                                      onExpired: () => context.read<RoomProvider>().clearMotionTimer(widget.roomId),
                                    ),
                                  )
                                else if (hasSensor)
                                  Padding(
                                    padding: const EdgeInsets.only(right: 8),
                                    child: Icon(
                                      Icons.sensors_rounded,
                                      size: 18,
                                      color: iconColor.withValues(alpha: 0.45),
                                    ),
                                  ),
                                Flexible(
                                  child: Text(
                                    room.name,
                                    maxLines: 1,
                                    overflow: TextOverflow.ellipsis,
                                    style: TextStyle(
                                      color: textColor,
                                      fontSize: 16,
                                      fontWeight: FontWeight.w600,
                                    ),
                                  ),
                                ),
                              ],
                            ),
                          ),
                          if (mode == RoomMode.idle)
                            Padding(
                              padding: const EdgeInsets.only(right: 8),
                              child: _IdlePill(
                                color: iconColor,
                                bgColor: cctColor.withValues(alpha: 0.08),
                              ),
                            ),
                          // Right: celestial toggle
                          _CelestialToggle(
                            mode: mode,
                            onModeChanged: _onModeChanged,
                            powerSave: widget.powerSave,
                          ),
                        ],
                      ),
                    ),
                    // Brightness slider
                    Padding(
                      padding: const EdgeInsets.fromLTRB(12, 4, 12, 10),
                      child: SliderTheme(
                        data: SliderThemeData(
                          trackHeight: 6,
                          thumbShape: const RoundSliderThumbShape(
                            enabledThumbRadius: 8,
                          ),
                          overlayShape: const RoundSliderOverlayShape(
                            overlayRadius: 16,
                          ),
                          // Active colors (ON mode)
                          activeTrackColor:
                              Colors.black.withValues(alpha: 0.08),
                          inactiveTrackColor:
                              Colors.black.withValues(alpha: 0.06),
                          thumbColor: Colors.white,
                          overlayColor: Colors.black.withValues(alpha: 0.06),
                          // Disabled colors (idle or off)
                          disabledActiveTrackColor: mode == RoomMode.idle
                              ? cctColor.withValues(alpha: 0.12)
                              : CelestialColors.orbitRing
                                  .withValues(alpha: 0.3),
                          disabledInactiveTrackColor: mode == RoomMode.idle
                              ? cctColor.withValues(alpha: 0.06)
                              : CelestialColors.orbitRing
                                  .withValues(alpha: 0.2),
                          disabledThumbColor: mode == RoomMode.idle
                              ? cctColor.withValues(alpha: 0.3)
                              : CelestialColors.textSecondary
                                  .withValues(alpha: 0.5),
                          trackShape: const RoundedRectSliderTrackShape(),
                        ),
                        child: Slider(
                          value: displayBrightness.toDouble().clamp(1, 100),
                          min: 1,
                          max: 100,
                          onChanged: sliderActive
                              ? (v) {
                                  setState(() {
                                    _sliderBrightness = v.round();
                                  });
                                }
                              : null,
                          onChangeEnd: sliderActive
                              ? (_) => _onBrightnessSliderEnd()
                              : null,
                        ),
                      ),
                    ),
                  ],
                ),
                // Rhythm-active horizon glow at bottom edge
                Positioned(
                  left: 0,
                  right: 0,
                  bottom: 0,
                  child: IgnorePointer(
                    child: AnimatedOpacity(
                      opacity: rhythmGlowActive ? 1.0 : 0.0,
                      duration: const Duration(milliseconds: 500),
                      curve: Curves.easeInOut,
                      child: _RhythmHorizonGlow(color: glowColor),
                    ),
                  ),
                ),
              ],
            ),
          ),
        );
      },
    );
  }
}

// ---------------------------------------------------------------------------
// Sub-widgets
// ---------------------------------------------------------------------------

/// Celestial toggle: three-state (OFF → IDLE → ON) or two-state (OFF ↔ ON)
/// when power save is active.
///
/// Three-state: Tap toggles ON ↔ IDLE. Long-press goes to OFF. Tap from OFF → ON.
/// Two-state:   Tap toggles ON ↔ OFF.
/// Track warms from dark to amber as mode brightens.
class _CelestialToggle extends StatelessWidget {
  final RoomMode mode;
  final ValueChanged<RoomMode> onModeChanged;
  final bool powerSave;

  const _CelestialToggle({
    required this.mode,
    required this.onModeChanged,
    this.powerSave = false,
  });

  void _onTap() {
    if (powerSave) {
      // Two-state: simple ON ↔ OFF toggle
      onModeChanged(mode == RoomMode.on ? RoomMode.off : RoomMode.on);
    } else {
      // Three-state: toggle ON ↔ IDLE; from OFF → ON
      switch (mode) {
        case RoomMode.on:
          onModeChanged(RoomMode.idle);
        case RoomMode.idle:
          onModeChanged(RoomMode.on);
        case RoomMode.off:
          onModeChanged(RoomMode.on);
      }
    }
  }

  void _onLongPress() {
    // Long-press: hard off from any state
    if (mode != RoomMode.off) {
      onModeChanged(RoomMode.off);
    }
  }

  @override
  Widget build(BuildContext context) {
    // Power save: two positions only (left/right). Normal: three positions.
    final alignment = switch (mode) {
      RoomMode.off => Alignment.centerLeft,
      RoomMode.idle => Alignment.center,
      RoomMode.on => Alignment.centerRight,
    };

    // Track gradient: dark → dim warm → warm amber
    final trackGradient = switch (mode) {
      RoomMode.off => const LinearGradient(
          colors: [Color(0xFF2A2F38), Color(0xFF30363D)],
        ),
      RoomMode.idle => const LinearGradient(
          colors: [Color(0xFF221C14), Color(0xFF2E2518)],
        ),
      RoomMode.on => const LinearGradient(
          colors: [Color(0xFF8B6B20), Color(0xFFD4A020)],
        ),
    };

    // Thumb colors: void → dim warm → bright
    final thumbColor = switch (mode) {
      RoomMode.off => CelestialColors.textSecondary,
      RoomMode.idle => const Color(0xFFCDBFAA), // dim warm
      RoomMode.on => Colors.white,
    };

    // Thumb glow per state
    final thumbShadow = switch (mode) {
      RoomMode.on => [
          BoxShadow(
            color: CelestialColors.sunWarm.withValues(alpha: 0.4),
            blurRadius: 8,
            spreadRadius: 1,
          ),
        ],
      RoomMode.idle => [
          BoxShadow(
            color: const Color(0xFFD4A574).withValues(alpha: 0.2),
            blurRadius: 6,
          ),
        ],
      RoomMode.off => <BoxShadow>[],
    };

    return GestureDetector(
      onTap: _onTap,
      onLongPress: _onLongPress,
      behavior: HitTestBehavior.opaque,
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 300),
        curve: Curves.easeInOut,
        width: 64,
        height: 28,
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(14),
          gradient: trackGradient,
        ),
        padding: const EdgeInsets.all(3),
        child: AnimatedAlign(
          duration: const Duration(milliseconds: 300),
          curve: Curves.easeInOut,
          alignment: alignment,
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 300),
            width: 22,
            height: 22,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: thumbColor,
              boxShadow: thumbShadow,
            ),
            child: null,
          ),
        ),
      ),
    );
  }
}

/// Idle indicator pill — dim-light icon + "Idle" label.
class _IdlePill extends StatelessWidget {
  final Color color;
  final Color bgColor;

  const _IdlePill({required this.color, required this.bgColor});

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 6),
      decoration: BoxDecoration(
        color: bgColor,
        borderRadius: BorderRadius.circular(20),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(
            Icons.brightness_low_rounded,
            color: color,
            size: 16,
          ),
          const SizedBox(width: 4),
          Text(
            'Idle',
            style: TextStyle(
              color: color,
              fontSize: 13,
              fontWeight: FontWeight.w600,
            ),
          ),
        ],
      ),
    );
  }
}

/// Compact motion indicator: walk icon when active, countdown text when timing out.
class _MotionIndicator extends StatefulWidget {
  final MotionTimerInfo info;
  final Color color;
  final VoidCallback? onExpired;

  const _MotionIndicator({required this.info, required this.color, this.onExpired});

  @override
  State<_MotionIndicator> createState() => _MotionIndicatorState();
}

class _MotionIndicatorState extends State<_MotionIndicator>
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
  void didUpdateWidget(_MotionIndicator oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.info != widget.info) {
      _syncFromInfo();
    }
  }

  void _syncFromInfo() {
    if (widget.info.motionActive) {
      _countdownTimer?.cancel();
      _countdownTimer = null;
      if (!_pulseController.isAnimating) {
        _pulseController.repeat(reverse: true);
      }
    } else if (widget.info.remainingSecs != null) {
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
      if (remaining <= 0) {
        _countdownTimer?.cancel();
        _countdownTimer = null;
        widget.onExpired?.call();
        return;
      }
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
    if (secs >= 60) return '${(secs / 60).ceil()}m';
    return '${secs}s';
  }

  @override
  Widget build(BuildContext context) {
    if (widget.info.motionActive) {
      return AnimatedBuilder(
        animation: _pulseController,
        builder: (context, child) {
          final scale = 1.0 + _pulseController.value * 0.1;
          return Transform.scale(scale: scale, child: child);
        },
        child: Icon(
          Icons.directions_walk_rounded,
          size: 20,
          color: widget.color,
        ),
      );
    }

    // Countdown mode: icon + remaining time
    final progress = widget.info.timeoutSecs > 0
        ? _interpolatedRemaining / widget.info.timeoutSecs
        : 0.0;

    return SizedBox(
      width: 28,
      height: 28,
      child: CustomPaint(
        painter: _MiniCountdownPainter(
          progress: progress,
          color: widget.color,
        ),
        child: Center(
          child: Text(
            _formatTime(_interpolatedRemaining),
            style: TextStyle(
              color: widget.color,
              fontSize: 9,
              fontWeight: FontWeight.w700,
              height: 1,
            ),
          ),
        ),
      ),
    );
  }
}

/// Tiny ring painter for the countdown indicator.
class _MiniCountdownPainter extends CustomPainter {
  final double progress;
  final Color color;

  _MiniCountdownPainter({required this.progress, required this.color});

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);
    final radius = size.width / 2 - 1.5;
    const strokeWidth = 2.5;

    // Track
    canvas.drawCircle(
      center,
      radius,
      Paint()
        ..color = color.withValues(alpha: 0.15)
        ..style = PaintingStyle.stroke
        ..strokeWidth = strokeWidth,
    );

    // Progress arc
    if (progress > 0) {
      canvas.drawArc(
        Rect.fromCircle(center: center, radius: radius),
        -math.pi / 2,
        2 * math.pi * progress,
        false,
        Paint()
          ..color = color.withValues(alpha: 0.7)
          ..style = PaintingStyle.stroke
          ..strokeWidth = strokeWidth
          ..strokeCap = StrokeCap.round,
      );
    }
  }

  @override
  bool shouldRepaint(_MiniCountdownPainter oldDelegate) =>
      oldDelegate.progress != progress || oldDelegate.color != color;
}

/// Horizon glow at the bottom of a room card when rhythm is active.
///
/// Two layers: a tall upward-fading gradient (the "glow") provides
/// visual mass visible on any background, plus a bright 3px accent
/// strip at the very bottom with a traveling shimmer highlight.
class _RhythmHorizonGlow extends StatefulWidget {
  final Color color;

  const _RhythmHorizonGlow({required this.color});

  @override
  State<_RhythmHorizonGlow> createState() => _RhythmHorizonGlowState();
}

class _RhythmHorizonGlowState extends State<_RhythmHorizonGlow>
    with SingleTickerProviderStateMixin {
  late AnimationController _shimmer;

  @override
  void initState() {
    super.initState();
    _shimmer = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 3000),
    )..repeat();
  }

  @override
  void dispose() {
    _shimmer.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: _shimmer,
      builder: (context, _) {
        final t = _shimmer.value;
        final center = -1.5 + t * 3.0;
        return Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            // Upper fade: gradient bleeding upward into the card
            Container(
              height: 28,
              decoration: BoxDecoration(
                gradient: LinearGradient(
                  begin: Alignment.topCenter,
                  end: Alignment.bottomCenter,
                  colors: [
                    Colors.transparent,
                    widget.color.withValues(alpha: 0.15),
                    widget.color.withValues(alpha: 0.45),
                  ],
                  stops: const [0.0, 0.5, 1.0],
                ),
              ),
            ),
            // Bottom accent strip with shimmer
            Container(
              height: 4,
              decoration: BoxDecoration(
                color: widget.color.withValues(alpha: 0.85),
              ),
              foregroundDecoration: BoxDecoration(
                gradient: LinearGradient(
                  begin: Alignment(center - 0.5, 0),
                  end: Alignment(center + 0.5, 0),
                  colors: [
                    Colors.transparent,
                    Colors.white.withValues(alpha: 0.9),
                    Colors.transparent,
                  ],
                ),
              ),
            ),
          ],
        );
      },
    );
  }
}

