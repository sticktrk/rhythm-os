import 'dart:math' as math;
import 'dart:async';
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import '../providers/server_sync_provider.dart';
import '../widgets/room_card.dart';
import '../widgets/solar_orbit.dart'; // For CelestialColors

/// All Rooms screen - a vertically scrollable list of Hue-style room cards.
///
/// Features:
/// - Dynamic grid that fits all orbs on one page
/// - Deep space background with optional starfield
/// - Batch-fetches all room states on load (one-shot)
class AllRoomsScreen extends StatefulWidget {
  final List<RoomDto> rooms;
  final CurveConfigDto globalConfig;
  final CurveData? curveData;

  const AllRoomsScreen({
    super.key,
    required this.rooms,
    required this.globalConfig,
    this.curveData,
  });

  @override
  State<AllRoomsScreen> createState() => _AllRoomsScreenState();
}

class _AllRoomsScreenState extends State<AllRoomsScreen> {
  Future<void> _onRefresh() async {
    final serverSync = context.read<ServerSyncProvider>();
    await serverSync.fullRefresh();
  }

  @override
  Widget build(BuildContext context) {
    final isLandscape =
        MediaQuery.of(context).orientation == Orientation.landscape;
    final bottomSafeArea = MediaQuery.of(context).padding.bottom;
    // Reserve space for bottom nav overlay (12 + 70 pill + 12 = 94dp) + safe area
    final bottomPad = bottomSafeArea + 104.0;

    return Stack(
      children: [
        // Celestial background (adapts to sunrise/sunset times)
        _CelestialBackground(curveData: widget.curveData),

        // Main content
        SafeArea(
          bottom: false,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              // Header
              _buildHeader(isLandscape),
              // Room cards list
              Expanded(
                child: RefreshIndicator(
                        onRefresh: _onRefresh,
                        color: CelestialColors.accentBlue,
                        backgroundColor: const Color(0xFF1A2E45),
                        child: Selector<ServerSyncProvider, (bool, int)>(
                          selector: (_, p) => (p.powerSave, p.softOffBrightness),
                          builder: (context, data, _) {
                            final (powerSave, softOffBrightness) = data;
                            final sortedRooms = List.of(widget.rooms)
                              ..sort((a, b) => a.name.toLowerCase().compareTo(b.name.toLowerCase()));
                            return ListView.builder(
                              padding: EdgeInsets.fromLTRB(16, 4, 16, bottomPad),
                              itemCount: sortedRooms.length,
                              itemBuilder: (context, index) {
                                final room = sortedRooms[index];
                                return Padding(
                                  padding: const EdgeInsets.only(bottom: 12),
                                  child: RoomCard(
                                    key: ValueKey(room.id),
                                    roomId: room.id,
                                    globalConfig: widget.globalConfig,
                                    curveData: widget.curveData,
                                    powerSave: powerSave,
                                    softOffBrightness: softOffBrightness,
                                  ),
                                );
                              },
                            );
                          },
                        ),
                      ),
              ),
            ],
          ),
        ),
      ],
    );
  }

  Widget _buildHeader(bool isLandscape) {
    final vPad = isLandscape ? 4.0 : 10.0;

    return Padding(
      padding: EdgeInsets.fromLTRB(20, vPad, 20, vPad),
      child: Text(
        'All Rooms',
        style: TextStyle(
          color: CelestialColors.textPrimary,
          fontSize: isLandscape ? 16 : 20,
          fontWeight: FontWeight.w600,
          letterSpacing: 0.3,
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Sky phase system — continuous daylight transitions
// ---------------------------------------------------------------------------

/// Represents a sky gradient at a specific phase of the day.
class _SkyPalette {
  /// Top-to-bottom gradient colors.
  final List<Color> colors;
  /// Gradient stops (must match colors length).
  final List<double> stops;
  /// Star visibility (0 = invisible, 1 = full).
  final double starOpacity;
  /// Sun/horizon glow intensity.
  final double glowIntensity;
  /// Color of the horizon glow.
  final Color glowColor;
  /// Vertical position of glow center (0 = top, 1 = bottom).
  final double glowY;

  const _SkyPalette({
    required this.colors,
    required this.stops,
    this.starOpacity = 0.0,
    this.glowIntensity = 0.0,
    this.glowColor = const Color(0x00000000),
    this.glowY = 0.3,
  });

  /// Linearly interpolate between two palettes.
  static _SkyPalette lerp(_SkyPalette a, _SkyPalette b, double t) {
    final clampedT = t.clamp(0.0, 1.0);
    // Interpolate colors (both must have same length — we normalize to 4 stops)
    final colors = <Color>[];
    final stops = <double>[];
    final count = math.min(a.colors.length, b.colors.length);
    for (int i = 0; i < count; i++) {
      colors.add(Color.lerp(a.colors[i], b.colors[i], clampedT)!);
      stops.add(a.stops[i] + (b.stops[i] - a.stops[i]) * clampedT);
    }
    return _SkyPalette(
      colors: colors,
      stops: stops,
      starOpacity: a.starOpacity + (b.starOpacity - a.starOpacity) * clampedT,
      glowIntensity:
          a.glowIntensity + (b.glowIntensity - a.glowIntensity) * clampedT,
      glowColor: Color.lerp(a.glowColor, b.glowColor, clampedT)!,
      glowY: a.glowY + (b.glowY - a.glowY) * clampedT,
    );
  }
}

/// All sky palettes keyed by phase of day.
class _SkyPalettes {
  // Deep night — inky blue-black, stars at full
  static const deepNight = _SkyPalette(
    colors: [
      Color(0xFF06080D), // Near-black zenith
      Color(0xFF0B0F18), // Deep indigo
      Color(0xFF0D1220), // Faint navy
      Color(0xFF0F1525), // Horizon hint
    ],
    stops: [0.0, 0.35, 0.7, 1.0],
    starOpacity: 1.0,
  );

  // Astronomical twilight — first hint of indigo at horizon
  static const astronomicalTwilight = _SkyPalette(
    colors: [
      Color(0xFF080C16), // Still very dark zenith
      Color(0xFF0E1428), // Deep indigo
      Color(0xFF162040), // Navy wash
      Color(0xFF1C2A52), // Indigo-blue horizon
    ],
    stops: [0.0, 0.3, 0.65, 1.0],
    starOpacity: 0.75,
    glowIntensity: 0.02,
    glowColor: Color(0xFF2A3B6E),
    glowY: 0.95,
  );

  // Nautical twilight — steel-navy sky, horizon warming
  static const nauticalTwilight = _SkyPalette(
    colors: [
      Color(0xFF0E1524), // Dark steel zenith
      Color(0xFF182840), // Steel navy
      Color(0xFF253A58), // Warming navy
      Color(0xFF3B4D6E), // Dusty blue horizon
    ],
    stops: [0.0, 0.3, 0.65, 1.0],
    starOpacity: 0.35,
    glowIntensity: 0.06,
    glowColor: Color(0xFF5A6B8E),
    glowY: 0.9,
  );

  // Civil twilight — rose-peach at horizon, stars fading
  static const civilTwilight = _SkyPalette(
    colors: [
      Color(0xFF162338), // Muted navy zenith
      Color(0xFF263A52), // Slate blue
      Color(0xFF4A5468), // Warm grey
      Color(0xFF7A6258), // Dusty rose horizon
    ],
    stops: [0.0, 0.3, 0.6, 1.0],
    starOpacity: 0.08,
    glowIntensity: 0.15,
    glowColor: Color(0xFFD4956A),
    glowY: 0.85,
  );

  // Golden sunrise — warm burst at horizon
  static const goldenSunrise = _SkyPalette(
    colors: [
      Color(0xFF1E3048), // Deep blue zenith
      Color(0xFF3A5068), // Steel-blue mid
      Color(0xFF6E6858), // Warm grey-gold
      Color(0xFFC48A50), // Amber-gold horizon
    ],
    stops: [0.0, 0.25, 0.6, 1.0],
    starOpacity: 0.0,
    glowIntensity: 0.30,
    glowColor: Color(0xFFE8A54B),
    glowY: 0.8,
  );

  // Morning — bright, clear sky settling in
  static const morning = _SkyPalette(
    colors: [
      Color(0xFF1A2E45), // Clean deep blue zenith
      Color(0xFF2A4260), // Open blue
      Color(0xFF3A556F), // Soft mid-blue
      Color(0xFF4A647A), // Pale horizon
    ],
    stops: [0.0, 0.3, 0.65, 1.0],
    starOpacity: 0.0,
    glowIntensity: 0.08,
    glowColor: Color(0xFFFFE4B5),
    glowY: 0.4,
  );

  // Midday — brightest, most open sky
  static const midday = _SkyPalette(
    colors: [
      Color(0xFF1C3550), // Rich blue zenith
      Color(0xFF284A68), // Strong blue
      Color(0xFF355D78), // Open blue
      Color(0xFF4A7088), // Luminous horizon
    ],
    stops: [0.0, 0.3, 0.65, 1.0],
    starOpacity: 0.0,
    glowIntensity: 0.12,
    glowColor: Color(0xFFE0D8C8),
    glowY: 0.2,
  );

  // Afternoon — slightly warmer, sun descending
  static const afternoon = _SkyPalette(
    colors: [
      Color(0xFF1A3048), // Deep blue zenith
      Color(0xFF2A4560), // Warm blue
      Color(0xFF3D5870), // Warming mid
      Color(0xFF506878), // Soft warm horizon
    ],
    stops: [0.0, 0.3, 0.65, 1.0],
    starOpacity: 0.0,
    glowIntensity: 0.10,
    glowColor: Color(0xFFEEC890),
    glowY: 0.35,
  );

  // Golden hour — rich warm light
  static const goldenHour = _SkyPalette(
    colors: [
      Color(0xFF1E2E42), // Darkening blue zenith
      Color(0xFF3A4858), // Muted blue-grey
      Color(0xFF6A5848), // Warm amber-grey
      Color(0xFFBE7A3E), // Rich amber horizon
    ],
    stops: [0.0, 0.25, 0.6, 1.0],
    starOpacity: 0.0,
    glowIntensity: 0.28,
    glowColor: Color(0xFFD4843A),
    glowY: 0.8,
  );

  // Sunset — dramatic crimson-amber
  static const sunset = _SkyPalette(
    colors: [
      Color(0xFF1A2236), // Deepening blue zenith
      Color(0xFF2E3448), // Purple-navy
      Color(0xFF6E4840), // Warm crimson-brown
      Color(0xFFC86030), // Burning orange horizon
    ],
    stops: [0.0, 0.25, 0.6, 1.0],
    starOpacity: 0.0,
    glowIntensity: 0.35,
    glowColor: Color(0xFFE0603A),
    glowY: 0.85,
  );

  // Civil dusk — violet-rose afterglow
  static const civilDusk = _SkyPalette(
    colors: [
      Color(0xFF141C30), // Deep navy zenith
      Color(0xFF24304A), // Purple-navy
      Color(0xFF4A4058), // Mauve
      Color(0xFF7A5858), // Dusty rose horizon
    ],
    stops: [0.0, 0.3, 0.6, 1.0],
    starOpacity: 0.10,
    glowIntensity: 0.12,
    glowColor: Color(0xFFA06858),
    glowY: 0.88,
  );

  // Nautical dusk — stars emerging, deep blue
  static const nauticalDusk = _SkyPalette(
    colors: [
      Color(0xFF0C1420), // Dark zenith
      Color(0xFF162236), // Deep steel
      Color(0xFF203050), // Navy
      Color(0xFF2A3A5A), // Cool blue horizon
    ],
    stops: [0.0, 0.3, 0.65, 1.0],
    starOpacity: 0.40,
    glowIntensity: 0.04,
    glowColor: Color(0xFF4A5878),
    glowY: 0.92,
  );

  // Astronomical dusk — nearly full dark
  static const astronomicalDusk = _SkyPalette(
    colors: [
      Color(0xFF080C16), // Near-black
      Color(0xFF0E1428), // Deep indigo
      Color(0xFF162040), // Navy wash
      Color(0xFF1C2A52), // Indigo horizon
    ],
    stops: [0.0, 0.3, 0.65, 1.0],
    starOpacity: 0.80,
    glowIntensity: 0.01,
    glowColor: Color(0xFF2A3560),
    glowY: 0.95,
  );
}

/// Computes the interpolated sky palette for the current time using solar data.
_SkyPalette _computeSkyPalette(double currentHour, CurveData? curveData) {
  // Extract solar times with sensible fallbacks
  final sunrise = curveData?.solar.sunrise ?? 6.5;
  final sunset = curveData?.solar.sunset ?? 18.0;
  final solarNoon = curveData?.solar.solarNoon ?? 12.0;

  // Twilight phases (fallback to offsets from sunrise/sunset)
  final dawnCivil = curveData?.solar.dawn?.civil ?? (sunrise - 0.5);
  final dawnNautical = curveData?.solar.dawn?.nautical ?? (sunrise - 1.0);
  final dawnAstro = curveData?.solar.dawn?.astronomical ?? (sunrise - 1.5);

  final duskCivil = curveData?.solar.dusk?.civil ?? (sunset + 0.5);
  final duskNautical = curveData?.solar.dusk?.nautical ?? (sunset + 1.0);
  final duskAstro = curveData?.solar.dusk?.astronomical ?? (sunset + 1.5);

  // Derived transition points
  final goldenMorningEnd = sunrise + 0.5; // 30 min after sunrise
  final morningEnd = sunrise + 1.5; // settling into day
  final afternoonStart = solarNoon + 1.5; // past peak
  final goldenEveStart = sunset - 1.0; // golden hour begins

  // Build ordered phase timeline
  // Each entry: (hour, palette)
  final phases = <(double, _SkyPalette)>[
    (dawnAstro, _SkyPalettes.deepNight),
    (dawnNautical, _SkyPalettes.astronomicalTwilight),
    (dawnCivil, _SkyPalettes.nauticalTwilight),
    (sunrise, _SkyPalettes.civilTwilight),
    (goldenMorningEnd, _SkyPalettes.goldenSunrise),
    (morningEnd, _SkyPalettes.morning),
    (solarNoon, _SkyPalettes.midday),
    (afternoonStart, _SkyPalettes.afternoon),
    (goldenEveStart, _SkyPalettes.goldenHour),
    (sunset, _SkyPalettes.sunset),
    (duskCivil, _SkyPalettes.civilDusk),
    (duskNautical, _SkyPalettes.nauticalDusk),
    (duskAstro, _SkyPalettes.astronomicalDusk),
    (duskAstro + 0.01, _SkyPalettes.deepNight), // snaps to night
  ];

  // Before first phase → deep night
  if (currentHour <= phases.first.$1) {
    return _SkyPalettes.deepNight;
  }
  // After last phase → deep night
  if (currentHour >= phases.last.$1) {
    return _SkyPalettes.deepNight;
  }

  // Find which two phases we're between
  for (int i = 0; i < phases.length - 1; i++) {
    final (startHour, startPalette) = phases[i];
    final (endHour, endPalette) = phases[i + 1];
    if (currentHour >= startHour && currentHour < endHour) {
      final t = (currentHour - startHour) / (endHour - startHour);
      return _SkyPalette.lerp(startPalette, endPalette, t);
    }
  }

  return _SkyPalettes.deepNight;
}

/// Celestial background with continuous daylight transitions.
///
/// Smoothly blends through: deep night → astronomical dawn → nautical twilight
/// → civil twilight → sunrise → morning → midday → afternoon → golden hour
/// → sunset → dusk phases → night. Uses actual solar/twilight times from
/// CurveData when available.
class _CelestialBackground extends StatefulWidget {
  final CurveData? curveData;

  const _CelestialBackground({this.curveData});

  @override
  State<_CelestialBackground> createState() => _CelestialBackgroundState();
}

class _CelestialBackgroundState extends State<_CelestialBackground>
    with SingleTickerProviderStateMixin {
  late AnimationController _twinkleController;
  late List<_Star> _stars;
  Timer? _timeCheckTimer;
  _SkyPalette _palette = _SkyPalettes.deepNight;

  @override
  void initState() {
    super.initState();
    _twinkleController = AnimationController(
      duration: const Duration(seconds: 10),
      vsync: this,
    )..repeat();

    // Generate random stars
    final random = math.Random(42);
    _stars = List.generate(60, (i) {
      return _Star(
        x: random.nextDouble(),
        y: random.nextDouble(),
        size: 0.5 + random.nextDouble() * 1.5,
        twinkleOffset: random.nextDouble() * 2 * math.pi,
        twinkleSpeed: 0.5 + random.nextDouble() * 1.5,
      );
    });

    _updateSkyPalette();
    // Re-compute every minute for smooth real-time transitions
    _timeCheckTimer = Timer.periodic(const Duration(minutes: 1), (_) {
      _updateSkyPalette();
    });
  }

  @override
  void didUpdateWidget(_CelestialBackground oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.curveData != oldWidget.curveData) {
      _updateSkyPalette();
    }
  }

  void _updateSkyPalette() {
    final now = DateTime.now();
    final currentHour = now.hour + now.minute / 60.0;
    final newPalette = _computeSkyPalette(currentHour, widget.curveData);
    if (mounted) {
      setState(() {
        _palette = newPalette;
      });
    }
  }

  @override
  void dispose() {
    _twinkleController.dispose();
    _timeCheckTimer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedContainer(
      duration: const Duration(milliseconds: 1200),
      curve: Curves.easeInOut,
      decoration: BoxDecoration(
        gradient: LinearGradient(
          begin: Alignment.topCenter,
          end: Alignment.bottomCenter,
          colors: _palette.colors,
          stops: _palette.stops,
        ),
      ),
      child: AnimatedBuilder(
        animation: _twinkleController,
        builder: (context, child) {
          return CustomPaint(
            painter: _CelestialPainter(
              stars: _stars,
              animationValue: _twinkleController.value,
              palette: _palette,
            ),
            size: Size.infinite,
          );
        },
      ),
    );
  }
}

class _Star {
  final double x;
  final double y;
  final double size;
  final double twinkleOffset;
  final double twinkleSpeed;

  _Star({
    required this.x,
    required this.y,
    required this.size,
    required this.twinkleOffset,
    required this.twinkleSpeed,
  });
}

class _CelestialPainter extends CustomPainter {
  final List<_Star> stars;
  final double animationValue;
  final _SkyPalette palette;

  _CelestialPainter({
    required this.stars,
    required this.animationValue,
    required this.palette,
  });

  @override
  void paint(Canvas canvas, Size size) {
    // Stars — opacity controlled by palette
    if (palette.starOpacity > 0.01) {
      _paintStars(canvas, size);
    }

    // Horizon / sun glow
    if (palette.glowIntensity > 0.005) {
      _paintGlow(canvas, size);
    }
  }

  void _paintStars(Canvas canvas, Size size) {
    final paint = Paint()..style = PaintingStyle.fill;

    for (final star in stars) {
      final twinkle = math.sin(
        animationValue * 2 * math.pi * star.twinkleSpeed + star.twinkleOffset,
      );
      // Base twinkle range scaled by palette's star opacity
      final opacity =
          (0.3 + (twinkle + 1) / 2 * 0.5) * palette.starOpacity;

      paint.color = CelestialColors.textPrimary.withValues(alpha: opacity);

      final x = star.x * size.width;
      final y = star.y * size.height;

      canvas.drawCircle(Offset(x, y), star.size, paint);
    }
  }

  void _paintGlow(Canvas canvas, Size size) {
    final paint = Paint()..style = PaintingStyle.fill;

    final glowCenter = Offset(size.width * 0.5, size.height * palette.glowY);
    final glowRadius = size.width * 1.2;

    // Subtle breathing animation on the glow
    final breathe =
        0.85 + math.sin(animationValue * 2 * math.pi * 0.3) * 0.15;
    final intensity = palette.glowIntensity * breathe;

    paint.shader = RadialGradient(
      colors: [
        palette.glowColor.withValues(alpha: intensity),
        palette.glowColor.withValues(alpha: intensity * 0.4),
        palette.glowColor.withValues(alpha: 0),
      ],
      stops: const [0.0, 0.35, 1.0],
    ).createShader(Rect.fromCircle(center: glowCenter, radius: glowRadius));

    canvas.drawCircle(glowCenter, glowRadius, paint);
  }

  @override
  bool shouldRepaint(covariant _CelestialPainter oldDelegate) {
    return animationValue != oldDelegate.animationValue ||
        !identical(palette, oldDelegate.palette);
  }
}
