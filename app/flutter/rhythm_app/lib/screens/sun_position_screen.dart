/// Celestial Sun Position — sacred solar visualization.
///
/// Full-screen immersive experience showing the sun's living movement across
/// the sky. Dynamic sky gradients shift with solar events, a glowing sun orb
/// traces its arc, and poetic threshold moments mark sunrise and sunset.
///
/// The sun is the protagonist. Time is the stage. The user is the witness.
library;

import 'dart:async';
import 'dart:math' as math;
import 'dart:ui';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';

import '../models/config_model.dart';
import '../providers/home_provider.dart';
import '../widgets/solar_clock/solar_clock_exports.dart';
import 'location_settings_screen.dart';

// =============================================================================
// Data
// =============================================================================

class _SolarData {
  final double latitude;
  final double longitude;
  final String timezone;
  final String? cityName;
  final SunTimesDto sunTimes;
  final TwilightTimesDto twilightTimes;

  const _SolarData({
    required this.latitude,
    required this.longitude,
    required this.timezone,
    this.cityName,
    required this.sunTimes,
    required this.twilightTimes,
  });

  bool get isPolarDay => sunTimes.dayLength >= 24.0;
  bool get isPolarNight => sunTimes.dayLength <= 0.0;

  SolarClockData get solarClockData => SolarClockData(
        sunTimes: sunTimes,
        twilightTimes: twilightTimes,
      );
}

// =============================================================================
// Sky gradient keyframe
// =============================================================================

class _SkyKey {
  final double hour;
  final Color top; // zenith
  final Color mid; // mid-sky
  final Color low; // near horizon
  final Color base; // below horizon

  const _SkyKey(this.hour, this.top, this.mid, this.low, this.base);

  static _SkyKey lerp(_SkyKey a, _SkyKey b, double t) {
    if (t <= 0) return a;
    if (t >= 1) return b;
    return _SkyKey(
      lerpDouble(a.hour, b.hour, t)!,
      Color.lerp(a.top, b.top, t)!,
      Color.lerp(a.mid, b.mid, t)!,
      Color.lerp(a.low, b.low, t)!,
      Color.lerp(a.base, b.base, t)!,
    );
  }
}

// =============================================================================
// Screen
// =============================================================================

class SunPositionScreen extends StatefulWidget {
  const SunPositionScreen({super.key});

  static Future<void> show(BuildContext context) {
    return Navigator.of(context).push(
      PageRouteBuilder(
        opaque: true,
        pageBuilder: (_, __, ___) => const SunPositionScreen(),
        transitionsBuilder: (_, animation, __, child) {
          return FadeTransition(
            opacity: CurvedAnimation(parent: animation, curve: Curves.easeOut),
            child: child,
          );
        },
        transitionDuration: const Duration(milliseconds: 600),
        reverseTransitionDuration: const Duration(milliseconds: 400),
      ),
    );
  }

  @override
  State<SunPositionScreen> createState() => _SunPositionScreenState();
}

class _SunPositionScreenState extends State<SunPositionScreen>
    with TickerProviderStateMixin {
  _SolarData? _data;
  CurveDataDto? _curveData;
  bool _noLocation = false;

  late final AnimationController _pulseCtl;
  late final Animation<double> _pulse;
  Timer? _tick;

  // Arc dragging
  double? _scrubHour;
  double? _prevScrubHour;
  bool _isDragging = false;

  // Cached keyframes (rebuilt when data changes)
  List<_SkyKey>? _skyKeys;
  _SolarData? _skyKeysSource;

  double get _now {
    final t = DateTime.now();
    return t.hour + t.minute / 60.0 + t.second / 3600.0;
  }

  double get _hour => _scrubHour ?? _now;
  bool get _isLive => _scrubHour == null;

  @override
  void initState() {
    super.initState();
    _pulseCtl = AnimationController(
      duration: const Duration(milliseconds: 4000),
      vsync: this,
    )..repeat(reverse: true);
    _pulse = Tween(begin: 0.0, end: 1.0).animate(
      CurvedAnimation(parent: _pulseCtl, curve: Curves.easeInOut),
    );
    _loadData();
    _tick = Timer.periodic(const Duration(seconds: 30), (_) {
      if (_isLive && mounted) setState(() {});
    });
  }

  @override
  void dispose() {
    _pulseCtl.dispose();
    _tick?.cancel();
    super.dispose();
  }

  // ---------------------------------------------------------------------------
  // Data loading
  // ---------------------------------------------------------------------------

  void _loadData() {
    final loc = context.read<HomeProvider>().currentHome?.location;
    if (loc == null) {
      setState(() => _noLocation = true);
      return;
    }
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

    // Generate curve data for arc coloring
    CurveDataDto? curve;
    try {
      final config = context.read<ConfigModel>().config;
      curve = generateCurveDataHighResWithSunTimes(
        config: config,
        latitude: loc.latitude,
        longitude: loc.longitude,
        year: now.year,
        month: now.month,
        day: now.day,
        timezone: tz,
        samplesPerHour: 4,
      );
    } catch (_) {
      curve = null;
    }

    setState(() {
      _noLocation = false;
      _curveData = curve;
      _data = _SolarData(
        latitude: loc.latitude,
        longitude: loc.longitude,
        timezone: tz,
        cityName: loc.cityName,
        sunTimes: sunTimes,
        twilightTimes: getTwilightTimes(
          latitude: loc.latitude,
          longitude: loc.longitude,
          year: now.year,
          month: now.month,
          day: now.day,
          timezone: tz,
        ),
      );
    });
  }

  // ---------------------------------------------------------------------------
  // Sky gradient
  // ---------------------------------------------------------------------------

  List<_SkyKey> _keys(_SolarData d) {
    if (identical(_skyKeysSource, d) && _skyKeys != null) return _skyKeys!;
    _skyKeysSource = d;

    if (d.isPolarNight) {
      _skyKeys = [
        const _SkyKey(0, Color(0xFF040810), Color(0xFF060C16),
            Color(0xFF0A1020), Color(0xFF0C1225)),
        const _SkyKey(24, Color(0xFF040810), Color(0xFF060C16),
            Color(0xFF0A1020), Color(0xFF0C1225)),
      ];
      return _skyKeys!;
    }

    if (d.isPolarDay) {
      _skyKeys = [
        const _SkyKey(0, Color(0xFF182858), Color(0xFF2858A0),
            Color(0xFF4080C0), Color(0xFF60A0D8)),
        const _SkyKey(12, Color(0xFF1850A0), Color(0xFF2878C0),
            Color(0xFF4898D4), Color(0xFF70B0E0)),
        const _SkyKey(24, Color(0xFF182858), Color(0xFF2858A0),
            Color(0xFF4080C0), Color(0xFF60A0D8)),
      ];
      return _skyKeys!;
    }

    final sr = d.sunTimes.sunrise;
    final ss = d.sunTimes.sunset;
    final noon = d.sunTimes.solarNoon;
    final cd = d.twilightTimes.dawn.civil ?? (sr - 0.5);
    final nd = d.twilightTimes.dawn.nautical ?? (sr - 1.0);
    final ad = d.twilightTimes.dawn.astronomical ?? (sr - 1.5);
    final cdu = d.twilightTimes.dusk.civil ?? (ss + 0.5);
    final ndu = d.twilightTimes.dusk.nautical ?? (ss + 1.0);
    final adu = d.twilightTimes.dusk.astronomical ?? (ss + 1.5);

    _skyKeys = [
      // Night
      const _SkyKey(0, Color(0xFF040810), Color(0xFF060C16), Color(0xFF0A1020),
          Color(0xFF0C1225)),
      // Astronomical dawn
      _SkyKey(ad, const Color(0xFF060A16), const Color(0xFF0A1024),
          const Color(0xFF0E1530), const Color(0xFF101835)),
      // Nautical dawn
      _SkyKey(nd, const Color(0xFF0C0E22), const Color(0xFF141838),
          const Color(0xFF1C2248), const Color(0xFF222850)),
      // Civil dawn
      _SkyKey(cd, const Color(0xFF18123A), const Color(0xFF2A2058),
          const Color(0xFF402D68), const Color(0xFF503570)),
      // Just before sunrise
      _SkyKey(sr - 0.25, const Color(0xFF201540), const Color(0xFF4A3060),
          const Color(0xFF7A4565), const Color(0xFFA05A60)),
      // Sunrise
      _SkyKey(sr, const Color(0xFF1A1040), const Color(0xFF6B3A5A),
          const Color(0xFFD07848), const Color(0xFFF0A838)),
      // Post-sunrise
      _SkyKey(sr + 0.75, const Color(0xFF2A4878), const Color(0xFF5070A0),
          const Color(0xFF90A0C0), const Color(0xFFD0C0A0)),
      // Morning
      _SkyKey(sr + 2.0, const Color(0xFF2060A8), const Color(0xFF3888C8),
          const Color(0xFF60AAD8), const Color(0xFF88C0E0)),
      // Noon
      _SkyKey(noon, const Color(0xFF1850A0), const Color(0xFF2878C0),
          const Color(0xFF4898D4), const Color(0xFF70B0E0)),
      // Afternoon
      _SkyKey(ss - 2.0, const Color(0xFF2060A8), const Color(0xFF3888C8),
          const Color(0xFF60AAD8), const Color(0xFF88C0D8)),
      // Pre-sunset
      _SkyKey(ss - 0.75, const Color(0xFF2A4878), const Color(0xFF5070A0),
          const Color(0xFF90A0C0), const Color(0xFFD0BCA0)),
      // Sunset
      _SkyKey(ss, const Color(0xFF1A1040), const Color(0xFF6B3A5A),
          const Color(0xFFD07040), const Color(0xFFE89838)),
      // Just after sunset
      _SkyKey(ss + 0.25, const Color(0xFF201540), const Color(0xFF4A3060),
          const Color(0xFF7A4565), const Color(0xFFA05A60)),
      // Civil dusk
      _SkyKey(cdu, const Color(0xFF18123A), const Color(0xFF2A2058),
          const Color(0xFF402D68), const Color(0xFF503570)),
      // Nautical dusk
      _SkyKey(ndu, const Color(0xFF0C0E22), const Color(0xFF141838),
          const Color(0xFF1C2248), const Color(0xFF222850)),
      // Astronomical dusk
      _SkyKey(adu, const Color(0xFF060A16), const Color(0xFF0A1024),
          const Color(0xFF0E1530), const Color(0xFF101835)),
      // Night
      const _SkyKey(24, Color(0xFF040810), Color(0xFF060C16), Color(0xFF0A1020),
          Color(0xFF0C1225)),
    ];
    return _skyKeys!;
  }

  _SkyKey _sky(double hour, _SolarData data) {
    final k = _keys(data);
    for (int i = 0; i < k.length - 1; i++) {
      if (hour <= k[i + 1].hour) {
        final range = k[i + 1].hour - k[i].hour;
        if (range <= 0) return k[i];
        return _SkyKey.lerp(
            k[i], k[i + 1], ((hour - k[i].hour) / range).clamp(0.0, 1.0));
      }
    }
    return k.last;
  }

  // ---------------------------------------------------------------------------
  // Angle math
  // ---------------------------------------------------------------------------

  String _fmtShort(double h) {
    return SolarUtils.formatHour(
      h,
      use24: MediaQuery.alwaysUse24HourFormatOf(context),
    );
  }

  void _arcDragStart(
      Offset pos, Offset arcCenter, double arcRadius, double solarNoon) {
    final geometry = SolarClockGeometry(
      center: arcCenter,
      radius: arcRadius,
      solarNoon: solarNoon,
    );
    final distToArc = ((pos - arcCenter).distance - arcRadius).abs();
    final sunPos = geometry.positionForHour(_hour);
    final distToSun = (pos - sunPos).distance;

    // Grab sun directly (generous zone) or touch near the arc
    if (distToSun < 80 || distToArc < 50) {
      _isDragging = true;
      _prevScrubHour = _hour;
      final hour = geometry.hourFromPosition(pos);
      setState(() => _scrubHour = hour);
    }
  }

  void _arcDragUpdate(Offset pos, Offset arcCenter, double solarNoon) {
    if (!_isDragging) return;
    final d = _data;
    if (d == null) return;
    final prev = _prevScrubHour ?? _hour;
    final geometry = SolarClockGeometry(
      center: arcCenter,
      radius: 0,
      solarNoon: solarNoon,
    );
    final h = geometry.hourFromPosition(pos);

    // Haptic on crossing solar events (ignore large jumps from wrapping)
    final hDiff = (h - prev).abs();
    if (hDiff < 2.0) {
      bool crossed(double event) =>
          (prev < event && h >= event) || (prev > event && h <= event);

      final tw = d.twilightTimes;
      if (crossed(d.sunTimes.sunrise) || crossed(d.sunTimes.sunset)) {
        HapticFeedback.mediumImpact();
      } else if (crossed(d.sunTimes.solarNoon)) {
        HapticFeedback.mediumImpact();
      } else if ((tw.dawn.civil != null && crossed(tw.dawn.civil!)) ||
          (tw.dusk.civil != null && crossed(tw.dusk.civil!))) {
        HapticFeedback.lightImpact();
      } else if ((tw.dawn.nautical != null && crossed(tw.dawn.nautical!)) ||
          (tw.dusk.nautical != null && crossed(tw.dusk.nautical!))) {
        HapticFeedback.lightImpact();
      } else if ((tw.dawn.astronomical != null &&
              crossed(tw.dawn.astronomical!)) ||
          (tw.dusk.astronomical != null && crossed(tw.dusk.astronomical!))) {
        HapticFeedback.selectionClick();
      }
    }

    _prevScrubHour = h;
    setState(() => _scrubHour = h);
  }

  void _arcDragEnd() {
    _isDragging = false;
    _prevScrubHour = null;
  }

  void _goLive() {
    HapticFeedback.lightImpact();
    setState(() => _scrubHour = null);
  }

  // ---------------------------------------------------------------------------
  // Build
  // ---------------------------------------------------------------------------

  @override
  Widget build(BuildContext context) {
    if (_noLocation) return _empty();
    final d = _data;
    if (d == null) return _loading();

    final sz = MediaQuery.sizeOf(context);
    final pad = MediaQuery.paddingOf(context);
    final hour = _hour;
    final sky = _sky(hour, d);

    // Layout
    final horizonY = sz.height * 0.52;
    final arcCenter = Offset(sz.width / 2, horizonY);
    final arcRadius = math.min(sz.width * 0.42, horizonY * 0.55);

    // Threshold proximity (0 = far, 1 = exactly at event)
    final minSR = (d.sunTimes.sunrise - hour).abs() * 60;
    final minSS = (d.sunTimes.sunset - hour).abs() * 60;
    final nearSR = !d.isPolarDay && !d.isPolarNight && minSR <= 30;
    final nearSS = !d.isPolarDay && !d.isPolarNight && minSS <= 30;
    final thr = nearSR
        ? (1.0 - minSR / 30.0).clamp(0.0, 1.0)
        : nearSS
            ? (1.0 - minSS / 30.0).clamp(0.0, 1.0)
            : 0.0;

    return AnnotatedRegion<SystemUiOverlayStyle>(
      value: SystemUiOverlayStyle.light,
      child: Scaffold(
        backgroundColor: sky.top,
        body: AnimatedBuilder(
          animation: _pulse,
          builder: (context, _) {
            return Stack(
              fit: StackFit.expand,
              children: [
                // Sky gradient
                DecoratedBox(
                  decoration: BoxDecoration(
                    gradient: LinearGradient(
                      begin: Alignment.topCenter,
                      end: Alignment.bottomCenter,
                      colors: [sky.top, sky.mid, sky.low, sky.base],
                      stops: const [0.0, 0.35, 0.62, 1.0],
                    ),
                  ),
                ),

                // Celestial elements
                CustomPaint(
                  painter: _CelestialPainter(
                    data: d,
                    hour: hour,
                    pulse: _pulse.value,
                    horizonY: horizonY,
                    center: arcCenter,
                    radius: arcRadius,
                    thresholdT: thr,
                    nearSunrise: nearSR,
                    use24: MediaQuery.alwaysUse24HourFormatOf(context),
                    curveData: _curveData == null
                        ? null
                        : SolarCurveSamples.fromCurveDataDto(_curveData!),
                  ),
                  size: Size.infinite,
                ),

                // Arc drag — grab the sun or touch the arc to scrub through time
                GestureDetector(
                  behavior: HitTestBehavior.translucent,
                  onPanStart: (e) => _arcDragStart(e.localPosition, arcCenter,
                      arcRadius, d.sunTimes.solarNoon),
                  onPanUpdate: (e) => _arcDragUpdate(
                      e.localPosition, arcCenter, d.sunTimes.solarNoon),
                  onPanEnd: (_) => _arcDragEnd(),
                  child: const SizedBox.expand(),
                ),

                // Close + NOW reset
                Positioned(
                  top: pad.top + 12,
                  left: 20,
                  right: 20,
                  child: Row(
                    children: [
                      GestureDetector(
                        onTap: _isLive ? null : _goLive,
                        child: Container(
                          padding: const EdgeInsets.symmetric(
                              horizontal: 14, vertical: 8),
                          decoration: BoxDecoration(
                            borderRadius: BorderRadius.circular(18),
                            color: Colors.black
                                .withValues(alpha: _isLive ? 0.2 : 0.45),
                          ),
                          child: Text(_isLive ? 'LIVE' : 'NOW',
                              style: TextStyle(
                                color: Colors.white
                                    .withValues(alpha: _isLive ? 0.5 : 1.0),
                                fontSize: 12,
                                fontWeight: FontWeight.w600,
                                letterSpacing: 2.5,
                              )),
                        ),
                      ),
                      const Spacer(),
                      GestureDetector(
                        onTap: () => Navigator.of(context).pop(),
                        child: Container(
                          width: 36,
                          height: 36,
                          decoration: BoxDecoration(
                            shape: BoxShape.circle,
                            color: Colors.black.withValues(alpha: 0.35),
                          ),
                          child: const Icon(Icons.close_rounded,
                              color: Colors.white, size: 18),
                        ),
                      ),
                    ],
                  ),
                ),

                // Next solar event — top of screen
                if (!d.isPolarDay && !d.isPolarNight)
                  Positioned(
                    left: 0,
                    right: 0,
                    top: pad.top + 48,
                    child: _nextEventWidget(d, hour),
                  ),

                // Current time — floats inside the arc
                Positioned(
                  left: 0,
                  right: 0,
                  top: horizonY - arcRadius * 0.5,
                  child: IgnorePointer(child: _currentTimeWidget(hour)),
                ),

                // Sunrise / Sunset + day info — bottom
                if (!d.isPolarDay && !d.isPolarNight)
                  Positioned(
                    left: 0,
                    right: 0,
                    bottom: pad.bottom + 24,
                    child: _sunTimesPanel(d),
                  ),

                // Polar label + day info
                if (d.isPolarDay || d.isPolarNight)
                  Positioned(
                    left: 0,
                    right: 0,
                    bottom: pad.bottom + 24,
                    child: Container(
                      margin: const EdgeInsets.symmetric(horizontal: 24),
                      padding: const EdgeInsets.symmetric(
                          horizontal: 16, vertical: 16),
                      decoration: BoxDecoration(
                        borderRadius: BorderRadius.circular(20),
                        color: Colors.black.withValues(alpha: 0.35),
                      ),
                      child: Column(
                        children: [
                          Text(
                            d.isPolarDay ? 'Midnight Sun' : 'Polar Night',
                            textAlign: TextAlign.center,
                            style: const TextStyle(
                              color: Colors.white,
                              fontSize: 15,
                              fontWeight: FontWeight.w400,
                              letterSpacing: 2.0,
                            ),
                          ),
                          const SizedBox(height: 12),
                          _dayInfo(d),
                        ],
                      ),
                    ),
                  ),
              ],
            );
          },
        ),
      ),
    );
  }

  // ---------------------------------------------------------------------------
  // Sub-widgets
  // ---------------------------------------------------------------------------

  Widget _sunTimesPanel(_SolarData d) {
    const sunriseColor = Color(0xFFF0A830);
    const sunsetColor = Color(0xFF7898D0);

    return Container(
      margin: const EdgeInsets.symmetric(horizontal: 24),
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(20),
        color: Colors.black.withValues(alpha: 0.35),
      ),
      child: Column(
        children: [
          // Sunrise — divider — Sunset row
          Row(
            children: [
              // Sunrise
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.center,
                  children: [
                    const Text('SUNRISE',
                        style: TextStyle(
                          color: sunriseColor,
                          fontSize: 11,
                          fontWeight: FontWeight.w600,
                          letterSpacing: 3.0,
                        )),
                    const SizedBox(height: 4),
                    Text(_fmtShort(d.sunTimes.sunrise),
                        style: const TextStyle(
                          color: sunriseColor,
                          fontSize: 28,
                          fontWeight: FontWeight.w300,
                          letterSpacing: 1.0,
                          fontFeatures: [FontFeature.tabularFigures()],
                        )),
                  ],
                ),
              ),

              // Center divider
              Container(
                width: 1,
                height: 36,
                color: Colors.white.withValues(alpha: 0.12),
              ),

              // Sunset
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.center,
                  children: [
                    const Text('SUNSET',
                        style: TextStyle(
                          color: sunsetColor,
                          fontSize: 11,
                          fontWeight: FontWeight.w600,
                          letterSpacing: 3.0,
                        )),
                    const SizedBox(height: 4),
                    Text(_fmtShort(d.sunTimes.sunset),
                        style: const TextStyle(
                          color: sunsetColor,
                          fontSize: 28,
                          fontWeight: FontWeight.w300,
                          letterSpacing: 1.0,
                          fontFeatures: [FontFeature.tabularFigures()],
                        )),
                  ],
                ),
              ),
            ],
          ),

          const SizedBox(height: 8),
          Container(height: 1, color: Colors.white.withValues(alpha: 0.08)),
          const SizedBox(height: 8),
          // Solar noon + day info
          Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              Text('SOLAR NOON',
                  style: TextStyle(
                    color: Colors.white.withValues(alpha: 0.45),
                    fontSize: 10,
                    fontWeight: FontWeight.w600,
                    letterSpacing: 2.5,
                  )),
              const SizedBox(width: 8),
              Text(_fmtShort(d.sunTimes.solarNoon),
                  style: TextStyle(
                    color: Colors.white.withValues(alpha: 0.7),
                    fontSize: 13,
                    fontWeight: FontWeight.w300,
                    letterSpacing: 0.5,
                    fontFeatures: const [FontFeature.tabularFigures()],
                  )),
              const SizedBox(width: 16),
              Text(
                  '${d.sunTimes.dayLength.floor()}h ${((d.sunTimes.dayLength - d.sunTimes.dayLength.floor()) * 60).round()}m of light',
                  style: TextStyle(
                    color: Colors.white.withValues(alpha: 0.5),
                    fontSize: 10,
                    fontWeight: FontWeight.w400,
                    letterSpacing: 1.0,
                  )),
            ],
          ),
        ],
      ),
    );
  }

  Widget _nextEventWidget(_SolarData d, double hour) {
    final events = d.solarClockData.events;

    // Find next event after current hour
    final idx = events.indexWhere((e) => e.hour > hour);
    final SolarEvent next;
    final double remaining;
    if (idx >= 0) {
      next = events[idx];
      remaining = next.hour - hour;
    } else {
      next = events.first;
      remaining = (24.0 - hour) + next.hour;
    }

    final h = remaining.floor();
    final m = ((remaining - h) * 60).round();
    final countdown = h > 0 ? '${h}h ${m}m' : '${m}m';

    return Center(
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 28, vertical: 16),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(24),
          color: Colors.black.withValues(alpha: 0.35),
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(next.label,
                style: TextStyle(
                  color: next.color,
                  fontSize: 12,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 3.0,
                )),
            const SizedBox(height: 6),
            Text(_fmtShort(next.hour),
                style: TextStyle(
                  color: next.color,
                  fontSize: 42,
                  fontWeight: FontWeight.w200,
                  letterSpacing: 2.0,
                  fontFeatures: const [FontFeature.tabularFigures()],
                )),
            const SizedBox(height: 4),
            Text(countdown,
                style: TextStyle(
                  color: Colors.white.withValues(alpha: 0.6),
                  fontSize: 14,
                  fontWeight: FontWeight.w400,
                  letterSpacing: 1.0,
                )),
          ],
        ),
      ),
    );
  }

  Widget _currentTimeWidget(double hour) {
    return Center(
      child: Text(_fmtShort(hour),
          style: TextStyle(
            color: Colors.white.withValues(alpha: _isLive ? 0.85 : 0.65),
            fontSize: 48,
            fontWeight: FontWeight.w200,
            letterSpacing: 4.0,
            fontFeatures: const [FontFeature.tabularFigures()],
            shadows: [
              Shadow(
                color: Colors.black.withValues(alpha: 0.6),
                blurRadius: 24,
              ),
            ],
          )),
    );
  }

  Widget _dayInfo(_SolarData d) {
    final h = d.sunTimes.dayLength.floor();
    final m = ((d.sunTimes.dayLength - h) * 60).round();
    return Column(
      children: [
        if (d.cityName != null)
          Padding(
            padding: const EdgeInsets.only(bottom: 5),
            child: Text(d.cityName!.toUpperCase(),
                style: TextStyle(
                  color: Colors.white.withValues(alpha: 0.7),
                  fontSize: 10,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 3.0,
                )),
          ),
        Text('${h}h ${m}m of light',
            style: TextStyle(
              color: Colors.white.withValues(alpha: 0.5),
              fontSize: 12,
              fontWeight: FontWeight.w400,
              letterSpacing: 1.0,
            )),
      ],
    );
  }

  Widget _empty() {
    return Scaffold(
      backgroundColor: const Color(0xFF060910),
      body: SafeArea(
        child: Stack(
          children: [
            Positioned(
              top: 12,
              right: 20,
              child: GestureDetector(
                onTap: () => Navigator.of(context).pop(),
                child: Icon(Icons.close_rounded,
                    color: Colors.white.withValues(alpha: 0.4), size: 24),
              ),
            ),
            Center(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(Icons.location_off_rounded,
                      color: const Color(0xFFF9A825).withValues(alpha: 0.4),
                      size: 48),
                  const SizedBox(height: 16),
                  const Text('No location configured',
                      style: TextStyle(
                          color: Color(0xFFE6EDF3),
                          fontSize: 16,
                          fontWeight: FontWeight.w500)),
                  const SizedBox(height: 8),
                  Text("Set your location to see\nthe sun's journey.",
                      textAlign: TextAlign.center,
                      style: TextStyle(
                          color: const Color(0xFF8B949E),
                          fontSize: 14,
                          height: 1.4)),
                  const SizedBox(height: 24),
                  GestureDetector(
                    onTap: () async {
                      final result = await LocationSettingsScreen.show(context);
                      if (result == true && mounted) _loadData();
                    },
                    child: Container(
                      padding: const EdgeInsets.symmetric(
                          horizontal: 24, vertical: 12),
                      decoration: BoxDecoration(
                        borderRadius: BorderRadius.circular(12),
                        color: const Color(0xFFF9A825).withValues(alpha: 0.15),
                        border: Border.all(
                          color: const Color(0xFFF9A825).withValues(alpha: 0.3),
                        ),
                      ),
                      child: const Text('Set Location',
                          style: TextStyle(
                            color: Color(0xFFF9A825),
                            fontSize: 15,
                            fontWeight: FontWeight.w600,
                          )),
                    ),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _loading() {
    return const Scaffold(
      backgroundColor: Color(0xFF060910),
      body: Center(
        child: CircularProgressIndicator(
          strokeWidth: 2,
          valueColor: AlwaysStoppedAnimation(Color(0xFF58A6FF)),
        ),
      ),
    );
  }
}

// =============================================================================
// Celestial Painter
// =============================================================================

class _CelestialPainter extends CustomPainter {
  final _SolarData data;
  final double hour;
  final double pulse;
  final double horizonY;
  final Offset center;
  final double radius;
  final double thresholdT;
  final bool nearSunrise;
  final bool use24;
  final SolarCurveSamples? curveData;

  static List<_Star>? _stars;
  static List<_Shimmer>? _shimmer;

  _CelestialPainter({
    required this.data,
    required this.hour,
    required this.pulse,
    required this.horizonY,
    required this.center,
    required this.radius,
    required this.thresholdT,
    required this.nearSunrise,
    required this.use24,
    this.curveData,
  });

  @override
  void paint(Canvas canvas, Size size) {
    final ang = SolarUtils.hourToAngle(hour, data.sunTimes.solarNoon);
    final sunPos = Offset(
      center.dx + radius * math.cos(ang),
      center.dy + radius * math.sin(ang),
    );
    final above = sunPos.dy <= horizonY;
    final elev =
        above ? ((horizonY - sunPos.dy) / radius).clamp(0.0, 1.0) : 0.0;

    _drawStars(canvas, size, elev);
    _drawHorizon(canvas, size);
    SolarArcPainter(
      data: data.solarClockData,
      geometry: SolarClockGeometry(
        center: center,
        radius: radius,
        solarNoon: data.sunTimes.solarNoon,
      ),
      use24: use24,
      curveData: curveData,
    ).paint(canvas, size);
    _drawSun(canvas, sunPos, above, elev);
    if (thresholdT > 0) _drawShimmer(canvas, sunPos);
  }

  // --------------------------------------------------------------------------
  // Stars
  // --------------------------------------------------------------------------

  void _drawStars(Canvas canvas, Size size, double elevation) {
    _stars ??= List.generate(100, (i) {
      final r = math.Random(42 + i);
      return _Star(r.nextDouble(), r.nextDouble(), 0.3 + r.nextDouble() * 1.3,
          0.15 + r.nextDouble() * 0.85, r.nextDouble() * math.pi * 2);
    });
    final vis = (1.0 - elevation * 2.5).clamp(0.0, 1.0);
    if (vis <= 0) return;

    for (final s in _stars!) {
      final sy = s.y * size.height * 0.58;
      if (sy > horizonY - 8) continue;
      final twinkle =
          0.4 + 0.6 * ((math.sin(pulse * math.pi * 2 + s.phase) + 1) / 2);
      canvas.drawCircle(
        Offset(s.x * size.width, sy),
        s.size,
        Paint()
          ..color =
              Colors.white.withValues(alpha: s.brightness * twinkle * vis),
      );
    }
  }

  // --------------------------------------------------------------------------
  // Horizon
  // --------------------------------------------------------------------------

  void _drawHorizon(Canvas canvas, Size size) {
    final path = Path()
      ..moveTo(0, horizonY)
      ..quadraticBezierTo(size.width / 2, horizonY - 4, size.width, horizonY);

    // Main horizon line — clearly visible
    canvas.drawPath(
        path,
        Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1.5
          ..shader = LinearGradient(colors: [
            Colors.white.withValues(alpha: 0.0),
            Colors.white.withValues(alpha: 0.25),
            Colors.white.withValues(alpha: 0.45),
            Colors.white.withValues(alpha: 0.45),
            Colors.white.withValues(alpha: 0.25),
            Colors.white.withValues(alpha: 0.0),
          ]).createShader(Rect.fromLTWH(0, horizonY - 1, size.width, 2)));

    // Subtle glow band along horizon
    final glowR = Rect.fromLTWH(0, horizonY - 20, size.width, 40);
    canvas.drawRect(
        glowR,
        Paint()
          ..shader = LinearGradient(
            begin: Alignment.topCenter,
            end: Alignment.bottomCenter,
            colors: [
              Colors.transparent,
              Colors.white.withValues(alpha: 0.04),
              Colors.white.withValues(alpha: 0.04),
              Colors.transparent,
            ],
          ).createShader(glowR));

    // Threshold horizon glow
    if (thresholdT > 0) {
      final r = Rect.fromLTWH(0, horizonY - 50, size.width, 100);
      canvas.drawRect(
          r,
          Paint()
            ..shader = LinearGradient(
              begin: Alignment.topCenter,
              end: Alignment.bottomCenter,
              colors: [
                Colors.transparent,
                const Color(0xFFF0A830).withValues(alpha: 0.15 * thresholdT),
                const Color(0xFFF0A830).withValues(alpha: 0.10 * thresholdT),
                Colors.transparent,
              ],
            ).createShader(r));
    }
  }

  // --------------------------------------------------------------------------
  // Sun
  // --------------------------------------------------------------------------

  void _drawSun(Canvas canvas, Offset pos, bool above, double elevation) {
    if (above) {
      _drawDaySun(canvas, pos, elevation);
    } else {
      _drawNightIndicator(canvas, pos);
    }
  }

  void _drawDaySun(Canvas canvas, Offset pos, double elevation) {
    final p = pulse;
    final power = 0.3 + elevation * 0.7;
    final boost = 1.0 + thresholdT * 0.8;

    // Layer 1 — Atmospheric wash (tighter to keep centered on arc)
    final washR = (120.0 + 30.0 * p) * power * boost;
    canvas.drawCircle(
        pos,
        washR,
        Paint()
          ..shader = RadialGradient(
            colors: [
              const Color(0xFFF0A830)
                  .withValues(alpha: (0.08 + 0.03 * p) * power * boost),
              const Color(0xFFF0A830).withValues(alpha: 0.02 * power),
              Colors.transparent,
            ],
            stops: const [0.0, 0.5, 1.0],
          ).createShader(Rect.fromCircle(center: pos, radius: washR)));

    // Layer 2 — Outer bloom
    final bloomR = (72.0 + 18.0 * p) * power * boost;
    canvas.drawCircle(
        pos,
        bloomR,
        Paint()
          ..shader = RadialGradient(
            colors: [
              const Color(0xFFF0B840)
                  .withValues(alpha: (0.14 + 0.06 * p) * power * boost),
              const Color(0xFFF0C050).withValues(alpha: 0.04 * power),
              Colors.transparent,
            ],
            stops: const [0.0, 0.55, 1.0],
          ).createShader(Rect.fromCircle(center: pos, radius: bloomR)));

    // Layer 3 — Corona
    final coronaR = (45.0 + 12.0 * p) * power * boost;
    canvas.drawCircle(
        pos,
        coronaR,
        Paint()
          ..shader = RadialGradient(
            colors: [
              const Color(0xFFF0B840)
                  .withValues(alpha: (0.22 + 0.10 * p) * power),
              const Color(0xFFF0A830).withValues(alpha: 0.06 * power),
              Colors.transparent,
            ],
            stops: const [0.0, 0.5, 1.0],
          ).createShader(Rect.fromCircle(center: pos, radius: coronaR)));

    // Layer 4 — Inner glow (less elevation-dependent for horizon visibility)
    final glowR = 24.0 + 18.0 * power + 6.0 * p;
    canvas.drawCircle(
        pos,
        glowR,
        Paint()
          ..shader = RadialGradient(
            colors: [
              Colors.white.withValues(alpha: (0.50 + 0.15 * p) * power),
              const Color(0xFFF0B840)
                  .withValues(alpha: (0.60 + 0.18 * p) * power),
              const Color(0xFFF0A830).withValues(alpha: 0.08 * power),
              Colors.transparent,
            ],
            stops: const [0.0, 0.25, 0.65, 1.0],
          ).createShader(Rect.fromCircle(center: pos, radius: glowR)));

    // Layer 5 — Disc (soft edge gradient instead of hard circle)
    final discR = 12.0 + 18.0 * power;
    canvas.drawCircle(
        pos,
        discR,
        Paint()
          ..shader = RadialGradient(
            colors: [
              const Color(0xFFFFF5E0),
              const Color(0xFFF0C848),
              const Color(0xFFF0B840),
              const Color(0xFFF0A830).withValues(alpha: 0.0),
            ],
            stops: const [0.0, 0.5, 0.85, 1.0],
          ).createShader(Rect.fromCircle(center: pos, radius: discR)));
  }

  void _drawNightIndicator(Canvas canvas, Offset pos) {
    final d = 0.5 + 0.5 * pulse;
    // Outer bloom (tighter)
    canvas.drawCircle(
        pos,
        45.0 + 12.0 * pulse,
        Paint()
          ..shader = RadialGradient(colors: [
            const Color(0xFF90B8E8).withValues(alpha: 0.16 * d),
            Colors.transparent,
          ]).createShader(
              Rect.fromCircle(center: pos, radius: 45.0 + 12.0 * pulse)));
    // Inner glow (tighter)
    canvas.drawCircle(
        pos,
        24.0 + 6.0 * pulse,
        Paint()
          ..shader = RadialGradient(colors: [
            const Color(0xFFA0C8F0).withValues(alpha: 0.30 * d),
            Colors.transparent,
          ]).createShader(
              Rect.fromCircle(center: pos, radius: 24.0 + 6.0 * pulse)));
    // Moon disc (more visible)
    canvas.drawCircle(pos, 12.0,
        Paint()..color = const Color(0xFFB0D0F0).withValues(alpha: 0.65));
    canvas.drawCircle(pos, 7.0,
        Paint()..color = const Color(0xFFD0E4F8).withValues(alpha: 0.50));
    canvas.drawCircle(
        pos, 3.5, Paint()..color = Colors.white.withValues(alpha: 0.25));
  }

  // --------------------------------------------------------------------------
  // Threshold shimmer particles
  // --------------------------------------------------------------------------

  void _drawShimmer(Canvas canvas, Offset sunPos) {
    _shimmer ??= List.generate(24, (i) {
      final r = math.Random(77 + i);
      return _Shimmer(
        r.nextDouble() * math.pi * 2,
        55.0 + r.nextDouble() * 130.0,
        0.5 + r.nextDouble() * 2.0,
        0.2 + r.nextDouble() * 0.8,
        r.nextDouble() * math.pi * 2,
      );
    });
    final color =
        nearSunrise ? const Color(0xFFF0C060) : const Color(0xFFE8A850);

    for (final s in _shimmer!) {
      final a = s.angle + pulse * s.speed * math.pi * 0.4;
      final dist =
          s.dist * (0.7 + 0.3 * math.sin(pulse * math.pi * 2 + s.phase));
      final alpha = thresholdT *
          0.75 *
          (0.25 + 0.75 * ((math.sin(pulse * math.pi * 2 + s.phase) + 1) / 2));
      canvas.drawCircle(
        Offset(sunPos.dx + dist * math.cos(a), sunPos.dy + dist * math.sin(a)),
        s.size,
        Paint()..color = color.withValues(alpha: alpha),
      );
    }
  }

  @override
  bool shouldRepaint(_CelestialPainter old) =>
      pulse != old.pulse ||
      hour != old.hour ||
      thresholdT != old.thresholdT ||
      curveData != old.curveData;
}

// =============================================================================
// Data classes
// =============================================================================

class _Star {
  final double x, y, size, brightness, phase;
  const _Star(this.x, this.y, this.size, this.brightness, this.phase);
}

class _Shimmer {
  final double angle, dist, size, speed, phase;
  const _Shimmer(this.angle, this.dist, this.size, this.speed, this.phase);
}
