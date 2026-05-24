import 'dart:async';
import 'dart:math' as math;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' show RoomModeState;
import '../providers/server_sync_provider.dart';
import '../providers/room_provider.dart';
import '../services/analytics_service.dart';
import 'device_detail_sheet.dart';
import 'room_settings_sheet.dart';
import 'solar_orbit.dart'; // For CelestialColors

/// Light mode for a room card.
enum RoomMode { on, idle, off }

/// Default idle brightness percentage — very dim nightlight level.
const _kDefaultIdleBrightness = 1;

/// Hue-style room card with CCT-tinted background, big segmented power
/// control (mood / off / on), rhythm controls, and brightness slider.
class RoomCard extends StatefulWidget {
  final String roomId;
  final CurveConfigDto globalConfig;
  final CurveData? curveData;
  final bool powerSave;

  const RoomCard({
    super.key,
    required this.roomId,
    required this.globalConfig,
    this.curveData,
    this.powerSave = false,
  });

  @override
  State<RoomCard> createState() => _RoomCardState();
}

class _RoomCardState extends State<RoomCard> {
  /// Non-null when the user is dragging the slider (local override).
  int? _sliderBrightness;
  int _lastResetGen = 0;

  String _analyticsModeForState(RoomModeState state) {
    final mode = switch (state) {
      RoomModeState.hardOff => RoomMode.off,
      RoomModeState.idle ||
      RoomModeState.warning =>
        widget.powerSave ? RoomMode.off : RoomMode.idle,
      RoomModeState.wake || RoomModeState.active => RoomMode.on,
    };
    return mode.name;
  }

  /// Handle three-state mode transitions. Driven by the segmented toggle:
  /// each segment is a tap target, plus horizontal drag for fluency.
  void _onModeChanged(RoomMode newMode) {
    final roomProvider = context.read<RoomProvider>();
    final room = roomProvider.getRoom(widget.roomId);
    if (room == null) return;
    final previousMode =
        _analyticsModeForState(roomProvider.getDisplayRoomState(widget.roomId));

    final serverSync = context.read<ServerSyncProvider>();

    // Optimistic local state update
    switch (newMode) {
      case RoomMode.on:
        HapticFeedback.mediumImpact();
        roomProvider.setRoomLightsOnLocal(widget.roomId, true);
        roomProvider.setRoomRhythmEnabled(widget.roomId, true);
        roomProvider.setRoomStateLocal(widget.roomId, RoomModeState.active);
      case RoomMode.idle:
        HapticFeedback.lightImpact();
        roomProvider.setRoomLightsOnLocal(widget.roomId, true);
        roomProvider.setRoomRhythmEnabled(widget.roomId, true);
        roomProvider.setRoomStateLocal(widget.roomId, RoomModeState.idle);
      case RoomMode.off:
        HapticFeedback.heavyImpact();
        roomProvider.setRoomLightsOnLocal(widget.roomId, false);
        roomProvider.setRoomStateLocal(widget.roomId, RoomModeState.hardOff);
    }

    switch (newMode) {
      case RoomMode.on:
        serverSync.pushNodePreferences(
          widget.roomId,
          rhythmEnabled: true,
          state: RoomModeState.active,
        );
      case RoomMode.idle:
        serverSync.pushNodePreferences(
          widget.roomId,
          rhythmEnabled: true,
          state: RoomModeState.idle,
        );
      case RoomMode.off:
        serverSync.pushNodePreferences(
          widget.roomId,
          state: RoomModeState.hardOff,
        );
    }

    setState(() {
      _sliderBrightness = null;
    });
    AnalyticsService().logRoomModeChanged(
      roomId: widget.roomId,
      previousMode: previousMode,
      nextMode: newMode.name,
    );
  }

  void _onBrightnessSliderEnd() {
    if (_sliderBrightness == null) return;
    final serverSync = context.read<ServerSyncProvider>();
    serverSync.dispatchNodeBrightness(widget.roomId, _sliderBrightness!);
    AnalyticsService().logRoomBrightnessAdjusted(
      roomId: widget.roomId,
      brightness: _sliderBrightness!,
    );
  }

  /// Reset this node to its adaptive curve position.
  void _resetRoom() {
    HapticFeedback.mediumImpact();
    final serverSync = context.read<ServerSyncProvider>();
    serverSync.dispatchResetNode(widget.roomId);
    AnalyticsService().logRoomResetToCurve(roomId: widget.roomId);
    setState(() {
      _sliderBrightness = null;
    });
  }

  @override
  Widget build(BuildContext context) {
    return Selector<
        RoomProvider,
        (
          RoomDto?,
          int,
          MotionTimerInfo?,
          RoomModeState,
          int?,
          int?,
          (int, int, int)?,
          bool
        )>(
      selector: (_, p) => (
        p.getRoom(widget.roomId),
        p.resetGeneration,
        p.getMotionTimer(widget.roomId),
        p.getDisplayRoomState(widget.roomId),
        p.getBrightness(widget.roomId),
        p.getKelvin(widget.roomId),
        p.getRoomColor(widget.roomId),
        p.hasMotionSensor(widget.roomId),
      ),
      builder: (context, data, _) {
        final (
          room,
          resetGen,
          motionTimer,
          roomState,
          serverBrightness,
          serverKelvin,
          serverColor,
          hasSensor
        ) = data;
        if (room == null) return const SizedBox.shrink();

        // Check if this room's hub is reachable.
        final hubConnected = context.select<ServerSyncProvider, bool>(
            (p) => p.isRoomHubConnected(room.source));

        // External reset bumps the generation counter — drop local overrides
        if (resetGen != _lastResetGen) {
          _lastResetGen = resetGen;
          _sliderBrightness = null;
        }

        final mode = switch (roomState) {
          RoomModeState.hardOff => RoomMode.off,
          RoomModeState.idle ||
          RoomModeState.warning =>
            widget.powerSave ? RoomMode.off : RoomMode.idle,
          RoomModeState.wake || RoomModeState.active => RoomMode.on,
        };

        // Use server-provided brightness and kelvin
        final brightness = serverBrightness ?? 50;
        final kelvin = serverKelvin ?? 3000;

        // Display brightness depends on mode
        final displayBrightness = switch (mode) {
          RoomMode.on => _sliderBrightness ?? brightness,
          RoomMode.idle => _kDefaultIdleBrightness,
          RoomMode.off => _sliderBrightness ?? brightness,
        };

        final cctColor = serverColor != null
            ? Color.fromARGB(
                255, serverColor.$1, serverColor.$2, serverColor.$3)
            : ColorUtils.cctToColor(kelvin);

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

        // Card background per mode. Mood reuses the same dark warm base as
        // ON's dim form, with a clear CCT tint so it reads as "softly lit
        // in the room's color" instead of looking off.
        final bgColor = switch (mode) {
          RoomMode.on => dimmedCct,
          RoomMode.idle =>
            Color.lerp(const Color(0xFF1A1410), cctColor, 0.32)!,
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
          RoomMode.idle => const Color(0xFFEFE0C4), // soft warm cream
          RoomMode.off => CelestialColors.textSecondary,
        };
        final iconColor = switch (mode) {
          RoomMode.on => onLight
              ? (kelvin < 4000
                  ? const Color(0xFF4A3828) // warm brown
                  : const Color(0xFF3A3C42)) // cool grey
              : Colors.white.withValues(alpha: 0.85),
          RoomMode.idle => const Color(0xFFD8C5A4),
          RoomMode.off => CelestialColors.textSecondary,
        };

        // Room is "off curve" when brightness or time has been manually adjusted
        final offCurve = mode == RoomMode.on &&
            (room.brightnessOffset != 0 || room.timeOffsetMinutes != 0);

        final sliderActive = mode == RoomMode.on;
        final rhythmGlowActive = mode == RoomMode.on && room.rhythmEnabled;
        // Warm amber on dark cards, contrast-aware dark tone on light cards
        final glowColor = onLight ? iconColor : CelestialColors.sunWarm;

        return IgnorePointer(
          ignoring: !hubConnected,
          child: AnimatedOpacity(
            opacity: hubConnected ? 1.0 : 0.35,
            duration: const Duration(milliseconds: 400),
            child: GestureDetector(
              onTap: () {
                if (room.kind == RoomNodeKind.lightDevice) {
                  final device =
                      context.read<ServerSyncProvider>().deviceForNode(room.id);
                  if (device != null) {
                    DeviceDetailSheet.show(
                      context,
                      device,
                      room.parentId ?? '',
                    );
                    return;
                  }
                }
                RoomSettingsSheet.show(context, room);
              },
              // Only register double-tap when off-curve to avoid tap delay on normal cards
              onDoubleTap: offCurve ? _resetRoom : null,
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
                    // Mood: prominent CCT-colored glow plus a softer counter-
                    // glow for atmospheric depth — sells "lights are softly on
                    // in this color" rather than reading as off.
                    if (mode == RoomMode.idle) ...[
                      Positioned.fill(
                        child: IgnorePointer(
                          child: DecoratedBox(
                            decoration: BoxDecoration(
                              borderRadius: BorderRadius.circular(20),
                              gradient: RadialGradient(
                                center: const Alignment(0.7, -0.6),
                                radius: 1.3,
                                colors: [
                                  cctColor.withValues(alpha: 0.45),
                                  cctColor.withValues(alpha: 0.12),
                                  Colors.transparent,
                                ],
                                stops: const [0.0, 0.5, 1.0],
                              ),
                            ),
                          ),
                        ),
                      ),
                      Positioned.fill(
                        child: IgnorePointer(
                          child: DecoratedBox(
                            decoration: BoxDecoration(
                              borderRadius: BorderRadius.circular(20),
                              gradient: RadialGradient(
                                center: const Alignment(-0.5, 0.9),
                                radius: 0.9,
                                colors: [
                                  cctColor.withValues(alpha: 0.20),
                                  Colors.transparent,
                                ],
                              ),
                            ),
                          ),
                        ),
                      ),
                    ],
                    // Main content
                    Column(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        // Top row: motion/sensor + room name
                        Padding(
                          padding: const EdgeInsets.fromLTRB(14, 14, 14, 0),
                          child: Row(
                            children: [
                              if (motionTimer != null)
                                Padding(
                                  padding: const EdgeInsets.only(right: 8),
                                  child: _MotionIndicator(
                                    info: motionTimer,
                                    color: iconColor,
                                    onExpired: () => context
                                        .read<RoomProvider>()
                                        .clearMotionTimer(widget.roomId),
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
                              if (room.kind == RoomNodeKind.lightDevice)
                                Padding(
                                  padding: const EdgeInsets.only(right: 8),
                                  child: Icon(
                                    Icons.lightbulb_outline_rounded,
                                    size: 18,
                                    color: iconColor.withValues(alpha: 0.55),
                                  ),
                                ),
                              Expanded(
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
                        // Big three-segment power control.
                        Padding(
                          padding:
                              const EdgeInsets.fromLTRB(12, 12, 12, 0),
                          child: _SegmentedToggle(
                            mode: mode,
                            onModeChanged: _onModeChanged,
                            powerSave: widget.powerSave,
                            offCurve: offCurve,
                            cctColor: cctColor,
                          ),
                        ),
                        // Chunky brightness slider — easy to grab. Hidden
                        // when the room is hard-off (nothing to dim).
                        if (mode != RoomMode.off)
                          Padding(
                            padding:
                                const EdgeInsets.fromLTRB(12, 10, 12, 14),
                            child: SliderTheme(
                            data: SliderThemeData(
                              trackHeight: 14,
                              thumbShape: const RoundSliderThumbShape(
                                enabledThumbRadius: 13,
                                elevation: 3,
                                pressedElevation: 6,
                              ),
                              overlayShape: const RoundSliderOverlayShape(
                                overlayRadius: 26,
                              ),
                              // Active colors (ON mode): bright fill on the
                              // left clearly reads as "how much brightness".
                              activeTrackColor:
                                  Colors.white.withValues(alpha: 0.55),
                              inactiveTrackColor:
                                  Colors.black.withValues(alpha: 0.20),
                              thumbColor: Colors.white,
                              overlayColor:
                                  Colors.white.withValues(alpha: 0.12),
                              // Disabled colors (idle or off)
                              disabledActiveTrackColor: mode == RoomMode.idle
                                  ? cctColor.withValues(alpha: 0.30)
                                  : CelestialColors.orbitRing
                                      .withValues(alpha: 0.35),
                              disabledInactiveTrackColor: mode == RoomMode.idle
                                  ? Colors.black.withValues(alpha: 0.15)
                                  : CelestialColors.orbitRing
                                      .withValues(alpha: 0.18),
                              disabledThumbColor: mode == RoomMode.idle
                                  ? Color.lerp(Colors.white, cctColor, 0.25)!
                                  : CelestialColors.textSecondary
                                      .withValues(alpha: 0.55),
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
                          )
                        else
                          const SizedBox(height: 14),
                      ],
                    ),
                    // Rhythm-active breathing border
                    Positioned.fill(
                      child: IgnorePointer(
                        child: _RhythmBorderGlow(
                          active: rhythmGlowActive,
                          color: glowColor,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
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

/// Big segmented power control: Mood | Off | On (or Off | On in power save).
///
/// Each segment is a discrete tap target with a stacked icon + label, so
/// every state is visible and discoverable — no hidden long-press. A
/// highlight pill slides behind the active segment. Horizontal drag also
/// works for users who prefer to flick.
class _SegmentedToggle extends StatefulWidget {
  final RoomMode mode;
  final ValueChanged<RoomMode> onModeChanged;
  final bool powerSave;
  final bool offCurve;
  final Color cctColor;

  const _SegmentedToggle({
    required this.mode,
    required this.onModeChanged,
    required this.cctColor,
    this.powerSave = false,
    this.offCurve = false,
  });

  @override
  State<_SegmentedToggle> createState() => _SegmentedToggleState();
}

class _SegmentedToggleState extends State<_SegmentedToggle> {
  static const double _height = 54;
  static const double _padding = 4;

  /// Index of the segment currently under the dragging finger; null when
  /// not dragging. Lets the highlight preview the drag in real time and
  /// commit to the segment under the finger on release.
  int? _dragIndex;

  static const List<_SegmentSpec> _threeState = [
    _SegmentSpec(RoomMode.idle, Icons.spa_rounded, 'Mood'),
    _SegmentSpec(RoomMode.off, Icons.power_settings_new_rounded, 'Off'),
    _SegmentSpec(RoomMode.on, Icons.wb_sunny_rounded, 'On'),
  ];
  static const List<_SegmentSpec> _twoState = [
    _SegmentSpec(RoomMode.off, Icons.power_settings_new_rounded, 'Off'),
    _SegmentSpec(RoomMode.on, Icons.wb_sunny_rounded, 'On'),
  ];

  List<_SegmentSpec> get _segments =>
      widget.powerSave ? _twoState : _threeState;

  int _indexFor(RoomMode m) {
    final segs = _segments;
    for (var i = 0; i < segs.length; i++) {
      if (segs[i].mode == m) return i;
    }
    return widget.powerSave ? 0 : 1;
  }

  int _indexAtX(double x, double trackWidth) {
    final segs = _segments;
    final usable = trackWidth - _padding * 2;
    if (usable <= 0) return 0;
    final segWidth = usable / segs.length;
    final localX = (x - _padding).clamp(0.0, usable);
    return (localX / segWidth).floor().clamp(0, segs.length - 1);
  }

  void _select(int i) {
    final target = _segments[i].mode;
    if (target != widget.mode) widget.onModeChanged(target);
  }

  @override
  Widget build(BuildContext context) {
    final segments = _segments;
    final restingIndex = _indexFor(widget.mode);
    final activeIndex = _dragIndex ?? restingIndex;
    final activeMode = segments[activeIndex].mode;
    final activeText = _activeTextColor(activeMode);
    final inactiveText = const Color(0xFF8B949E);

    return LayoutBuilder(
      builder: (context, constraints) {
        final trackWidth = constraints.maxWidth;
        final usable = trackWidth - _padding * 2;
        final segWidth = usable / segments.length;

        return GestureDetector(
          behavior: HitTestBehavior.opaque,
          onHorizontalDragStart: (details) {
            setState(() {
              _dragIndex = _indexAtX(details.localPosition.dx, trackWidth);
            });
          },
          onHorizontalDragUpdate: (details) {
            final idx = _indexAtX(details.localPosition.dx, trackWidth);
            if (idx != _dragIndex) {
              setState(() => _dragIndex = idx);
            }
          },
          onHorizontalDragEnd: (_) {
            final idx = _dragIndex;
            if (idx == null) return;
            setState(() => _dragIndex = null);
            _select(idx);
          },
          onHorizontalDragCancel: () {
            if (_dragIndex == null) return;
            setState(() => _dragIndex = null);
          },
          child: Container(
            height: _height,
            clipBehavior: Clip.antiAlias,
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(_height / 2),
              gradient: const LinearGradient(
                colors: [Color(0xFF161B22), Color(0xFF1C222B)],
              ),
            ),
            padding: const EdgeInsets.all(_padding),
            child: Stack(
              children: [
                // Sliding highlight pill behind the active segment.
                AnimatedPositioned(
                  duration: _dragIndex != null
                      ? const Duration(milliseconds: 120)
                      : const Duration(milliseconds: 280),
                  curve: Curves.easeOutCubic,
                  left: activeIndex * segWidth,
                  top: 0,
                  bottom: 0,
                  width: segWidth,
                  child: AnimatedContainer(
                    duration: const Duration(milliseconds: 280),
                    decoration: BoxDecoration(
                      borderRadius: BorderRadius.circular(
                        (_height - _padding * 2) / 2,
                      ),
                      gradient: _highlightGradient(activeMode),
                      boxShadow: _highlightShadow(activeMode),
                    ),
                  ),
                ),
                // Tap targets + icon/label stacks.
                Row(
                  children: [
                    for (var i = 0; i < segments.length; i++)
                      Expanded(
                        child: GestureDetector(
                          behavior: HitTestBehavior.opaque,
                          onTap: () => _select(i),
                          child: Center(
                            child: Column(
                              mainAxisSize: MainAxisSize.min,
                              children: [
                                AnimatedSwitcher(
                                  duration:
                                      const Duration(milliseconds: 200),
                                  child: Icon(
                                    segments[i].icon,
                                    key: ValueKey(
                                        '${segments[i].label}-${i == activeIndex}'),
                                    size: 21,
                                    color: i == activeIndex
                                        ? activeText
                                        : inactiveText,
                                  ),
                                ),
                                const SizedBox(height: 3),
                                AnimatedDefaultTextStyle(
                                  duration:
                                      const Duration(milliseconds: 220),
                                  style: TextStyle(
                                    color: i == activeIndex
                                        ? activeText
                                        : inactiveText,
                                    fontSize: 12,
                                    fontWeight: FontWeight.w600,
                                    letterSpacing: 0.3,
                                  ),
                                  child: Text(segments[i].label),
                                ),
                              ],
                            ),
                          ),
                        ),
                      ),
                  ],
                ),
              ],
            ),
          ),
        );
      },
    );
  }

  Gradient _highlightGradient(RoomMode m) {
    switch (m) {
      case RoomMode.on:
        return widget.offCurve
            ? const LinearGradient(
                begin: Alignment.topLeft,
                end: Alignment.bottomRight,
                colors: [Color(0xFF8C6E2C), Color(0xFFC79832)],
              )
            : const LinearGradient(
                begin: Alignment.topLeft,
                end: Alignment.bottomRight,
                colors: [Color(0xFFB48420), Color(0xFFEDB72A)],
              );
      case RoomMode.idle:
        return LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [
            Color.lerp(const Color(0xFF2A1F14), widget.cctColor, 0.55)!,
            Color.lerp(const Color(0xFF1F1610), widget.cctColor, 0.35)!,
          ],
        );
      case RoomMode.off:
        return const LinearGradient(
          colors: [Color(0xFF353B45), Color(0xFF3F454F)],
        );
    }
  }

  List<BoxShadow> _highlightShadow(RoomMode m) {
    switch (m) {
      case RoomMode.on:
        return [
          BoxShadow(
            color: widget.offCurve
                ? const Color(0xFFD4A574).withValues(alpha: 0.30)
                : CelestialColors.sunWarm.withValues(alpha: 0.40),
            blurRadius: 14,
            spreadRadius: -2,
          ),
        ];
      case RoomMode.idle:
        return [
          BoxShadow(
            color: widget.cctColor.withValues(alpha: 0.35),
            blurRadius: 12,
            spreadRadius: -2,
          ),
        ];
      case RoomMode.off:
        return const <BoxShadow>[];
    }
  }

  Color _activeTextColor(RoomMode m) {
    switch (m) {
      case RoomMode.on:
        return widget.offCurve
            ? const Color(0xFF2A1E0C)
            : const Color(0xFF1A1208);
      case RoomMode.idle:
        return const Color(0xFFFFF3DC);
      case RoomMode.off:
        return const Color(0xFFE6E8EB);
    }
  }
}

class _SegmentSpec {
  final RoomMode mode;
  final IconData icon;
  final String label;
  const _SegmentSpec(this.mode, this.icon, this.label);
}

/// Compact motion indicator: walk icon when active, countdown text when timing out.
class _MotionIndicator extends StatefulWidget {
  final MotionTimerInfo info;
  final Color color;
  final VoidCallback? onExpired;

  const _MotionIndicator(
      {required this.info, required this.color, this.onExpired});

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

/// Solid border around a room card when rhythm is active.
///
/// Fades in/out smoothly when rhythm state changes.
class _RhythmBorderGlow extends StatelessWidget {
  final bool active;
  final Color color;

  const _RhythmBorderGlow({required this.active, required this.color});

  @override
  Widget build(BuildContext context) {
    return AnimatedOpacity(
      opacity: active ? 1.0 : 0.0,
      duration: Duration(milliseconds: active ? 400 : 500),
      curve: Curves.easeInOut,
      child: DecoratedBox(
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(20),
          border: Border.all(
            color: color.withValues(alpha: 0.6),
            width: 2,
          ),
        ),
      ),
    );
  }
}
