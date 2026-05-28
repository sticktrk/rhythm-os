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
import 'mood_color_sheet.dart';
import 'room_settings_sheet.dart';
import 'solar_orbit.dart'; // For CelestialColors

/// Light mode for a room card.
enum RoomMode { mood, standby, on, off }

/// Hue-style room card with CCT-tinted background, big segmented power
/// control (mood / off / on), rhythm controls, and brightness slider.
class RoomCard extends StatefulWidget {
  final String roomId;
  final CurveConfigDto globalConfig;
  final CurveData? curveData;

  const RoomCard({
    super.key,
    required this.roomId,
    required this.globalConfig,
    this.curveData,
  });

  @override
  State<RoomCard> createState() => _RoomCardState();
}

class _RoomCardState extends State<RoomCard> {
  /// Non-null when the user is dragging the slider (local override).
  int? _sliderBrightness;
  int _lastResetGen = 0;
  bool _cctMode = false;
  int? _sliderKelvin;
  static const int _minKelvin = 2000;
  static const int _maxKelvin = 6500;

  String _analyticsModeForState(RoomModeState state) {
    final mode = switch (state) {
      RoomModeState.mood => RoomMode.mood,
      RoomModeState.standby || RoomModeState.idle => RoomMode.standby,
      RoomModeState.hardOff => RoomMode.off,
      RoomModeState.warning ||
      RoomModeState.wake ||
      RoomModeState.active =>
        RoomMode.on,
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

    if (newMode == RoomMode.mood &&
        roomProvider.getDisplayRoomState(widget.roomId) == RoomModeState.mood) {
      HapticFeedback.lightImpact();
      _showMoodColorPicker(
        roomProvider,
        currentBrightness: _currentMoodBrightness(roomProvider),
      );
      return;
    }

    // Optimistic local state update
    switch (newMode) {
      case RoomMode.mood:
        HapticFeedback.lightImpact();
        roomProvider.setRoomLightsOnLocal(widget.roomId, true);
        roomProvider.setRoomRhythmEnabled(widget.roomId, true);
        roomProvider.setMoodEnabledLocal(widget.roomId, true);
        roomProvider.setRoomStateLocal(widget.roomId, RoomModeState.mood);
      case RoomMode.on:
        HapticFeedback.mediumImpact();
        roomProvider.setRoomLightsOnLocal(widget.roomId, true);
        roomProvider.setRoomRhythmEnabled(widget.roomId, true);
        roomProvider.setRoomStateLocal(widget.roomId, RoomModeState.active);
      case RoomMode.standby:
        HapticFeedback.mediumImpact();
        roomProvider.setRoomLightsOnLocal(widget.roomId, true);
        roomProvider.setRoomRhythmEnabled(widget.roomId, true);
        roomProvider.setRoomStateLocal(widget.roomId, RoomModeState.standby);
      case RoomMode.off:
        HapticFeedback.heavyImpact();
        roomProvider.setRoomLightsOnLocal(widget.roomId, false);
        roomProvider.setRoomStateLocal(widget.roomId, RoomModeState.hardOff);
    }

    switch (newMode) {
      case RoomMode.mood:
        serverSync.pushNodePreferences(
          widget.roomId,
          rhythmEnabled: true,
          state: RoomModeState.mood,
          profileSettings: const {'mood_enabled': true},
        );
      case RoomMode.on:
        serverSync.pushNodePreferences(
          widget.roomId,
          rhythmEnabled: true,
          state: RoomModeState.active,
        );
      case RoomMode.standby:
        serverSync.pushNodePreferences(
          widget.roomId,
          rhythmEnabled: true,
          state: RoomModeState.standby,
        );
      case RoomMode.off:
        serverSync.pushNodePreferences(
          widget.roomId,
          state: RoomModeState.hardOff,
        );
    }

    setState(() {
      _sliderBrightness = null;
      _cctMode = false;
      _sliderKelvin = null;
    });
    AnalyticsService().logRoomModeChanged(
      roomId: widget.roomId,
      previousMode: previousMode,
      nextMode: newMode.name,
    );
  }

  int _currentMoodBrightness(RoomProvider roomProvider) =>
      (_sliderBrightness ?? roomProvider.getMoodBrightness(widget.roomId) ?? 1)
          .clamp(1, 100)
          .toInt();

  void _showMoodColorPicker(
    RoomProvider roomProvider, {
    required int currentBrightness,
  }) {
    final existingColor = roomProvider.getMoodColor(widget.roomId) ??
        roomProvider.getRoomColor(widget.roomId);
    final initialColor = existingColor != null
        ? Color.fromARGB(
            255, existingColor.$1, existingColor.$2, existingColor.$3)
        : null;

    MoodColorSheet.show(
      context,
      initialColor: initialColor,
      onColorChanged: (color) {
        final r = (color.r * 255).round();
        final g = (color.g * 255).round();
        final b = (color.b * 255).round();
        context.read<ServerSyncProvider>().dispatchNodeColor(
              widget.roomId,
              r,
              g,
              b,
              scope: 'mood',
              brightness: currentBrightness,
            );
      },
    );
  }

  (int, int, int) _rgbFromColor(Color color) => (
        (color.r * 255).round().clamp(0, 255),
        (color.g * 255).round().clamp(0, 255),
        (color.b * 255).round().clamp(0, 255),
      );

  void _onBrightnessSliderEnd({
    required RoomMode mode,
    required Color currentColor,
  }) {
    if (_sliderBrightness == null) return;
    final serverSync = context.read<ServerSyncProvider>();
    if (mode == RoomMode.mood) {
      final rgb = _rgbFromColor(currentColor);
      serverSync.dispatchNodeColor(
        widget.roomId,
        rgb.$1,
        rgb.$2,
        rgb.$3,
        scope: 'mood',
        brightness: _sliderBrightness!,
      );
    } else {
      serverSync.dispatchNodeBrightness(widget.roomId, _sliderBrightness!);
    }
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
      _sliderKelvin = null;
      _cctMode = false;
    });
  }

  void _toggleSliderMode() {
    HapticFeedback.lightImpact();
    setState(() {
      _cctMode = !_cctMode;
      _sliderKelvin = null;
      _sliderBrightness = null;
    });
  }

  void _onKelvinSliderEnd() {
    // TODO: dispatch kelvin to server once endpoint exists
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
          int?,
          (int, int, int)?,
          (int, int, int)?,
          bool,
          bool
        )>(
      selector: (_, p) => (
        p.getRoom(widget.roomId),
        p.resetGeneration,
        p.getMotionTimer(widget.roomId),
        p.getDisplayRoomState(widget.roomId),
        p.getBrightness(widget.roomId),
        p.getMoodBrightness(widget.roomId),
        p.getKelvin(widget.roomId),
        p.getRoomColor(widget.roomId),
        p.getMoodColor(widget.roomId),
        p.hasMotionSensor(widget.roomId),
        p.isRoomTransitioning(widget.roomId),
      ),
      builder: (context, data, _) {
        final (
          room,
          resetGen,
          motionTimer,
          roomState,
          serverBrightness,
          serverMoodBrightness,
          serverKelvin,
          serverColor,
          moodColor,
          hasSensor,
          isTransitioning
        ) = data;
        if (room == null) return const SizedBox.shrink();
        final roomProvider = context.read<RoomProvider>();

        // Check if this room's hub is reachable.
        final hubConnected = context.select<ServerSyncProvider, bool>(
            (p) => p.isRoomHubConnected(room.source));
        final standbyEnabled = context.select<ServerSyncProvider, bool>(
            (p) => p.standbyEnabledForNode(widget.roomId));

        // External reset bumps the generation counter — drop local overrides
        if (resetGen != _lastResetGen) {
          _lastResetGen = resetGen;
          _sliderBrightness = null;
          _sliderKelvin = null;
        }

        final mode = switch (roomState) {
          RoomModeState.mood => RoomMode.mood,
          RoomModeState.standby || RoomModeState.idle => RoomMode.standby,
          RoomModeState.hardOff => RoomMode.off,
          RoomModeState.warning ||
          RoomModeState.wake ||
          RoomModeState.active =>
            RoomMode.on,
        };

        // Mood has its own profile brightness. Do not reuse the active-mode
        // room brightness when the room is in Mood.
        final brightness = mode == RoomMode.mood
            ? serverMoodBrightness ?? 1
            : serverBrightness ?? 50;
        final kelvin = serverKelvin ?? 3000;

        // Display brightness depends on mode
        final displayBrightness = switch (mode) {
          RoomMode.mood => _sliderBrightness ?? brightness,
          RoomMode.standby => brightness,
          RoomMode.on => _sliderBrightness ?? brightness,
          RoomMode.off => _sliderBrightness ?? brightness,
        };

        final displayColor =
            mode == RoomMode.mood ? serverColor ?? moodColor : serverColor;
        final cctColor = _cctMode && _sliderKelvin != null
            ? ColorUtils.cctToColor(_sliderKelvin!)
            : displayColor != null
                ? Color.fromARGB(
                    255, displayColor.$1, displayColor.$2, displayColor.$3)
                : ColorUtils.cctToColor(kelvin);

        // Blend directly from a neutral dark base toward the CCT color —
        // brightness scales the mix so hue stays clear at every level.
        final dimT = displayBrightness / 100.0;
        const darkBase = Color(0xFF141210);
        final bgColor = switch (mode) {
          RoomMode.mood => Color.lerp(darkBase, cctColor, 0.22)!,
          RoomMode.standby => Color.lerp(darkBase, cctColor, 0.14)!,
          RoomMode.on => Color.lerp(darkBase, cctColor, 0.10 + dimT * 0.50)!,
          RoomMode.off => CelestialColors.backgroundCard,
        };

        // Text and icon colors per mode — use luminance-based contrast
        // (same approach as _RhythmPill) so names stay readable at any brightness.
        final onLight = mode == RoomMode.on && bgColor.computeLuminance() > 0.4;
        final textColor = switch (mode) {
          RoomMode.mood => const Color(0xFFEFE0C4),
          RoomMode.standby => const Color(0xFFD9CBB1),
          RoomMode.on => onLight
              ? (kelvin < 4000
                  ? const Color(0xFF3A2A1A) // warm dark brown
                  : const Color(0xFF2A2C30)) // cool dark grey
              : Colors.white,
          RoomMode.off => CelestialColors.textSecondary,
        };
        final iconColor = switch (mode) {
          RoomMode.mood => const Color(0xFFD8C5A4),
          RoomMode.standby => const Color(0xFFCDBE9D),
          RoomMode.on => onLight
              ? (kelvin < 4000
                  ? const Color(0xFF4A3828) // warm brown
                  : const Color(0xFF3A3C42)) // cool grey
              : Colors.white.withValues(alpha: 0.85),
          RoomMode.off => CelestialColors.textSecondary,
        };

        // Room is "off curve" when brightness or time has been manually adjusted
        final offCurve = mode == RoomMode.on &&
            (room.brightnessOffset != 0 || room.timeOffsetMinutes != 0);

        final sliderActive =
            !isTransitioning && (mode == RoomMode.on || mode == RoomMode.mood);
        final sliderInCctMode = _cctMode && mode == RoomMode.on;
        final rhythmGlowActive = mode == RoomMode.on && room.rhythmEnabled;
        final glowColor = cctColor;
        final sliderActiveTrackColor =
            Color.lerp(cctColor, Colors.white, 0.15)!.withValues(alpha: 0.85);
        final sliderInactiveTrackColor = Colors.black.withValues(alpha: 0.20);
        final sliderThumbColor = Colors.white;
        final sliderOverlayColor = cctColor.withValues(alpha: 0.15);
        final titleIcon = switch (room.kind) {
          RoomNodeKind.lightDevice => Icons.lightbulb_outline_rounded,
          RoomNodeKind.room => Icons.meeting_room_rounded,
          _ => null,
        };

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
                    if (mode == RoomMode.mood) ...[
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
                              if (titleIcon != null)
                                Padding(
                                  padding: const EdgeInsets.only(right: 8),
                                  child: Icon(
                                    titleIcon,
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
                              const SizedBox(width: 8),
                              SizedBox(
                                width: 18,
                                height: 18,
                                child: AnimatedSwitcher(
                                  duration: const Duration(milliseconds: 160),
                                  child: isTransitioning
                                      ? _RoomTransitionSpinner(
                                          key: const ValueKey(
                                            'room_transition_spinner',
                                          ),
                                          color: iconColor,
                                        )
                                      : const SizedBox.shrink(
                                          key: ValueKey(
                                            'room_transition_idle',
                                          ),
                                        ),
                                ),
                              ),
                            ],
                          ),
                        ),
                        // Big three-segment power control.
                        Padding(
                          padding: const EdgeInsets.fromLTRB(12, 12, 12, 0),
                          child: _SegmentedToggle(
                            mode: mode,
                            onModeChanged: _onModeChanged,
                            offCurve: offCurve,
                            cctColor: cctColor,
                            standbyEnabled: standbyEnabled,
                            enabled: !isTransitioning,
                          ),
                        ),
                        if (mode == RoomMode.on ||
                            mode == RoomMode.mood ||
                            mode == RoomMode.standby)
                          GestureDetector(
                            behavior: HitTestBehavior.opaque,
                            onTap: mode == RoomMode.standby ? () {} : null,
                            onLongPress: () {},
                            child: Padding(
                              padding: const EdgeInsets.fromLTRB(12, 10, 12, 4),
                              child: Row(
                                children: [
                                  // Mode toggle button
                                  GestureDetector(
                                    onTap: sliderActive
                                        ? () {
                                            if (mode == RoomMode.mood) {
                                              _showMoodColorPicker(
                                                roomProvider,
                                                currentBrightness:
                                                    displayBrightness,
                                              );
                                            } else {
                                              _toggleSliderMode();
                                            }
                                          }
                                        : null,
                                    child: AnimatedContainer(
                                      duration:
                                          const Duration(milliseconds: 220),
                                      width: 32,
                                      height: 32,
                                      decoration: BoxDecoration(
                                        borderRadius: BorderRadius.circular(10),
                                        color: sliderInCctMode
                                            ? ColorUtils.cctToColor(
                                                    (_sliderKelvin ?? kelvin)
                                                        .clamp(_minKelvin,
                                                            _maxKelvin))
                                                .withValues(alpha: 0.20)
                                            : Colors.white
                                                .withValues(alpha: 0.07),
                                        border: Border.all(
                                          color: sliderInCctMode
                                              ? ColorUtils.cctToColor(
                                                      (_sliderKelvin ?? kelvin)
                                                          .clamp(_minKelvin,
                                                              _maxKelvin))
                                                  .withValues(alpha: 0.35)
                                              : Colors.white
                                                  .withValues(alpha: 0.12),
                                          width: 1,
                                        ),
                                        boxShadow: [
                                          BoxShadow(
                                            color: Colors.black
                                                .withValues(alpha: 0.35),
                                            blurRadius: 4,
                                            offset: const Offset(0, 2),
                                          ),
                                        ],
                                      ),
                                      child: Center(
                                        child: AnimatedSwitcher(
                                          duration:
                                              const Duration(milliseconds: 220),
                                          transitionBuilder: (child, anim) =>
                                              ScaleTransition(
                                            scale: anim,
                                            child: FadeTransition(
                                              opacity: anim,
                                              child: child,
                                            ),
                                          ),
                                          child: Icon(
                                            mode == RoomMode.mood
                                                ? Icons.palette_rounded
                                                : mode == RoomMode.standby
                                                    ? Icons
                                                        .lightbulb_outline_rounded
                                                    : sliderInCctMode
                                                        ? Icons.contrast_rounded
                                                        : Icons
                                                            .wb_sunny_rounded,
                                            key: ValueKey(
                                                '$mode-$sliderInCctMode'),
                                            size: 18,
                                            color: sliderInCctMode
                                                ? ColorUtils.cctToColor(
                                                    (_sliderKelvin ?? kelvin)
                                                        .clamp(_minKelvin,
                                                            _maxKelvin))
                                                : iconColor.withValues(
                                                    alpha: 0.7),
                                          ),
                                        ),
                                      ),
                                    ),
                                  ),
                                  const SizedBox(width: 4),
                                  // Slider fills remaining space
                                  Expanded(
                                    child: SliderTheme(
                                      data: SliderThemeData(
                                        trackHeight: 14,
                                        thumbShape: _SunSliderThumbShape(
                                          icon: sliderInCctMode
                                              ? Icons.contrast_rounded
                                              : Icons.wb_sunny_rounded,
                                        ),
                                        overlayShape:
                                            const RoundSliderOverlayShape(
                                          overlayRadius: 28,
                                        ),
                                        padding: const EdgeInsets.symmetric(
                                          horizontal:
                                              _SunSliderThumbShape.radius + 2,
                                          vertical: 10,
                                        ),
                                        trackShape: sliderInCctMode
                                            ? const _CCTGradientTrackShape()
                                            : const RoundedRectSliderTrackShape(),
                                        activeTrackColor:
                                            sliderActiveTrackColor,
                                        inactiveTrackColor:
                                            sliderInactiveTrackColor,
                                        thumbColor: sliderThumbColor,
                                        overlayColor: sliderOverlayColor,
                                        disabledActiveTrackColor: mode ==
                                                RoomMode.mood
                                            ? cctColor.withValues(alpha: 0.30)
                                            : mode == RoomMode.standby
                                                ? cctColor.withValues(
                                                    alpha: 0.24,
                                                  )
                                                : CelestialColors.orbitRing
                                                    .withValues(
                                                    alpha: 0.35,
                                                  ),
                                        disabledInactiveTrackColor:
                                            mode == RoomMode.mood ||
                                                    mode == RoomMode.standby
                                                ? Colors.black
                                                    .withValues(alpha: 0.15)
                                                : CelestialColors.orbitRing
                                                    .withValues(alpha: 0.18),
                                        disabledThumbColor:
                                            mode == RoomMode.mood ||
                                                    mode == RoomMode.standby
                                                ? Color.lerp(
                                                    Colors.white,
                                                    cctColor,
                                                    mode == RoomMode.standby
                                                        ? 0.45
                                                        : 0.25,
                                                  )!
                                                : CelestialColors.textSecondary
                                                    .withValues(alpha: 0.55),
                                      ),
                                      child: Slider(
                                        value: sliderInCctMode
                                            ? (_sliderKelvin ?? kelvin)
                                                .toDouble()
                                                .clamp(_minKelvin.toDouble(),
                                                    _maxKelvin.toDouble())
                                            : displayBrightness
                                                .toDouble()
                                                .clamp(1, 100),
                                        min: sliderInCctMode
                                            ? _minKelvin.toDouble()
                                            : 1,
                                        max: sliderInCctMode
                                            ? _maxKelvin.toDouble()
                                            : 100,
                                        onChanged: sliderActive
                                            ? (v) {
                                                setState(() {
                                                  if (sliderInCctMode) {
                                                    _sliderKelvin = v.round();
                                                  } else {
                                                    _sliderBrightness =
                                                        v.round();
                                                  }
                                                });
                                              }
                                            : null,
                                        onChangeEnd: sliderActive
                                            ? (_) {
                                                if (sliderInCctMode) {
                                                  _onKelvinSliderEnd();
                                                } else {
                                                  _onBrightnessSliderEnd(
                                                    mode: mode,
                                                    currentColor: cctColor,
                                                  );
                                                }
                                              }
                                            : null,
                                      ),
                                    ),
                                  ),
                                ],
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

class _RoomTransitionSpinner extends StatelessWidget {
  const _RoomTransitionSpinner({super.key, required this.color});

  final Color color;

  @override
  Widget build(BuildContext context) {
    return Semantics(
      label: 'Room updating',
      liveRegion: true,
      child: Padding(
        padding: const EdgeInsets.all(1),
        child: CircularProgressIndicator(
          strokeWidth: 1.8,
          valueColor: AlwaysStoppedAnimation<Color>(
            color.withValues(alpha: 0.78),
          ),
        ),
      ),
    );
  }
}

class _SunSliderThumbShape extends SliderComponentShape {
  const _SunSliderThumbShape({this.icon = Icons.wb_sunny_rounded});

  final IconData icon;
  static const radius = 14.0;
  static const _elevation = 3.0;
  static const _pressedElevation = 6.0;

  @override
  Size getPreferredSize(bool isEnabled, bool isDiscrete) {
    return const Size.fromRadius(radius);
  }

  @override
  void paint(
    PaintingContext context,
    Offset center, {
    required Animation<double> activationAnimation,
    required Animation<double> enableAnimation,
    required bool isDiscrete,
    required TextPainter labelPainter,
    required RenderBox parentBox,
    required SliderThemeData sliderTheme,
    required TextDirection textDirection,
    required double value,
    required double textScaleFactor,
    required Size sizeWithOverflow,
  }) {
    final canvas = context.canvas;
    final color = ColorTween(
      begin: sliderTheme.disabledThumbColor,
      end: sliderTheme.thumbColor,
    ).evaluate(enableAnimation)!;
    final elevation = Tween<double>(
      begin: _elevation,
      end: _pressedElevation,
    ).evaluate(activationAnimation);

    final path = Path()
      ..addOval(Rect.fromCircle(center: center, radius: radius));
    canvas.drawShadow(path, Colors.black, elevation, true);
    canvas.drawCircle(center, radius, Paint()..color = color);
    canvas.drawCircle(
      center,
      radius,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1
        ..color = color.computeLuminance() > 0.45
            ? Colors.black.withValues(alpha: 0.08)
            : Colors.white.withValues(alpha: 0.18),
    );

    final sunIconColor = color.computeLuminance() > 0.45
        ? const Color(0xFF5E4308)
        : const Color(0xFFFFF3DC);
    final iconPainter = TextPainter(
      text: TextSpan(
        text: String.fromCharCode(icon.codePoint),
        style: TextStyle(
          color: sunIconColor,
          fontFamily: icon.fontFamily,
          package: icon.fontPackage,
          fontSize: 15,
        ),
      ),
      textDirection: textDirection,
    )..layout();

    iconPainter.paint(
      canvas,
      center - Offset(iconPainter.width / 2, iconPainter.height / 2),
    );
  }
}

/// Warm-to-cool gradient track for CCT slider mode.
class _CCTGradientTrackShape extends SliderTrackShape
    with BaseSliderTrackShape {
  const _CCTGradientTrackShape();

  static final _gradientColors = [
    ColorUtils.cctToColor(2000),
    ColorUtils.cctToColor(2700),
    ColorUtils.cctToColor(4000),
    ColorUtils.cctToColor(5500),
    ColorUtils.cctToColor(6500),
  ];

  @override
  void paint(
    PaintingContext context,
    Offset offset, {
    required RenderBox parentBox,
    required SliderThemeData sliderTheme,
    required Animation<double> enableAnimation,
    required Offset thumbCenter,
    Offset? secondaryOffset,
    bool isEnabled = false,
    bool isDiscrete = false,
    required TextDirection textDirection,
  }) {
    final trackRect = getPreferredRect(
      parentBox: parentBox,
      offset: offset,
      sliderTheme: sliderTheme,
    );
    final trackHeight = sliderTheme.trackHeight ?? 14;
    final radius = Radius.circular(trackHeight / 2);
    final rrect = RRect.fromRectAndRadius(trackRect, radius);
    final gradient = LinearGradient(colors: _gradientColors);

    final canvas = context.canvas;
    canvas.save();
    canvas.clipRRect(rrect);

    canvas.drawRect(
      trackRect,
      Paint()..shader = gradient.createShader(trackRect),
    );

    // Subtle dark overlay to integrate with the card's dark theme
    canvas.drawRect(
      trackRect,
      Paint()..color = Colors.black.withValues(alpha: 0.15),
    );

    canvas.restore();
  }
}

/// Big segmented power control: Mood | Off | On.
///
/// Each segment is a discrete tap target with a stacked icon + label, so
/// every state is visible and discoverable — no hidden long-press. A
/// highlight pill slides behind the active segment. Horizontal drag also
/// works for users who prefer to flick.
class _SegmentedToggle extends StatefulWidget {
  final RoomMode mode;
  final ValueChanged<RoomMode> onModeChanged;
  final bool offCurve;
  final Color cctColor;
  final bool standbyEnabled;
  final bool enabled;

  const _SegmentedToggle({
    required this.mode,
    required this.onModeChanged,
    required this.cctColor,
    this.standbyEnabled = false,
    this.offCurve = false,
    this.enabled = true,
  });

  @override
  State<_SegmentedToggle> createState() => _SegmentedToggleState();
}

class _SegmentedToggleState extends State<_SegmentedToggle> {
  static const double _height = 56;
  static const double _padding = 4;

  /// Index of the segment currently under the dragging finger; null when
  /// not dragging. Lets the highlight preview the drag in real time and
  /// commit to the segment under the finger on release.
  int? _dragIndex;

  static const List<_SegmentSpec> _segments = [
    _SegmentSpec(RoomMode.mood, Icons.spa_rounded, 'Mood'),
    _SegmentSpec(RoomMode.off, Icons.power_settings_new_rounded, 'Off'),
    _SegmentSpec(RoomMode.on, Icons.lightbulb_rounded, 'On'),
  ];

  int _indexFor(RoomMode m) {
    if (m == RoomMode.standby) return _indexFor(RoomMode.on);
    final segs = _segments;
    for (var i = 0; i < segs.length; i++) {
      if (segs[i].mode == m) return i;
    }
    return 1;
  }

  int _indexAtX(double x, double trackWidth) {
    final segs = _segments;
    final usable = trackWidth - _padding * 2;
    if (usable <= 0) return 0;
    final segWidth = usable / segs.length;
    final localX = (x - _padding).clamp(0.0, usable);
    return (localX / segWidth).floor().clamp(0, segs.length - 1);
  }

  void _select(int i, {bool repeatTap = false}) {
    if (!widget.enabled) return;
    final target = _segments[i].mode;
    if (repeatTap &&
        target == RoomMode.on &&
        widget.mode == RoomMode.on &&
        widget.standbyEnabled) {
      widget.onModeChanged(RoomMode.standby);
      return;
    }
    if (target != widget.mode || (repeatTap && target == RoomMode.mood)) {
      widget.onModeChanged(target);
    }
  }

  @override
  Widget build(BuildContext context) {
    final segments = _displaySegments();
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
            if (!widget.enabled) return;
            setState(() {
              _dragIndex = _indexAtX(details.localPosition.dx, trackWidth);
            });
          },
          onHorizontalDragUpdate: (details) {
            if (!widget.enabled) return;
            final idx = _indexAtX(details.localPosition.dx, trackWidth);
            if (idx != _dragIndex) {
              setState(() => _dragIndex = idx);
            }
          },
          onHorizontalDragEnd: (_) {
            if (!widget.enabled) return;
            final idx = _dragIndex;
            if (idx == null) return;
            setState(() => _dragIndex = null);
            _select(idx);
          },
          onHorizontalDragCancel: () {
            if (_dragIndex == null) return;
            setState(() => _dragIndex = null);
          },
          child: AnimatedOpacity(
            duration: const Duration(milliseconds: 160),
            opacity: widget.enabled ? 1.0 : 0.58,
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
                        boxShadow: widget.enabled
                            ? _highlightShadow(activeMode)
                            : const [],
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
                            onTap: () => _select(i, repeatTap: true),
                            child: Center(
                              child: Column(
                                mainAxisSize: MainAxisSize.min,
                                children: [
                                  AnimatedSwitcher(
                                    duration: const Duration(milliseconds: 200),
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
                                    duration: const Duration(milliseconds: 220),
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
          ),
        );
      },
    );
  }

  List<_SegmentSpec> _displaySegments() {
    if (widget.mode != RoomMode.standby) return _segments;
    return const [
      _SegmentSpec(RoomMode.mood, Icons.spa_rounded, 'Mood'),
      _SegmentSpec(RoomMode.off, Icons.power_settings_new_rounded, 'Off'),
      _SegmentSpec(
          RoomMode.standby, Icons.lightbulb_outline_rounded, 'Standby'),
    ];
  }

  Gradient _highlightGradient(RoomMode m) {
    const base = Color(0xFF1C1C1C);
    switch (m) {
      case RoomMode.standby:
        return LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [
            Color.lerp(base, widget.cctColor, 0.28)!,
            Color.lerp(base, widget.cctColor, 0.18)!,
          ],
        );
      case RoomMode.on:
        final start = Color.lerp(base, widget.cctColor, 0.50)!;
        final end = Color.lerp(base, widget.cctColor, 0.65)!;
        if (widget.offCurve) {
          const grey = Color(0xFF555555);
          return LinearGradient(
            begin: Alignment.topLeft,
            end: Alignment.bottomRight,
            colors: [
              Color.lerp(start, grey, 0.25)!,
              Color.lerp(end, grey, 0.25)!,
            ],
          );
        }
        return LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [start, end],
        );
      case RoomMode.mood:
        return LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [
            Color.lerp(base, widget.cctColor, 0.35)!,
            Color.lerp(base, widget.cctColor, 0.25)!,
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
      case RoomMode.standby:
        return [
          BoxShadow(
            color: widget.cctColor.withValues(alpha: 0.18),
            blurRadius: 10,
            spreadRadius: -3,
          ),
        ];
      case RoomMode.on:
        return [
          BoxShadow(
            color: widget.cctColor
                .withValues(alpha: widget.offCurve ? 0.20 : 0.35),
            blurRadius: 14,
            spreadRadius: -2,
          ),
        ];
      case RoomMode.mood:
        return [
          BoxShadow(
            color: widget.cctColor.withValues(alpha: 0.25),
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
      case RoomMode.standby:
        return const Color(0xFFEADCC0);
      case RoomMode.on:
        final probe =
            Color.lerp(const Color(0xFF1C1C1C), widget.cctColor, 0.60)!;
        return probe.computeLuminance() > 0.30
            ? const Color(0xFF1A1A1E)
            : const Color(0xFFFFF3DC);
      case RoomMode.mood:
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
