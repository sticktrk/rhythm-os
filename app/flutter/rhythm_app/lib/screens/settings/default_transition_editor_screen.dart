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
  static const double _minDurationSeconds = 10.0;
  static const double _maxDurationSeconds = 300.0;
  static const double _durationStepSeconds = 10.0;

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
  late final ServerSyncProvider _serverSync;
  SolarClockData? _solarClockData;
  RhythmMode? _dragMode;
  double? _dragPreviewHour;
  _TriggerAnchor? _proximateAnchor;
  double _anchorProximity = 0.0;

  SunTimesDto? get _sunTimes => _solarClockData?.sunTimes;
  TwilightTimesDto? get _twilightTimes => _solarClockData?.twilightTimes;
  RhythmModeTransitionConfig get _config => _transitionConfigs[_selectedMode]!;

  @override
  void initState() {
    super.initState();
    _serverSync = context.read<ServerSyncProvider>();
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
    for (final timer in _updateDebounceTimers.values) {
      timer.cancel();
    }
    for (final pending in _pendingUpdates.values) {
      _serverSync.dispatchUpdateTransition(pending);
    }
    _breatheController.dispose();
    _flowController.dispose();
    super.dispose();
  }

  Map<RhythmMode, RhythmModeTransitionConfig> _initialTransitionConfigs() {
    final transitions = _serverSync.modeTransitions;
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
    String? activeProfileId;
    for (final modeConfig in _serverSync.modeConfigs) {
      if (modeConfig.mode == mode) {
        activeProfileId = modeConfig.activeProfileId;
        break;
      }
    }
    if (activeProfileId == null || activeProfileId.isEmpty) return null;

    for (final profile in _serverSync.profiles) {
      if (profile.id == activeProfileId) return profile;
    }
    return null;
  }

  void _updateConfig(
    RhythmModeTransitionConfig updated, {
    bool selectUpdatedMode = true,
  }) {
    setState(() {
      _transitionConfigs[updated.toMode] = updated;
      if (selectUpdatedMode) {
        _selectedMode = updated.toMode;
      }
    });
    _pendingUpdates[updated.toMode] = updated;
    _updateDebounceTimers[updated.toMode]?.cancel();
    _updateDebounceTimers[updated.toMode] = Timer(
      _updateDebounceDuration,
      () => _flushPendingUpdate(updated.toMode),
    );
  }

  void _updateSelectedDuration(TransitionDuration duration) {
    _updateConfig(
      _config.copyWith(duration: duration),
      selectUpdatedMode: false,
    );
  }

  void _flushPendingUpdate(RhythmMode mode) {
    _updateDebounceTimers.remove(mode)?.cancel();
    final pending = _pendingUpdates.remove(mode);
    if (pending == null || !mounted) return;
    _serverSync.dispatchUpdateTransition(pending);
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
              'Daily Rhythm',
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
              'Drag handles to set when each mode begins',
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
            showEventMarkers: false,
            showHourLabels: false,
            showLowerArc: false,
            horizonFactor: 0.56,
            radiusWidthFactor: 0.35,
            radiusHeightFactor: 0.74,
            underlayBuilder: (context, geometry) => _buildClockRing(
              geometry: geometry,
              dayColor: dayColor,
              sleepColor: sleepColor,
              use24: MediaQuery.alwaysUse24HourFormatOf(context),
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
    final isDragging = _dragMode != null;

    // During drag, show the approaching anchor or current drag position
    final String triggerLabel;
    final double? triggerHour;
    if (isDragging && _proximateAnchor != null && _anchorProximity > 0.5) {
      triggerLabel = _shortAnchorLabel(_proximateAnchor!);
      triggerHour = _proximateAnchor!.hour;
    } else if (isDragging) {
      triggerLabel = '';
      triggerHour = _dragPreviewHour;
    } else {
      triggerLabel = _shortTriggerLabel(_config.trigger);
      triggerHour = _triggerTimeHours();
    }

    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 8),
      decoration: BoxDecoration(
        color: modeColor.withValues(alpha: 0.08),
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: modeColor.withValues(alpha: 0.15)),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          Icon(_modeIcon(_selectedMode), size: 16, color: modeColor),
          const SizedBox(width: 8),
          Text(
            '${_modeLabel(_selectedMode)} Start',
            style: TextStyle(
              color: modeColor,
              fontSize: 13,
              fontWeight: FontWeight.w700,
              letterSpacing: 0.3,
            ),
          ),
          if (triggerLabel.isNotEmpty || triggerHour != null) ...[
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 10),
              child: Container(
                width: 1,
                height: 16,
                color: modeColor.withValues(alpha: 0.2),
              ),
            ),
            if (triggerLabel.isNotEmpty)
              Text(
                triggerLabel,
                style: TextStyle(
                  color: CelestialColors.textPrimary.withValues(alpha: 0.7),
                  fontSize: 12,
                  fontWeight: FontWeight.w500,
                ),
              ),
            if (triggerLabel.isNotEmpty && triggerHour != null)
              const SizedBox(width: 6),
            if (triggerHour != null)
              Text(
                _fmtTime(triggerHour),
                style: TextStyle(
                  color: CelestialColors.textPrimary.withValues(alpha: 0.9),
                  fontSize: 13,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 0.2,
                ),
              ),
          ],
        ],
      ),
    );
  }

  Widget _buildClockRing({
    required SolarClockGeometry geometry,
    required Color dayColor,
    required Color sleepColor,
    required bool use24,
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
        use24: use24,
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
          ..._buildTwilightAnchorIndicators(
            geometry: geometry,
            dayColor: dayColor,
            sleepColor: sleepColor,
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
    final isSelected = handle.mode == _selectedMode;
    final center = geometry.positionForHour(handle.hour);
    final width = isSelected ? 70.0 : 62.0;
    final orbRadius = (isSelected ? 44.0 : 40.0) / 2;

    return Positioned(
      left: center.dx - width / 2,
      top: center.dy - orbRadius,
      child: _ClockModeOrb(
        mode: handle.mode,
        color: handle.color,
        timeLabel: _fmtTime(handle.hour),
        selected: isSelected,
      ),
    );
  }

  List<Widget> _buildTwilightAnchorIndicators({
    required SolarClockGeometry geometry,
    required Color dayColor,
    required Color sleepColor,
  }) {
    final mode = _dragMode ?? _selectedMode;
    final isDragging = _dragMode != null;
    final modeColor = mode == RhythmMode.day ? dayColor : sleepColor;
    final isDawn = mode == RhythmMode.day;
    final baseColor =
        isDawn ? SolarClockData.dawnColor : SolarClockData.duskColor;
    final anchors = _triggerAnchorsForMode(mode);
    final handleHour =
        _displayHourForMode(mode, fallback: _handleHourForMode(mode) ?? 0.0);
    final activeSolarEvent =
        _config.trigger.isSolar ? _config.trigger.event : null;

    final widgets = <Widget>[
      Positioned.fill(
        child: IgnorePointer(
          child: CustomPaint(
            painter: _ClockSolarAnchorPainter(
              geometry: geometry,
              anchors: anchors,
              accentColor: Color.lerp(baseColor, modeColor, 0.45)!,
              activeEvent: activeSolarEvent,
              proximateAnchor: _proximateAnchor,
              anchorProximity: _anchorProximity,
              handleHour: handleHour,
              handleWidth: mode == _selectedMode ? 70.0 : 62.0,
              dragHour: _dragPreviewHour,
              pulse: _breatheAnimation.value,
              isDragging: isDragging,
            ),
          ),
        ),
      ),
    ];

    for (final anchor in anchors) {
      final isProximate = isDragging && _proximateAnchor?.event == anchor.event;
      final isActive = !isDragging && activeSolarEvent == anchor.event;
      final proximity = isProximate ? _anchorProximity : 0.0;

      if (!isDragging && !isActive) continue;

      final pos = geometry.positionForHour(anchor.hour);

      // Dot sizing: active pulses gently, dragging scales with proximity
      final breathe = _breatheAnimation.value;
      final dotSize = isActive ? 7.0 + breathe * 1.5 : 4.0 + proximity * 7.0;
      final dotAlpha = isActive ? 0.65 : 0.2 + proximity * 0.8;
      final glowRadius = isActive ? 6.0 + breathe * 3.0 : proximity * 16.0;
      final effectiveColor =
          isActive ? modeColor : Color.lerp(baseColor, modeColor, proximity)!;

      final totalSize = dotSize + glowRadius * 2;
      widgets.add(
        Positioned(
          left: pos.dx - totalSize / 2,
          top: pos.dy - totalSize / 2,
          child: IgnorePointer(
            child: SizedBox(
              width: totalSize,
              height: totalSize,
              child: Center(
                child: Container(
                  width: dotSize,
                  height: dotSize,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: effectiveColor.withValues(alpha: dotAlpha),
                    boxShadow: [
                      if (glowRadius > 0)
                        BoxShadow(
                          color: effectiveColor.withValues(
                            alpha: dotAlpha * 0.5,
                          ),
                          blurRadius: glowRadius,
                          spreadRadius: 1,
                        ),
                    ],
                  ),
                ),
              ),
            ),
          ),
        ),
      );
    }

    return widgets;
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
    final initialHour = _handleHourForMode(mode) ?? handleByMode[mode]?.hour;
    setState(() {
      _dragMode = mode;
      _selectedMode = mode;
      _dragPreviewHour = initialHour;
      if (initialHour != null) _computeDragProximity(mode, initialHour);
    });
  }

  void _handleClockPanUpdate(Offset position, SolarClockGeometry geometry) {
    final mode = _dragMode;
    if (mode == null) return;

    final draggedHour = _clampDraggedHour(
      mode,
      geometry.hourFromPosition(position),
    );

    // Snap visually during drag so the orb locks onto solar events
    final snappedAnchor = _snappedAnchorForMode(mode, draggedHour);
    final displayHour = snappedAnchor?.hour ?? draggedHour;

    final previousProximate = _proximateAnchor;
    setState(() {
      _dragPreviewHour = displayHour;
      _handleHours[mode] = draggedHour;
      _computeDragProximity(mode, draggedHour);
    });
    if (_proximateAnchor != null &&
        _proximateAnchor != previousProximate &&
        _anchorProximity > 0.75) {
      HapticFeedback.selectionClick();
    }
  }

  void _handleClockPanEnd() {
    final mode = _dragMode;
    if (mode == null) return;
    final finalHour = _dragPreviewHour ?? _handleHourForMode(mode);

    setState(() {
      _dragMode = null;
      _dragPreviewHour = null;
      _proximateAnchor = null;
      _anchorProximity = 0.0;
    });

    if (finalHour != null) {
      final snappedAnchor = _snappedAnchorForMode(mode, finalHour);
      _setModeHour(
        mode,
        snappedAnchor?.hour ?? finalHour,
        snappedAnchor: snappedAnchor,
      );
      _flushPendingUpdate(mode);
    }
  }

  RhythmMode? _hitTestHandle(
    Offset position,
    SolarClockGeometry geometry,
    Map<RhythmMode, _TransitionHandleSpec> handleByMode,
  ) {
    const threshold = 38.0;
    for (final entry in handleByMode.entries) {
      final center = geometry.positionForHour(entry.value.hour);
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

  void _computeDragProximity(RhythmMode mode, double dragHour) {
    final anchors = _triggerAnchorsForMode(mode)
        .where((a) => _isHourAllowedForMode(mode, a.hour))
        .toList();
    if (anchors.isEmpty) {
      _proximateAnchor = null;
      _anchorProximity = 0.0;
      return;
    }
    _TriggerAnchor? nearest;
    var nearestDist = double.infinity;
    for (final a in anchors) {
      final dist = (dragHour - a.hour).abs();
      if (dist < nearestDist) {
        nearestDist = dist;
        nearest = a;
      }
    }
    const awarenessRadius = 1.5;
    if (nearest != null && nearestDist <= awarenessRadius) {
      _proximateAnchor = nearest;
      _anchorProximity = (1.0 - nearestDist / awarenessRadius).clamp(0.0, 1.0);
    } else {
      _proximateAnchor = null;
      _anchorProximity = 0.0;
    }
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
            label: 'Astro Dawn',
            hour: tw.dawn.astronomical!,
          ),
        if (tw.dawn.nautical != null)
          _TriggerAnchor(
            event: 'nautical_twilight',
            label: 'Nautical Dawn',
            hour: tw.dawn.nautical!,
          ),
        if (tw.dawn.civil != null)
          _TriggerAnchor(
            event: 'civil_twilight',
            label: 'Civil Dawn',
            hour: tw.dawn.civil!,
          ),
        _TriggerAnchor(
          event: 'sunrise',
          label: 'Sunrise',
          hour: st.sunrise,
        ),
      ];
    }

    return [
      _TriggerAnchor(
        event: 'sunset',
        label: 'Sunset',
        hour: st.sunset,
      ),
      if (tw.dusk.civil != null)
        _TriggerAnchor(
          event: 'civil_twilight',
          label: 'Civil Dusk',
          hour: tw.dusk.civil!,
        ),
      if (tw.dusk.nautical != null)
        _TriggerAnchor(
          event: 'nautical_twilight',
          label: 'Nautical Dusk',
          hour: tw.dusk.nautical!,
        ),
      if (tw.dusk.astronomical != null)
        _TriggerAnchor(
          event: 'astronomical_twilight',
          label: 'Astro Dusk',
          hour: tw.dusk.astronomical!,
        ),
    ];
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
    final modeKey = _selectedMode.name;
    final clampedDurationSeconds = (_config.durationMs / 1000.0)
        .clamp(_minDurationSeconds, _maxDurationSeconds)
        .toDouble();
    final sliderSeconds =
        (clampedDurationSeconds / _durationStepSeconds).round() *
            _durationStepSeconds;

    return Container(
      padding: const EdgeInsets.fromLTRB(20, 14, 16, 14),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(18),
      ),
      child: Column(
        children: [
          Row(
            key: ValueKey('duration-header-$modeKey'),
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
                  '${_modeLabel(_selectedMode)} Transition Duration',
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
                    _updateSelectedDuration(
                      const TransitionDuration.fixed(30000),
                    );
                  } else {
                    _updateSelectedDuration(const TransitionDuration.auto());
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
            key: ValueKey('duration-body-$modeKey'),
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
                            activeTickMarkColor:
                                accentColor.withValues(alpha: 0.45),
                            inactiveTickMarkColor:
                                accentColor.withValues(alpha: 0.24),
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
                            key: ValueKey('duration-slider-$modeKey'),
                            value: sliderSeconds,
                            min: _minDurationSeconds,
                            max: _maxDurationSeconds,
                            divisions:
                                ((_maxDurationSeconds - _minDurationSeconds) /
                                        _durationStepSeconds)
                                    .round(),
                            onChanged: (v) {
                              final roundedSeconds =
                                  (v / _durationStepSeconds).round() *
                                      _durationStepSeconds;
                              _updateSelectedDuration(
                                TransitionDuration.fixed(
                                  (roundedSeconds * 1000).round(),
                                ),
                              );
                            },
                          ),
                        ),
                        Padding(
                          padding: const EdgeInsets.symmetric(horizontal: 6),
                          child: Row(
                            mainAxisAlignment: MainAxisAlignment.spaceBetween,
                            children: [
                              Text(
                                '10s',
                                style: TextStyle(
                                  color: CelestialColors.textSecondary
                                      .withValues(alpha: 0.4),
                                  fontSize: 11,
                                ),
                              ),
                              Text(
                                '5m',
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
  final double hour;
  final Color color;

  const _TransitionHandleSpec({
    required this.mode,
    required this.hour,
    required this.color,
  });
}

class _TriggerAnchor {
  final String event;
  final String label;
  final double hour;

  const _TriggerAnchor({
    required this.event,
    required this.label,
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
  final String timeLabel;
  final bool selected;

  const _ClockModeOrb({
    required this.mode,
    required this.color,
    required this.timeLabel,
    required this.selected,
  });

  @override
  Widget build(BuildContext context) {
    final width = selected ? 70.0 : 62.0;
    final orbSize = selected ? 44.0 : 40.0;

    return SizedBox(
      width: width,
      child: Container(
        width: orbSize,
        height: orbSize,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          color: Color.lerp(
            const Color(0xFF1A1A2E),
            color,
            selected ? 0.35 : 0.25,
          ),
          border: Border.all(
            color: color.withValues(alpha: selected ? 0.85 : 0.6),
            width: selected ? 2.0 : 1.6,
          ),
          boxShadow: [
            BoxShadow(
              color: color.withValues(alpha: selected ? 0.45 : 0.25),
              blurRadius: selected ? 18 : 12,
            ),
          ],
        ),
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Icon(
              _modeIcon(mode),
              size: selected ? 18 : 16,
              color: color,
            ),
            const SizedBox(height: 1),
            Text(
              timeLabel,
              style: TextStyle(
                color: Colors.white.withValues(alpha: selected ? 0.82 : 0.6),
                fontSize: 8.5,
                fontWeight: FontWeight.w600,
                letterSpacing: 0.1,
              ),
            ),
          ],
        ),
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
      if (!isSelected) continue;

      final anchor = geometry.positionForHour(handle.hour);

      // Glow behind the selected orb
      canvas.drawCircle(
        anchor,
        24,
        Paint()
          ..color = handle.color.withValues(alpha: 0.22)
          ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 14),
      );
    }
  }

  @override
  bool shouldRepaint(covariant _ClockHandlesPainter oldDelegate) {
    return geometry != oldDelegate.geometry ||
        handles != oldDelegate.handles ||
        selectedMode != oldDelegate.selectedMode ||
        flowProgress != oldDelegate.flowProgress;
  }
}

class _ClockSolarAnchorPainter extends CustomPainter {
  final SolarClockGeometry geometry;
  final List<_TriggerAnchor> anchors;
  final Color accentColor;
  final String? activeEvent;
  final _TriggerAnchor? proximateAnchor;
  final double anchorProximity;
  final double handleHour;
  final double handleWidth;
  final double? dragHour;
  final double pulse;
  final bool isDragging;

  const _ClockSolarAnchorPainter({
    required this.geometry,
    required this.anchors,
    required this.accentColor,
    required this.activeEvent,
    required this.proximateAnchor,
    required this.anchorProximity,
    required this.handleHour,
    required this.handleWidth,
    required this.dragHour,
    required this.pulse,
    required this.isDragging,
  });

  @override
  void paint(Canvas canvas, Size size) {
    if (anchors.isEmpty) return;

    final layouts = <_ClockSolarAnchorLayout>[];

    for (final anchor in anchors) {
      final anchorPoint = geometry.positionForHour(anchor.hour);
      final radial = anchorPoint - geometry.center;
      final radialDistance = radial.distance;
      if (radialDistance == 0) continue;

      final normal = radial / radialDistance;
      final isActive = !isDragging && activeEvent == anchor.event;
      final isProximate = isDragging && proximateAnchor?.event == anchor.event;
      final dragFocus = isDragging && dragHour != null
          ? (1.0 - (dragHour! - anchor.hour).abs() / 1.35).clamp(0.0, 1.0)
          : 0.0;
      final emphasis = isActive
          ? 1.0
          : math.max(
              isProximate ? anchorProximity : 0.0,
              dragFocus * 0.92,
            );
      final zoom = 1.0 + emphasis * (0.75 + pulse * 0.35);
      final tickExtent = 9.0 + zoom * 4.2;
      final strokeWidth = 2.6 + emphasis * 2.0;
      final tickColor = Color.lerp(
        accentColor,
        Colors.white,
        0.14 + emphasis * 0.26,
      )!;
      final textPainter = TextPainter(
        text: TextSpan(
          text: anchor.label,
          style: TextStyle(
            color: tickColor.withValues(alpha: 0.68 + emphasis * 0.28),
            fontSize: 8.8 + emphasis * 1.2,
            fontWeight: FontWeight.w700,
            letterSpacing: 0.1,
            shadows: [
              Shadow(
                color: Colors.black.withValues(alpha: 0.45),
                blurRadius: 6,
              ),
            ],
          ),
        ),
        textDirection: TextDirection.ltr,
      )..layout();
      final inner = geometry.center + normal * (geometry.radius - tickExtent);
      final outer = geometry.center + normal * (geometry.radius + tickExtent);
      final innerLeader =
          geometry.center + normal * (geometry.radius - tickExtent - 10);

      canvas.drawLine(
        inner,
        outer,
        Paint()
          ..color = tickColor.withValues(alpha: 0.16 + emphasis * 0.18)
          ..strokeWidth = strokeWidth * (3.8 + emphasis * 0.5)
          ..strokeCap = StrokeCap.round
          ..maskFilter = MaskFilter.blur(
            BlurStyle.normal,
            5 + emphasis * 4,
          ),
      );
      canvas.drawLine(
        inner,
        outer,
        Paint()
          ..color = tickColor.withValues(alpha: 0.82 + emphasis * 0.18)
          ..strokeWidth = strokeWidth
          ..strokeCap = StrokeCap.round,
      );
      canvas.drawCircle(
        outer,
        2.2 + emphasis * (3.6 + pulse * 1.8),
        Paint()
          ..color = tickColor.withValues(alpha: 0.16 + emphasis * 0.16)
          ..maskFilter = MaskFilter.blur(
            BlurStyle.normal,
            4 + emphasis * 4,
          ),
      );
      canvas.drawCircle(
        outer,
        1.2 + emphasis * 1.4,
        Paint()..color = tickColor.withValues(alpha: 0.75 + emphasis * 0.20),
      );

      layouts.add(
        _ClockSolarAnchorLayout(
          leaderStart: innerLeader,
          normal: normal,
          tickColor: tickColor,
          emphasis: emphasis,
          textPainter: textPainter,
          preferredTop: anchorPoint.dy - textPainter.height / 2,
        ),
      );
    }

    if (layouts.isEmpty) return;

    final leftLayouts =
        layouts.where((layout) => layout.normal.dx < 0).toList();
    final rightLayouts =
        layouts.where((layout) => layout.normal.dx >= 0).toList();

    _paintClockSolarAnchorSide(
      canvas,
      size,
      geometry: geometry,
      layouts: leftLayouts,
      handleHour: handleHour,
      handleWidth: handleWidth,
      isRightSide: false,
    );
    _paintClockSolarAnchorSide(
      canvas,
      size,
      geometry: geometry,
      layouts: rightLayouts,
      handleHour: handleHour,
      handleWidth: handleWidth,
      isRightSide: true,
    );
  }

  @override
  bool shouldRepaint(covariant _ClockSolarAnchorPainter oldDelegate) {
    return geometry != oldDelegate.geometry ||
        anchors != oldDelegate.anchors ||
        accentColor != oldDelegate.accentColor ||
        activeEvent != oldDelegate.activeEvent ||
        proximateAnchor != oldDelegate.proximateAnchor ||
        anchorProximity != oldDelegate.anchorProximity ||
        handleHour != oldDelegate.handleHour ||
        handleWidth != oldDelegate.handleWidth ||
        dragHour != oldDelegate.dragHour ||
        pulse != oldDelegate.pulse ||
        isDragging != oldDelegate.isDragging;
  }
}

class _ClockSolarAnchorLayout {
  final Offset leaderStart;
  final Offset normal;
  final Color tickColor;
  final double emphasis;
  final TextPainter textPainter;
  final double preferredTop;
  double top;

  _ClockSolarAnchorLayout({
    required this.leaderStart,
    required this.normal,
    required this.tickColor,
    required this.emphasis,
    required this.textPainter,
    required this.preferredTop,
  }) : top = preferredTop;
}

void _paintClockSolarAnchorSide(
  Canvas canvas,
  Size size, {
  required SolarClockGeometry geometry,
  required List<_ClockSolarAnchorLayout> layouts,
  required double handleHour,
  required double handleWidth,
  required bool isRightSide,
}) {
  if (layouts.isEmpty) return;

  final maxTextWidth = layouts.fold<double>(
    0.0,
    (maxWidth, item) => math.max(maxWidth, item.textPainter.width),
  );
  final handleCenter = geometry.positionForHour(handleHour);
  final handleOnRight = handleCenter.dx >= geometry.center.dx;
  const handleGap = 16.0;
  final preferredColumnLeft = isRightSide
      ? geometry.center.dx + geometry.radius * 0.06
      : geometry.center.dx - geometry.radius * 0.06 - maxTextWidth;
  var columnLeft = preferredColumnLeft;

  if (isRightSide && handleOnRight) {
    columnLeft = math.min(
      columnLeft,
      handleCenter.dx - handleWidth / 2 - maxTextWidth - handleGap,
    );
  } else if (!isRightSide && !handleOnRight) {
    columnLeft = math.max(
      columnLeft,
      handleCenter.dx + handleWidth / 2 + handleGap,
    );
  }

  final minColumnLeft = isRightSide ? geometry.center.dx - 10.0 : 12.0;
  final maxColumnLeft = isRightSide
      ? size.width - maxTextWidth - 12.0
      : geometry.center.dx - maxTextWidth * 0.35;
  columnLeft = columnLeft.clamp(minColumnLeft, maxColumnLeft).toDouble();

  _resolveClockSolarAnchorLabelTops(
    layouts,
    minTop: math.max(14.0, geometry.center.dy - geometry.radius * 0.78),
    maxBottom: math.min(
      size.height - 14.0,
      geometry.center.dy + geometry.radius * 0.78,
    ),
    gap: 10.0,
  );

  for (final layout in layouts) {
    final labelLeft = isRightSide
        ? columnLeft
        : columnLeft + (maxTextWidth - layout.textPainter.width);
    final labelCenterY = layout.top + layout.textPainter.height / 2;
    final connectorEnd = Offset(
      isRightSide
          ? labelLeft + layout.textPainter.width + 6.0
          : labelLeft - 6.0,
      labelCenterY,
    );

    canvas.drawLine(
      layout.leaderStart,
      connectorEnd,
      Paint()
        ..color = layout.tickColor.withValues(
          alpha: 0.16 + layout.emphasis * 0.14,
        )
        ..strokeWidth = 1.2 + layout.emphasis * 0.7
        ..strokeCap = StrokeCap.round,
    );

    layout.textPainter.paint(canvas, Offset(labelLeft, layout.top));
  }
}

void _resolveClockSolarAnchorLabelTops(
  List<_ClockSolarAnchorLayout> layouts, {
  required double minTop,
  required double maxBottom,
  required double gap,
}) {
  layouts.sort((a, b) => a.preferredTop.compareTo(b.preferredTop));

  for (var i = 0; i < layouts.length; i++) {
    final layout = layouts[i];
    var nextTop = layout.preferredTop.clamp(
      minTop,
      maxBottom - layout.textPainter.height,
    );
    if (i > 0) {
      final previous = layouts[i - 1];
      final minAllowedTop = previous.top + previous.textPainter.height + gap;
      if (nextTop < minAllowedTop) nextTop = minAllowedTop;
    }
    layout.top = nextTop;
  }

  final overflow =
      layouts.last.top + layouts.last.textPainter.height - maxBottom;
  if (overflow > 0) {
    layouts.last.top -= overflow;
    for (var i = layouts.length - 2; i >= 0; i--) {
      final current = layouts[i];
      final next = layouts[i + 1];
      final maxAllowedTop = next.top - current.textPainter.height - gap;
      if (current.top > maxAllowedTop) current.top = maxAllowedTop;
    }
  }

  final underflow = minTop - layouts.first.top;
  if (underflow > 0) {
    for (final layout in layouts) {
      layout.top += underflow;
    }
  }
}

class _RhythmClockRingPainter extends CustomPainter {
  final SolarClockGeometry geometry;
  final double dayStartHour;
  final double sleepStartHour;
  final _ModeCurveVisual dayVisual;
  final _ModeCurveVisual sleepVisual;
  final bool use24;

  const _RhythmClockRingPainter({
    required this.geometry,
    required this.dayStartHour,
    required this.sleepStartHour,
    required this.dayVisual,
    required this.sleepVisual,
    required this.use24,
  });

  @override
  void paint(Canvas canvas, Size size) {
    _drawRing(canvas);
    _drawClockTicks(canvas);
    _drawHourLabels(canvas);
  }

  void _drawRing(Canvas canvas) {
    const segments = 144;
    final rect = geometry.arcRect;
    final paint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 16
      ..strokeCap = StrokeCap.round;
    const hourStep = 24.0 / segments;
    final sweep = (2 * math.pi) / segments;

    for (int i = 0; i < segments; i++) {
      final startHour = i * hourStep;
      final midHour = startHour + hourStep / 2;
      final isDayHour = _isWithinDaySegment(
        midHour,
        dayStartHour,
        sleepStartHour,
      );
      final (color, opacity) =
          (isDayHour ? dayVisual : sleepVisual).styleAt(midHour);
      final effectiveOpacity = isDayHour ? opacity : math.max(opacity, 0.38);
      paint.color = color.withValues(alpha: effectiveOpacity);
      canvas.drawArc(
        rect,
        geometry.angleForHour(startHour),
        sweep + 0.02,
        false,
        paint,
      );
    }
  }

  /// Draws watch-style index marks around the ring perimeter.
  ///
  /// Hierarchy mirrors a traditional clock face:
  /// - Cardinal (0, 6, 12, 18) — tallest, boldest
  /// - 3-hour (3, 9, 15, 21) — medium
  /// - Hourly — short
  /// - Half-hour — subtle subdivisions
  void _drawClockTicks(Canvas canvas) {
    final ringOuter = geometry.radius + 8;
    final tickPaint = Paint()..strokeCap = StrokeCap.round;

    for (int i = 0; i < 48; i++) {
      final hour = i * 0.5;
      final isWholeHour = i.isEven;
      final hourInt = i ~/ 2;

      // Skip ticks at labeled positions — the number serves as the marker
      if (isWholeHour && hourInt % 3 == 0) continue;

      final angle = geometry.angleForHour(hour);
      final cosA = math.cos(angle);
      final sinA = math.sin(angle);

      double tickLen, strokeW, alpha;

      if (isWholeHour) {
        tickLen = 3.5;
        strokeW = 1.0;
        alpha = 0.18;
      } else {
        tickLen = 2.0;
        strokeW = 0.7;
        alpha = 0.10;
      }

      final innerR = ringOuter + 1.5;
      final outerR = ringOuter + 1.5 + tickLen;

      tickPaint
        ..color = Colors.white.withValues(alpha: alpha)
        ..strokeWidth = strokeW;

      canvas.drawLine(
        Offset(
          geometry.center.dx + innerR * cosA,
          geometry.center.dy + innerR * sinA,
        ),
        Offset(
          geometry.center.dx + outerR * cosA,
          geometry.center.dy + outerR * sinA,
        ),
        tickPaint,
      );
    }
  }

  void _drawHourLabels(Canvas canvas) {
    const labeledHours = [0, 3, 6, 9, 12, 15, 18, 21];

    for (final hour in labeledHours) {
      final angle = geometry.angleForHour(hour.toDouble());
      final cosA = math.cos(angle);
      final sinA = math.sin(angle);
      final isCardinal = hour % 6 == 0;

      final label = use24
          ? hour.toString().padLeft(2, '0')
          : hour == 0
              ? '12 AM'
              : hour == 12
                  ? '12 PM'
                  : hour < 12
                      ? '$hour AM'
                      : '${hour - 12} PM';

      final textPainter = TextPainter(
        text: TextSpan(
          text: label,
          style: TextStyle(
            color: Colors.white.withValues(alpha: isCardinal ? 0.72 : 0.42),
            fontSize: isCardinal ? 12 : 10,
            fontWeight: isCardinal ? FontWeight.w600 : FontWeight.w500,
            letterSpacing: 0.3,
          ),
        ),
        textDirection: TextDirection.ltr,
      )..layout();

      final distance = geometry.radius + 22;
      textPainter.paint(
        canvas,
        Offset(
          geometry.center.dx + distance * cosA - textPainter.width / 2,
          geometry.center.dy + distance * sinA - textPainter.height / 2,
        ),
      );
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
        sleepVisual != oldDelegate.sleepVisual ||
        use24 != oldDelegate.use24;
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

String _shortAnchorLabel(_TriggerAnchor anchor) {
  return anchor.label;
}

String _shortTriggerLabel(RhythmTransitionTrigger trigger) {
  if (trigger.isScheduled) return '';
  if (trigger.kind == 'manual') return '';
  return switch (trigger.event) {
    'sunrise' => 'Sunrise',
    'civil_twilight' => 'Civil Twilight',
    'nautical_twilight' => 'Nautical Twilight',
    'astronomical_twilight' => 'Astronomical Twilight',
    'sunset' => 'Sunset',
    _ => trigger.event?.replaceAll('_', ' ') ?? '?',
  };
}

Color _fallbackModeColor(RhythmMode mode) => switch (mode) {
      RhythmMode.day => const Color(0xFFF9A825),
      RhythmMode.sleep => const Color(0xFFE57373),
    };

IconData _modeIcon(RhythmMode mode) => switch (mode) {
      RhythmMode.day => Icons.wb_sunny_rounded,
      RhythmMode.sleep => Icons.bedtime_rounded,
    };

String _modeLabel(RhythmMode mode) => switch (mode) {
      RhythmMode.day => 'Day',
      RhythmMode.sleep => 'Sleep',
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
