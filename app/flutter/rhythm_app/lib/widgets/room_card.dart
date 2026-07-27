import 'dart:async';
import 'dart:math' as math;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show
        RhythmDispatchFailure,
        RhythmSceneDefinition,
        RhythmSceneSourceKind,
        RoomModeState;
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

enum _RoomDetailControl { brightness, color }

const double _roomHeaderActionHitSize = 28;

/// Hue-style room card with a CCT-tinted background, four compact control
/// jewels, rhythm controls, and on-demand brightness/CCT sliders.
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
  _RoomDetailControl? _expandedControl;
  RoomMode? _lastRenderedMode;
  bool _lastMoodSelectionActive = false;
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
    _closeExpandedControl();

    // The very first time someone reaches for Mood, introduce the concept
    // before acting on it — then drop them straight into the picker.
    if (newMode == RoomMode.mood &&
        !FirstRunExplainer.hasSeen(_moodExplainerId)) {
      _introduceMood();
      return;
    }
    _applyModeChange(newMode);
  }

  /// The Scenes jewel opens the picker without changing the room. An inactive
  /// room starts on Color at its current CCT; Mood becomes active only after
  /// the user chooses a color or scene.
  void _onScenesJewelPressed() {
    final roomProvider = context.read<RoomProvider>();
    final serverSync = context.read<ServerSyncProvider>();
    final alreadyInMood =
        roomProvider.getDisplayRoomState(widget.roomId) == RoomModeState.mood;
    if (!alreadyInMood) {
      _closeExpandedControl();
    } else if (serverSync.moodSceneIdForRoom(widget.roomId) != null ||
        roomProvider.getMoodColor(widget.roomId) != null) {
      setState(() => _expandedControl = _RoomDetailControl.brightness);
    }

    if (!FirstRunExplainer.hasSeen(_moodExplainerId)) {
      _introduceMood();
      return;
    }

    _showMoodScenePicker(previewCurrentCt: !alreadyInMood);
  }

  /// Show the one-time Mood explainer, then open the non-mutating picker so the
  /// user can choose whether to activate a color or scene.
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
    final alreadyInMood =
        context.read<RoomProvider>().getDisplayRoomState(widget.roomId) ==
            RoomModeState.mood;
    _showMoodScenePicker(previewCurrentCt: !alreadyInMood);
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
      _expandedControl = null;
    });
    AnalyticsService().logRoomModeChanged(
      roomId: widget.roomId,
      previousMode: previousMode,
      nextMode: effectiveMode.name,
    );
    return push;
  }

  void _toggleExpandedControl(
    _RoomDetailControl control, {
    required RoomMode mode,
  }) {
    HapticFeedback.selectionClick();
    final expanded = _expandedControl != control;
    setState(() {
      _expandedControl = expanded ? control : null;
    });
    AnalyticsService().logRoomCardDetailToggled(
      control: control.name,
      roomMode: mode.name,
      expanded: expanded,
    );
  }

  void _closeExpandedControl() {
    if (_expandedControl == null) return;
    setState(() => _expandedControl = null);
  }

  void _showMoodScenePicker({bool previewCurrentCt = false}) {
    final sync = context.read<ServerSyncProvider>();
    final roomProvider = context.read<RoomProvider>();

    final currentKelvin = roomProvider.getKelvin(widget.roomId) ?? 3000;
    final currentCtColor = ColorUtils.cctToColor(currentKelvin);
    final existingColor = roomProvider.getMoodColor(widget.roomId) ??
        roomProvider.getRoomColor(widget.roomId);
    final initialColor = previewCurrentCt
        ? currentCtColor
        : existingColor != null
            ? Color.fromARGB(
                255, existingColor.$1, existingColor.$2, existingColor.$3)
            : currentCtColor;
    final activeSceneId =
        previewCurrentCt ? null : sync.moodSceneIdForRoom(widget.roomId);
    final activeScene =
        activeSceneId == null ? null : sync.sceneById(activeSceneId);
    final roomSource =
        roomProvider.getNode(widget.roomId)?.source ?? RoomSourceDto.unknown;
    final showHueTab = sync.roomHasHueBinding(widget.roomId);
    final initialTab = previewCurrentCt || activeSceneId == null
        ? MoodTab.color
        : showHueTab && activeScene?.isImportedHueScene == true
            ? MoodTab.hue
            : MoodTab.scenes;
    final initialBrightness = previewCurrentCt
        ? roomProvider.getBrightness(widget.roomId) ?? 50
        : roomProvider.getMoodBrightness(widget.roomId) ??
            roomProvider.getBrightness(widget.roomId) ??
            50;

    AnalyticsService().logMoodPickerOpened(
      roomSource: roomSource.name,
      hasHueTab: showHueTab,
    );

    MoodSheet.show(
      context,
      // Open straight to whichever kind of mood the room is currently using.
      initialTab: initialTab,
      initialColor: initialColor,
      initialSceneId: activeSceneId,
      initialBrightness: initialBrightness,
      initialScenes: sync.scenesForRoom(widget.roomId),
      scenesLoader: () => sync.fetchScenes(roomId: widget.roomId),
      showHueTab: showHueTab,
      onTabChanged: (tab) {
        AnalyticsService().logMoodPickerTabChanged(
          roomSource: roomSource.name,
          tab: tab.name,
        );
      },
      onBrightnessChanged: (brightness) {
        if (roomProvider.getDisplayRoomState(widget.roomId) !=
            RoomModeState.mood) {
          return;
        }
        setState(() => _sliderBrightness = brightness);
        _onSceneBrightnessSliderEnd();
      },
      onColorChanged: (color, brightness) {
        final r = (color.r * 255).round();
        final g = (color.g * 255).round();
        final b = (color.b * 255).round();
        final dispatched = sync.dispatchNodeColor(
          widget.roomId,
          r,
          g,
          b,
          scope: 'mood',
          brightness: brightness,
        );
        if (dispatched) {
          _activateMoodPresentation(brightness);
        }
      },
      onSceneSelected: (scene, brightness) async {
        final applied = await sync.applyMoodScene(
          widget.roomId,
          scene.id,
          color: rhythmSceneRgb(scene),
        );
        final sceneCategory = scene.isHuePaletteScene
            ? 'hue_palette'
            : scene.isImportedHueScene
                ? 'hue_other'
                : scene.source.kind == RhythmSceneSourceKind.imported
                    ? 'other_imported'
                    : 'rhythm';
        AnalyticsService().logMoodSceneSelected(
          roomSource: roomSource.name,
          sceneCategory: sceneCategory,
          success: applied,
        );
        if (applied && mounted) {
          _activateMoodPresentation(brightness);
          _onSceneBrightnessSliderEnd();
        }
        return applied;
      },
    );
  }

  void _activateMoodPresentation(int brightness) {
    if (!mounted) return;
    setState(() {
      _sliderBrightness = brightness;
      _expandedControl = _RoomDetailControl.brightness;
    });
    final roomProvider = context.read<RoomProvider>();
    roomProvider.setRoomLightsOnLocal(widget.roomId, true);
    roomProvider.setRoomRhythmEnabled(widget.roomId, true);
    roomProvider.setMoodEnabledLocal(widget.roomId, true);
    roomProvider.setMoodBrightnessLocal(widget.roomId, brightness);
    roomProvider.setRoomStateLocal(widget.roomId, RoomModeState.mood);
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
    _lastRenderedMode = RoomMode.on;
    roomProvider.setRoomStateLocal(widget.roomId, RoomModeState.active);
    AnalyticsService().logRoomModeChanged(
      roomId: widget.roomId,
      previousMode: RoomMode.standby.name,
      nextMode: RoomMode.on.name,
    );
  }

  void _onSceneBrightnessSliderEnd() {
    if (_sliderBrightness == null) return;
    context
        .read<RoomProvider>()
        .setMoodBrightnessLocal(widget.roomId, _sliderBrightness!);
    context
        .read<ServerSyncProvider>()
        .dispatchNodeBrightness(widget.roomId, _sliderBrightness!);
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
      _expandedControl = null;
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
      _expandedControl = null;
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

  Widget _buildBrightnessSlider({
    required RoomMode mode,
    required int displayBrightness,
    required Color cctColor,
    required Color activeTrackColor,
    required Color inactiveTrackColor,
    required Color thumbColor,
    required Color overlayColor,
    required bool enabled,
  }) {
    final isMoodBrightness = mode == RoomMode.mood;
    return Semantics(
      container: true,
      label:
          isMoodBrightness ? 'Scene brightness control' : 'Brightness control',
      child: GestureDetector(
        key: ValueKey(
          'room-card-expanded-brightness-${widget.roomId}',
        ),
        behavior: HitTestBehavior.opaque,
        onTap: () {},
        onLongPress: () {},
        child: SliderTheme(
          data: SliderThemeData(
            trackHeight: 24,
            thumbShape: const _SunSliderThumbShape(
              icon: Icons.wb_sunny_rounded,
            ),
            overlayShape: const RoundSliderOverlayShape(
              overlayRadius: 36,
            ),
            padding: const EdgeInsets.symmetric(
              horizontal: _SunSliderThumbShape.radius - 8,
              vertical: 10,
            ),
            trackShape: const RoundedRectSliderTrackShape(),
            activeTrackColor: activeTrackColor,
            inactiveTrackColor: inactiveTrackColor,
            thumbColor: thumbColor,
            overlayColor: overlayColor,
            disabledActiveTrackColor:
                CelestialColors.orbitRing.withValues(alpha: 0.35),
            disabledInactiveTrackColor:
                CelestialColors.orbitRing.withValues(alpha: 0.18),
            disabledThumbColor: isMoodBrightness
                ? Color.lerp(Colors.white, cctColor, 0.25)
                : CelestialColors.textSecondary.withValues(alpha: 0.55),
          ),
          child: Slider(
            key: ValueKey(
              isMoodBrightness
                  ? 'room-card-scene-brightness-slider-${widget.roomId}'
                  : 'room-card-brightness-slider-${widget.roomId}',
            ),
            value: displayBrightness.toDouble().clamp(1, 100),
            min: 1,
            max: 100,
            onChanged: enabled
                ? (value) {
                    if (!isMoodBrightness) {
                      _activateLowGlowRoomForSlider();
                    }
                    setState(() {
                      _sliderBrightness = value.round();
                    });
                  }
                : null,
            onChangeEnd: enabled
                ? (_) {
                    if (isMoodBrightness) {
                      _onSceneBrightnessSliderEnd();
                    } else {
                      _onBrightnessSliderEnd();
                    }
                  }
                : null,
          ),
        ),
      ),
    );
  }

  Widget _buildColorTemperatureSlider({
    required int kelvin,
    required _CctSideRange range,
    required Color cctColor,
    required Color overlayColor,
    required bool enabled,
  }) {
    return Semantics(
      container: true,
      label: 'Color temperature control',
      child: GestureDetector(
        key: ValueKey(
          'room-card-expanded-color-${widget.roomId}',
        ),
        behavior: HitTestBehavior.opaque,
        onTap: () {},
        onLongPress: () {},
        child: SliderTheme(
          data: SliderThemeData(
            trackHeight: 24,
            thumbShape: const _SunSliderThumbShape(
              icon: Icons.contrast_rounded,
            ),
            overlayShape: const RoundSliderOverlayShape(
              overlayRadius: 36,
            ),
            padding: const EdgeInsets.symmetric(
              horizontal: _SunSliderThumbShape.radius - 8,
              vertical: 10,
            ),
            trackShape: _CCTGradientTrackShape(
              minKelvin: range.minKelvin,
              maxKelvin: range.maxKelvin,
            ),
            thumbColor: Colors.white,
            overlayColor: overlayColor,
            disabledActiveTrackColor: cctColor.withValues(alpha: 0.30),
            disabledInactiveTrackColor: Colors.black.withValues(alpha: 0.15),
            disabledThumbColor:
                CelestialColors.textSecondary.withValues(alpha: 0.55),
          ),
          child: Slider(
            key: ValueKey(
              'room-card-cct-slider-${widget.roomId}',
            ),
            value: range.clampKelvin(_sliderKelvin ?? kelvin).toDouble(),
            min: range.minKelvin.toDouble(),
            max: range.maxKelvin.toDouble(),
            onChanged: enabled
                ? (value) {
                    _activateLowGlowRoomForSlider();
                    setState(() {
                      _sliderKelvin = value.round();
                    });
                  }
                : null,
            onChangeEnd: enabled ? (_) => _onKelvinSliderEnd() : null,
          ),
        ),
      ),
    );
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
          _expandedControl = null;
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

        // Explicit mode actions clear this state before updating the provider.
        // This guard handles authoritative/external mode changes. The one
        // intentional Low glow -> On slider transition updates
        // [_lastRenderedMode] before changing provider state so the active
        // slider stays under the user's finger.
        final enteringMood =
            mode == RoomMode.mood && _lastRenderedMode != RoomMode.mood;
        final hasActiveMoodSelection =
            mode == RoomMode.mood && (moodSceneId != null || moodColor != null);
        final moodSelectionBecameActive =
            hasActiveMoodSelection && !_lastMoodSelectionActive;
        if (_lastRenderedMode != null && _lastRenderedMode != mode) {
          _sliderBrightness = null;
          _sliderKelvin = null;
          _expandedControl = enteringMood && hasActiveMoodSelection
              ? _RoomDetailControl.brightness
              : null;
        } else if (enteringMood && moodSelectionBecameActive) {
          _expandedControl = _RoomDetailControl.brightness;
        }
        _lastRenderedMode = mode;
        _lastMoodSelectionActive = hasActiveMoodSelection;

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
        final lowGlowTint =
            (0.20 + math.sqrt(dimT) * 0.30).clamp(0.20, 0.50).toDouble();
        const darkBase = Color(0xFF141210);
        final lightColorBg = switch (mode) {
          RoomMode.mood => Color.lerp(darkBase, cctColor, 0.22)!,
          RoomMode.standby => Color.lerp(darkBase, cctColor, lowGlowTint)!,
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
          RoomMode.standby => Color.lerp(cctColor, Colors.white, 0.78)!,
          RoomMode.on => onLight
              ? (kelvin < 4000
                  ? const Color(0xFF3A2A1A) // warm dark brown
                  : const Color(0xFF2A2C30)) // cool dark grey
              : Colors.white,
          RoomMode.off => const Color(0xFFE8EBF0),
        };
        final iconColor = switch (mode) {
          RoomMode.mood => const Color(0xFFD8C5A4),
          RoomMode.standby => Color.lerp(cctColor, Colors.white, 0.42)!,
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

        final adaptiveControlsAvailable =
            mode == RoomMode.on || mode == RoomMode.standby;
        final controlInteractionEnabled = hubConnected && !isTransitioning;
        final brightnessControlAvailable = mode != RoomMode.off;
        final colorControlAvailable = adaptiveControlsAvailable;
        if ((_expandedControl == _RoomDetailControl.brightness &&
                !brightnessControlAvailable) ||
            (_expandedControl == _RoomDetailControl.color &&
                !colorControlAvailable)) {
          _expandedControl = null;
        }
        final brightnessSliderActive =
            controlInteractionEnabled && brightnessControlAvailable;
        final cctSliderActive =
            controlInteractionEnabled && colorControlAvailable;
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
                        // Four stable choices stay visible while the two
                        // adjustment details expand only when requested.
                        Padding(
                          padding: const EdgeInsets.fromLTRB(12, 12, 12, 8),
                          child: Column(
                            mainAxisSize: MainAxisSize.min,
                            children: [
                              _RoomControlRow(
                                key: ValueKey(
                                  'room-card-jewel-row-${widget.roomId}',
                                ),
                                scenes: _RoomScenesJewel(
                                  roomId: widget.roomId,
                                  active: mode == RoomMode.mood,
                                  palette: moodPalette,
                                  enabled: controlInteractionEnabled,
                                  onPressed: _onScenesJewelPressed,
                                ),
                                brightness: _RoomDetailJewel(
                                  roomId: widget.roomId,
                                  control: _RoomDetailControl.brightness,
                                  label: 'Brightness',
                                  icon: Icons.wb_sunny_rounded,
                                  selected: _expandedControl ==
                                      _RoomDetailControl.brightness,
                                  enabled: controlInteractionEnabled &&
                                      brightnessControlAvailable,
                                  primaryColor: cctColor,
                                  secondaryColor: Color.lerp(
                                    const Color(0xFF12161C),
                                    cctColor,
                                    0.28,
                                  )!,
                                  semanticsValue: '$displayBrightness percent',
                                  semanticsHint: !brightnessControlAvailable
                                      ? 'Turn the room on or choose Scenes to adjust brightness'
                                      : _expandedControl ==
                                              _RoomDetailControl.brightness
                                          ? 'Hide brightness control'
                                          : 'Show brightness control',
                                  onPressed: () => _toggleExpandedControl(
                                    _RoomDetailControl.brightness,
                                    mode: mode,
                                  ),
                                ),
                                color: _RoomDetailJewel(
                                  roomId: widget.roomId,
                                  control: _RoomDetailControl.color,
                                  label: 'Color',
                                  icon: Icons.contrast_rounded,
                                  selected: _expandedControl ==
                                      _RoomDetailControl.color,
                                  enabled: controlInteractionEnabled &&
                                      colorControlAvailable,
                                  primaryColor: ColorUtils.cctToColor(
                                    cctRange.minKelvin,
                                  ),
                                  secondaryColor: ColorUtils.cctToColor(
                                    cctRange.maxKelvin,
                                  ),
                                  semanticsValue:
                                      '${cctRange.clampKelvin(_sliderKelvin ?? kelvin)} kelvin',
                                  semanticsHint: mode == RoomMode.mood
                                      ? 'Color temperature is unavailable while Scenes is active'
                                      : !colorControlAvailable
                                          ? 'Turn the room on to adjust color temperature'
                                          : _expandedControl ==
                                                  _RoomDetailControl.color
                                              ? 'Hide color temperature control'
                                              : 'Show color temperature control',
                                  onPressed: () => _toggleExpandedControl(
                                    _RoomDetailControl.color,
                                    mode: mode,
                                  ),
                                ),
                                power: _RoomPowerControl(
                                  roomId: widget.roomId,
                                  mode: mode,
                                  standbyEnabled: standbyEnabled,
                                  onModeChanged: _onModeChanged,
                                  onReset: _resetToOn,
                                  cctColor: cctColor,
                                  enabled: controlInteractionEnabled,
                                ),
                              ),
                              AnimatedSize(
                                key: ValueKey(
                                  'room-card-expanded-control-${widget.roomId}',
                                ),
                                duration: const Duration(milliseconds: 220),
                                curve: Curves.easeOutCubic,
                                alignment: Alignment.topCenter,
                                child: _expandedControl == null
                                    ? const SizedBox.shrink()
                                    : Padding(
                                        padding: const EdgeInsets.only(top: 8),
                                        child: switch (_expandedControl!) {
                                          _RoomDetailControl.brightness =>
                                            _buildBrightnessSlider(
                                              mode: mode,
                                              displayBrightness:
                                                  displayBrightness,
                                              cctColor: cctColor,
                                              activeTrackColor:
                                                  sliderActiveTrackColor,
                                              inactiveTrackColor:
                                                  sliderInactiveTrackColor,
                                              thumbColor: sliderThumbColor,
                                              overlayColor: sliderOverlayColor,
                                              enabled: brightnessSliderActive,
                                            ),
                                          _RoomDetailControl.color =>
                                            _buildColorTemperatureSlider(
                                              kelvin: kelvin,
                                              range: cctRange,
                                              cctColor: cctColor,
                                              overlayColor: sliderOverlayColor,
                                              enabled: cctSliderActive,
                                            ),
                                        },
                                      ),
                              ),
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
        width: 88,
        child: _RoomPowerSwitch(
          key: ValueKey('room-card-control-pill-$roomId-off'),
          roomId: roomId,
          state: powerState,
          lowGlowEnabled:
              standbyEnabled || powerState == _PowerToggleState.standby,
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
    final palette = widget.palette.isEmpty ? fallbackPalette : widget.palette;
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
          onTapDown:
              widget.enabled ? (_) => setState(() => _pressed = true) : null,
          onTapUp:
              widget.enabled ? (_) => setState(() => _pressed = false) : null,
          onTapCancel:
              widget.enabled ? () => setState(() => _pressed = false) : null,
          child: SizedBox(
            width: 56,
            height: 68,
            child: Column(
              children: [
                AnimatedScale(
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
                            primary,
                            const Color(0xFF2A2133),
                            widget.active ? 0.12 : 0.26,
                          )!,
                          Color.lerp(
                            secondary,
                            const Color(0xFF151A24),
                            widget.active ? 0.12 : 0.26,
                          )!,
                        ],
                      ),
                      border: Border.all(
                        color: Colors.white.withValues(
                          alpha: widget.active ? 0.62 : 0.34,
                        ),
                        width: widget.active ? 1.5 : 1,
                      ),
                      boxShadow: [
                        BoxShadow(
                          color: primary.withValues(
                            alpha: widget.active ? 0.42 : 0.24,
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
                        const Positioned.fill(
                          child: _RoomJewelDepthOverlay(),
                        ),
                        Icon(
                          Icons.auto_awesome_rounded,
                          size: 19,
                          color: Colors.white.withValues(alpha: 0.96),
                          shadows: [
                            Shadow(
                              color: Colors.black.withValues(alpha: 0.48),
                              blurRadius: 6,
                              offset: const Offset(0, 1),
                            ),
                          ],
                        ),
                      ],
                    ),
                  ),
                ),
                const SizedBox(height: 2),
                _RoomJewelLabel(
                  key: ValueKey(
                    'room-card-control-label-${widget.roomId}-scenes',
                  ),
                  label: 'Scenes',
                  enabled: widget.enabled,
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _RoomJewelLabel extends StatelessWidget {
  const _RoomJewelLabel({
    super.key,
    required this.label,
    required this.enabled,
  });

  final String label;
  final bool enabled;

  @override
  Widget build(BuildContext context) {
    return Text(
      label,
      maxLines: 1,
      style: TextStyle(
        color: Colors.white.withValues(alpha: enabled ? 0.92 : 0.64),
        fontSize: label.length > 6 ? 9 : 10,
        height: 1,
        fontWeight: FontWeight.w700,
        letterSpacing: -0.1,
      ),
    );
  }
}

class _RoomJewelDepthOverlay extends StatelessWidget {
  const _RoomJewelDepthOverlay();

  @override
  Widget build(BuildContext context) {
    return DecoratedBox(
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        gradient: LinearGradient(
          begin: Alignment.topCenter,
          end: Alignment.bottomCenter,
          stops: const [0, 0.42, 1],
          colors: [
            Colors.white.withValues(alpha: 0.34),
            Colors.transparent,
            Colors.black.withValues(alpha: 0.20),
          ],
        ),
        border: Border.all(
          color: Colors.white.withValues(alpha: 0.14),
          width: 2,
        ),
      ),
    );
  }
}

/// Keeps four control centers evenly spaced while allowing Power to carry a
/// larger visual and hit surface than the circular jewels.
class _RoomControlRow extends StatelessWidget {
  const _RoomControlRow({
    super.key,
    required this.scenes,
    required this.brightness,
    required this.color,
    required this.power,
  });

  static const double _jewelSize = 56;
  static const double _jewelHeight = 68;
  static const double _powerWidth = 88;
  static const double _powerHeight = 64;
  static const double _rowHeight = 68;

  final Widget scenes;
  final Widget brightness;
  final Widget color;
  final Widget power;

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        final firstCenter = _jewelSize / 2;
        final lastCenter = constraints.maxWidth - (_powerWidth / 2);
        final centerStep = (lastCenter - firstCenter) / 3;

        Widget place({
          required Widget child,
          required double center,
          required double width,
          required double height,
        }) {
          return Positioned(
            left: center - (width / 2),
            top: 0,
            width: width,
            height: height,
            child: child,
          );
        }

        return SizedBox(
          height: _rowHeight,
          child: Stack(
            children: [
              place(
                child: scenes,
                center: firstCenter,
                width: _jewelSize,
                height: _jewelHeight,
              ),
              place(
                child: brightness,
                center: firstCenter + centerStep,
                width: _jewelSize,
                height: _jewelHeight,
              ),
              place(
                child: color,
                center: firstCenter + (centerStep * 2),
                width: _jewelSize,
                height: _jewelHeight,
              ),
              place(
                child: power,
                center: lastCenter,
                width: _powerWidth,
                height: _powerHeight,
              ),
            ],
          ),
        );
      },
    );
  }
}

class _RoomDetailJewel extends StatefulWidget {
  const _RoomDetailJewel({
    required this.roomId,
    required this.control,
    required this.label,
    required this.icon,
    required this.selected,
    required this.enabled,
    required this.primaryColor,
    required this.secondaryColor,
    required this.semanticsValue,
    required this.semanticsHint,
    required this.onPressed,
  });

  final String roomId;
  final _RoomDetailControl control;
  final String label;
  final IconData icon;
  final bool selected;
  final bool enabled;
  final Color primaryColor;
  final Color secondaryColor;
  final String semanticsValue;
  final String semanticsHint;
  final VoidCallback onPressed;

  @override
  State<_RoomDetailJewel> createState() => _RoomDetailJewelState();
}

class _RoomDetailJewelState extends State<_RoomDetailJewel> {
  bool _pressed = false;

  @override
  Widget build(BuildContext context) {
    final jewelGradient = switch (widget.control) {
      _RoomDetailControl.brightness => RadialGradient(
          center: const Alignment(-0.28, -0.36),
          radius: 0.95,
          stops: const [0, 0.48, 1],
          colors: [
            Colors.white,
            Color.lerp(
              widget.primaryColor,
              Colors.white,
              widget.selected ? 0.62 : 0.50,
            )!,
            Color.lerp(
              widget.primaryColor,
              const Color(0xFFFFB63F),
              widget.selected ? 0.40 : 0.28,
            )!,
          ],
        ),
      _RoomDetailControl.color => LinearGradient(
          begin: Alignment.centerLeft,
          end: Alignment.centerRight,
          stops: const [0, 0.46, 0.54, 1],
          colors: [
            widget.primaryColor,
            Color.lerp(widget.primaryColor, Colors.white, 0.34)!,
            Color.lerp(widget.secondaryColor, Colors.white, 0.24)!,
            widget.secondaryColor,
          ],
        ),
    };
    final iconColor = switch (widget.control) {
      _RoomDetailControl.brightness => const Color(0xFF5A3908),
      _RoomDetailControl.color => Colors.white,
    };

    return AnimatedOpacity(
      duration: const Duration(milliseconds: 160),
      opacity: widget.enabled ? 1.0 : 0.42,
      child: Semantics(
        excludeSemantics: true,
        button: true,
        enabled: widget.enabled,
        selected: widget.selected,
        label: widget.label,
        value: widget.semanticsValue,
        hint: widget.semanticsHint,
        onTap: widget.enabled ? widget.onPressed : null,
        child: Tooltip(
          message: '${widget.label} · ${widget.semanticsValue}',
          child: GestureDetector(
            key: ValueKey(
              'room-card-segment-${widget.roomId}-${widget.control.name}',
            ),
            behavior: HitTestBehavior.opaque,
            onTap: widget.enabled ? widget.onPressed : null,
            onTapDown:
                widget.enabled ? (_) => setState(() => _pressed = true) : null,
            onTapUp:
                widget.enabled ? (_) => setState(() => _pressed = false) : null,
            onTapCancel:
                widget.enabled ? () => setState(() => _pressed = false) : null,
            child: SizedBox(
              width: 56,
              height: 68,
              child: Column(
                children: [
                  AnimatedScale(
                    scale: _pressed ? 0.90 : 1.0,
                    duration: const Duration(milliseconds: 110),
                    curve: Curves.easeOut,
                    child: AnimatedContainer(
                      key: ValueKey(
                        'room-card-control-pill-${widget.roomId}-${widget.control.name}',
                      ),
                      duration: const Duration(milliseconds: 220),
                      curve: Curves.easeOutCubic,
                      width: 56,
                      height: 56,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        gradient: jewelGradient,
                        border: Border.all(
                          color: Colors.white.withValues(
                            alpha: widget.selected ? 0.88 : 0.54,
                          ),
                          width: widget.selected ? 1.5 : 1,
                        ),
                        boxShadow: [
                          BoxShadow(
                            color: widget.primaryColor.withValues(
                              alpha: widget.selected ? 0.30 : 0.12,
                            ),
                            blurRadius: widget.selected ? 16 : 9,
                            spreadRadius: widget.selected ? 1 : 0,
                          ),
                          BoxShadow(
                            color: Colors.black.withValues(alpha: 0.28),
                            blurRadius: 5,
                            offset: const Offset(0, 2),
                          ),
                        ],
                      ),
                      child: Stack(
                        alignment: Alignment.center,
                        children: [
                          const Positioned.fill(
                            child: _RoomJewelDepthOverlay(),
                          ),
                          if (widget.control == _RoomDetailControl.color)
                            Container(
                              width: 29,
                              height: 29,
                              decoration: BoxDecoration(
                                shape: BoxShape.circle,
                                color: const Color(0xFF202733).withValues(
                                  alpha: 0.30,
                                ),
                                border: Border.all(
                                  color: Colors.white.withValues(alpha: 0.22),
                                ),
                              ),
                            ),
                          Icon(
                            widget.icon,
                            size:
                                widget.control == _RoomDetailControl.brightness
                                    ? 22
                                    : 19,
                            color: iconColor.withValues(
                              alpha: widget.enabled ? 0.92 : 0.62,
                            ),
                            shadows: [
                              Shadow(
                                color: Colors.black.withValues(
                                  alpha: widget.control ==
                                          _RoomDetailControl.brightness
                                      ? 0.18
                                      : 0.48,
                                ),
                                blurRadius: 5,
                                offset: const Offset(0, 1),
                              ),
                            ],
                          ),
                        ],
                      ),
                    ),
                  ),
                  const SizedBox(height: 2),
                  _RoomJewelLabel(
                    key: ValueKey(
                      'room-card-control-label-${widget.roomId}-${widget.control.name}',
                    ),
                    label: widget.label,
                    enabled: widget.enabled,
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

enum _PowerToggleState { off, standby, on }

/// Tap-cycle room power switch with exact drag selection. Low-glow-enabled
/// rooms expose Off, Low glow, and On; other rooms expose binary Off/On.
class _RoomPowerSwitch extends StatefulWidget {
  final String roomId;
  final _PowerToggleState state;
  final bool lowGlowEnabled;
  final ValueChanged<_PowerToggleState> onChanged;
  final bool enabled;
  final Color cctColor;

  const _RoomPowerSwitch({
    super.key,
    required this.roomId,
    required this.state,
    required this.lowGlowEnabled,
    required this.onChanged,
    required this.enabled,
    required this.cctColor,
  });

  @override
  State<_RoomPowerSwitch> createState() => _RoomPowerSwitchState();
}

class _RoomPowerSwitchState extends State<_RoomPowerSwitch> {
  static const double _trackWidth = 86;
  Offset? _dragStartPosition;
  Offset? _dragPosition;

  String _labelFor(_PowerToggleState state) => switch (state) {
        _PowerToggleState.off => 'Off',
        _PowerToggleState.standby => 'Low glow',
        _PowerToggleState.on => 'On',
      };

  Alignment get _thumbAlignment => switch (widget.state) {
        _PowerToggleState.off => Alignment.centerLeft,
        _PowerToggleState.standby => Alignment.center,
        _PowerToggleState.on => Alignment.centerRight,
      };

  List<_PowerToggleState> get _positions => widget.lowGlowEnabled
      ? const [
          _PowerToggleState.off,
          _PowerToggleState.standby,
          _PowerToggleState.on,
        ]
      : const [_PowerToggleState.off, _PowerToggleState.on];

  void _select(_PowerToggleState state) {
    if (!widget.enabled || state == widget.state) return;
    widget.onChanged(state);
  }

  void _selectAt(double x) {
    final clampedX = x.clamp(0.0, _trackWidth);
    final positions = _positions;
    final index = ((clampedX / _trackWidth) * positions.length).floor().clamp(
          0,
          positions.length - 1,
        );
    _select(positions[index]);
  }

  void _increase() {
    final positions = _positions;
    final current = positions.indexOf(widget.state);
    if (current >= 0 && current < positions.length - 1) {
      _select(positions[current + 1]);
    }
  }

  void _decrease() {
    final positions = _positions;
    final current = positions.indexOf(widget.state);
    if (current > 0) {
      _select(positions[current - 1]);
    }
  }

  void _cycle() {
    final positions = _positions;
    final current = positions.indexOf(widget.state);
    _select(positions[(current + 1) % positions.length]);
  }

  @override
  Widget build(BuildContext context) {
    final activeColor = Color.lerp(widget.cctColor, Colors.white, 0.30)!;
    const inactiveColor = Color(0xFF727B87);
    final lowGlowColor = Color.lerp(widget.cctColor, Colors.white, 0.16)!;
    final stateColor = switch (widget.state) {
      _PowerToggleState.off => inactiveColor,
      _PowerToggleState.standby => lowGlowColor,
      _PowerToggleState.on => activeColor,
    };
    final stateLabel = _labelFor(widget.state);
    final positions = _positions;
    final currentIndex = positions.indexOf(widget.state);
    final increasedValue =
        currentIndex >= 0 && currentIndex < positions.length - 1
            ? _labelFor(positions[currentIndex + 1])
            : null;
    final decreasedValue =
        currentIndex > 0 ? _labelFor(positions[currentIndex - 1]) : null;
    final trackTint = switch (widget.state) {
      _PowerToggleState.off => const Color(0xFF242A33),
      _PowerToggleState.standby =>
        Color.lerp(const Color(0xFF242A33), widget.cctColor, 0.38)!,
      _PowerToggleState.on =>
        Color.lerp(const Color(0xFF242A33), widget.cctColor, 0.68)!,
    };
    final thumbColor = switch (widget.state) {
      _PowerToggleState.off => const Color(0xFF8B949F),
      _PowerToggleState.standby =>
        Color.lerp(widget.cctColor, Colors.white, 0.42)!,
      _PowerToggleState.on => Colors.white,
    };

    return Semantics(
      container: true,
      excludeSemantics: true,
      enabled: widget.enabled,
      toggled:
          widget.lowGlowEnabled ? null : widget.state == _PowerToggleState.on,
      label: 'Room power',
      value: stateLabel,
      hint: widget.lowGlowEnabled
          ? 'Tap to cycle; swipe to choose Off, Low glow, or On'
          : widget.state == _PowerToggleState.on
              ? 'Turn completely off'
              : 'Turn on and reset to the curve',
      increasedValue: increasedValue,
      decreasedValue: decreasedValue,
      onIncrease: widget.enabled && increasedValue != null ? _increase : null,
      onDecrease: widget.enabled && decreasedValue != null ? _decrease : null,
      onTap: widget.enabled ? _cycle : null,
      child: Tooltip(
        message: widget.lowGlowEnabled
            ? 'Power · $stateLabel · Tap to cycle'
            : 'Power · $stateLabel',
        child: SizedBox(
          key: ValueKey('room-card-power-toggle-${widget.roomId}'),
          width: 88,
          height: 64,
          child: Column(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              GestureDetector(
                key: ValueKey(
                  'room-card-power-switch-${widget.roomId}',
                ),
                behavior: HitTestBehavior.opaque,
                // Let Flutter's gesture arena distinguish taps from a parent
                // scroll before changing power. A raw pointer-up handler would
                // also fire after a vertical swipe that began on the switch.
                onTap: widget.enabled ? _cycle : null,
                onHorizontalDragStart: widget.enabled
                    ? (details) {
                        _dragStartPosition = details.localPosition;
                        _dragPosition = details.localPosition;
                      }
                    : null,
                onHorizontalDragUpdate: widget.enabled
                    ? (details) => _dragPosition = details.localPosition
                    : null,
                onHorizontalDragEnd: widget.enabled
                    ? (_) {
                        final start = _dragStartPosition;
                        final end = _dragPosition;
                        _dragStartPosition = null;
                        _dragPosition = null;
                        if (start != null &&
                            end != null &&
                            (end.dx - start.dx).abs() >=
                                (end.dy - start.dy).abs()) {
                          _selectAt(end.dx);
                        }
                      }
                    : null,
                onHorizontalDragCancel: widget.enabled
                    ? () {
                        _dragStartPosition = null;
                        _dragPosition = null;
                      }
                    : null,
                child: AnimatedContainer(
                  key: ValueKey(
                    'room-card-power-track-${widget.roomId}',
                  ),
                  duration: const Duration(milliseconds: 220),
                  curve: Curves.easeOutCubic,
                  width: _trackWidth,
                  height: 40,
                  padding: const EdgeInsets.all(5),
                  decoration: BoxDecoration(
                    color: trackTint,
                    borderRadius: BorderRadius.circular(20),
                    border: Border.all(
                      color: widget.state == _PowerToggleState.off
                          ? const Color(0xFF56606C)
                          : widget.cctColor.withValues(
                              alpha: widget.state == _PowerToggleState.standby
                                  ? 0.56
                                  : 0.78,
                            ),
                      width: 1.4,
                    ),
                    boxShadow: [
                      BoxShadow(
                        color: Colors.black.withValues(alpha: 0.30),
                        blurRadius: 8,
                        offset: const Offset(0, 3),
                      ),
                      if (widget.state != _PowerToggleState.off)
                        BoxShadow(
                          color: widget.cctColor.withValues(
                            alpha: widget.state == _PowerToggleState.standby
                                ? 0.20
                                : 0.32,
                          ),
                          blurRadius: 18,
                          spreadRadius: 2,
                        ),
                    ],
                  ),
                  child: Stack(
                    children: [
                      Align(
                        alignment: Alignment.centerLeft,
                        child: _PowerPositionTick(
                          key: ValueKey(
                            'room-card-power-tick-${widget.roomId}-off',
                          ),
                          active: widget.state == _PowerToggleState.off,
                        ),
                      ),
                      if (widget.lowGlowEnabled)
                        Align(
                          child: _PowerPositionTick(
                            key: ValueKey(
                              'room-card-power-tick-${widget.roomId}-standby',
                            ),
                            active: widget.state == _PowerToggleState.standby,
                          ),
                        ),
                      Align(
                        alignment: Alignment.centerRight,
                        child: _PowerPositionTick(
                          key: ValueKey(
                            'room-card-power-tick-${widget.roomId}-on',
                          ),
                          active: widget.state == _PowerToggleState.on,
                        ),
                      ),
                      AnimatedAlign(
                        key: ValueKey(
                          'room-card-power-thumb-${widget.roomId}',
                        ),
                        duration: const Duration(milliseconds: 220),
                        curve: Curves.easeOutCubic,
                        alignment: _thumbAlignment,
                        child: AnimatedContainer(
                          duration: const Duration(milliseconds: 220),
                          width: 30,
                          height: 30,
                          decoration: BoxDecoration(
                            shape: BoxShape.circle,
                            color: thumbColor,
                            border: Border.all(
                              color: Colors.white.withValues(alpha: 0.50),
                              width: 1,
                            ),
                            boxShadow: [
                              BoxShadow(
                                color: Colors.black.withValues(alpha: 0.40),
                                blurRadius: 7,
                                offset: const Offset(0, 3),
                              ),
                            ],
                          ),
                          child: Icon(
                            switch (widget.state) {
                              _PowerToggleState.off =>
                                Icons.power_settings_new_rounded,
                              _PowerToggleState.standby =>
                                Icons.brightness_low_rounded,
                              _PowerToggleState.on => Icons.check_rounded,
                            },
                            key: ValueKey(
                              'room-card-power-toggle-icon-${widget.roomId}',
                            ),
                            size: 17,
                            color: Color.lerp(
                              const Color(0xFF171C23),
                              widget.cctColor,
                              widget.state == _PowerToggleState.standby
                                  ? 0.28
                                  : 0.10,
                            ),
                          ),
                        ),
                      ),
                    ],
                  ),
                ),
              ),
              const SizedBox(height: 3),
              Text(
                stateLabel,
                key: ValueKey(
                  'room-card-power-toggle-label-${widget.roomId}',
                ),
                maxLines: 1,
                style: TextStyle(
                  color: stateColor.withValues(alpha: 0.94),
                  fontSize: stateLabel.length > 3 ? 9.5 : 10.5,
                  height: 1,
                  fontWeight: FontWeight.w800,
                  letterSpacing: stateLabel.length > 3 ? -0.1 : 0.15,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _PowerPositionTick extends StatelessWidget {
  const _PowerPositionTick({
    super.key,
    required this.active,
  });

  final bool active;

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: 30,
      height: 30,
      child: Center(
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 160),
          width: 5,
          height: 5,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: Colors.white.withValues(alpha: active ? 0.08 : 0.48),
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
