import 'dart:async';
import 'dart:math' as math;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show RhythmDispatchFailure, RhythmSceneDefinition, RoomModeState;
import '../providers/server_sync_provider.dart';
import '../providers/room_provider.dart';
import '../services/analytics_service.dart';
import 'device_detail_sheet.dart';
import 'first_run_explainer.dart';
import 'mood_sheet.dart';
import 'room_settings_sheet.dart';
import 'solar_orbit.dart'; // For CelestialColors

/// Light mode for a room card.
enum RoomMode { mood, standby, on, off }

const double _roomHeaderActionHitSize = 28;

/// Hue-style room card with a CCT-tinted background, compact Power/Scenes
/// jewels, rhythm controls, and brightness/CCT sliders.
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
  static const _minimumActionFeedbackDuration = Duration(milliseconds: 400);

  /// Non-null when the user is dragging the slider (local override).
  int? _sliderBrightness;
  int _lastResetGen = 0;
  int? _sliderKelvin;
  bool _localActionPending = false;
  Timer? _localActionFeedbackTimer;
  static const int _minKelvin = 2000;
  static const int _maxKelvin = 6500;

  void _beginActionFeedback() {
    _localActionFeedbackTimer?.cancel();
    if (!_localActionPending) {
      setState(() => _localActionPending = true);
    }
    _localActionFeedbackTimer = Timer(_minimumActionFeedbackDuration, () {
      _localActionFeedbackTimer = null;
      if (!mounted || !_localActionPending) return;
      setState(() => _localActionPending = false);
    });
  }

  @override
  void dispose() {
    _localActionFeedbackTimer?.cancel();
    super.dispose();
  }

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

  /// Persistence id for the one-time "what is Mood?" explainer.
  static const String _moodExplainerId = 'mood';

  /// Handle three-state mode transitions. Driven by the segmented toggle:
  /// each segment is a tap target, plus horizontal drag for fluency.
  void _onModeChanged(RoomMode newMode) {
    // The very first time someone reaches for Mood, introduce the concept
    // before acting on it — then drop them straight into the picker.
    if (newMode == RoomMode.mood &&
        !FirstRunExplainer.hasSeen(_moodExplainerId)) {
      _introduceMood();
      return;
    }
    _applyModeChange(newMode);
  }

  /// The Scenes jewel is both a mode switch and the doorway into the picker.
  /// Entering Scenes should not require a second tap just to choose something.
  void _onScenesJewelPressed() {
    if (!FirstRunExplainer.hasSeen(_moodExplainerId)) {
      _introduceMood();
      return;
    }

    final alreadyInMood = context
            .read<RoomProvider>()
            .getDisplayRoomState(widget.roomId) ==
        RoomModeState.mood;
    _applyModeChange(RoomMode.mood);
    if (!alreadyInMood && mounted) {
      _showMoodScenePicker();
    }
  }

  /// Show the one-time Mood explainer, then switch the room to Mood and open
  /// the scene/color picker so the user can act on what they just learned.
  Future<void> _introduceMood() async {
    HapticFeedback.lightImpact();
    await FirstRunExplainer.maybeShow(
      context,
      id: _moodExplainerId,
      eyebrow: 'NEW',
      title: 'Meet Mood',
      subtitle: 'A calm, hand-picked light that stays exactly how you set it.',
      icon: Icons.spa_rounded,
      accent: const Color(0xFFFFB23E),
      ctaLabel: 'Choose a Mood',
      points: const [
        ExplainerPoint(
          icon: Icons.palette_rounded,
          title: 'Pick a scene or a color',
          body: 'Choose one of the saved scenes, or set a single fixed '
              'color for your lights — whatever fits the moment.',
        ),
        ExplainerPoint(
          icon: Icons.lock_outline_rounded,
          title: 'It stays put',
          body: "While Mood is on, your lights hold steady — they won't drift "
              'with the day’s natural rhythm.',
        ),
        ExplainerPoint(
          icon: Icons.bookmark_added_rounded,
          title: 'Saved for next time',
          body: 'Your most recent Mood is always remembered, ready to switch '
              'back to whenever you like.',
        ),
      ],
    );
    if (!mounted) return;
    _applyModeChange(RoomMode.mood);
    if (!mounted) return;
    _showMoodScenePicker();
  }

  Future<void> _applyModeChange(RoomMode newMode) {
    final roomProvider = context.read<RoomProvider>();
    final room = roomProvider.getRoom(widget.roomId);
    if (room == null) return Future.value();
    final previousMode =
        _analyticsModeForState(roomProvider.getDisplayRoomState(widget.roomId));

    final serverSync = context.read<ServerSyncProvider>();

    if (newMode == RoomMode.mood &&
        roomProvider.getDisplayRoomState(widget.roomId) == RoomModeState.mood) {
      HapticFeedback.lightImpact();
      _showMoodScenePicker();
      return Future.value();
    }

    final effectiveMode = newMode;

    // Optimistic local state update
    switch (effectiveMode) {
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

    final push = switch (effectiveMode) {
      RoomMode.mood => serverSync.pushNodePreferences(
          widget.roomId,
          rhythmEnabled: true,
          state: RoomModeState.mood,
          profileSettings: const {'mood_enabled': true},
        ),
      RoomMode.on => serverSync.pushNodePreferences(
          widget.roomId,
          rhythmEnabled: true,
          state: RoomModeState.active,
        ),
      RoomMode.standby => serverSync.pushNodePreferences(
          widget.roomId,
          rhythmEnabled: true,
          state: RoomModeState.standby,
        ),
      RoomMode.off => serverSync.pushNodePreferences(
          widget.roomId,
          state: RoomModeState.hardOff,
        ),
    };

    setState(() {
      _sliderBrightness = null;
      _sliderKelvin = null;
    });
    AnalyticsService().logRoomModeChanged(
      roomId: widget.roomId,
      previousMode: previousMode,
      nextMode: effectiveMode.name,
    );
    return push;
  }

  void _showMoodScenePicker() {
    final sync = context.read<ServerSyncProvider>();
    final roomProvider = context.read<RoomProvider>();

    final existingColor = roomProvider.getMoodColor(widget.roomId) ??
        roomProvider.getRoomColor(widget.roomId);
    final initialColor = existingColor != null
        ? Color.fromARGB(
            255, existingColor.$1, existingColor.$2, existingColor.$3)
        : null;
    final activeSceneId = sync.moodSceneIdForRoom(widget.roomId);

    MoodSheet.show(
      context,
      // Open straight to whichever kind of mood the room is currently using.
      initialTab: activeSceneId != null ? MoodTab.scenes : MoodTab.color,
      initialColor: initialColor,
      initialSceneId: activeSceneId,
      initialScenes: sync.scenesForRoom(widget.roomId),
      scenesLoader: () => sync.fetchScenes(roomId: widget.roomId),
      onColorChanged: (color) {
        final r = (color.r * 255).round();
        final g = (color.g * 255).round();
        final b = (color.b * 255).round();
        final moodBrightness = (roomProvider.getMoodBrightness(widget.roomId) ??
                roomProvider.getBrightness(widget.roomId) ??
                1)
            .clamp(1, 100)
            .toInt();
        sync.dispatchNodeColor(
          widget.roomId,
          r,
          g,
          b,
          scope: 'mood',
          brightness: moodBrightness,
        );
      },
      onSceneSelected: (scene) => sync.applyMoodScene(
        widget.roomId,
        scene.id,
        color: rhythmSceneRgb(scene),
      ),
    );
  }

  void _onBrightnessSliderEnd() {
    if (_sliderBrightness == null) return;
    context
        .read<ServerSyncProvider>()
        .dispatchNodeCurveBrightness(widget.roomId, _sliderBrightness!);
    AnalyticsService().logRoomBrightnessAdjusted(
      roomId: widget.roomId,
      brightness: _sliderBrightness!,
    );
  }

  /// A Low glow room remains directly adjustable. The first slider movement
  /// makes that transition visible immediately; the normal curve-modifier
  /// request sent on change-end clears the backend Standby flag and becomes
  /// the authoritative active state.
  void _activateLowGlowRoomForSlider() {
    final roomProvider = context.read<RoomProvider>();
    final currentState = roomProvider.getDisplayRoomState(widget.roomId);
    if (currentState != RoomModeState.standby &&
        currentState != RoomModeState.idle) {
      return;
    }

    roomProvider.setRoomLightsOnLocal(widget.roomId, true);
    roomProvider.setRoomRhythmEnabled(widget.roomId, true);
    roomProvider.setRoomStateLocal(widget.roomId, RoomModeState.active);
    AnalyticsService().logRoomModeChanged(
      roomId: widget.roomId,
      previousMode: RoomMode.standby.name,
      nextMode: RoomMode.on.name,
    );
  }

  void _onSceneBrightnessSliderEnd(Color currentColor) {
    if (_sliderBrightness == null) return;
    final r = (currentColor.r * 255).round().clamp(0, 255);
    final g = (currentColor.g * 255).round().clamp(0, 255);
    final b = (currentColor.b * 255).round().clamp(0, 255);
    context.read<ServerSyncProvider>().dispatchNodeColor(
          widget.roomId,
          r,
          g,
          b,
          scope: 'mood',
          brightness: _sliderBrightness!,
        );
    AnalyticsService().logRoomBrightnessAdjusted(
      roomId: widget.roomId,
      brightness: _sliderBrightness!,
    );
  }

  /// "On" from any non-adaptive state (Off / Mood / standby):
  /// turn the room on and reset it to the live adaptive curve — same outcome as
  /// the "Reset to curve" affordance.
  void _resetToOn() {
    HapticFeedback.mediumImpact();
    final roomProvider = context.read<RoomProvider>();
    final serverSync = context.read<ServerSyncProvider>();
    final previousMode =
        _analyticsModeForState(roomProvider.getDisplayRoomState(widget.roomId));

    // Reset is the complete server action: it enables Rhythm, clears offsets,
    // leaves off states, and dispatches the live curve. Sending an Active
    // preference first duplicates the physical Matter command.
    roomProvider.setRoomLightsOnLocal(widget.roomId, true);
    roomProvider.setRoomRhythmEnabled(widget.roomId, true);
    roomProvider.setRoomStateLocal(widget.roomId, RoomModeState.active);
    if (serverSync.dispatchResetNode(widget.roomId)) {
      _beginActionFeedback();
    }
    AnalyticsService().logRoomModeChanged(
      roomId: widget.roomId,
      previousMode: previousMode,
      nextMode: RoomMode.on.name,
    );
    setState(() {
      _sliderBrightness = null;
      _sliderKelvin = null;
    });
  }

  /// Reset this node to its adaptive curve position.
  void _resetRoom() {
    HapticFeedback.mediumImpact();
    final serverSync = context.read<ServerSyncProvider>();
    if (serverSync.dispatchResetNode(widget.roomId)) {
      _beginActionFeedback();
    }
    AnalyticsService().logRoomResetToCurve(roomId: widget.roomId);
    setState(() {
      _sliderBrightness = null;
      _sliderKelvin = null;
    });
  }

  void _onKelvinSliderEnd() {
    if (_sliderKelvin == null) return;
    context.read<ServerSyncProvider>().dispatchNodeCurveColorTemperature(
          widget.roomId,
          _sliderKelvin!,
          preserveBrightness: true,
        );
  }

  Future<void> _setMotionActivationEnabled(bool enabled) async {
    HapticFeedback.lightImpact();
    final applied = await context
        .read<ServerSyncProvider>()
        .setNodeMotionActivationEnabled(widget.roomId, enabled);
    if (!mounted || applied) return;
    ScaffoldMessenger.of(context).showSnackBar(
      const SnackBar(
        content: Text('Motion activation could not be updated.'),
      ),
    );
  }

  void _showMotionActivationUnavailable(String roomName) {
    HapticFeedback.lightImpact();
    final version = context.read<ServerSyncProvider>().firmwareVersion;
    final versionSuffix = version == '0.0.0' ? '' : ' ($version)';
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(
          'Update the Rhythm appliance$versionSuffix to control motion for $roomName.',
        ),
      ),
    );
  }

  double _effectiveCurveHour(RoomDto room) {
    final now = DateTime.now();
    final hour =
        now.hour + (now.minute / 60.0) + (room.timeOffsetMinutes / 60.0);
    return _CctSideRange.normalizeHour(hour);
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
        p.isNodeDispatchPending(widget.roomId),
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
          isTransitioning,
          isDispatchPending
        ) = data;
        if (room == null) return const SizedBox.shrink();

        // Check if this room's hub is reachable.
        final hubConnected = context.select<ServerSyncProvider, bool>(
            (p) => p.isRoomHubConnected(room.source));
        final standbyEnabled = context.select<ServerSyncProvider, bool>(
          (p) => p.standbyEnabledForNode(widget.roomId),
        );
        final motionActivationEnabled =
            context.select<ServerSyncProvider, bool>(
          (p) => p.motionActivationEnabledForNode(widget.roomId),
        );
        final motionActivationSupported =
            context.select<ServerSyncProvider, bool>(
          (p) => p.motionActivationSupportedForNode(widget.roomId),
        );
        final motionActivationPending =
            context.select<ServerSyncProvider, bool>(
          (p) => p.motionActivationPendingForNode(widget.roomId),
        );
        // A recent light command that failed to physically reach its target.
        // Shown in the spinner slot once the in-flight state clears.
        final dispatchFailure =
            context.select<ServerSyncProvider, RhythmDispatchFailure?>(
                (p) => p.recentDispatchFailureForNode(widget.roomId));
        // Scene currently bound as this room's mood (null = custom color mood).
        final moodSceneId = context.select<ServerSyncProvider, String?>(
            (p) => p.moodSceneIdForRoom(widget.roomId));

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
          RoomMode.standby => _sliderBrightness ?? brightness,
          RoomMode.on => _sliderBrightness ?? brightness,
          RoomMode.off => _sliderBrightness ?? brightness,
        };

        final displayColor =
            mode == RoomMode.mood ? moodColor ?? serverColor : serverColor;
        final cctColor = _sliderKelvin != null
            ? ColorUtils.cctToColor(_sliderKelvin!)
            : displayColor != null
                ? Color.fromARGB(
                    255, displayColor.$1, displayColor.$2, displayColor.$3)
                : ColorUtils.cctToColor(kelvin);

        // The room's mood palette: a scene's colors when scene-backed,
        // otherwise the single custom mood color. Drives the mood glow and the
        // palette badge so the card reflects whatever the mood actually is.
        final moodPalette = <Color>[];
        if (mode == RoomMode.mood) {
          // select (not read): scenes load asynchronously after the card
          // builds, and the badge must pick up the palette when they land.
          final scene = moodSceneId != null
              ? context.select<ServerSyncProvider, RhythmSceneDefinition?>(
                  (p) => p.sceneById(moodSceneId))
              : null;
          if (scene != null) {
            moodPalette.addAll(rhythmSceneSwatch(scene));
          } else {
            moodPalette.add(cctColor);
          }
        }
        final moodPrimary =
            moodPalette.isNotEmpty ? moodPalette.first : cctColor;
        final moodSecondary =
            moodPalette.length > 1 ? moodPalette[1] : moodPrimary;

        final showActivitySpinner =
            _localActionPending || isTransitioning || isDispatchPending;
        final VoidCallback? motionIndicatorTap =
            motionTimer?.remainingSecs != null
                ? null
                : !motionActivationSupported
                    ? () => _showMotionActivationUnavailable(room.name)
                    : motionActivationEnabled
                        ? () => _setMotionActivationEnabled(false)
                        : null;

        // Blend directly from a neutral dark base toward the CCT color —
        // brightness scales the mix so hue stays clear at every level.
        //
        // While a command is in flight, deliberately avoid using the reported
        // light color for the card surface. Some hubs briefly report warning
        // or stale RGB values during turn-on, and showing those as the card
        // background makes users think the light itself changed color.
        final dimT = displayBrightness / 100.0;
        const darkBase = Color(0xFF141210);
        final lightColorBg = switch (mode) {
          RoomMode.mood => Color.lerp(darkBase, cctColor, 0.22)!,
          // Low glow is an ambient state, not a sampled paint color. Keep the
          // card neutral and let the localized halo below carry the warmth.
          RoomMode.standby => const Color(0xFF171A20),
          RoomMode.on => Color.lerp(darkBase, cctColor, 0.10 + dimT * 0.50)!,
          RoomMode.off => CelestialColors.backgroundCard,
        };
        final bgColor =
            showActivitySpinner ? const Color(0xFF1B222C) : lightColorBg;

        // Text and icon colors per mode — use luminance-based contrast
        // (same approach as _RhythmPill) so names stay readable at any brightness.
        final onLight = mode == RoomMode.on && bgColor.computeLuminance() > 0.4;
        final titleColor = switch (mode) {
          RoomMode.mood => const Color(0xFFFFF3DF),
          RoomMode.standby => const Color(0xFFFFF0D5),
          RoomMode.on => onLight
              ? (kelvin < 4000
                  ? const Color(0xFF3A2A1A) // warm dark brown
                  : const Color(0xFF2A2C30)) // cool dark grey
              : Colors.white,
          RoomMode.off => const Color(0xFFE8EBF0),
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
        final resetAvailable = offCurve || mode == RoomMode.mood;

        final adaptiveSlidersVisible =
            mode == RoomMode.on || mode == RoomMode.standby;
        final brightnessSliderActive =
            !isTransitioning && adaptiveSlidersVisible;
        final cctSliderActive = !isTransitioning && adaptiveSlidersVisible;
        final sceneSliderActive = !isTransitioning && mode == RoomMode.mood;
        final cctRange = _CctSideRange.fromCurveData(
          widget.curveData,
          effectiveHour: _effectiveCurveHour(room),
          fallbackMin: _minKelvin,
          fallbackMax: _maxKelvin,
        );
        final rhythmGlowActive = mode == RoomMode.on && room.rhythmEnabled;
        final glowColor = showActivitySpinner
            ? CelestialColors.accentBlue.withValues(alpha: 0.65)
            : cctColor;
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
              // Only register double-tap when reset is available to avoid tap
              // delay on cards that are already following the curve.
              onDoubleTap: resetAvailable ? _resetRoom : null,
              child: AnimatedContainer(
                key: ValueKey('room-card-surface-${widget.roomId}'),
                duration: const Duration(milliseconds: 400),
                curve: Curves.easeInOut,
                clipBehavior: Clip.antiAlias,
                decoration: BoxDecoration(
                  color: bgColor,
                  borderRadius: BorderRadius.circular(20),
                ),
                child: Stack(
                  children: [
                    if (mode == RoomMode.standby)
                      Positioned.fill(
                        child: IgnorePointer(
                          child: DecoratedBox(
                            key: ValueKey(
                              'room-card-low-glow-halo-${widget.roomId}',
                            ),
                            decoration: BoxDecoration(
                              borderRadius: BorderRadius.circular(20),
                              gradient: RadialGradient(
                                center: const Alignment(-0.82, 0.48),
                                radius: 1.05,
                                colors: [
                                  cctColor.withValues(alpha: 0.16),
                                  cctColor.withValues(alpha: 0.045),
                                  Colors.transparent,
                                ],
                                stops: const [0.0, 0.48, 1.0],
                              ),
                            ),
                          ),
                        ),
                      ),
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
                                  moodPrimary.withValues(alpha: 0.45),
                                  moodPrimary.withValues(alpha: 0.12),
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
                                  moodSecondary.withValues(alpha: 0.24),
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
                        // Top row: room name + activity, with motion docked right.
                        Padding(
                          padding: const EdgeInsets.fromLTRB(14, 14, 14, 0),
                          child: Row(
                            children: [
                              if (titleIcon != null)
                                Padding(
                                  padding: const EdgeInsets.only(right: 8),
                                  child: Icon(
                                    titleIcon,
                                    size: 20,
                                    color: iconColor.withValues(alpha: 0.68),
                                  ),
                                ),
                              Expanded(
                                child: Row(
                                  children: [
                                    Flexible(
                                      fit: FlexFit.loose,
                                      child: Text(
                                        room.name,
                                        key: ValueKey(
                                          'room-card-title-${widget.roomId}',
                                        ),
                                        maxLines: 2,
                                        overflow: TextOverflow.ellipsis,
                                        style: TextStyle(
                                          color: titleColor,
                                          fontSize: 20,
                                          fontWeight: FontWeight.w700,
                                          letterSpacing: -0.35,
                                          height: 1.08,
                                          shadows: onLight
                                              ? const []
                                              : [
                                                  Shadow(
                                                    color: Colors.black
                                                        .withValues(
                                                            alpha: 0.42),
                                                    blurRadius: 10,
                                                    offset: const Offset(0, 1),
                                                  ),
                                                ],
                                        ),
                                      ),
                                    ),
                                    const SizedBox(width: 8),
                                    SizedBox(
                                      key: ValueKey(
                                        'room-card-activity-${widget.roomId}',
                                      ),
                                      width: 18,
                                      height: 18,
                                      child: AnimatedSwitcher(
                                        duration:
                                            const Duration(milliseconds: 160),
                                        child: showActivitySpinner
                                            ? _RoomTransitionSpinner(
                                                key: const ValueKey(
                                                  'room_transition_spinner',
                                                ),
                                                color: iconColor,
                                              )
                                            : dispatchFailure != null
                                                ? _DispatchFailureBadge(
                                                    key: const ValueKey(
                                                      'room_dispatch_failure_badge',
                                                    ),
                                                    failure: dispatchFailure,
                                                    roomName: room.name,
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
                              if (motionActivationPending)
                                Padding(
                                  padding: const EdgeInsets.only(left: 8),
                                  child: Semantics(
                                    liveRegion: true,
                                    label:
                                        'Updating motion activation for ${room.name}',
                                    child: GestureDetector(
                                      key: ValueKey(
                                        'room-card-motion-pending-${widget.roomId}',
                                      ),
                                      behavior: HitTestBehavior.opaque,
                                      onTap: () {},
                                      child: SizedBox(
                                        width: _roomHeaderActionHitSize,
                                        height: _roomHeaderActionHitSize,
                                        child: const Align(
                                          alignment: Alignment.centerRight,
                                          child: SizedBox(
                                            width: 28,
                                            height: 28,
                                            child: Padding(
                                              padding: EdgeInsets.all(6),
                                              child: CircularProgressIndicator(
                                                strokeWidth: 2,
                                              ),
                                            ),
                                          ),
                                        ),
                                      ),
                                    ),
                                  ),
                                )
                              else if (motionTimer != null)
                                Padding(
                                  padding: const EdgeInsets.only(left: 8),
                                  child: _MotionIndicator(
                                    key: ValueKey(
                                      'room-card-motion-${widget.roomId}',
                                    ),
                                    info: motionTimer,
                                    color: iconColor,
                                    roomName: room.name,
                                    onTap: motionIndicatorTap,
                                    onExpired: () => context
                                        .read<RoomProvider>()
                                        .clearMotionTimer(widget.roomId),
                                  ),
                                )
                              else if (hasSensor)
                                Padding(
                                  padding: const EdgeInsets.only(left: 8),
                                  child: Semantics(
                                    button: true,
                                    label: motionActivationSupported
                                        ? motionActivationEnabled
                                            ? 'Turn off motion activation for ${room.name}'
                                            : 'Turn on motion activation for ${room.name}'
                                        : 'Motion control for ${room.name} '
                                            'requires an appliance update',
                                    child: GestureDetector(
                                      key: ValueKey(
                                        'room-card-motion-${widget.roomId}',
                                      ),
                                      behavior: HitTestBehavior.opaque,
                                      onTap: motionActivationSupported
                                          ? () => _setMotionActivationEnabled(
                                                !motionActivationEnabled,
                                              )
                                          : () =>
                                              _showMotionActivationUnavailable(
                                                room.name,
                                              ),
                                      child: SizedBox(
                                        width: _roomHeaderActionHitSize,
                                        height: _roomHeaderActionHitSize,
                                        child: Align(
                                          alignment: Alignment.centerRight,
                                          child: SizedBox(
                                            width: 28,
                                            height: 28,
                                            child: Icon(
                                              motionActivationEnabled
                                                  ? Icons.sensors_rounded
                                                  : Icons.sensors_off_rounded,
                                              size: 18,
                                              color: iconColor.withValues(
                                                alpha: motionActivationEnabled
                                                    ? motionActivationSupported
                                                        ? 0.45
                                                        : 0.18
                                                    : 0.30,
                                              ),
                                            ),
                                          ),
                                        ),
                                      ),
                                    ),
                                  ),
                                ),
                            ],
                          ),
                        ),
                        // Power and Scenes mirror each other across the active
                        // sliders so the primary choices anchor both edges.
                        Padding(
                          padding: const EdgeInsets.fromLTRB(12, 12, 12, 8),
                          child: Column(
                            mainAxisSize: MainAxisSize.min,
                            children: [
                              Row(
                                crossAxisAlignment: CrossAxisAlignment.center,
                                children: [
                                  _RoomPowerControl(
                                    roomId: widget.roomId,
                                    mode: mode,
                                    standbyEnabled: standbyEnabled,
                                    onModeChanged: _onModeChanged,
                                    onReset: _resetToOn,
                                    cctColor: cctColor,
                                    enabled: !isTransitioning,
                                  ),
                                  if (adaptiveSlidersVisible) ...[
                                    const SizedBox(width: 8),
                                    Expanded(
                                      child: Column(
                                        key: ValueKey(
                                          'room-card-slider-stack-${widget.roomId}',
                                        ),
                                        mainAxisSize: MainAxisSize.min,
                                        children: [
                                          GestureDetector(
                                            behavior: HitTestBehavior.opaque,
                                            onTap: () {},
                                            onLongPress: () {},
                                            child: SliderTheme(
                                              data: SliderThemeData(
                                                trackHeight: 24,
                                                thumbShape:
                                                    const _SunSliderThumbShape(
                                                  icon: Icons.wb_sunny_rounded,
                                                ),
                                                overlayShape:
                                                    const RoundSliderOverlayShape(
                                                  overlayRadius: 36,
                                                ),
                                                padding:
                                                    const EdgeInsets.fromLTRB(
                                                  _SunSliderThumbShape.radius -
                                                      8,
                                                  10,
                                                  _SunSliderThumbShape.radius -
                                                      8,
                                                  4,
                                                ),
                                                trackShape:
                                                    const RoundedRectSliderTrackShape(),
                                                activeTrackColor:
                                                    sliderActiveTrackColor,
                                                inactiveTrackColor:
                                                    sliderInactiveTrackColor,
                                                thumbColor: sliderThumbColor,
                                                overlayColor:
                                                    sliderOverlayColor,
                                                disabledActiveTrackColor:
                                                    CelestialColors.orbitRing
                                                        .withValues(
                                                            alpha: 0.35),
                                                disabledInactiveTrackColor:
                                                    CelestialColors.orbitRing
                                                        .withValues(
                                                            alpha: 0.18),
                                                disabledThumbColor:
                                                    CelestialColors
                                                        .textSecondary
                                                        .withValues(
                                                            alpha: 0.55),
                                              ),
                                              child: Slider(
                                                key: ValueKey(
                                                  'room-card-brightness-slider-${widget.roomId}',
                                                ),
                                                value: displayBrightness
                                                    .toDouble()
                                                    .clamp(1, 100),
                                                min: 1,
                                                max: 100,
                                                onChanged:
                                                    brightnessSliderActive
                                                        ? (value) {
                                                            _activateLowGlowRoomForSlider();
                                                            setState(() {
                                                              _sliderBrightness =
                                                                  value.round();
                                                            });
                                                          }
                                                        : null,
                                                onChangeEnd: brightnessSliderActive
                                                    ? (_) =>
                                                        _onBrightnessSliderEnd()
                                                    : null,
                                              ),
                                            ),
                                          ),
                                          GestureDetector(
                                            behavior: HitTestBehavior.opaque,
                                            onTap: () {},
                                            onLongPress: () {},
                                            child: SliderTheme(
                                              data: SliderThemeData(
                                                trackHeight: 24,
                                                thumbShape:
                                                    const _SunSliderThumbShape(
                                                  icon: Icons.contrast_rounded,
                                                ),
                                                overlayShape:
                                                    const RoundSliderOverlayShape(
                                                  overlayRadius: 36,
                                                ),
                                                padding:
                                                    const EdgeInsets.fromLTRB(
                                                  _SunSliderThumbShape.radius -
                                                      8,
                                                  4,
                                                  _SunSliderThumbShape.radius -
                                                      8,
                                                  10,
                                                ),
                                                trackShape:
                                                    _CCTGradientTrackShape(
                                                  minKelvin: cctRange.minKelvin,
                                                  maxKelvin: cctRange.maxKelvin,
                                                ),
                                                activeTrackColor:
                                                    sliderActiveTrackColor,
                                                inactiveTrackColor:
                                                    sliderInactiveTrackColor,
                                                thumbColor: sliderThumbColor,
                                                overlayColor:
                                                    sliderOverlayColor,
                                                disabledActiveTrackColor:
                                                    cctColor.withValues(
                                                        alpha: 0.30),
                                                disabledInactiveTrackColor:
                                                    Colors.black.withValues(
                                                        alpha: 0.15),
                                                disabledThumbColor:
                                                    CelestialColors
                                                        .textSecondary
                                                        .withValues(
                                                            alpha: 0.55),
                                              ),
                                              child: Slider(
                                                key: ValueKey(
                                                  'room-card-cct-slider-${widget.roomId}',
                                                ),
                                                value: cctRange
                                                    .clampKelvin(
                                                      _sliderKelvin ?? kelvin,
                                                    )
                                                    .toDouble(),
                                                min: cctRange.minKelvin
                                                    .toDouble(),
                                                max: cctRange.maxKelvin
                                                    .toDouble(),
                                                onChanged: cctSliderActive
                                                    ? (value) {
                                                        _activateLowGlowRoomForSlider();
                                                        setState(() {
                                                          _sliderKelvin =
                                                              value.round();
                                                        });
                                                      }
                                                    : null,
                                                onChangeEnd: cctSliderActive
                                                    ? (_) =>
                                                        _onKelvinSliderEnd()
                                                    : null,
                                              ),
                                            ),
                                          ),
                                        ],
                                      ),
                                    ),
                                  ] else if (mode == RoomMode.mood) ...[
                                    const SizedBox(width: 8),
                                    Expanded(
                                      child: GestureDetector(
                                        behavior: HitTestBehavior.opaque,
                                        onTap: () {},
                                        onLongPress: () {},
                                        child: Row(
                                          key: ValueKey(
                                            'room-card-scene-slider-row-${widget.roomId}',
                                          ),
                                          children: [
                                            GestureDetector(
                                              key: ValueKey(
                                                'room-card-scene-picker-${widget.roomId}',
                                              ),
                                              behavior: HitTestBehavior.opaque,
                                              onTap: sceneSliderActive
                                                  ? _showMoodScenePicker
                                                  : null,
                                              child: AnimatedContainer(
                                                duration: const Duration(
                                                  milliseconds: 220,
                                                ),
                                                width: 32,
                                                height: 32,
                                                decoration: BoxDecoration(
                                                  borderRadius:
                                                      BorderRadius.circular(14),
                                                  color: Colors.white
                                                      .withValues(alpha: 0.07),
                                                  border: Border.all(
                                                    color: Colors.white
                                                        .withValues(
                                                            alpha: 0.12),
                                                  ),
                                                  boxShadow: [
                                                    BoxShadow(
                                                      color: Colors.black
                                                          .withValues(
                                                              alpha: 0.35),
                                                      blurRadius: 4,
                                                      offset:
                                                          const Offset(0, 2),
                                                    ),
                                                  ],
                                                ),
                                                child: Center(
                                                  child: MoodPaletteBadge(
                                                    key: ValueKey(
                                                      'room-card-scene-palette-${widget.roomId}',
                                                    ),
                                                    colors: moodPalette.isEmpty
                                                        ? [cctColor]
                                                        : moodPalette,
                                                    size: 20,
                                                    glow: false,
                                                  ),
                                                ),
                                              ),
                                            ),
                                            Expanded(
                                              child: SliderTheme(
                                                data: SliderThemeData(
                                                  trackHeight: 24,
                                                  thumbShape:
                                                      const _SunSliderThumbShape(
                                                    icon:
                                                        Icons.wb_sunny_rounded,
                                                  ),
                                                  overlayShape:
                                                      const RoundSliderOverlayShape(
                                                    overlayRadius: 36,
                                                  ),
                                                  padding: const EdgeInsets
                                                      .symmetric(
                                                    horizontal:
                                                        _SunSliderThumbShape
                                                                .radius -
                                                            8,
                                                    vertical: 10,
                                                  ),
                                                  trackShape:
                                                      const RoundedRectSliderTrackShape(),
                                                  activeTrackColor:
                                                      sliderActiveTrackColor,
                                                  inactiveTrackColor:
                                                      sliderInactiveTrackColor,
                                                  thumbColor: sliderThumbColor,
                                                  overlayColor:
                                                      sliderOverlayColor,
                                                  disabledActiveTrackColor:
                                                      cctColor.withValues(
                                                          alpha: 0.30),
                                                  disabledInactiveTrackColor:
                                                      Colors.black.withValues(
                                                          alpha: 0.15),
                                                  disabledThumbColor:
                                                      Color.lerp(
                                                    Colors.white,
                                                    cctColor,
                                                    0.25,
                                                  ),
                                                ),
                                                child: Slider(
                                                  key: ValueKey(
                                                    'room-card-scene-brightness-slider-${widget.roomId}',
                                                  ),
                                                  value: displayBrightness
                                                      .toDouble()
                                                      .clamp(1, 100),
                                                  min: 1,
                                                  max: 100,
                                                  onChanged: sceneSliderActive
                                                      ? (value) {
                                                          setState(() {
                                                            _sliderBrightness =
                                                                value.round();
                                                          });
                                                        }
                                                      : null,
                                                  onChangeEnd: sceneSliderActive
                                                      ? (_) =>
                                                          _onSceneBrightnessSliderEnd(
                                                            cctColor,
                                                          )
                                                      : null,
                                                ),
                                              ),
                                            ),
                                          ],
                                        ),
                                      ),
                                    ),
                                  ],
                                  if (!adaptiveSlidersVisible &&
                                      mode != RoomMode.mood)
                                    const Spacer(),
                                  const SizedBox(width: 8),
                                  _RoomScenesJewel(
                                    roomId: widget.roomId,
                                    active: mode == RoomMode.mood,
                                    palette: moodPalette,
                                    enabled: !isTransitioning,
                                    onPressed: _onScenesJewelPressed,
                                  ),
                                ],
                              ),
                              if (mode == RoomMode.standby) ...[
                                const SizedBox(height: 6),
                                _LowGlowHardOffAction(
                                  roomId: widget.roomId,
                                  enabled: !isTransitioning,
                                  onPressed: () => _onModeChanged(RoomMode.off),
                                ),
                              ],
                            ],
                          ),
                        ),
                      ],
                    ),
                    // Rhythm-active breathing border — solid while the room
                    // is actually tracking the curve. Yields to the broken
                    // orbit ring when the room has drifted off-curve.
                    Positioned.fill(
                      child: IgnorePointer(
                        child: _RhythmBorderGlow(
                          active: rhythmGlowActive && !resetAvailable,
                          color: glowColor,
                        ),
                      ),
                    ),
                    // Reset-available "broken orbit" ring — drifting amber
                    // dashes signal a manual adjustment or active Scene, so
                    // the room is not currently following the curve.
                    Positioned.fill(
                      child: IgnorePointer(
                        child: _OffCurveOrbitRing(
                          opacityKey: ValueKey(
                            'room-card-reset-ring-${widget.roomId}',
                          ),
                          active: resetAvailable,
                          color: glowColor,
                        ),
                      ),
                    ),
                    // Re-sync "clasp" docked on the broken ring's top-right
                    // corner — tap to snap the room back onto the curve,
                    // closing the orbit. Present for drift and active Scenes.
                    Positioned(
                      top: 0,
                      right: 0,
                      child: _OffCurveResetButton(
                        opacityKey: ValueKey(
                          'room-card-reset-control-opacity-${widget.roomId}',
                        ),
                        controlKey: ValueKey(
                          'room-card-reset-control-${widget.roomId}',
                        ),
                        active: resetAvailable,
                        color: glowColor,
                        onReset: _resetRoom,
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

/// The failure signal color — a warm coral-red that sits naturally on the
/// card's deep celestial surfaces instead of a raw material red.
const _kDispatchFailureRed = Color(0xFFFF6159);

/// Red (i) badge shown in the spinner slot after a light command failed to
/// physically reach its target. Tapping reveals an anchored popover with the
/// failure narrative.
class _DispatchFailureBadge extends StatefulWidget {
  const _DispatchFailureBadge({
    super.key,
    required this.failure,
    required this.roomName,
  });

  final RhythmDispatchFailure failure;
  final String roomName;

  @override
  State<_DispatchFailureBadge> createState() => _DispatchFailureBadgeState();
}

class _DispatchFailureBadgeState extends State<_DispatchFailureBadge> {
  final LayerLink _link = LayerLink();
  OverlayEntry? _popover;

  @override
  void dispose() {
    _removePopover();
    super.dispose();
  }

  void _removePopover() {
    _popover?.remove();
    _popover = null;
  }

  void _showPopover() {
    if (_popover != null) {
      _removePopover();
      return;
    }
    HapticFeedback.lightImpact();
    final entry = OverlayEntry(
      builder: (context) => Stack(
        children: [
          // Tap-outside barrier.
          Positioned.fill(
            child: GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTap: _removePopover,
            ),
          ),
          CompositedTransformFollower(
            link: _link,
            targetAnchor: Alignment.bottomRight,
            followerAnchor: Alignment.topRight,
            offset: const Offset(9, 8),
            child: _DispatchFailurePopover(
              failure: widget.failure,
              roomName: widget.roomName,
            ),
          ),
        ],
      ),
    );
    Overlay.of(context).insert(entry);
    _popover = entry;
  }

  @override
  Widget build(BuildContext context) {
    return CompositedTransformTarget(
      link: _link,
      child: Semantics(
        button: true,
        label: 'Light command failed — details',
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTap: _showPopover,
          // Settle-in: the badge lands where the spinner just was, so it
          // arrives with a small overshoot instead of just appearing.
          child: TweenAnimationBuilder<double>(
            tween: Tween(begin: 0.6, end: 1),
            duration: const Duration(milliseconds: 340),
            curve: Curves.easeOutBack,
            builder: (context, scale, child) =>
                Transform.scale(scale: scale, child: child),
            child: Container(
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                boxShadow: [
                  BoxShadow(
                    color: _kDispatchFailureRed.withValues(alpha: 0.45),
                    blurRadius: 9,
                    spreadRadius: 0.5,
                  ),
                ],
              ),
              child: const Icon(
                Icons.info,
                size: 18,
                color: _kDispatchFailureRed,
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// Compact anchored panel: "Failed to [action] [device]", nothing more.
class _DispatchFailurePopover extends StatelessWidget {
  const _DispatchFailurePopover({
    required this.failure,
    required this.roomName,
  });

  final RhythmDispatchFailure failure;
  final String roomName;

  String get _message {
    final action = switch (failure.kind) {
      'turn_on' => 'turn on',
      'turn_off' => 'turn off',
      _ => 'reach',
    };
    return 'Failed to $action $roomName';
  }

  String get _relativeTime {
    if (failure.epochMs <= 0) return 'just now';
    final elapsed = DateTime.now().difference(
      DateTime.fromMillisecondsSinceEpoch(failure.epochMs),
    );
    if (elapsed.inSeconds < 5) return 'just now';
    if (elapsed.inSeconds < 60) return '${elapsed.inSeconds}s ago';
    return '${elapsed.inMinutes}m ago';
  }

  @override
  Widget build(BuildContext context) {
    return TweenAnimationBuilder<double>(
      tween: Tween(begin: 0, end: 1),
      duration: const Duration(milliseconds: 180),
      curve: Curves.easeOutCubic,
      builder: (context, t, child) => Opacity(
        opacity: t,
        child: Transform.scale(
          scale: 0.94 + 0.06 * t,
          alignment: Alignment.topRight,
          child: child,
        ),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.end,
        mainAxisSize: MainAxisSize.min,
        children: [
          // Caret pointing up at the badge.
          Padding(
            padding: const EdgeInsets.only(right: 12),
            child: CustomPaint(
              size: const Size(14, 7),
              painter: _PopoverCaretPainter(),
            ),
          ),
          Material(
            color: Colors.transparent,
            child: Container(
              padding: const EdgeInsets.fromLTRB(13, 10, 13, 10),
              constraints: const BoxConstraints(maxWidth: 240),
              decoration: BoxDecoration(
                color: const Color(0xF20E141B),
                borderRadius: BorderRadius.circular(12),
                border: Border.all(
                  color: _kDispatchFailureRed.withValues(alpha: 0.28),
                ),
                boxShadow: [
                  const BoxShadow(
                    color: Color(0xB3000000),
                    blurRadius: 24,
                    offset: Offset(0, 10),
                  ),
                  BoxShadow(
                    color: _kDispatchFailureRed.withValues(alpha: 0.08),
                    blurRadius: 32,
                  ),
                ],
              ),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Container(
                        width: 7,
                        height: 7,
                        decoration: BoxDecoration(
                          shape: BoxShape.circle,
                          color: _kDispatchFailureRed,
                          boxShadow: [
                            BoxShadow(
                              color: _kDispatchFailureRed.withValues(
                                alpha: 0.6,
                              ),
                              blurRadius: 6,
                            ),
                          ],
                        ),
                      ),
                      const SizedBox(width: 8),
                      Flexible(
                        child: Text(
                          _message,
                          style: const TextStyle(
                            color: CelestialColors.textPrimary,
                            fontSize: 13,
                            fontWeight: FontWeight.w600,
                            height: 1.35,
                          ),
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: 3),
                  Padding(
                    padding: const EdgeInsets.only(left: 15),
                    child: Text(
                      _relativeTime,
                      style: TextStyle(
                        color: CelestialColors.textSecondary.withValues(
                          alpha: 0.8,
                        ),
                        fontSize: 11,
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _PopoverCaretPainter extends CustomPainter {
  @override
  void paint(Canvas canvas, Size size) {
    final path = Path()
      ..moveTo(size.width / 2, 0)
      ..lineTo(size.width, size.height)
      ..lineTo(0, size.height)
      ..close();
    canvas.drawPath(path, Paint()..color = const Color(0xF20E141B));
    final edge = Paint()
      ..color = _kDispatchFailureRed.withValues(alpha: 0.28)
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1;
    canvas.drawLine(
      Offset(size.width / 2, 0),
      Offset(0, size.height),
      edge,
    );
    canvas.drawLine(
      Offset(size.width / 2, 0),
      Offset(size.width, size.height),
      edge,
    );
  }

  @override
  bool shouldRepaint(covariant CustomPainter oldDelegate) => false;
}

class _SunSliderThumbShape extends SliderComponentShape {
  const _SunSliderThumbShape({this.icon = Icons.wb_sunny_rounded});

  final IconData icon;
  static const radius = 18.0;
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
          fontSize: 19,
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

class _CctSideRange {
  const _CctSideRange({
    required this.minKelvin,
    required this.maxKelvin,
  });

  final int minKelvin;
  final int maxKelvin;
  static const _sampleCount = 64;

  static _CctSideRange fromCurveData(
    CurveData? data, {
    required double effectiveHour,
    required int fallbackMin,
    required int fallbackMax,
  }) {
    if (data == null || data.hours.isEmpty || data.kelvin.isEmpty) {
      return _fallback(fallbackMin: fallbackMin, fallbackMax: fallbackMax);
    }

    final solarNoon = normalizeHour(data.solar.solarNoon);
    final normalizedHour = normalizeHour(effectiveHour);
    final onMorningSide = normalizedHour <= solarNoon;
    final startHour = onMorningSide ? 0.0 : solarNoon;
    final endHour = onMorningSide ? solarNoon : 24.0;

    if ((endHour - startHour).abs() < 0.25) {
      return _fallback(fallbackMin: fallbackMin, fallbackMax: fallbackMax);
    }

    final samples = <int>[];
    for (var i = 0; i <= _sampleCount; i++) {
      final t = i / _sampleCount;
      final hour = startHour + (endHour - startHour) * t;
      final kelvin = SolarUtils.kelvinAtHour(
        hour,
        hours: data.hours,
        kelvin: data.kelvin,
      );
      if (kelvin > 0) {
        samples.add(kelvin.clamp(fallbackMin, fallbackMax).toInt());
      }
    }

    if (samples.length < 2) {
      return _fallback(fallbackMin: fallbackMin, fallbackMax: fallbackMax);
    }

    final minKelvin = samples.reduce(math.min);
    final maxKelvin = samples.reduce(math.max);
    if (maxKelvin <= minKelvin) {
      return _fallback(fallbackMin: fallbackMin, fallbackMax: fallbackMax);
    }

    return _CctSideRange(minKelvin: minKelvin, maxKelvin: maxKelvin);
  }

  static double normalizeHour(double hour) => ((hour % 24) + 24) % 24;

  static _CctSideRange _fallback({
    required int fallbackMin,
    required int fallbackMax,
  }) =>
      _CctSideRange(minKelvin: fallbackMin, maxKelvin: fallbackMax);

  int clampKelvin(int kelvin) => kelvin.clamp(minKelvin, maxKelvin).toInt();
}

/// Warm-to-cool gradient track for CCT slider mode.
class _CCTGradientTrackShape extends SliderTrackShape
    with BaseSliderTrackShape {
  const _CCTGradientTrackShape({
    required this.minKelvin,
    required this.maxKelvin,
  });

  final int minKelvin;
  final int maxKelvin;
  static const _gradientSteps = 4;

  // Match RoundedRectSliderTrackShape so the thumb travel is inset from the
  // rounded caps — otherwise the CCT thumb runs to the very edge while the
  // brightness thumb stops short.
  @override
  bool get isRounded => true;

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
    final baseRect = getPreferredRect(
      parentBox: parentBox,
      offset: offset,
      sliderTheme: sliderTheme,
    );
    // RoundedRectSliderTrackShape draws its active segment
    // `additionalActiveTrackHeight` (default 2px) taller than the base track.
    // Inflate vertically to match the brightness slider's thickness.
    const additionalActiveTrackHeight = 2.0;
    final trackRect = Rect.fromLTRB(
      baseRect.left,
      baseRect.top - additionalActiveTrackHeight / 2,
      baseRect.right,
      baseRect.bottom + additionalActiveTrackHeight / 2,
    );
    final radius = Radius.circular(trackRect.height / 2);
    final rrect = RRect.fromRectAndRadius(trackRect, radius);
    final colors = [
      for (var i = 0; i <= _gradientSteps; i++)
        ColorUtils.curveColorForCCT(
          (minKelvin + (maxKelvin - minKelvin) * (i / _gradientSteps)).round(),
        ),
    ];
    final gradient = LinearGradient(colors: colors);

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

/// Primary room power control used in the upper room-card control row.
class _RoomPowerControl extends StatelessWidget {
  final String roomId;
  final RoomMode mode;
  final bool standbyEnabled;
  final ValueChanged<RoomMode> onModeChanged;

  /// Tapping "On" from any non-adaptive state resets the room to its live
  /// curve (same as the "Reset to curve" action), rather than a plain On.
  final VoidCallback onReset;
  final Color cctColor;
  final bool enabled;

  const _RoomPowerControl({
    required this.roomId,
    required this.mode,
    required this.standbyEnabled,
    required this.onModeChanged,
    required this.onReset,
    required this.cctColor,
    this.enabled = true,
  });

  @override
  Widget build(BuildContext context) {
    final powerState = switch (mode) {
      RoomMode.off => _PowerToggleState.off,
      RoomMode.standby => _PowerToggleState.standby,
      RoomMode.on || RoomMode.mood => _PowerToggleState.on,
    };

    return AnimatedOpacity(
      key: ValueKey('room-card-power-control-$roomId'),
      duration: const Duration(milliseconds: 160),
      opacity: enabled ? 1.0 : 0.58,
      child: SizedBox(
        width: 56,
        child: _RoomPowerTogglePill(
          key: ValueKey('room-card-control-pill-$roomId-off'),
          roomId: roomId,
          state: powerState,
          standbyAvailable: standbyEnabled,
          onChanged: (nextState) {
            switch (nextState) {
              case _PowerToggleState.on:
                onReset();
              case _PowerToggleState.standby:
                onModeChanged(RoomMode.standby);
              case _PowerToggleState.off:
                onModeChanged(RoomMode.off);
            }
          },
          enabled: enabled,
          cctColor: cctColor,
        ),
      ),
    );
  }
}

/// Jewel-shaped Scenes control mirrored opposite the Power jewel.
class _RoomScenesJewel extends StatefulWidget {
  final String roomId;
  final bool active;
  final List<Color> palette;
  final bool enabled;
  final VoidCallback onPressed;

  const _RoomScenesJewel({
    required this.roomId,
    required this.active,
    required this.palette,
    required this.enabled,
    required this.onPressed,
  });

  @override
  State<_RoomScenesJewel> createState() => _RoomScenesJewelState();
}

class _RoomScenesJewelState extends State<_RoomScenesJewel> {
  bool _pressed = false;

  @override
  Widget build(BuildContext context) {
    const fallbackPalette = [
      Color(0xFFFF7BC8),
      Color(0xFF8E7CFF),
      Color(0xFF5EE7F7),
    ];
    final palette =
        widget.palette.isEmpty ? fallbackPalette : widget.palette;
    final primary = palette.first;
    final secondary = palette.length > 1 ? palette[1] : primary;

    return AnimatedOpacity(
      duration: const Duration(milliseconds: 160),
      opacity: widget.enabled ? 1.0 : 0.58,
      child: Semantics(
        excludeSemantics: true,
        button: true,
        enabled: widget.enabled,
        label: 'Scenes',
        value: widget.active ? 'Active' : 'Inactive',
        hint: widget.active ? 'Choose a scene' : 'Use a scene',
        onTap: widget.enabled ? widget.onPressed : null,
        child: GestureDetector(
          key: ValueKey('room-card-segment-${widget.roomId}-scenes'),
          behavior: HitTestBehavior.opaque,
          onTap: widget.enabled ? widget.onPressed : null,
          onTapDown: widget.enabled
              ? (_) => setState(() => _pressed = true)
              : null,
          onTapUp: widget.enabled
              ? (_) => setState(() => _pressed = false)
              : null,
          onTapCancel: widget.enabled
              ? () => setState(() => _pressed = false)
              : null,
          child: AnimatedScale(
            scale: _pressed ? 0.90 : 1.0,
            duration: const Duration(milliseconds: 110),
            curve: Curves.easeOut,
            child: AnimatedContainer(
              key: ValueKey(
                'room-card-control-pill-${widget.roomId}-scenes',
              ),
              duration: const Duration(milliseconds: 280),
              curve: Curves.easeOutCubic,
              width: 56,
              height: 56,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                gradient: LinearGradient(
                  begin: Alignment.topLeft,
                  end: Alignment.bottomRight,
                  colors: [
                    Color.lerp(
                      const Color(0xFF24202D),
                      primary,
                      widget.active ? 0.58 : 0.34,
                    )!,
                    Color.lerp(
                      const Color(0xFF121821),
                      secondary,
                      widget.active ? 0.42 : 0.22,
                    )!,
                  ],
                ),
                border: Border.all(
                  color: Colors.white.withValues(
                    alpha: widget.active ? 0.42 : 0.20,
                  ),
                  width: widget.active ? 1.5 : 1,
                ),
                boxShadow: [
                  BoxShadow(
                    color: primary.withValues(
                      alpha: widget.active ? 0.34 : 0.18,
                    ),
                    blurRadius: widget.active ? 18 : 12,
                    spreadRadius: widget.active ? 1 : 0,
                  ),
                  BoxShadow(
                    color: Colors.black.withValues(alpha: 0.30),
                    blurRadius: 6,
                    offset: const Offset(0, 3),
                  ),
                ],
              ),
              child: Stack(
                alignment: Alignment.center,
                children: [
                  Positioned(
                    top: 12,
                    child: Icon(
                      Icons.auto_awesome_rounded,
                      size: 18,
                      color: Colors.white.withValues(alpha: 0.95),
                    ),
                  ),
                  Positioned(
                    bottom: 10,
                    child: Text(
                      'Scenes',
                      style: TextStyle(
                        color: Colors.white.withValues(alpha: 0.92),
                        fontSize: 7.5,
                        height: 1,
                        fontWeight: FontWeight.w700,
                        letterSpacing: -0.1,
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

enum _PowerToggleState { on, standby, off }

/// Power toggle used by every room card. Ordinary rooms alternate between On
/// and hard Off. Low-glow-enabled rooms alternate between On and the existing
/// Standby state; full Off is a separate explicit action while Low glow is
/// active.
class _RoomPowerTogglePill extends StatefulWidget {
  final String roomId;
  final _PowerToggleState state;
  final bool standbyAvailable;
  final ValueChanged<_PowerToggleState> onChanged;
  final bool enabled;
  final Color cctColor;

  const _RoomPowerTogglePill({
    super.key,
    required this.roomId,
    required this.state,
    required this.standbyAvailable,
    required this.onChanged,
    required this.enabled,
    required this.cctColor,
  });

  @override
  State<_RoomPowerTogglePill> createState() => _RoomPowerTogglePillState();
}

class _RoomPowerTogglePillState extends State<_RoomPowerTogglePill> {
  bool _pressed = false;

  @override
  Widget build(BuildContext context) {
    final activeColor = Color.lerp(widget.cctColor, Colors.white, 0.30)!;
    const inactiveColor = Color(0xFF727B87);
    const standbyColor = Color(0xFFFFC857);
    final deepColor =
        Color.lerp(const Color(0xFF12161C), widget.cctColor, 0.22)!;
    final isOn = widget.state == _PowerToggleState.on;
    final isStandby = widget.state == _PowerToggleState.standby;
    final isOff = widget.state == _PowerToggleState.off;
    final stateColor = isOn
        ? activeColor
        : isStandby
            ? standbyColor
            : inactiveColor;
    final stateLabel = switch (widget.state) {
      _PowerToggleState.on => 'On',
      _PowerToggleState.standby => 'Low glow',
      _PowerToggleState.off => 'Off',
    };
    final nextState = switch (widget.state) {
      _PowerToggleState.on => widget.standbyAvailable
          ? _PowerToggleState.standby
          : _PowerToggleState.off,
      _PowerToggleState.standby => _PowerToggleState.on,
      _PowerToggleState.off => _PowerToggleState.on,
    };
    final actionHint = switch (nextState) {
      _PowerToggleState.on => 'Turn on and reset to the curve',
      _PowerToggleState.standby => 'Use low glow',
      _PowerToggleState.off => 'Turn completely off',
    };

    return Semantics(
      excludeSemantics: true,
      button: true,
      enabled: widget.enabled,
      label: 'Room power',
      value: stateLabel,
      hint: actionHint,
      onTap: widget.enabled ? () => widget.onChanged(nextState) : null,
      child: Tooltip(
        message: '$stateLabel · $actionHint',
        child: GestureDetector(
          key: ValueKey('room-card-power-toggle-${widget.roomId}'),
          behavior: HitTestBehavior.opaque,
          onTap: widget.enabled ? () => widget.onChanged(nextState) : null,
          onTapDown:
              widget.enabled ? (_) => setState(() => _pressed = true) : null,
          onTapUp:
              widget.enabled ? (_) => setState(() => _pressed = false) : null,
          onTapCancel:
              widget.enabled ? () => setState(() => _pressed = false) : null,
          child: AnimatedScale(
            scale: _pressed ? 0.90 : 1.0,
            duration: const Duration(milliseconds: 110),
            curve: Curves.easeOut,
            child: AnimatedContainer(
              key: ValueKey(
                'room-card-power-toggle-surface-${widget.roomId}',
              ),
              duration: const Duration(milliseconds: 280),
              curve: Curves.easeOutCubic,
              width: 56,
              height: 56,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                gradient: LinearGradient(
                  begin: Alignment.topLeft,
                  end: Alignment.bottomRight,
                  colors: isOn
                      ? <Color>[
                          Color.lerp(deepColor, widget.cctColor, 0.22)!,
                          deepColor,
                        ]
                      : isStandby
                          ? const <Color>[
                              Color(0xFF322B18),
                              Color(0xFF1C1B17),
                            ]
                          : const <Color>[
                              Color(0xFF222832),
                              Color(0xFF151A21),
                            ],
                ),
                border: Border.all(
                  color: isOn
                      ? widget.cctColor.withValues(alpha: 0.58)
                      : isStandby
                          ? standbyColor.withValues(alpha: 0.62)
                          : const Color(0xFF353D48),
                  width: isOff ? 1 : 1.5,
                ),
                boxShadow: !isOff && widget.enabled
                    ? [
                        BoxShadow(
                          color: (isStandby ? standbyColor : widget.cctColor)
                              .withValues(alpha: isStandby ? 0.20 : 0.24),
                          blurRadius: 16,
                          spreadRadius: 1,
                        ),
                        BoxShadow(
                          color: Colors.black.withValues(alpha: 0.32),
                          blurRadius: 6,
                          offset: const Offset(0, 3),
                        ),
                      ]
                    : [
                        BoxShadow(
                          color: Colors.black.withValues(alpha: 0.24),
                          blurRadius: 5,
                          offset: const Offset(0, 2),
                        ),
                      ],
              ),
              child: Center(
                child: AnimatedContainer(
                  duration: const Duration(milliseconds: 280),
                  width: 38,
                  height: 38,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: isOn
                        ? widget.cctColor.withValues(alpha: 0.12)
                        : isStandby
                            ? standbyColor.withValues(alpha: 0.10)
                            : Colors.transparent,
                    border: Border.all(
                      color: isOn
                          ? widget.cctColor.withValues(alpha: 0.18)
                          : isStandby
                              ? standbyColor.withValues(alpha: 0.20)
                              : Colors.white.withValues(alpha: 0.04),
                    ),
                  ),
                  child: Column(
                    mainAxisAlignment: MainAxisAlignment.center,
                    children: [
                      Icon(
                        isStandby
                            ? Icons.brightness_low_rounded
                            : Icons.power_settings_new_rounded,
                        key: ValueKey(
                          'room-card-power-toggle-icon-${widget.roomId}',
                        ),
                        size: isStandby ? 17 : 18,
                        color: stateColor,
                      ),
                      const SizedBox(height: 1),
                      Text(
                        stateLabel,
                        key: ValueKey(
                          'room-card-power-toggle-label-${widget.roomId}',
                        ),
                        maxLines: 1,
                        style: TextStyle(
                          color: stateColor.withValues(alpha: 0.92),
                          fontSize: stateLabel.length > 3 ? 7.5 : 8.5,
                          height: 1,
                          fontWeight: FontWeight.w700,
                          letterSpacing: stateLabel.length > 3 ? -0.15 : 0.1,
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _LowGlowHardOffAction extends StatelessWidget {
  final String roomId;
  final bool enabled;
  final VoidCallback onPressed;

  const _LowGlowHardOffAction({
    required this.roomId,
    required this.enabled,
    required this.onPressed,
  });

  @override
  Widget build(BuildContext context) {
    return Align(
      alignment: Alignment.centerLeft,
      child: Semantics(
        excludeSemantics: true,
        button: true,
        enabled: enabled,
        label: 'Turn room off completely',
        onTap: enabled ? onPressed : null,
        child: SizedBox(
          height: 30,
          child: TextButton.icon(
            key: ValueKey('room-card-hard-off-$roomId'),
            onPressed: enabled ? onPressed : null,
            icon: const Icon(Icons.power_settings_new_rounded, size: 13),
            label: const Text('Fully off'),
            style: TextButton.styleFrom(
              foregroundColor:
                  CelestialColors.textSecondary.withValues(alpha: 0.82),
              backgroundColor: Colors.white.withValues(alpha: 0.045),
              padding: const EdgeInsets.symmetric(horizontal: 10),
              minimumSize: Size.zero,
              tapTargetSize: MaterialTapTargetSize.shrinkWrap,
              textStyle: const TextStyle(
                fontSize: 10.5,
                fontWeight: FontWeight.w600,
                letterSpacing: 0.1,
              ),
              shape: const StadiumBorder(),
            ),
          ),
        ),
      ),
    );
  }
}

/// Compact motion indicator: walk icon when active, countdown text when timing out.
class _MotionIndicator extends StatefulWidget {
  final MotionTimerInfo info;
  final Color color;
  final String roomName;
  final VoidCallback? onTap;
  final VoidCallback? onExpired;

  const _MotionIndicator({
    super.key,
    required this.info,
    required this.color,
    required this.roomName,
    this.onTap,
    this.onExpired,
  });

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
    } else {
      // Inactive with no countdown: stop everything, or the pulse ticker
      // keeps running invisibly.
      _countdownTimer?.cancel();
      _countdownTimer = null;
      _pulseController.stop();
      _pulseController.value = 0;
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
    final indicator = widget.info.motionActive
        ? AnimatedBuilder(
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
          )
        : SizedBox(
            width: 28,
            height: 28,
            child: CustomPaint(
              painter: _MiniCountdownPainter(
                progress: widget.info.timeoutSecs > 0
                    ? _interpolatedRemaining / widget.info.timeoutSecs
                    : 0.0,
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

    final isTappable = widget.onTap != null;
    return Semantics(
      button: isTappable,
      label: isTappable
          ? 'Turn off motion activation for ${widget.roomName}'
          : 'Motion timer for ${widget.roomName}',
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        // Consume countdown taps so they do not open the room settings sheet.
        onTap: widget.onTap ?? () {},
        child: SizedBox(
          width: _roomHeaderActionHitSize,
          height: _roomHeaderActionHitSize,
          child: Align(
            alignment: Alignment.centerRight,
            child: SizedBox(
              width: 28,
              height: 28,
              child: Center(child: indicator),
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

/// Dashed "broken orbit" ring drawn around a room card when it can be reset to
/// the curve (manual brightness/time drift or an active Scene). The dashes
/// slowly orbit the perimeter so the card reads as "not locked to the curve" —
/// deliberately distinct from the solid [_RhythmBorderGlow] shown while the
/// room is tracking rhythm.
class _OffCurveOrbitRing extends StatefulWidget {
  final bool active;
  final Color color;
  final Key? opacityKey;

  const _OffCurveOrbitRing({
    required this.active,
    required this.color,
    this.opacityKey,
  });

  @override
  State<_OffCurveOrbitRing> createState() => _OffCurveOrbitRingState();
}

class _OffCurveOrbitRingState extends State<_OffCurveOrbitRing>
    with SingleTickerProviderStateMixin {
  late final AnimationController _controller = AnimationController(
    vsync: this,
    duration: const Duration(seconds: 6),
  );

  @override
  void initState() {
    super.initState();
    if (widget.active) _controller.repeat();
  }

  @override
  void didUpdateWidget(_OffCurveOrbitRing oldWidget) {
    super.didUpdateWidget(oldWidget);
    // Only spin the controller while drifted — keeps idle cards cheap.
    if (widget.active && !_controller.isAnimating) {
      _controller.repeat();
    } else if (!widget.active && _controller.isAnimating) {
      _controller.stop();
    }
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedOpacity(
      key: widget.opacityKey,
      opacity: widget.active ? 1.0 : 0.0,
      duration: Duration(milliseconds: widget.active ? 400 : 500),
      curve: Curves.easeInOut,
      child: RepaintBoundary(
        child: AnimatedBuilder(
          animation: _controller,
          builder: (context, _) => CustomPaint(
            painter: _OrbitRingPainter(
              phase: _controller.value,
              color: widget.color,
            ),
          ),
        ),
      ),
    );
  }
}

/// Paints a dashed rounded-rect stroke whose dashes drift around the path as
/// [phase] sweeps 0→1, producing a slow orbital motion.
class _OrbitRingPainter extends CustomPainter {
  final double phase; // 0..1, drives dash drift
  final Color color;

  static const _inset = 1.0;
  static const _radius = 20.0;
  static const _dash = 11.0;
  static const _gap = 9.0;

  _OrbitRingPainter({required this.phase, required this.color});

  @override
  void paint(Canvas canvas, Size size) {
    final rrect = RRect.fromRectAndRadius(
      Rect.fromLTWH(
        _inset,
        _inset,
        size.width - _inset * 2,
        size.height - _inset * 2,
      ),
      const Radius.circular(_radius - _inset),
    );
    final path = Path()..addRRect(rrect);

    final paint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 2.0
      ..strokeCap = StrokeCap.round
      ..color = color.withValues(alpha: 0.8);

    const period = _dash + _gap;
    // Negative start makes the dashes travel clockwise as phase advances.
    final start = -phase * period;

    for (final metric in path.computeMetrics()) {
      var distance = start;
      while (distance < metric.length) {
        final segStart = distance.clamp(0.0, metric.length);
        final segEnd = (distance + _dash).clamp(0.0, metric.length);
        if (segEnd > segStart) {
          canvas.drawPath(metric.extractPath(segStart, segEnd), paint);
        }
        distance += period;
      }
    }
  }

  @override
  bool shouldRepaint(_OrbitRingPainter old) =>
      old.phase != phase || old.color != color;
}

/// Circular "re-sync" button docked on the [_OffCurveOrbitRing] at the card's
/// top-right corner. Styled to sit on the ring (same color, matching 2px
/// stroke) and pops in with a slight overshoot when the room drifts off-curve.
/// Tapping snaps the room back to the curve, which closes the ring.
class _OffCurveResetButton extends StatelessWidget {
  final bool active;
  final Color color;
  final VoidCallback onReset;
  final Key? opacityKey;
  final Key? controlKey;

  const _OffCurveResetButton({
    required this.active,
    required this.color,
    required this.onReset,
    this.opacityKey,
    this.controlKey,
  });

  @override
  Widget build(BuildContext context) {
    return IgnorePointer(
      ignoring: !active,
      child: AnimatedScale(
        scale: active ? 1.0 : 0.6,
        duration: Duration(milliseconds: active ? 280 : 180),
        curve: active ? Curves.easeOutBack : Curves.easeIn,
        child: AnimatedOpacity(
          key: opacityKey,
          opacity: active ? 1.0 : 0.0,
          duration: Duration(milliseconds: active ? 240 : 160),
          curve: Curves.easeInOut,
          child: Semantics(
            button: true,
            label: 'Reset to curve',
            child: GestureDetector(
              key: controlKey,
              onTap: () {
                HapticFeedback.selectionClick();
                onReset();
              },
              behavior: HitTestBehavior.opaque,
              // Transparent padding keeps a generous tap target around the
              // compact reset pill tucked into the corner.
              child: Padding(
                padding: const EdgeInsets.all(6),
                child: Container(
                  height: 30,
                  padding: const EdgeInsets.symmetric(horizontal: 10),
                  decoration: BoxDecoration(
                    borderRadius: BorderRadius.circular(15),
                    color: Color.lerp(const Color(0xFF141210), color, 0.18)!
                        .withValues(alpha: 0.92),
                    border: Border.all(
                      color: color.withValues(alpha: 0.9),
                      width: 2,
                    ),
                    boxShadow: [
                      BoxShadow(
                        color: Colors.black.withValues(alpha: 0.35),
                        blurRadius: 6,
                        offset: const Offset(0, 2),
                      ),
                    ],
                  ),
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Text(
                        'Reset',
                        style: TextStyle(
                          color: Colors.white.withValues(alpha: 0.92),
                          fontSize: 11,
                          fontWeight: FontWeight.w700,
                          letterSpacing: 0.2,
                        ),
                      ),
                      const SizedBox(width: 4),
                      Icon(
                        Icons.sync_rounded,
                        size: 16,
                        color: Colors.white.withValues(alpha: 0.92),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}
