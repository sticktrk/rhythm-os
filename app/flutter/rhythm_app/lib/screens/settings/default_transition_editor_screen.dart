import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart' hide Home, Hub, HubType;
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../api/hybrid_client.dart' show sdkCurveConfigToDto;
import '../../providers/home_provider.dart';
import '../../providers/server_sync_provider.dart';
import '../../widgets/solar_clock/solar_clock_exports.dart';
import '../../widgets/solar_orbit.dart';

class DefaultTransitionEditorScreen extends StatefulWidget {
  final Map<RhythmMode, Color> profileColors;

  const DefaultTransitionEditorScreen({
    super.key,
    required this.profileColors,
  });

  static Future<void> show(
    BuildContext context, {
    required Map<RhythmMode, Color> profileColors,
  }) {
    return Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => DefaultTransitionEditorScreen(
          profileColors: profileColors,
        ),
      ),
    );
  }

  @override
  State<DefaultTransitionEditorScreen> createState() =>
      _DefaultTransitionEditorScreenState();
}

class _DefaultTransitionEditorScreenState
    extends State<DefaultTransitionEditorScreen> with TickerProviderStateMixin {
  static const Duration _updateDebounceDuration = Duration(milliseconds: 250);
  static const double _eventSnapThresholdHours = 0.25;
  static const double _minimumHandleGapHours = 0.25;
  static const double _maxEditableHour = 23.99;

  late Map<RhythmMode, RhythmModeTransitionConfig> _transitionConfigs;
  late RhythmMode _selectedMode;
  late AnimationController _breatheController;
  late Animation<double> _breatheAnimation;
  late AnimationController _flowController;
  late Animation<double> _flowAnimation;
  final Map<RhythmMode, Timer> _updateDebounceTimers = {};
  final Map<RhythmMode, RhythmModeTransitionConfig> _pendingUpdates = {};
  final Map<RhythmMode, double> _handleHours = {};
  final Map<RhythmMode, _ModeCurveVisual> _curveVisuals = {};
  SolarClockData? _solarClockData;
  RhythmMode? _dragMode;
  double? _dragPreviewHour;

  SunTimesDto? get _sunTimes => _solarClockData?.sunTimes;
  TwilightTimesDto? get _twilightTimes => _solarClockData?.twilightTimes;
  RhythmModeTransitionConfig get _config => _transitionConfigs[_selectedMode]!;

  @override
  void initState() {
    super.initState();
    _transitionConfigs = _initialTransitionConfigs();
    _selectedMode = RhythmMode.day;
    _breatheController = AnimationController(
      duration: const Duration(milliseconds: 3500),
      vsync: this,
    )..repeat(reverse: true);
    _breatheAnimation = Tween<double>(
      begin: 0.0,
      end: 1.0,
    ).animate(
      CurvedAnimation(parent: _breatheController, curve: Curves.easeInOut),
    );
    _flowController = AnimationController(
      duration: const Duration(milliseconds: 2800),
      vsync: this,
    )..repeat();
    _flowAnimation = CurvedAnimation(
      parent: _flowController,
      curve: Curves.easeInOut,
    );
    _loadSolarTimes();
  }

  @override
  void dispose() {
    final serverSync = context.read<ServerSyncProvider>();
    for (final timer in _updateDebounceTimers.values) {
      timer.cancel();
    }
    for (final pending in _pendingUpdates.values) {
      serverSync.dispatchUpdateTransition(pending);
    }
    _breatheController.dispose();
    _flowController.dispose();
    super.dispose();
  }

  Map<RhythmMode, RhythmModeTransitionConfig> _initialTransitionConfigs() {
    final transitions = context.read<ServerSyncProvider>().modeTransitions;
    final configs = <RhythmMode, RhythmModeTransitionConfig>{};

    for (final transition in transitions) {
      if (_isSupportedTransition(transition)) {
        configs[transition.toMode] = transition;
      }
    }

    configs.putIfAbsent(
      RhythmMode.day,
      () => _defaultTransitionForMode(RhythmMode.day),
    );
    configs.putIfAbsent(
      RhythmMode.sleep,
      () => _defaultTransitionForMode(RhythmMode.sleep),
    );
    return configs;
  }

  bool _isSupportedTransition(RhythmModeTransitionConfig transition) {
    return (transition.fromMode == RhythmMode.sleep &&
            transition.toMode == RhythmMode.day) ||
        (transition.fromMode == RhythmMode.day &&
            transition.toMode == RhythmMode.sleep);
  }

  RhythmModeTransitionConfig _defaultTransitionForMode(RhythmMode mode) {
    final isDay = mode == RhythmMode.day;
    return RhythmModeTransitionConfig(
      fromMode: isDay ? RhythmMode.sleep : RhythmMode.day,
      toMode: mode,
      trigger: RhythmTransitionTrigger.solar(isDay ? 'sunrise' : 'sunset'),
      duration: const TransitionDuration.auto(),
      preserveHardOff: true,
    );
  }

  void _loadSolarTimes() {
    try {
      final home = context.read<HomeProvider>().currentHome;
      final loc = home?.location;
      if (loc == null) return;
      final tz =
          home?.timezone ?? SolarUtils.timezoneFromLongitude(loc.longitude);
      final now = DateTime.now();
      final sunTimes = getSunTimes(
        latitude: loc.latitude,
        longitude: loc.longitude,
        year: now.year,
        month: now.month,
        day: now.day,
        timezone: tz,
      );
      final twilightTimes = getTwilightTimes(
        latitude: loc.latitude,
        longitude: loc.longitude,
        year: now.year,
        month: now.month,
        day: now.day,
        timezone: tz,
      );
      _solarClockData = SolarClockData(
        sunTimes: sunTimes,
        twilightTimes: twilightTimes,
      );
      _curveVisuals
        ..clear()
        ..addAll(
          _buildModeCurveVisuals(
            latitude: loc.latitude,
            longitude: loc.longitude,
            year: now.year,
            month: now.month,
            day: now.day,
            timezone: tz,
          ),
        );
    } catch (_) {
      _solarClockData = null;
      _curveVisuals.clear();
    }
  }

  Map<RhythmMode, _ModeCurveVisual> _buildModeCurveVisuals({
    required double latitude,
    required double longitude,
    required int year,
    required int month,
    required int day,
    required String timezone,
  }) {
    return {
      RhythmMode.day: _buildModeCurveVisual(
        mode: RhythmMode.day,
        latitude: latitude,
        longitude: longitude,
        year: year,
        month: month,
        day: day,
        timezone: timezone,
      ),
      RhythmMode.sleep: _buildModeCurveVisual(
        mode: RhythmMode.sleep,
        latitude: latitude,
        longitude: longitude,
        year: year,
        month: month,
        day: day,
        timezone: timezone,
      ),
    };
  }

  _ModeCurveVisual _buildModeCurveVisual({
    required RhythmMode mode,
    required double latitude,
    required double longitude,
    required int year,
    required int month,
    required int day,
    required String timezone,
  }) {
    final fallbackColor =
        widget.profileColors[mode] ?? _fallbackModeColor(mode);
    final profile = _activeProfileForMode(mode);
    if (profile == null) {
      return _ModeCurveVisual(
        fallbackColor: fallbackColor,
        fallbackBrightness: 50,
      );
    }

    final curve = profile.curve;
    final fallbackBrightness = switch (curve) {
      RhythmConstantCurve() => curve.brightness,
      _ => ((profile.minBrightness + profile.maxBrightness) / 2).round(),
    };

    if (curve is! RhythmSuperGaussianCurve) {
      return _ModeCurveVisual(
        fallbackColor: fallbackColor,
        fallbackBrightness: fallbackBrightness,
      );
    }

    try {
      final curveData = generateCurveDataWithSunTimes(
        config: sdkCurveConfigToDto(profile),
        latitude: latitude,
        longitude: longitude,
        year: year,
        month: month,
        day: day,
        timezone: timezone,
      );
      return _ModeCurveVisual(
        samples: SolarCurveSamples.fromCurveDataDto(curveData),
        fallbackColor: fallbackColor,
        fallbackBrightness: fallbackBrightness,
      );
    } catch (_) {
      return _ModeCurveVisual(
        fallbackColor: fallbackColor,
        fallbackBrightness: fallbackBrightness,
      );
    }
  }

  RhythmCurveConfig? _activeProfileForMode(RhythmMode mode) {
    final serverSync = context.read<ServerSyncProvider>();

    String? activeProfileId;
    for (final modeConfig in serverSync.modeConfigs) {
      if (modeConfig.mode == mode) {
        activeProfileId = modeConfig.activeProfileId;
        break;
      }
    }
    if (activeProfileId == null || activeProfileId.isEmpty) return null;

    for (final profile in serverSync.profiles) {
      if (profile.id == activeProfileId) return profile;
    }
    return null;
  }

  void _updateConfig(RhythmModeTransitionConfig updated) {
    setState(() {
      _transitionConfigs[updated.toMode] = updated;
      _selectedMode = updated.toMode;
    });
    _pendingUpdates[updated.toMode] = updated;
    _updateDebounceTimers[updated.toMode]?.cancel();
    _updateDebounceTimers[updated.toMode] = Timer(
      _updateDebounceDuration,
      () => _flushPendingUpdate(updated.toMode),
    );
  }

  void _flushPendingUpdate(RhythmMode mode) {
    _updateDebounceTimers.remove(mode)?.cancel();
    final pending = _pendingUpdates.remove(mode);
    if (pending == null || !mounted) return;
    context.read<ServerSyncProvider>().dispatchUpdateTransition(pending);
  }

  void _focusMode(RhythmMode mode) {
    if (_selectedMode == mode) return;
    HapticFeedback.selectionClick();
    setState(() => _selectedMode = mode);
  }

  void _setModeHour(
    RhythmMode mode,
    double hour, {
    _TriggerAnchor? snappedAnchor,
  }) {
    final current = _transitionConfigs[mode];
    if (current == null) return;
    final normalizedHour = SolarUtils.normalizeHour(hour);

    if (snappedAnchor != null) {
      final needsSolarUpdate = !current.trigger.isSolar ||
          current.trigger.event != snappedAnchor.event;
      final hadLocalHour = _handleHours.containsKey(mode);

      if (!needsSolarUpdate && !hadLocalHour) {
        if (_selectedMode != mode) {
          setState(() => _selectedMode = mode);
        }
        return;
      }

      setState(() {
        _selectedMode = mode;
        _handleHours.remove(mode);
      });

      if (needsSolarUpdate) {
        _updateConfig(
          current.copyWith(
            trigger: RhythmTransitionTrigger.solar(snappedAnchor.event),
          ),
        );
      }
      return;
    }

    final scheduledTime = _formatScheduledTriggerTime(normalizedHour);
    final scheduledHour = _scheduledTimeToHour(scheduledTime) ?? normalizedHour;
    final previousHour = _handleHourForMode(mode);
    final hourChanged =
        previousHour == null || (previousHour - scheduledHour).abs() > 0.001;
    final needsTriggerUpdate =
        !current.trigger.isScheduled || current.trigger.time != scheduledTime;

    if (!hourChanged && !needsTriggerUpdate) {
      if (_selectedMode != mode) {
        setState(() => _selectedMode = mode);
      }
      return;
    }

    setState(() {
      _selectedMode = mode;
      _handleHours[mode] = scheduledHour;
    });

    if (needsTriggerUpdate) {
      _updateConfig(
        current.copyWith(
            trigger: RhythmTransitionTrigger.scheduled(scheduledTime)),
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _buildHeader(context),
            Expanded(
              child: SingleChildScrollView(
                padding: const EdgeInsets.symmetric(horizontal: 20),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    const SizedBox(height: 8),
                    _buildHero(),
                    const SizedBox(height: 20),
                    _buildDurationRow(),
                    const SizedBox(height: 40),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildHeader(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      child: Row(
        children: [
          GestureDetector(
            onTap: () => Navigator.of(context).pop(),
            child: Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: CelestialColors.accentBlue.withValues(alpha: 0.2),
              ),
              child: const Icon(
                Icons.chevron_left,
                color: CelestialColors.accentBlue,
                size: 24,
              ),
            ),
          ),
          const Expanded(
            child: Text(
              'Rhythm',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 18,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
          const SizedBox(width: 40),
        ],
      ),
    );
  }

  Widget _buildHero() {
    final dayColor = widget.profileColors[RhythmMode.day] ??
        _fallbackModeColor(RhythmMode.day);
    final sleepColor = widget.profileColors[RhythmMode.sleep] ??
        _fallbackModeColor(RhythmMode.sleep);
    final accentColor = _selectedMode == RhythmMode.day ? dayColor : sleepColor;

    return AnimatedBuilder(
      animation: _breatheAnimation,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(20, 28, 20, 24),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Text(
              'Default Transition Editor',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 20,
                fontWeight: FontWeight.w300,
                letterSpacing: 1.0,
              ),
            ),
            const SizedBox(height: 6),
            Text(
              'Drag Day Start and Sleep Start anywhere on the clock',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.62),
                fontSize: 12,
                fontWeight: FontWeight.w500,
                letterSpacing: 0.4,
              ),
            ),
            const SizedBox(height: 20),
            if (_solarClockData != null)
              _buildInteractiveTransitionFlow(
                dayColor: dayColor,
                sleepColor: sleepColor,
              )
            else
              _buildNoSolarState(),
            const SizedBox(height: 14),
            _buildFocusedTransitionSummary(
              dayColor: dayColor,
              sleepColor: sleepColor,
            ),
          ],
        ),
      ),
      builder: (context, child) {
        final breathe = _breatheAnimation.value;
        return Container(
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(20),
            color: CelestialColors.backgroundCard,
          ),
          clipBehavior: Clip.antiAlias,
          child: Stack(
            children: [
              Positioned.fill(
                child: DecoratedBox(
                  decoration: BoxDecoration(
                    gradient: RadialGradient(
                      center: const Alignment(0, -0.6),
                      radius: 1.15,
                      colors: [
                        accentColor.withValues(alpha: 0.12 + breathe * 0.04),
                        CelestialColors.backgroundCard,
                        CelestialColors.backgroundDark,
                      ],
                      stops: const [0.0, 0.55, 1.0],
                    ),
                  ),
                ),
              ),
              Positioned.fill(
                child: DecoratedBox(
                  decoration: BoxDecoration(
                    gradient: LinearGradient(
                      begin: Alignment.centerLeft,
                      end: Alignment.centerRight,
                      colors: [
                        sleepColor.withValues(alpha: 0.10 + breathe * 0.04),
                        Colors.transparent,
                        Colors.transparent,
                        dayColor.withValues(alpha: 0.10 + breathe * 0.04),
                      ],
                      stops: const [0.0, 0.35, 0.65, 1.0],
                    ),
                  ),
                ),
              ),
              child!,
            ],
          ),
        );
      },
    );
  }

  Widget _buildInteractiveTransitionFlow({
    required Color dayColor,
    required Color sleepColor,
  }) {
    final solarClockData = _solarClockData!;

    return SizedBox(
      height: 380,
      child: AnimatedBuilder(
        animation: Listenable.merge([_flowAnimation, _breatheAnimation]),
        builder: (context, _) {
          return SolarClock(
            data: solarClockData,
            use24: MediaQuery.alwaysUse24HourFormatOf(context),
            showUpperArc: false,
            showEventMarkers: true,
            showHourLabels: true,
            showLowerArc: false,
            horizonFactor: 0.58,
            radiusWidthFactor: 0.44,
            radiusHeightFactor: 0.76,
            underlayBuilder: (context, geometry) => _buildClockRing(
              geometry: geometry,
              dayColor: dayColor,
              sleepColor: sleepColor,
            ),
            overlayBuilder: (context, geometry) => _buildClockOverlay(
              geometry: geometry,
              dayColor: dayColor,
              sleepColor: sleepColor,
            ),
          );
        },
      ),
    );
  }

  Widget _buildNoSolarState() {
    return Container(
      height: 280,
      alignment: Alignment.center,
      child: Text(
        'Set a home location to unlock the solar rhythm editor.',
        textAlign: TextAlign.center,
        style: TextStyle(
          color: CelestialColors.textSecondary.withValues(alpha: 0.7),
          fontSize: 14,
          fontWeight: FontWeight.w500,
        ),
      ),
    );
  }

  Widget _buildFocusedTransitionSummary({
    required Color dayColor,
    required Color sleepColor,
  }) {
    final modeColor = _selectedMode == RhythmMode.day ? dayColor : sleepColor;
    final triggerLabel = _shortTriggerLabel(_config.trigger);
    final triggerHour = _triggerTimeHours();

    return Row(
      mainAxisAlignment: MainAxisAlignment.center,
      children: [
        Container(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
          decoration: BoxDecoration(
            color: modeColor.withValues(alpha: 0.12),
            borderRadius: BorderRadius.circular(999),
            border: Border.all(color: modeColor.withValues(alpha: 0.25)),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(_modeIcon(_selectedMode), size: 14, color: modeColor),
              const SizedBox(width: 6),
              Text(
                _modeLabel(_selectedMode),
                style: TextStyle(
                  color: modeColor,
                  fontSize: 12,
                  fontWeight: FontWeight.w700,
                  letterSpacing: 0.3,
                ),
              ),
            ],
          ),
        ),
        if (triggerLabel.isNotEmpty) ...[
          const SizedBox(width: 10),
          Text(
            triggerLabel.toUpperCase(),
            style: TextStyle(
              color: modeColor.withValues(alpha: 0.8),
              fontSize: 11,
              fontWeight: FontWeight.w700,
              letterSpacing: 1.2,
            ),
          ),
        ],
        if (triggerHour != null) ...[
          const SizedBox(width: 8),
          Text(
            _fmtTime(triggerHour),
            style: TextStyle(
              color: CelestialColors.textPrimary.withValues(alpha: 0.76),
              fontSize: 12,
              fontWeight: FontWeight.w500,
              letterSpacing: 0.2,
            ),
          ),
        ],
      ],
    );
  }

  Widget _buildClockRing({
    required SolarClockGeometry geometry,
    required Color dayColor,
    required Color sleepColor,
  }) {
    final dayHour = _displayHourForMode(
      RhythmMode.day,
      fallback: _handleHourForMode(RhythmMode.day) ?? 6.0,
    );
    final sleepHour = _displayHourForMode(
      RhythmMode.sleep,
      fallback: _handleHourForMode(RhythmMode.sleep) ?? 22.0,
    );

    return CustomPaint(
      painter: _RhythmClockRingPainter(
        geometry: geometry,
        dayStartHour: dayHour,
        sleepStartHour: sleepHour,
        dayVisual: _curveVisuals[RhythmMode.day] ??
            _ModeCurveVisual(fallbackColor: dayColor),
        sleepVisual: _curveVisuals[RhythmMode.sleep] ??
            _ModeCurveVisual(fallbackColor: sleepColor),
      ),
    );
  }

  Widget _buildClockOverlay({
    required SolarClockGeometry geometry,
    required Color dayColor,
    required Color sleepColor,
  }) {
    final handles = _handleSpecs(dayColor: dayColor, sleepColor: sleepColor);
    final handleByMode = {for (final handle in handles) handle.mode: handle};

    return GestureDetector(
      behavior: HitTestBehavior.translucent,
      onTapUp: (details) =>
          _handleClockTap(details.localPosition, geometry, handleByMode),
      onPanStart: (details) =>
          _handleClockPanStart(details.localPosition, geometry, handleByMode),
      onPanUpdate: (details) =>
          _handleClockPanUpdate(details.localPosition, geometry),
      onPanEnd: (_) => _handleClockPanEnd(),
      onPanCancel: _handleClockPanEnd,
      child: Stack(
        clipBehavior: Clip.none,
        children: [
          Positioned.fill(
            child: CustomPaint(
              painter: _ClockHandlesPainter(
                geometry: geometry,
                handles: handles,
                selectedMode: _selectedMode,
                flowProgress: _flowAnimation.value,
              ),
            ),
          ),
          for (final handle in handles)
            _buildPositionedHandle(handle: handle, geometry: geometry),
        ],
      ),
    );
  }

  Widget _buildPositionedHandle({
    required _TransitionHandleSpec handle,
    required SolarClockGeometry geometry,
  }) {
    final center = _orbCenterForHour(
      handle.hour,
      geometry,
      outwardDistance: handle.mode == _selectedMode ? 18 : 14,
    );
    final size = handle.mode == _selectedMode ? 70.0 : 62.0;

    return Positioned(
      left: center.dx - size / 2,
      top: center.dy - size / 2,
      child: _ClockModeOrb(
        mode: handle.mode,
        color: handle.color,
        label: handle.label,
        timeLabel: _fmtTime(handle.hour),
        selected: handle.mode == _selectedMode,
      ),
    );
  }

  List<_TransitionHandleSpec> _handleSpecs({
    required Color dayColor,
    required Color sleepColor,
  }) {
    return [
      _handleSpecForMode(RhythmMode.day, dayColor),
      _handleSpecForMode(RhythmMode.sleep, sleepColor),
    ].whereType<_TransitionHandleSpec>().toList();
  }

  _TransitionHandleSpec? _handleSpecForMode(RhythmMode mode, Color color) {
    final hour = _handleHourForMode(mode);
    if (hour == null) return null;

    return _TransitionHandleSpec(
      mode: mode,
      label: _modeLabel(mode),
      hour: _displayHourForMode(mode, fallback: hour),
      color: color,
    );
  }

  double? _handleHourForMode(RhythmMode mode) {
    final draggedHour = _handleHours[mode];
    if (draggedHour != null) return draggedHour;

    final config = _transitionConfigs[mode];
    final scheduledTime = config?.trigger.time;
    if (config?.trigger.isScheduled == true && scheduledTime != null) {
      final scheduledHour = _scheduledTimeToHour(scheduledTime);
      if (scheduledHour != null) return scheduledHour;
    }

    final triggerEvent = config?.trigger.event;
    if (config?.trigger.isSolar == true && triggerEvent != null) {
      final eventHour = _eventTimeHours(mode, triggerEvent);
      if (eventHour != null) return eventHour;
    }

    return switch (mode) {
      RhythmMode.day => _sunTimes?.sunrise,
      RhythmMode.sleep => _sunTimes?.sunset,
    };
  }

  void _handleClockTap(
    Offset position,
    SolarClockGeometry geometry,
    Map<RhythmMode, _TransitionHandleSpec> handleByMode,
  ) {
    final mode = _hitTestHandle(position, geometry, handleByMode);
    if (mode != null) _focusMode(mode);
  }

  void _handleClockPanStart(
    Offset position,
    SolarClockGeometry geometry,
    Map<RhythmMode, _TransitionHandleSpec> handleByMode,
  ) {
    final mode = _hitTestHandle(position, geometry, handleByMode);
    if (mode == null) return;
    HapticFeedback.lightImpact();
    setState(() {
      _dragMode = mode;
      _selectedMode = mode;
      _dragPreviewHour = _handleHourForMode(mode) ?? handleByMode[mode]?.hour;
    });
  }

  void _handleClockPanUpdate(Offset position, SolarClockGeometry geometry) {
    final mode = _dragMode;
    if (mode == null) return;

    final draggedHour = _clampDraggedHour(
      mode,
      geometry.hourFromPosition(position),
    );
    final snappedAnchor = _snappedAnchorForMode(mode, draggedHour);
    final displayHour = snappedAnchor?.hour ?? draggedHour;

    setState(() => _dragPreviewHour = displayHour);
    _setModeHour(mode, displayHour, snappedAnchor: snappedAnchor);
  }

  void _handleClockPanEnd() {
    final dragMode = _dragMode;
    if (_dragMode == null) return;
    setState(() {
      _dragMode = null;
      _dragPreviewHour = null;
    });
    if (dragMode != null) {
      _flushPendingUpdate(dragMode);
    }
  }

  RhythmMode? _hitTestHandle(
    Offset position,
    SolarClockGeometry geometry,
    Map<RhythmMode, _TransitionHandleSpec> handleByMode,
  ) {
    const threshold = 38.0;
    for (final entry in handleByMode.entries) {
      final center = _orbCenterForHour(entry.value.hour, geometry);
      if ((position - center).distance <= threshold) {
        return entry.key;
      }
    }
    return null;
  }

  double _displayHourForMode(RhythmMode mode, {required double fallback}) {
    if (_dragMode == mode && _dragPreviewHour != null) {
      return _dragPreviewHour!;
    }
    return _handleHourForMode(mode) ?? fallback;
  }

  double _clampDraggedHour(RhythmMode mode, double rawHour) {
    final normalized = SolarUtils.normalizeHour(rawHour);
    final dayHour = _handleHourForMode(RhythmMode.day) ?? 6.0;
    final sleepHour = _handleHourForMode(RhythmMode.sleep) ?? 22.0;

    if (mode == RhythmMode.day) {
      final maxDayHour = math.max(0.0, sleepHour - _minimumHandleGapHours);
      return normalized.clamp(0.0, maxDayHour).toDouble();
    }

    final minSleepHour =
        math.min(_maxEditableHour, dayHour + _minimumHandleGapHours);
    return normalized.clamp(minSleepHour, _maxEditableHour).toDouble();
  }

  _TriggerAnchor? _snappedAnchorForMode(RhythmMode mode, double hour) {
    final anchors = _triggerAnchorsForMode(mode)
        .where((anchor) => _isHourAllowedForMode(mode, anchor.hour))
        .toList();
    if (anchors.isEmpty) return null;

    _TriggerAnchor? bestAnchor;
    var bestDistance = double.infinity;

    for (final anchor in anchors) {
      final distance = (hour - anchor.hour).abs();
      if (distance < bestDistance) {
        bestDistance = distance;
        bestAnchor = anchor;
      }
    }

    if (bestDistance <= _eventSnapThresholdHours) {
      return bestAnchor;
    }
    return null;
  }

  bool _isHourAllowedForMode(RhythmMode mode, double hour) {
    final normalized = SolarUtils.normalizeHour(hour);
    final dayHour = _handleHourForMode(RhythmMode.day) ?? 6.0;
    final sleepHour = _handleHourForMode(RhythmMode.sleep) ?? 22.0;

    if (mode == RhythmMode.day) {
      return normalized <= sleepHour - _minimumHandleGapHours;
    }
    return normalized >= dayHour + _minimumHandleGapHours;
  }

  List<_TriggerAnchor> _triggerAnchorsForMode(RhythmMode mode) {
    final tw = _twilightTimes;
    final st = _sunTimes;
    if (tw == null || st == null) return const [];

    if (mode == RhythmMode.day) {
      return [
        if (tw.dawn.astronomical != null)
          _TriggerAnchor(
            event: 'astronomical_twilight',
            hour: tw.dawn.astronomical!,
          ),
        if (tw.dawn.nautical != null)
          _TriggerAnchor(
            event: 'nautical_twilight',
            hour: tw.dawn.nautical!,
          ),
        if (tw.dawn.civil != null)
          _TriggerAnchor(
            event: 'civil_twilight',
            hour: tw.dawn.civil!,
          ),
        _TriggerAnchor(event: 'sunrise', hour: st.sunrise),
      ];
    }

    return [
      _TriggerAnchor(event: 'sunset', hour: st.sunset),
      if (tw.dusk.civil != null)
        _TriggerAnchor(
          event: 'civil_twilight',
          hour: tw.dusk.civil!,
        ),
      if (tw.dusk.nautical != null)
        _TriggerAnchor(
          event: 'nautical_twilight',
          hour: tw.dusk.nautical!,
        ),
      if (tw.dusk.astronomical != null)
        _TriggerAnchor(
          event: 'astronomical_twilight',
          hour: tw.dusk.astronomical!,
        ),
    ];
  }

  Offset _orbCenterForHour(
    double hour,
    SolarClockGeometry geometry, {
    double outwardDistance = 14,
  }) {
    final anchor = geometry.positionForHour(hour);
    final vector = anchor - geometry.center;
    final distance = vector.distance;
    if (distance == 0) return anchor;
    final dx = vector.dx / distance;
    final dy = vector.dy / distance;
    return Offset(
      anchor.dx + dx * outwardDistance,
      anchor.dy + dy * outwardDistance,
    );
  }

  double? _triggerTimeHours() {
    if (_config.trigger.isScheduled) {
      final scheduledTime = _config.trigger.time;
      if (scheduledTime != null) {
        return _scheduledTimeToHour(scheduledTime);
      }
    }
    if (_config.trigger.kind == 'manual') {
      return _handleHourForMode(_selectedMode);
    }
    final event = _config.trigger.event;
    if (event == null || _config.trigger.kind != 'solar') return null;
    return _eventTimeHours(_selectedMode, event);
  }

  double? _eventTimeHours(RhythmMode mode, String event) {
    final isDawn = mode == RhythmMode.day;
    return switch (event) {
      'sunrise' => _sunTimes?.sunrise,
      'sunset' => _sunTimes?.sunset,
      'civil_twilight' =>
        isDawn ? _twilightTimes?.dawn.civil : _twilightTimes?.dusk.civil,
      'nautical_twilight' =>
        isDawn ? _twilightTimes?.dawn.nautical : _twilightTimes?.dusk.nautical,
      'astronomical_twilight' => isDawn
          ? _twilightTimes?.dawn.astronomical
          : _twilightTimes?.dusk.astronomical,
      _ => null,
    };
  }

  String _fmtTime(double h) {
    return SolarUtils.formatHour(
      h,
      use24: MediaQuery.alwaysUse24HourFormatOf(context),
    );
  }

  bool get _durationAuto => _config.duration.isAuto;

  Widget _buildDurationRow() {
    final accentColor = widget.profileColors[_selectedMode] ??
        _fallbackModeColor(_selectedMode);

    return Container(
      padding: const EdgeInsets.fromLTRB(20, 14, 16, 14),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(18),
      ),
      child: Column(
        children: [
          Row(
            children: [
              Container(
                width: 30,
                height: 30,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: accentColor.withValues(alpha: 0.1),
                ),
                child: Icon(Icons.timer_outlined, color: accentColor, size: 15),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Text(
                  '${_modeLabel(_selectedMode)} Duration',
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 14,
                    fontWeight: FontWeight.w500,
                  ),
                ),
              ),
              GestureDetector(
                onTap: () {
                  HapticFeedback.selectionClick();
                  if (_durationAuto) {
                    _updateConfig(_config.copyWith(
                        duration: const TransitionDuration.fixed(30000)));
                  } else {
                    _updateConfig(
                      _config.copyWith(
                          duration: const TransitionDuration.auto()),
                    );
                  }
                },
                child: Container(
                  padding:
                      const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
                  decoration: BoxDecoration(
                    color: _durationAuto
                        ? accentColor.withValues(alpha: 0.12)
                        : accentColor.withValues(alpha: 0.06),
                    borderRadius: BorderRadius.circular(6),
                    border: Border.all(
                      color: _durationAuto
                          ? accentColor.withValues(alpha: 0.25)
                          : accentColor.withValues(alpha: 0.12),
                    ),
                  ),
                  child: Text(
                    _durationAuto
                        ? 'Auto'
                        : _formatDuration(_config.durationMs),
                    style: TextStyle(
                      color: _durationAuto
                          ? accentColor
                          : accentColor.withValues(alpha: 0.7),
                      fontSize: 11,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
              ),
            ],
          ),
          AnimatedSize(
            duration: const Duration(milliseconds: 250),
            curve: Curves.easeOutCubic,
            alignment: Alignment.topCenter,
            child: _durationAuto
                ? const SizedBox.shrink()
                : Padding(
                    padding: const EdgeInsets.only(top: 8),
                    child: Column(
                      children: [
                        SliderTheme(
                          data: SliderThemeData(
                            activeTrackColor: accentColor,
                            inactiveTrackColor:
                                accentColor.withValues(alpha: 0.12),
                            thumbColor: accentColor,
                            overlayColor: accentColor.withValues(alpha: 0.12),
                            trackHeight: 4,
                            thumbShape: const RoundSliderThumbShape(
                              enabledThumbRadius: 8,
                            ),
                            overlayShape: const RoundSliderOverlayShape(
                              overlayRadius: 18,
                            ),
                          ),
                          child: Slider(
                            value:
                                (_config.durationMs / 1000.0).clamp(1.0, 120.0),
                            min: 1,
                            max: 120,
                            divisions: 119,
                            onChanged: (v) {
                              _updateConfig(_config.copyWith(
                                duration: TransitionDuration.fixed(
                                    (v * 1000).round()),
                              ));
                            },
                          ),
                        ),
                        Padding(
                          padding: const EdgeInsets.symmetric(horizontal: 6),
                          child: Row(
                            mainAxisAlignment: MainAxisAlignment.spaceBetween,
                            children: [
                              Text(
                                '1s',
                                style: TextStyle(
                                  color: CelestialColors.textSecondary
                                      .withValues(alpha: 0.4),
                                  fontSize: 11,
                                ),
                              ),
                              Text(
                                '2m',
                                style: TextStyle(
                                  color: CelestialColors.textSecondary
                                      .withValues(alpha: 0.4),
                                  fontSize: 11,
                                ),
                              ),
                            ],
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

class _TransitionHandleSpec {
  final RhythmMode mode;
  final String label;
  final double hour;
  final Color color;

  const _TransitionHandleSpec({
    required this.mode,
    required this.label,
    required this.hour,
    required this.color,
  });
}

class _TriggerAnchor {
  final String event;
  final double hour;

  const _TriggerAnchor({
    required this.event,
    required this.hour,
  });
}

class _ModeCurveVisual {
  final SolarCurveSamples? samples;
  final Color fallbackColor;
  final int fallbackBrightness;

  const _ModeCurveVisual({
    this.samples,
    required this.fallbackColor,
    this.fallbackBrightness = 50,
  });

  (Color, double) styleAt(double hour) {
    final curveSamples = samples;
    if (curveSamples == null || curveSamples.isEmpty) {
      final opacity =
          0.18 + (fallbackBrightness.clamp(0, 100).toDouble() / 100) * 0.55;
      return (fallbackColor, opacity);
    }

    final brightness = SolarUtils.interpolateValue(
      curveSamples.hours,
      curveSamples.brightness,
      hour,
    );
    final color = SolarUtils.curveColorAt(
      hour,
      hours: curveSamples.hours,
      kelvin: curveSamples.kelvin,
      fallback: fallbackColor,
    );
    final opacity = 0.18 + (brightness / 100) * 0.55;
    return (color, opacity);
  }
}

class _ClockModeOrb extends StatelessWidget {
  final RhythmMode mode;
  final Color color;
  final String label;
  final String timeLabel;
  final bool selected;

  const _ClockModeOrb({
    required this.mode,
    required this.color,
    required this.label,
    required this.timeLabel,
    required this.selected,
  });

  @override
  Widget build(BuildContext context) {
    final size = selected ? 70.0 : 62.0;
    final orbSize = selected ? 44.0 : 40.0;

    return SizedBox(
      width: size,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: orbSize,
            height: orbSize,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: color.withValues(alpha: selected ? 0.18 : 0.12),
              border: Border.all(
                color: color.withValues(alpha: selected ? 0.55 : 0.35),
                width: selected ? 1.6 : 1.2,
              ),
              boxShadow: [
                BoxShadow(
                  color: color.withValues(alpha: selected ? 0.28 : 0.18),
                  blurRadius: selected ? 16 : 12,
                ),
              ],
            ),
            child: Icon(
              _modeIcon(mode),
              size: selected ? 20 : 18,
              color: color,
            ),
          ),
          const SizedBox(height: 6),
          Text(
            label,
            style: TextStyle(
              color: color.withValues(alpha: selected ? 0.96 : 0.88),
              fontSize: selected ? 11.5 : 11,
              fontWeight: FontWeight.w700,
              letterSpacing: 0.3,
            ),
          ),
          const SizedBox(height: 1),
          Text(
            timeLabel,
            style: TextStyle(
              color: Colors.white.withValues(alpha: selected ? 0.72 : 0.52),
              fontSize: 9.5,
              fontWeight: FontWeight.w500,
              letterSpacing: 0.2,
            ),
          ),
        ],
      ),
    );
  }
}

class _ClockHandlesPainter extends CustomPainter {
  final SolarClockGeometry geometry;
  final List<_TransitionHandleSpec> handles;
  final RhythmMode selectedMode;
  final double flowProgress;

  const _ClockHandlesPainter({
    required this.geometry,
    required this.handles,
    required this.selectedMode,
    required this.flowProgress,
  });

  @override
  void paint(Canvas canvas, Size size) {
    for (final handle in handles) {
      final isSelected = handle.mode == selectedMode;
      final anchor = geometry.positionForHour(handle.hour);
      final orbCenter = _orbCenterForHour(
        handle.hour,
        outwardDistance: isSelected ? 18 : 14,
      );

      canvas.drawLine(
        anchor,
        orbCenter,
        Paint()
          ..color = handle.color.withValues(alpha: isSelected ? 0.28 : 0.16)
          ..strokeWidth = isSelected ? 2.0 : 1.4
          ..strokeCap = StrokeCap.round,
      );

      canvas.drawCircle(
        anchor,
        isSelected ? 4.0 : 3.0,
        Paint()
          ..color = handle.color.withValues(alpha: isSelected ? 0.95 : 0.75),
      );

      canvas.drawCircle(
        anchor,
        isSelected ? 8 : 6,
        Paint()
          ..color = handle.color.withValues(alpha: isSelected ? 0.18 : 0.10)
          ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 6),
      );

      if (isSelected) {
        final shimmer = Offset.lerp(
          anchor,
          orbCenter,
          0.3 + 0.2 * math.sin(flowProgress * math.pi * 2),
        );
        if (shimmer != null) {
          canvas.drawCircle(
            shimmer,
            3.0,
            Paint()..color = Colors.white.withValues(alpha: 0.82),
          );
          canvas.drawCircle(
            shimmer,
            8.0,
            Paint()
              ..color = handle.color.withValues(alpha: 0.18)
              ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 5),
          );
        }
      }
    }
  }

  Offset _orbCenterForHour(double hour, {double outwardDistance = 14}) {
    final anchor = geometry.positionForHour(hour);
    final vector = anchor - geometry.center;
    final distance = vector.distance;
    if (distance == 0) return anchor;
    final dx = vector.dx / distance;
    final dy = vector.dy / distance;
    return Offset(
      anchor.dx + dx * outwardDistance,
      anchor.dy + dy * outwardDistance,
    );
  }

  @override
  bool shouldRepaint(covariant _ClockHandlesPainter oldDelegate) {
    return geometry != oldDelegate.geometry ||
        handles != oldDelegate.handles ||
        selectedMode != oldDelegate.selectedMode ||
        flowProgress != oldDelegate.flowProgress;
  }
}

class _RhythmClockRingPainter extends CustomPainter {
  final SolarClockGeometry geometry;
  final double dayStartHour;
  final double sleepStartHour;
  final _ModeCurveVisual dayVisual;
  final _ModeCurveVisual sleepVisual;

  const _RhythmClockRingPainter({
    required this.geometry,
    required this.dayStartHour,
    required this.sleepStartHour,
    required this.dayVisual,
    required this.sleepVisual,
  });

  @override
  void paint(Canvas canvas, Size size) {
    const segments = 144;
    final rect = geometry.arcRect;
    final paint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 4.6
      ..strokeCap = StrokeCap.round;
    final sweep = (2 * math.pi) / segments;

    for (int i = 0; i < segments; i++) {
      final startAngle = -math.pi + i * sweep;
      final midHour = SolarUtils.angleToHour(
        startAngle + sweep / 2,
        geometry.solarNoon,
      );
      final isDayHour = _isWithinDaySegment(
        midHour,
        dayStartHour,
        sleepStartHour,
      );
      final (color, opacity) =
          (isDayHour ? dayVisual : sleepVisual).styleAt(midHour);
      paint.color = color.withValues(alpha: opacity);
      canvas.drawArc(rect, startAngle, sweep + 0.02, false, paint);
    }
  }

  bool _isWithinDaySegment(double hour, double dayStart, double sleepStart) {
    final normalizedHour = SolarUtils.normalizeHour(hour);
    final normalizedDayStart = SolarUtils.normalizeHour(dayStart);
    final normalizedSleepStart = SolarUtils.normalizeHour(sleepStart);

    if (normalizedDayStart <= normalizedSleepStart) {
      return normalizedHour >= normalizedDayStart &&
          normalizedHour < normalizedSleepStart;
    }

    return normalizedHour >= normalizedDayStart ||
        normalizedHour < normalizedSleepStart;
  }

  @override
  bool shouldRepaint(covariant _RhythmClockRingPainter oldDelegate) {
    return geometry != oldDelegate.geometry ||
        dayStartHour != oldDelegate.dayStartHour ||
        sleepStartHour != oldDelegate.sleepStartHour ||
        dayVisual != oldDelegate.dayVisual ||
        sleepVisual != oldDelegate.sleepVisual;
  }
}

String _formatDuration(int ms) {
  final seconds = ms ~/ 1000;
  if (seconds < 60) return '${seconds}s';
  final minutes = seconds ~/ 60;
  final remainingSeconds = seconds % 60;
  if (remainingSeconds == 0) return '$minutes min';
  return '${minutes}m ${remainingSeconds}s';
}

String _shortTriggerLabel(RhythmTransitionTrigger trigger) {
  if (trigger.isScheduled) return '';
  if (trigger.kind == 'manual') return '';
  return switch (trigger.event) {
    'sunrise' => 'Sunrise',
    'civil_twilight' => 'Civil',
    'nautical_twilight' => 'Nautical',
    'astronomical_twilight' => 'Astro',
    'sunset' => 'Sunset',
    _ => trigger.event?.replaceAll('_', ' ') ?? '?',
  };
}

Color _fallbackModeColor(RhythmMode mode) => switch (mode) {
      RhythmMode.day => const Color(0xFFF9A825),
      RhythmMode.sleep => const Color(0xFF7C4DFF),
    };

IconData _modeIcon(RhythmMode mode) => switch (mode) {
      RhythmMode.day => Icons.wb_sunny_rounded,
      RhythmMode.sleep => Icons.bedtime_rounded,
    };

String _modeLabel(RhythmMode mode) => switch (mode) {
      RhythmMode.day => 'Day Start',
      RhythmMode.sleep => 'Sleep Start',
    };

String _formatScheduledTriggerTime(double hour) {
  final totalMinutes =
      (SolarUtils.normalizeHour(hour) * 60).round().clamp(0, 1439);
  final hours = totalMinutes ~/ 60;
  final minutes = totalMinutes % 60;
  return '${hours.toString().padLeft(2, '0')}:${minutes.toString().padLeft(2, '0')}';
}

double? _scheduledTimeToHour(String time) {
  final parts = time.split(':');
  if (parts.length != 2) return null;
  final hours = int.tryParse(parts[0]);
  final minutes = int.tryParse(parts[1]);
  if (hours == null || minutes == null) return null;
  if (hours < 0 || hours > 23 || minutes < 0 || minutes > 59) return null;
  return hours + minutes / 60.0;
}
