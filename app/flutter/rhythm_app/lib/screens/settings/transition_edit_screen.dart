import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart' hide Home, Hub, HubType;
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../providers/home_provider.dart';
import '../../providers/server_sync_provider.dart';
import '../../widgets/solar_clock/solar_clock_exports.dart';
import '../../widgets/solar_orbit.dart';

/// Full-screen designer for editing a mode transition.
///
/// Features an atmospheric hero visualization with breathing gradients,
/// tappable mode orbs, inline trigger chips, and a large-format
/// duration display.
class TransitionEditScreen extends StatefulWidget {
  final RhythmModeTransitionConfig transition;
  final Map<RhythmMode, Color> profileColors;

  const TransitionEditScreen({
    super.key,
    required this.transition,
    required this.profileColors,
  });

  static Future<void> show(
    BuildContext context, {
    required RhythmModeTransitionConfig transition,
    required Map<RhythmMode, Color> profileColors,
  }) {
    return Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => TransitionEditScreen(
          transition: transition,
          profileColors: profileColors,
        ),
      ),
    );
  }

  @override
  State<TransitionEditScreen> createState() => _TransitionEditScreenState();
}

class _TransitionEditScreenState extends State<TransitionEditScreen>
    with TickerProviderStateMixin {
  late Map<RhythmMode, RhythmModeTransitionConfig> _transitionConfigs;
  late RhythmMode _selectedMode;
  late AnimationController _breatheController;
  late Animation<double> _breatheAnimation;
  late AnimationController _flowController;
  late Animation<double> _flowAnimation;
  SolarClockData? _solarClockData;
  RhythmMode? _dragMode;

  SunTimesDto? get _sunTimes => _solarClockData?.sunTimes;
  TwilightTimesDto? get _twilightTimes => _solarClockData?.twilightTimes;
  RhythmModeTransitionConfig get _config => _transitionConfigs[_selectedMode]!;

  @override
  void initState() {
    super.initState();
    _transitionConfigs = _initialTransitionConfigs();
    _selectedMode = widget.transition.toMode;
    _breatheController = AnimationController(
      duration: const Duration(milliseconds: 3500),
      vsync: this,
    )..repeat(reverse: true);
    _breatheAnimation = Tween<double>(begin: 0.0, end: 1.0).animate(
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

  void _loadSolarTimes() {
    try {
      final loc = context.read<HomeProvider>().currentHome?.location;
      if (loc == null) return;
      final tz = SolarUtils.timezoneFromLongitude(loc.longitude);
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
    } catch (_) {
      // Solar times unavailable — trigger time won't display.
    }
  }

  Map<RhythmMode, RhythmModeTransitionConfig> _initialTransitionConfigs() {
    final transitions = context.read<ServerSyncProvider>().modeTransitions;
    final configs = <RhythmMode, RhythmModeTransitionConfig>{};

    for (final transition in transitions) {
      if (_isSupportedTransition(transition)) {
        configs[transition.toMode] = transition;
      }
    }

    if (_isSupportedTransition(widget.transition)) {
      configs[widget.transition.toMode] = widget.transition;
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

  @override
  void dispose() {
    _breatheController.dispose();
    _flowController.dispose();
    super.dispose();
  }

  void _updateConfig(RhythmModeTransitionConfig updated) {
    setState(() {
      _transitionConfigs[updated.toMode] = updated;
      _selectedMode = updated.toMode;
    });
    context.read<ServerSyncProvider>().dispatchUpdateTransition(updated);
  }

  void _focusMode(RhythmMode mode) {
    if (_selectedMode == mode) return;
    HapticFeedback.selectionClick();
    setState(() => _selectedMode = mode);
  }

  void _setModeTrigger(RhythmMode mode, String event) {
    final current = _transitionConfigs[mode];
    if (current == null) return;
    final unchanged = current.trigger.isSolar && current.trigger.event == event;
    if (unchanged) {
      if (_selectedMode != mode) {
        setState(() => _selectedMode = mode);
      }
      return;
    }

    HapticFeedback.selectionClick();
    _updateConfig(
      current.copyWith(
        trigger: RhythmTransitionTrigger.solar(event),
      ),
    );
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
                    const SizedBox(height: 16),
                    _buildTriggerSelector(),
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

  // ──────────────────────────────────────────────────────────────────────────
  // Header
  // ──────────────────────────────────────────────────────────────────────────

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
              'Transition',
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

  // ──────────────────────────────────────────────────────────────────────────
  // Atmospheric hero card
  // ──────────────────────────────────────────────────────────────────────────

  Widget _buildHero() {
    final accentColor = _triggerColor(_config.trigger);
    final dayColor = widget.profileColors[RhythmMode.day] ??
        _fallbackModeColor(RhythmMode.day);
    final sleepColor = widget.profileColors[RhythmMode.sleep] ??
        _fallbackModeColor(RhythmMode.sleep);
    final fromColor = widget.profileColors[_config.fromMode] ??
        _fallbackModeColor(_config.fromMode);
    final toColor = widget.profileColors[_config.toMode] ??
        _fallbackModeColor(_config.toMode);

    return AnimatedBuilder(
      animation: _breatheAnimation,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(20, 28, 20, 24),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            GestureDetector(
              onTap: _showLabelDialog,
              child: Column(
                children: [
                  Text(
                    _config.label.isNotEmpty
                        ? _config.label
                        : '${_modeLabel(_config.fromMode)} to ${_modeLabel(_config.toMode)}',
                    textAlign: TextAlign.center,
                    style: TextStyle(
                      color:
                          CelestialColors.textPrimary.withValues(alpha: 0.85),
                      fontSize: 20,
                      fontWeight: FontWeight.w300,
                      letterSpacing: 1.0,
                    ),
                  ),
                  const SizedBox(height: 6),
                  Text(
                    'Drag Day and Sleep around the solar clock',
                    textAlign: TextAlign.center,
                    style: TextStyle(
                      color:
                          CelestialColors.textSecondary.withValues(alpha: 0.62),
                      fontSize: 12,
                      fontWeight: FontWeight.w500,
                      letterSpacing: 0.4,
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(height: 20),
            if (_solarClockData != null)
              _buildInteractiveTransitionFlow(
                dayColor: dayColor,
                sleepColor: sleepColor,
              )
            else
              _buildLegacyTransitionFlow(fromColor, toColor, accentColor),
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
    final solarClockData = _solarClockData;
    if (solarClockData == null) {
      return _buildLegacyTransitionFlow(
        widget.profileColors[_config.fromMode] ??
            _fallbackModeColor(_config.fromMode),
        widget.profileColors[_config.toMode] ??
            _fallbackModeColor(_config.toMode),
        _triggerColor(_config.trigger),
      );
    }

    return SizedBox(
      height: 360,
      child: AnimatedBuilder(
        animation: Listenable.merge([_flowAnimation, _breatheAnimation]),
        builder: (context, _) {
          return SolarClock(
            data: solarClockData,
            use24: MediaQuery.alwaysUse24HourFormatOf(context),
            showEventMarkers: true,
            showHourLabels: true,
            showLowerArc: true,
            horizonFactor: 0.56,
            radiusWidthFactor: 0.41,
            radiusHeightFactor: 0.68,
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

  Widget _buildFocusedTransitionSummary({
    required Color dayColor,
    required Color sleepColor,
  }) {
    final modeColor = _selectedMode == RhythmMode.day ? dayColor : sleepColor;
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
        const SizedBox(width: 10),
        Text(
          _shortTriggerLabel(_config.trigger).toUpperCase(),
          style: TextStyle(
            color: modeColor.withValues(alpha: 0.8),
            fontSize: 11,
            fontWeight: FontWeight.w700,
            letterSpacing: 1.2,
          ),
        ),
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

  Widget _buildLegacyTransitionFlow(
      Color fromColor, Color toColor, Color accentColor) {
    return SizedBox(
      height: 130,
      child: Stack(
        clipBehavior: Clip.none,
        children: [
          // Solar arc — positioned so endpoints align with orb circle centers
          Positioned(
            left: 34,
            right: 34,
            top: 0,
            bottom: 47,
            child: AnimatedBuilder(
              animation: Listenable.merge([_flowAnimation, _breatheAnimation]),
              builder: (context, _) {
                final isDawn = _config.fromMode == RhythmMode.sleep;
                final ref = isDawn ? _sunTimes?.sunrise : _sunTimes?.sunset;
                final triggerH = _triggerTimeHours();
                return CustomPaint(
                  painter: _TransitionArcPainter(
                    fromColor: fromColor,
                    toColor: toColor,
                    accentColor: accentColor,
                    triggerLabel: _shortTriggerLabel(_config.trigger),
                    triggerIconCodePoint:
                        _triggerIcon(_config.trigger).codePoint,
                    triggerTime: triggerH != null ? _fmtTime(triggerH) : null,
                    triggerPosition: _triggerArcPosition(),
                    flowProgress: _flowAnimation.value,
                    pulse: _breatheAnimation.value,
                    arcStartHour: ref != null ? ref - 2.0 : null,
                    arcEndHour: ref != null ? ref + 2.0 : null,
                    use24: MediaQuery.alwaysUse24HourFormatOf(context),
                  ),
                );
              },
            ),
          ),
          // From mode orb — left, circle center at arc left endpoint
          Positioned(
            left: 0,
            bottom: 0,
            child: _ModeOrb(
              mode: _config.fromMode,
              color: fromColor,
              onTap: () => _focusMode(_config.fromMode),
            ),
          ),
          // To mode orb — right, circle center at arc right endpoint
          Positioned(
            right: 0,
            bottom: 0,
            child: _ModeOrb(
              mode: _config.toMode,
              color: toColor,
              onTap: () => _focusMode(_config.toMode),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildClockOverlay({
    required SolarClockGeometry geometry,
    required Color dayColor,
    required Color sleepColor,
  }) {
    final handles = _handleSpecs(dayColor: dayColor, sleepColor: sleepColor);
    final handleByMode = {
      for (final handle in handles) handle.mode: handle,
    };

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
            _buildPositionedHandle(
              handle: handle,
              geometry: geometry,
            ),
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
    final config = _transitionConfigs[mode];
    if (config == null) return null;

    final anchors = _triggerAnchorsForMode(mode);
    if (anchors.isEmpty) return null;

    final active = anchors.firstWhere(
      (anchor) => anchor.event == config.trigger.event,
      orElse: () => anchors.first,
    );

    return _TransitionHandleSpec(
      mode: mode,
      event: active.event,
      label: _modeLabel(mode),
      hour: active.hour,
      color: color,
    );
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
    });
  }

  void _handleClockPanUpdate(Offset position, SolarClockGeometry geometry) {
    final mode = _dragMode;
    if (mode == null) return;

    final anchors = _triggerAnchorsForMode(mode);
    if (anchors.isEmpty) return;

    final next = anchors.reduce((best, candidate) {
      final bestCenter = _orbCenterForHour(best.hour, geometry);
      final candidateCenter = _orbCenterForHour(candidate.hour, geometry);
      final bestDistance = (position - bestCenter).distanceSquared;
      final candidateDistance = (position - candidateCenter).distanceSquared;
      return candidateDistance < bestDistance ? candidate : best;
    });

    _setModeTrigger(mode, next.event);
  }

  void _handleClockPanEnd() {
    if (_dragMode == null) return;
    setState(() => _dragMode = null);
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

  double _triggerArcPosition() {
    final isDawn = _config.fromMode == RhythmMode.sleep;
    final triggerHour = _triggerTimeHours();

    // If solar data available, position within a ~4h window around the
    // reference event (sunrise for dawn, sunset for dusk).
    if (triggerHour != null && _sunTimes != null) {
      final ref = isDawn ? _sunTimes!.sunrise : _sunTimes!.sunset;
      final t = (triggerHour - (ref - 2.0)) / 4.0;
      return t.clamp(0.05, 0.95);
    }

    // Fallback: fixed positions within the narrower arc.
    // Dawn transitions: astro → nautical → civil → sunrise (left to right)
    // Dusk transitions: sunset → civil → nautical → astro (left to right)
    if (isDawn) {
      return switch (_config.trigger.event) {
        'astronomical_twilight' => 0.20,
        'nautical_twilight' => 0.35,
        'civil_twilight' => 0.50,
        'sunrise' => 0.70,
        _ => 0.50,
      };
    } else {
      return switch (_config.trigger.event) {
        'sunset' => 0.30,
        'civil_twilight' => 0.50,
        'nautical_twilight' => 0.65,
        'astronomical_twilight' => 0.80,
        _ => 0.50,
      };
    }
  }

  // ──────────────────────────────────────────────────────────────────────────
  // Trigger — label with effective time + horizontal chips
  // ──────────────────────────────────────────────────────────────────────────

  double? _triggerTimeHours() {
    final event = _config.trigger.event;
    if (event == null || _config.trigger.kind != 'solar') return null;
    return _eventTimeHours(event);
  }

  double? _eventTimeHours(String event) {
    final isDawn = _config.fromMode == RhythmMode.sleep;
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

  Widget _buildTriggerSelector() {
    final isDawn = _config.fromMode == RhythmMode.sleep;
    // Chronological order matching the arc left → right
    // (event, label, subtitle, icon, color)
    final events = <(String, String, String?, IconData, Color)>[
      if (isDawn) ...[
        (
          'astronomical_twilight',
          'Astro',
          'Twilight',
          Icons.dark_mode_rounded,
          const Color(0xFF5C6BC0)
        ),
        (
          'nautical_twilight',
          'Nautical',
          'Twilight',
          Icons.nights_stay_rounded,
          const Color(0xFF7C4DFF)
        ),
        (
          'civil_twilight',
          'Civil',
          'Twilight',
          Icons.wb_twilight_rounded,
          const Color(0xFFFFB74D)
        ),
        (
          'sunrise',
          'Sunrise',
          null,
          Icons.wb_sunny_rounded,
          const Color(0xFFF9A825)
        ),
      ] else ...[
        (
          'sunset',
          'Sunset',
          null,
          Icons.wb_twilight_rounded,
          const Color(0xFFFF7043)
        ),
        (
          'civil_twilight',
          'Civil',
          'Twilight',
          Icons.wb_twilight_rounded,
          const Color(0xFFFFB74D)
        ),
        (
          'nautical_twilight',
          'Nautical',
          'Twilight',
          Icons.nights_stay_rounded,
          const Color(0xFF7C4DFF)
        ),
        (
          'astronomical_twilight',
          'Astro',
          'Twilight',
          Icons.dark_mode_rounded,
          const Color(0xFF5C6BC0)
        ),
      ],
    ];

    return Row(
      children: [
        for (int i = 0; i < events.length; i++) ...[
          if (i > 0) const SizedBox(width: 8),
          Expanded(
            child: Builder(builder: (_) {
              final (event, label, subtitle, icon, color) = events[i];
              final selected = _config.trigger.kind == 'solar' &&
                  _config.trigger.event == event;
              final timeH = _eventTimeHours(event);
              return _TriggerChip(
                label: label,
                subtitle: subtitle,
                time: selected && timeH != null ? _fmtTime(timeH) : null,
                icon: icon,
                color: color,
                selected: selected,
                onTap: () {
                  if (!selected) {
                    HapticFeedback.selectionClick();
                    _updateConfig(_config.copyWith(
                      trigger: RhythmTransitionTrigger.solar(event),
                    ));
                  }
                },
              );
            }),
          ),
        ],
      ],
    );
  }

  // ──────────────────────────────────────────────────────────────────────────
  // Duration — header + expandable slider
  // ──────────────────────────────────────────────────────────────────────────

  bool get _durationAuto => _config.duration.isAuto;

  Widget _buildDurationRow() {
    final accentColor = _triggerColor(_config.trigger);

    return Container(
      padding: const EdgeInsets.fromLTRB(20, 14, 16, 14),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(18),
      ),
      child: Column(
        children: [
          // Header row
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
              const Expanded(
                child: Text(
                  'Duration',
                  style: TextStyle(
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
                    // Switch to manual — default 30s
                    _updateConfig(_config.copyWith(
                        duration: const TransitionDuration.fixed(30000)));
                  } else {
                    _updateConfig(_config.copyWith(
                        duration: const TransitionDuration.auto()));
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
          // Expandable slider
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
                                enabledThumbRadius: 8),
                            overlayShape: const RoundSliderOverlayShape(
                                overlayRadius: 18),
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
                              Text('1s',
                                  style: TextStyle(
                                      color: CelestialColors.textSecondary
                                          .withValues(alpha: 0.4),
                                      fontSize: 11)),
                              Text('2m',
                                  style: TextStyle(
                                      color: CelestialColors.textSecondary
                                          .withValues(alpha: 0.4),
                                      fontSize: 11)),
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

  // ──────────────────────────────────────────────────────────────────────────
  // Label dialog
  // ──────────────────────────────────────────────────────────────────────────

  void _showLabelDialog() {
    final controller = TextEditingController(text: _config.label);
    showDialog(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: const Color(0xFF1C2128),
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(14),
        ),
        title: const Text(
          'Transition Name',
          style: TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 17,
            fontWeight: FontWeight.w600,
          ),
        ),
        content: TextField(
          controller: controller,
          style: const TextStyle(color: CelestialColors.textPrimary),
          cursorColor: CelestialColors.accentBlue,
          decoration: InputDecoration(
            hintText: 'e.g. Morning Wake',
            hintStyle: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.5),
            ),
            enabledBorder: UnderlineInputBorder(
              borderSide: BorderSide(
                color: CelestialColors.orbitRing.withValues(alpha: 0.5),
              ),
            ),
            focusedBorder: const UnderlineInputBorder(
              borderSide: BorderSide(color: CelestialColors.accentBlue),
            ),
          ),
          autofocus: true,
          textCapitalization: TextCapitalization.words,
          onSubmitted: (value) {
            _updateConfig(_config.copyWith(label: value.trim()));
            Navigator.pop(ctx);
          },
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx),
            child: Text(
              'Cancel',
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.7),
              ),
            ),
          ),
          TextButton(
            onPressed: () {
              _updateConfig(_config.copyWith(label: controller.text.trim()));
              Navigator.pop(ctx);
            },
            child: const Text(
              'Save',
              style: TextStyle(
                color: CelestialColors.accentBlue,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Interactive solar-clock handle models
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

class _TransitionHandleSpec {
  final RhythmMode mode;
  final String event;
  final String label;
  final double hour;
  final Color color;

  const _TransitionHandleSpec({
    required this.mode,
    required this.event,
    required this.label,
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

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Mode orb — glowing circular mode indicator, tap to toggle
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

class _ModeOrb extends StatelessWidget {
  final RhythmMode mode;
  final Color color;
  final VoidCallback onTap;

  const _ModeOrb({
    required this.mode,
    required this.color,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onTap,
      child: SizedBox(
        width: 68,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Container(
              width: 50,
              height: 50,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: color.withValues(alpha: 0.12),
                border: Border.all(
                  color: color.withValues(alpha: 0.35),
                  width: 1.5,
                ),
                boxShadow: [
                  BoxShadow(
                    color: color.withValues(alpha: 0.2),
                    blurRadius: 16,
                  ),
                ],
              ),
              child: Icon(_modeIcon(mode), size: 22, color: color),
            ),
            const SizedBox(height: 8),
            Text(
              _modeLabel(mode),
              style: TextStyle(
                color: color.withValues(alpha: 0.9),
                fontSize: 13,
                fontWeight: FontWeight.w600,
                letterSpacing: 0.3,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Trigger chip — inline selectable trigger with animated state
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

class _TriggerChip extends StatelessWidget {
  final String label;
  final String? subtitle;
  final String? time;
  final IconData icon;
  final Color color;
  final bool selected;
  final VoidCallback onTap;

  const _TriggerChip({
    required this.label,
    this.subtitle,
    this.time,
    required this.icon,
    required this.color,
    required this.selected,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final dim = CelestialColors.textSecondary.withValues(alpha: 0.4);

    return GestureDetector(
      onTap: onTap,
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 200),
        curve: Curves.easeOut,
        padding: const EdgeInsets.symmetric(vertical: 10),
        decoration: BoxDecoration(
          color: selected ? color.withValues(alpha: 0.12) : Colors.transparent,
          borderRadius: BorderRadius.circular(14),
          border: Border.all(
            color: selected
                ? color.withValues(alpha: 0.35)
                : CelestialColors.orbitRing.withValues(alpha: 0.2),
            width: selected ? 1.5 : 0.5,
          ),
          boxShadow: selected
              ? [
                  BoxShadow(
                    color: color.withValues(alpha: 0.12),
                    blurRadius: 12,
                  )
                ]
              : null,
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              icon,
              size: 20,
              color: selected ? color : dim,
            ),
            const SizedBox(height: 4),
            Text(
              label,
              style: TextStyle(
                color: selected ? color : dim,
                fontSize: 11,
                fontWeight: selected ? FontWeight.w600 : FontWeight.w400,
                letterSpacing: 0.3,
              ),
            ),
            if (subtitle != null)
              Padding(
                padding: const EdgeInsets.only(top: 1),
                child: Text(
                  subtitle!,
                  style: TextStyle(
                    color: selected
                        ? color.withValues(alpha: 0.5)
                        : CelestialColors.textSecondary.withValues(alpha: 0.25),
                    fontSize: 9,
                    fontWeight: FontWeight.w400,
                    letterSpacing: 0.2,
                  ),
                ),
              ),
            if (time != null)
              Padding(
                padding: const EdgeInsets.only(top: 4),
                child: Text(
                  time!,
                  style: TextStyle(
                    color: color.withValues(alpha: 0.7),
                    fontSize: 10,
                    fontWeight: FontWeight.w500,
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Transition arc painter — solar-path arc with trigger marker
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

class _TransitionArcPainter extends CustomPainter {
  final Color fromColor;
  final Color toColor;
  final Color accentColor;
  final String triggerLabel;
  final int triggerIconCodePoint;
  final String? triggerTime;
  final double triggerPosition; // 0=left endpoint, 1=right endpoint
  final double flowProgress;
  final double pulse;
  final double? arcStartHour; // null = no time ticks
  final double? arcEndHour;
  final bool use24;

  _TransitionArcPainter({
    required this.fromColor,
    required this.toColor,
    required this.accentColor,
    required this.triggerLabel,
    required this.triggerIconCodePoint,
    this.triggerTime,
    required this.triggerPosition,
    required this.flowProgress,
    required this.pulse,
    this.arcStartHour,
    this.arcEndHour,
    this.use24 = false,
  });

  // Arc constants — 120° sweep for a ~3h solar window
  static const _startAngle = 7 * math.pi / 6;
  static const _sweepAngle = 2 * math.pi / 3;
  static const _peakPad = 5.0;

  @override
  void paint(Canvas canvas, Size size) {
    final w = size.width;
    final h = size.height;
    final cx = w / 2;

    // Ellipse geometry: 120° arc from (0, h) through (w/2, peakPad) to (w, h)
    final semiA = w / math.sqrt(3);
    final semiB = 2.0 * (h - _peakPad);
    final arcCy = 2.0 * h - _peakPad;

    final arcRect = Rect.fromCenter(
      center: Offset(cx, arcCy),
      width: semiA * 2,
      height: semiB * 2,
    );

    Offset arcAt(double t) {
      final angle = _startAngle + _sweepAngle * t;
      return Offset(
        cx + semiA * math.cos(angle),
        arcCy + semiB * math.sin(angle),
      );
    }

    // ── 1. Horizon line ──────────────────────────────
    canvas.drawLine(
      Offset(0, h),
      Offset(w, h),
      Paint()
        ..shader = LinearGradient(
          colors: [
            Colors.white.withValues(alpha: 0.0),
            Colors.white.withValues(alpha: 0.08),
            Colors.white.withValues(alpha: 0.08),
            Colors.white.withValues(alpha: 0.0),
          ],
          stops: const [0.0, 0.15, 0.85, 1.0],
        ).createShader(Rect.fromLTWH(0, h - 1, w, 2))
        ..strokeWidth = 1.0,
    );

    // ── 2. Arc glow ──────────────────────────────────
    canvas.drawArc(
      arcRect,
      _startAngle,
      _sweepAngle,
      false,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 14.0
        ..shader = LinearGradient(
          colors: [
            fromColor.withValues(alpha: 0.10),
            Color.lerp(fromColor, toColor, 0.5)!.withValues(alpha: 0.16),
            toColor.withValues(alpha: 0.10),
          ],
        ).createShader(arcRect)
        ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 8),
    );

    // ── 3. Segmented gradient arc ────────────────────
    const segments = 40;
    const segSweep = _sweepAngle / segments;
    final arcPaint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 3.0
      ..strokeCap = StrokeCap.butt;

    for (int i = 0; i < segments; i++) {
      final t = (i + 0.5) / segments;
      final segStart = _startAngle + i * segSweep;

      Color segColor;
      double segAlpha;
      if (t < triggerPosition - 0.08) {
        segColor = fromColor;
        segAlpha = 0.40 + 0.30 * (t / triggerPosition);
      } else if (t > triggerPosition + 0.08) {
        segColor = toColor;
        segAlpha = 0.40 + 0.30 * ((1 - t) / (1 - triggerPosition));
      } else {
        final blend = ((t - triggerPosition + 0.08) / 0.16).clamp(0.0, 1.0);
        segColor = Color.lerp(fromColor, toColor, blend)!;
        segAlpha = 0.70;
      }

      arcPaint.color = segColor.withValues(alpha: segAlpha);
      canvas.drawArc(arcRect, segStart, segSweep + 0.01, false, arcPaint);
    }

    // ── 3b. Time ticks ──────────────────────────────
    if (arcStartHour != null && arcEndHour != null) {
      final span = arcEndHour! - arcStartHour!;
      final interval = span > 3 ? 1.0 : 0.5;
      final firstTick = (arcStartHour! / interval).ceil() * interval;

      final tickPaint = Paint()
        ..strokeWidth = 1.0
        ..strokeCap = StrokeCap.round;

      for (var tickH = firstTick; tickH < arcEndHour!; tickH += interval) {
        final t = (tickH - arcStartHour!) / span;
        if (t < 0.06 || t > 0.94) continue;

        // Skip if too close to the trigger marker
        if ((t - triggerPosition).abs() < 0.08) continue;

        final pt = arcAt(t);

        // Outward normal (away from ellipse center)
        final ox = pt.dx - cx;
        final oy = pt.dy - arcCy;
        final oLen = math.sqrt(ox * ox + oy * oy);
        final nx = ox / oLen;
        final ny = oy / oLen;

        // Tick mark
        tickPaint.color = Colors.white.withValues(alpha: 0.12);
        canvas.drawLine(
          Offset(pt.dx + nx * 5, pt.dy + ny * 5),
          Offset(pt.dx + nx * 11, pt.dy + ny * 11),
          tickPaint,
        );

        // Time label
        final hr = tickH.floor() % 24;
        final mn = ((tickH - tickH.floor()) * 60).round();
        String label;
        if (use24) {
          label =
              '${hr.toString().padLeft(2, '0')}:${mn.toString().padLeft(2, '0')}';
        } else {
          final p = hr < 12 ? 'a' : 'p';
          final h12 = hr == 0 ? 12 : (hr > 12 ? hr - 12 : hr);
          label =
              mn == 0 ? '$h12$p' : '$h12:${mn.toString().padLeft(2, '0')}$p';
        }

        final tp = TextPainter(
          text: TextSpan(
            text: label,
            style: TextStyle(
              color: Colors.white.withValues(alpha: 0.18),
              fontSize: 8,
              fontWeight: FontWeight.w500,
              letterSpacing: 0.3,
            ),
          ),
          textDirection: TextDirection.ltr,
        )..layout();

        tp.paint(
          canvas,
          Offset(
            (pt.dx + nx * 18 - tp.width / 2).clamp(0, w - tp.width),
            pt.dy + ny * 18 - tp.height / 2,
          ),
        );
      }
    }

    // ── 4. Trigger marker ────────────────────────────
    final trigPt = arcAt(triggerPosition);

    // Pulsing outer glow
    canvas.drawCircle(
      trigPt,
      12 + pulse * 4,
      Paint()
        ..color = accentColor.withValues(alpha: 0.12 + pulse * 0.08)
        ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 8),
    );

    // Inner glow
    canvas.drawCircle(
      trigPt,
      6,
      Paint()
        ..color = accentColor.withValues(alpha: 0.30)
        ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 3),
    );

    // Core dot
    canvas.drawCircle(trigPt, 3.5, Paint()..color = accentColor);

    // Inward direction — toward ellipse center
    final dirX = cx - trigPt.dx;
    final dirY = arcCy - trigPt.dy;
    final dirLen = math.sqrt(dirX * dirX + dirY * dirY);
    if (dirLen > 0) {
      final nx = dirX / dirLen;
      final ny = dirY / dirLen;

      // Tick line
      canvas.drawLine(
        Offset(trigPt.dx + nx * 6, trigPt.dy + ny * 6),
        Offset(trigPt.dx + nx * 14, trigPt.dy + ny * 14),
        Paint()
          ..color = accentColor.withValues(alpha: 0.25)
          ..strokeWidth = 1.5
          ..strokeCap = StrokeCap.round,
      );

      // Trigger icon inside the arc
      final iconCenter = Offset(
        trigPt.dx + nx * 24,
        trigPt.dy + ny * 24,
      );
      final iconTp = TextPainter(
        text: TextSpan(
          text: String.fromCharCode(triggerIconCodePoint),
          style: TextStyle(
            fontFamily: 'MaterialIcons',
            fontSize: 14,
            color: accentColor.withValues(alpha: 0.70),
          ),
        ),
        textDirection: TextDirection.ltr,
      )..layout();
      iconTp.paint(
        canvas,
        iconCenter - Offset(iconTp.width / 2, iconTp.height / 2),
      );

      // Label below icon
      final labelTp = TextPainter(
        text: TextSpan(
          text: triggerLabel,
          style: TextStyle(
            color: accentColor.withValues(alpha: 0.45),
            fontSize: 9,
            fontWeight: FontWeight.w600,
            letterSpacing: 0.3,
          ),
        ),
        textDirection: TextDirection.ltr,
      )..layout();
      labelTp.paint(
        canvas,
        Offset(
          (iconCenter.dx - labelTp.width / 2).clamp(0, w - labelTp.width),
          iconCenter.dy + 9,
        ),
      );

      // Time below label
      if (triggerTime != null) {
        final timeTp = TextPainter(
          text: TextSpan(
            text: triggerTime!,
            style: TextStyle(
              color: accentColor.withValues(alpha: 0.6),
              fontSize: 11,
              fontWeight: FontWeight.w500,
              letterSpacing: 0.3,
            ),
          ),
          textDirection: TextDirection.ltr,
        )..layout();
        timeTp.paint(
          canvas,
          Offset(
            (iconCenter.dx - timeTp.width / 2).clamp(0, w - timeTp.width),
            iconCenter.dy + 21,
          ),
        );
      }
    }

    // ── 5. Traveling light ───────────────────────────
    final dotOpacity = _fadeCurve(flowProgress);
    if (dotOpacity > 0) {
      final dotPt = arcAt(flowProgress);
      final dotColor = Color.lerp(fromColor, toColor, flowProgress)!;

      canvas.drawCircle(
        dotPt,
        7,
        Paint()
          ..color = dotColor.withValues(alpha: 0.25 * dotOpacity)
          ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 5),
      );
      canvas.drawCircle(
        dotPt,
        2.5,
        Paint()..color = Color.fromRGBO(255, 255, 255, 0.85 * dotOpacity),
      );
    }
  }

  double _fadeCurve(double t) {
    if (t < 0.08) return t / 0.08;
    if (t > 0.92) return (1.0 - t) / 0.08;
    return 1.0;
  }

  @override
  bool shouldRepaint(covariant _TransitionArcPainter old) =>
      old.flowProgress != flowProgress ||
      old.pulse != pulse ||
      old.fromColor != fromColor ||
      old.toColor != toColor ||
      old.triggerPosition != triggerPosition ||
      old.accentColor != accentColor;
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Helpers
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

String _formatDuration(int ms) {
  final seconds = ms ~/ 1000;
  if (seconds < 60) return '${seconds}s';
  final minutes = seconds ~/ 60;
  final remainingSeconds = seconds % 60;
  if (remainingSeconds == 0) return '$minutes min';
  return '${minutes}m ${remainingSeconds}s';
}

String _shortTriggerLabel(RhythmTransitionTrigger trigger) {
  if (trigger.kind == 'manual') return 'Manual';
  return switch (trigger.event) {
    'sunrise' => 'Sunrise',
    'civil_twilight' => 'Civil',
    'nautical_twilight' => 'Nautical',
    'astronomical_twilight' => 'Astro',
    'sunset' => 'Sunset',
    _ => trigger.event?.replaceAll('_', ' ') ?? '?',
  };
}

IconData _triggerIcon(RhythmTransitionTrigger trigger) {
  if (trigger.kind == 'manual') return Icons.schedule;
  return switch (trigger.event) {
    'sunrise' => Icons.wb_sunny_rounded,
    'civil_twilight' => Icons.wb_twilight_rounded,
    'nautical_twilight' => Icons.nights_stay_rounded,
    'astronomical_twilight' => Icons.dark_mode_rounded,
    'sunset' => Icons.wb_twilight_rounded,
    _ => Icons.schedule,
  };
}

Color _triggerColor(RhythmTransitionTrigger trigger) {
  if (trigger.kind == 'manual') return CelestialColors.accentBlue;
  return switch (trigger.event) {
    'sunrise' => const Color(0xFFF9A825),
    'civil_twilight' => const Color(0xFFFFB74D),
    'nautical_twilight' => const Color(0xFF7C4DFF),
    'astronomical_twilight' => const Color(0xFF5C6BC0),
    'sunset' => const Color(0xFFFF7043),
    _ => CelestialColors.accentBlue,
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
      RhythmMode.day => 'Day',
      RhythmMode.sleep => 'Sleep',
    };
