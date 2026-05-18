import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../providers/onboarding_provider.dart';
import '../widgets/onboarding_orbit.dart';
import '../widgets/sun_glow_button.dart';
import '../../services/analytics_service.dart';
import '../../services/virtual_experience_service.dart';

/// Welcome screen — celestial introduction matching the rest of the app.
///
/// Aesthetic: a deep cosmic backdrop, a slow-moving starfield, and a hero
/// orbit at center that shows the rhythm of a day — warm sun arc bleeding
/// into a cool moon arc, with a small earth dot tracing the cycle. The CTA
/// is the same `SunGlowButton` used across onboarding so this screen feels
/// continuous with `AccountScreen`, `LocationScreen`, etc.
class WelcomeScreen extends StatefulWidget {
  const WelcomeScreen({super.key});

  @override
  State<WelcomeScreen> createState() => _WelcomeScreenState();
}

class _WelcomeScreenState extends State<WelcomeScreen>
    with TickerProviderStateMixin {
  late final AnimationController _orbit;
  late final AnimationController _twinkle;
  late final AnimationController _reveal;

  @override
  void initState() {
    super.initState();
    AnalyticsService().logOnboardingStarted();

    // One slow rotation of the earth dot per ~24s — calm, not racing.
    _orbit = AnimationController(
      duration: const Duration(seconds: 24),
      vsync: this,
    )..repeat();

    // Star twinkle / parallax breath.
    _twinkle = AnimationController(
      duration: const Duration(seconds: 8),
      vsync: this,
    )..repeat();

    // Staggered fade-in for the foreground composition.
    _reveal = AnimationController(
      duration: const Duration(milliseconds: 1600),
      vsync: this,
    );
    WidgetsBinding.instance.addPostFrameCallback((_) => _reveal.forward());
  }

  @override
  void dispose() {
    _orbit.dispose();
    _twinkle.dispose();
    _reveal.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return ColoredBox(
      color: OnboardingColors.backgroundDark,
      child: Stack(
        fit: StackFit.expand,
        children: [
          // Layered atmosphere — far back to near front.
          const _AuroraGlow(),
          AnimatedBuilder(
            animation: _twinkle,
            builder: (context, _) =>
                CustomPaint(painter: _StarfieldPainter(t: _twinkle.value)),
          ),
          const _HorizonGlow(),

          // Foreground composition.
          SafeArea(
            child: LayoutBuilder(
              builder: (context, constraints) {
                // Reserve room on each side for the SUNRISE / SUNSET labels
                // so they never collide with the screen edge.
                const labelGutter = 68.0;
                final widthAvailable =
                    constraints.maxWidth - (2 * labelGutter);
                final orbitSize = math
                    .min(widthAvailable, constraints.maxHeight * 0.42)
                    .clamp(200.0, 320.0);

                return Padding(
                  padding: const EdgeInsets.fromLTRB(28, 16, 28, 28),
                  child: Column(
                    children: [
                      _Reveal(
                        controller: _reveal,
                        start: 0.0,
                        child: const _BrandStamp(),
                      ),
                      const Spacer(flex: 3),

                      // Hero orbit.
                      _Reveal(
                        controller: _reveal,
                        start: 0.05,
                        child: _HeroOrbit(
                          orbit: _orbit,
                          size: orbitSize,
                        ),
                      ),

                      const Spacer(flex: 2),

                      // Wordmark.
                      _Reveal(
                        controller: _reveal,
                        start: 0.28,
                        child: const _Wordmark(),
                      ),
                      const SizedBox(height: 14),

                      // Tagline.
                      _Reveal(
                        controller: _reveal,
                        start: 0.42,
                        child: const _Tagline(),
                      ),

                      const Spacer(flex: 4),

                      // Primary CTA — same SunGlowButton used elsewhere.
                      _Reveal(
                        controller: _reveal,
                        start: 0.6,
                        child: SunGlowButton(
                          text: 'Get Started',
                          onPressed: () =>
                              context.read<OnboardingProvider>().nextPage(),
                        ),
                      ),
                      const SizedBox(height: 18),

                      // Secondary CTA — drop into the demo without setup.
                      _Reveal(
                        controller: _reveal,
                        start: 0.72,
                        child: _VirtualExperienceLink(
                          onPressed: () {
                            VirtualExperienceService.instance.enter();
                          },
                        ),
                      ),
                      const SizedBox(height: 30),
                    ],
                  ),
                );
              },
            ),
          ),
        ],
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Virtual Experience link — secondary entry point that drops a curious user
// into a fully populated demo without sign-in or hardware. Styled as a quiet
// text link with a small "live" dot so it reads as "explore" rather than
// "alternative sign-in path".
// ---------------------------------------------------------------------------

class _VirtualExperienceLink extends StatelessWidget {
  final VoidCallback onPressed;
  const _VirtualExperienceLink({required this.onPressed});

  @override
  Widget build(BuildContext context) {
    return Material(
      color: Colors.transparent,
      child: InkWell(
        onTap: onPressed,
        borderRadius: BorderRadius.circular(12),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Container(
                width: 6,
                height: 6,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: OnboardingColors.sunWarm,
                  boxShadow: [
                    BoxShadow(
                      color: OnboardingColors.sunWarm.withValues(alpha: 0.5),
                      blurRadius: 6,
                      spreadRadius: 0.5,
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 10),
              Text(
                'Try a Virtual Experience',
                style: TextStyle(
                  color: OnboardingColors.textPrimary.withValues(alpha: 0.85),
                  fontSize: 14,
                  fontWeight: FontWeight.w500,
                  letterSpacing: 0.4,
                ),
              ),
              const SizedBox(width: 6),
              Icon(
                Icons.arrow_forward_rounded,
                size: 16,
                color: OnboardingColors.textPrimary.withValues(alpha: 0.7),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Reveal — staggered fade + small upward translate driven by one controller.
// ---------------------------------------------------------------------------

class _Reveal extends StatelessWidget {
  final AnimationController controller;
  final double start;
  final Widget child;

  const _Reveal({
    required this.controller,
    required this.start,
    required this.child,
  });

  @override
  Widget build(BuildContext context) {
    final curve = CurvedAnimation(
      parent: controller,
      curve: Interval(
        start,
        math.min(1.0, start + 0.5),
        curve: Curves.easeOutCubic,
      ),
    );
    return AnimatedBuilder(
      animation: curve,
      builder: (context, child) {
        final v = curve.value;
        return Opacity(
          opacity: v,
          child: Transform.translate(
            offset: Offset(0, (1 - v) * 12),
            child: child,
          ),
        );
      },
      child: child,
    );
  }
}

// ---------------------------------------------------------------------------
// Brand stamp — small mark at the top of the screen for orientation.
// ---------------------------------------------------------------------------

class _BrandStamp extends StatelessWidget {
  const _BrandStamp();

  @override
  Widget build(BuildContext context) {
    return Row(
      mainAxisAlignment: MainAxisAlignment.center,
      children: [
        // A tiny sun glyph.
        Container(
          width: 6,
          height: 6,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: OnboardingColors.sunWarm,
            boxShadow: [
              BoxShadow(
                color: OnboardingColors.sunWarm.withValues(alpha: 0.6),
                blurRadius: 8,
                spreadRadius: 1,
              ),
            ],
          ),
        ),
        const SizedBox(width: 10),
        Text(
          'PERFORMANCE LIGHTING',
          style: TextStyle(
            color: OnboardingColors.textPrimary.withValues(alpha: 0.75),
            fontSize: 10.5,
            fontWeight: FontWeight.w700,
            letterSpacing: 3.6,
          ),
        ),
      ],
    );
  }
}

// ---------------------------------------------------------------------------
// Wordmark — "rhythm" rendered with a sun→moon gradient, sized to feel like
// the title moment of the screen.
// ---------------------------------------------------------------------------

class _Wordmark extends StatelessWidget {
  const _Wordmark();

  @override
  Widget build(BuildContext context) {
    return ShaderMask(
      shaderCallback: (rect) => ui.Gradient.linear(
        Offset(rect.left, 0),
        Offset(rect.right, 0),
        const [
          OnboardingColors.sunWarm,
          Color(0xFFFFE0B2),
          OnboardingColors.moonGlow,
        ],
        const [0.0, 0.5, 1.0],
      ),
      blendMode: BlendMode.srcIn,
      child: RichText(
        text: const TextSpan(
          style: TextStyle(
            color: Colors.white,
            fontSize: 56,
            fontWeight: FontWeight.w300,
            letterSpacing: -0.5,
            height: 1.0,
          ),
          children: [
            TextSpan(text: 'rhythm'),
            TextSpan(
              text: '.lighting',
              style: TextStyle(
                fontWeight: FontWeight.w200,
                fontSize: 40,
                letterSpacing: -0.2,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Tagline — refined single-line statement of intent.
// ---------------------------------------------------------------------------

class _Tagline extends StatelessWidget {
  const _Tagline();

  @override
  Widget build(BuildContext context) {
    return Text(
      'Performance Lighting for your Body',
      style: TextStyle(
        color: OnboardingColors.textSecondary.withValues(alpha: 0.85),
        fontSize: 16,
        fontWeight: FontWeight.w400,
        letterSpacing: 0.4,
        height: 1.4,
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Hero orbit — central sun, day/night arc gradient, slowly orbiting earth dot.
// Bigger and more atmospheric than the standard `OnboardingOrbit` so the
// welcome moment carries weight.
// ---------------------------------------------------------------------------

class _HeroOrbit extends StatelessWidget {
  final AnimationController orbit;
  final double size;

  const _HeroOrbit({required this.orbit, required this.size});

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: size,
      height: size,
      child: Stack(
        alignment: Alignment.center,
        clipBehavior: Clip.none,
        children: [
          // Outer atmospheric halo.
          Container(
            width: size,
            height: size,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              gradient: RadialGradient(
                colors: [
                  OnboardingColors.sunWarm.withValues(alpha: 0.10),
                  OnboardingColors.sunWarm.withValues(alpha: 0.03),
                  Colors.transparent,
                ],
                stops: const [0.0, 0.55, 1.0],
              ),
            ),
          ),

          // Day/night arc — a sweep gradient ring representing 24h of light.
          CustomPaint(
            size: Size(size, size),
            painter: _DayNightArcPainter(
              radius: size * 0.42,
            ),
          ),

          // Subtle inner orbit ring.
          Container(
            width: size * 0.84,
            height: size * 0.84,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              border: Border.all(
                color: OnboardingColors.orbitRing.withValues(alpha: 0.35),
                width: 1,
              ),
            ),
          ),

          // Central sun (breathing).
          _BreathingSun(size: size * 0.34),

          // Orbiting earth dot with a short comet trail.
          AnimatedBuilder(
            animation: orbit,
            builder: (context, _) {
              final angle = orbit.value * 2 * math.pi - math.pi / 2;
              return CustomPaint(
                size: Size(size, size),
                painter: _EarthOrbitPainter(
                  angle: angle,
                  radius: size * 0.42,
                ),
              );
            },
          ),

          // Day-cycle markers — sunrise/sunset and the twilight bracket
          // around them. Hints at how the system thinks about the day
          // without overwhelming the welcome moment.
          _OrbitMarkers(size: size),
        ],
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Orbit markers — a sparse set of labels around the day/night ring that
// introduce the user to the language of the system: sunrise, sunset, and the
// twilight transitions on either side.
// ---------------------------------------------------------------------------

class _MarkerSpec {
  final double hour; // 0..24
  final String label;
  final bool primary;
  const _MarkerSpec({
    required this.hour,
    required this.label,
    required this.primary,
  });

  /// Angle on the orbit. Top = noon, going clockwise:
  /// 12 → top, 18 → right, 00 → bottom, 06 → left.
  double get angle =>
      -math.pi / 2 + (((hour - 12) % 24 + 24) % 24) / 24 * 2 * math.pi;
}

class _OrbitMarkers extends StatelessWidget {
  final double size;
  const _OrbitMarkers({required this.size});

  static const _markers = <_MarkerSpec>[
    _MarkerSpec(hour: 6, label: 'Sunrise', primary: true),
    _MarkerSpec(hour: 18, label: 'Sunset', primary: true),
    _MarkerSpec(hour: 5, label: 'Dawn', primary: false),
    _MarkerSpec(hour: 19, label: 'Dusk', primary: false),
  ];

  @override
  Widget build(BuildContext context) {
    final ringRadius = size * 0.42;
    return Stack(
      clipBehavior: Clip.none,
      fit: StackFit.expand,
      children: [
        // Radial ticks crossing the ring at each marker's angle.
        IgnorePointer(
          child: CustomPaint(
            size: Size(size, size),
            painter: _MarkerTickPainter(
              ringRadius: ringRadius,
              markers: _markers,
            ),
          ),
        ),
        // Text labels positioned outside the ring on the appropriate side.
        for (final m in _markers)
          _MarkerLabel(size: size, ringRadius: ringRadius, spec: m),
      ],
    );
  }
}

class _MarkerTickPainter extends CustomPainter {
  final double ringRadius;
  final List<_MarkerSpec> markers;

  _MarkerTickPainter({required this.ringRadius, required this.markers});

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);

    for (final m in markers) {
      final cosA = math.cos(m.angle);
      final sinA = math.sin(m.angle);
      final innerR = ringRadius - (m.primary ? 5 : 3);
      final outerR = ringRadius + (m.primary ? 8 : 5);
      final inner = Offset(center.dx + innerR * cosA, center.dy + innerR * sinA);
      final outer = Offset(center.dx + outerR * cosA, center.dy + outerR * sinA);

      final paint = Paint()
        ..color = m.primary
            ? OnboardingColors.textPrimary.withValues(alpha: 0.85)
            : OnboardingColors.textSecondary.withValues(alpha: 0.45)
        ..strokeWidth = m.primary ? 1.8 : 1.0
        ..strokeCap = StrokeCap.round;

      canvas.drawLine(inner, outer, paint);

      // Soft halo for primary markers so they read as the anchors.
      if (m.primary) {
        final haloPaint = Paint()
          ..color = OnboardingColors.sunWarm.withValues(alpha: 0.35)
          ..strokeWidth = 4
          ..strokeCap = StrokeCap.round
          ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 3);
        canvas.drawLine(inner, outer, haloPaint);
      }
    }
  }

  @override
  bool shouldRepaint(covariant _MarkerTickPainter oldDelegate) => false;
}

class _MarkerLabel extends StatelessWidget {
  final double size;
  final double ringRadius;
  final _MarkerSpec spec;

  const _MarkerLabel({
    required this.size,
    required this.ringRadius,
    required this.spec,
  });

  @override
  Widget build(BuildContext context) {
    final cx = size / 2;
    final cy = size / 2;
    // Place labels just outside the tick so they sit clear of the ring.
    final labelRadius = ringRadius + (spec.primary ? 14 : 11);
    final x = cx + labelRadius * math.cos(spec.angle);
    final y = cy + labelRadius * math.sin(spec.angle);
    final isLeft = math.cos(spec.angle) < 0;

    return Positioned(
      left: isLeft ? null : x + 2,
      right: isLeft ? (size - x) + 2 : null,
      // Roughly center the text vertically on the marker line.
      top: y - (spec.primary ? 7 : 6),
      child: Text(
        spec.label.toUpperCase(),
        style: TextStyle(
          color: spec.primary
              ? OnboardingColors.textPrimary.withValues(alpha: 0.85)
              : OnboardingColors.textSecondary.withValues(alpha: 0.65),
          fontSize: spec.primary ? 9.5 : 8.5,
          fontWeight: spec.primary ? FontWeight.w700 : FontWeight.w500,
          letterSpacing: spec.primary ? 1.8 : 1.5,
        ),
      ),
    );
  }
}

class _BreathingSun extends StatefulWidget {
  final double size;
  const _BreathingSun({required this.size});

  @override
  State<_BreathingSun> createState() => _BreathingSunState();
}

class _BreathingSunState extends State<_BreathingSun>
    with SingleTickerProviderStateMixin {
  late final AnimationController _c;

  @override
  void initState() {
    super.initState();
    _c = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 3200),
    )..repeat(reverse: true);
  }

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: _c,
      builder: (context, _) {
        final s = widget.size * (1.0 + 0.04 * _c.value);
        return Container(
          width: s,
          height: s,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            gradient: const RadialGradient(
              colors: [
                Colors.white,
                Color(0xFFFFE3A6),
                OnboardingColors.sunWarm,
                Color(0x00F9A825),
              ],
              stops: [0.0, 0.35, 0.65, 1.0],
            ),
            boxShadow: [
              BoxShadow(
                color: OnboardingColors.sunWarm
                    .withValues(alpha: 0.45 + 0.15 * _c.value),
                blurRadius: widget.size * 0.55,
                spreadRadius: widget.size * 0.05,
              ),
              BoxShadow(
                color: OnboardingColors.sunWarm
                    .withValues(alpha: 0.18 + 0.08 * _c.value),
                blurRadius: widget.size * 1.2,
                spreadRadius: widget.size * 0.18,
              ),
            ],
          ),
        );
      },
    );
  }
}

/// Sweep-gradient ring that visualizes a full 24h of light — warm day in the
/// upper half, cool indigo through the lower half. This is the brand idea
/// expressed graphically: rhythm = the daily light cycle.
class _DayNightArcPainter extends CustomPainter {
  final double radius;
  _DayNightArcPainter({required this.radius});

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);
    final rect = Rect.fromCircle(center: center, radius: radius);

    final shader = SweepGradient(
      // Start at solar noon (top) and rotate clockwise through dusk → night
      // → dawn → noon. Indices match an analog clock running on a 24h cycle.
      startAngle: -math.pi / 2,
      endAngle: 3 * math.pi / 2,
      colors: const [
        Color(0xFFFFD68A), // noon — warm white-gold
        Color(0xFFFFAA55), // afternoon → golden hour
        Color(0xFF7C5BB0), // dusk — twilight violet
        Color(0xFF2D3A5C), // night — deep indigo
        Color(0xFF1E2642), // midnight
        Color(0xFF2D3A5C), // pre-dawn
        Color(0xFFE89B6E), // dawn — warm rose
        Color(0xFFFFD68A), // back to noon
      ],
      stops: const [0.0, 0.15, 0.28, 0.4, 0.5, 0.6, 0.78, 1.0],
    ).createShader(rect);

    final ringPaint = Paint()
      ..shader = shader
      ..style = PaintingStyle.stroke
      ..strokeWidth = 2.0
      ..strokeCap = StrokeCap.round;

    final glowPaint = Paint()
      ..shader = shader
      ..style = PaintingStyle.stroke
      ..strokeWidth = 10.0
      ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 6)
      ..color = Colors.white.withValues(alpha: 0.4);

    canvas.drawArc(rect, 0, 2 * math.pi, false, glowPaint);
    canvas.drawArc(rect, 0, 2 * math.pi, false, ringPaint);

    // Hour ticks at the four cardinal hours (00, 06, 12, 18).
    final tickPaint = Paint()
      ..color = OnboardingColors.textPrimary.withValues(alpha: 0.35)
      ..strokeWidth = 1.2
      ..strokeCap = StrokeCap.round;

    for (var i = 0; i < 4; i++) {
      final a = -math.pi / 2 + i * math.pi / 2;
      final inner = Offset(
        center.dx + (radius - 6) * math.cos(a),
        center.dy + (radius - 6) * math.sin(a),
      );
      final outer = Offset(
        center.dx + (radius + 8) * math.cos(a),
        center.dy + (radius + 8) * math.sin(a),
      );
      canvas.drawLine(inner, outer, tickPaint);
    }
  }

  @override
  bool shouldRepaint(covariant _DayNightArcPainter oldDelegate) =>
      oldDelegate.radius != radius;
}

/// Orbiting earth dot + short trailing comet, drawn as a single layer so the
/// trail and the dot stay perfectly aligned.
class _EarthOrbitPainter extends CustomPainter {
  final double angle;
  final double radius;
  _EarthOrbitPainter({required this.angle, required this.radius});

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);

    // Trail: a fading arc behind the dot.
    const trailLen = 0.6; // radians
    const segments = 14;
    for (var i = 0; i < segments; i++) {
      final t = i / segments;
      final a = angle - trailLen * t;
      final pos = Offset(
        center.dx + radius * math.cos(a),
        center.dy + radius * math.sin(a),
      );
      final paint = Paint()
        ..color = OnboardingColors.accentBlue.withValues(alpha: 0.35 * (1 - t))
        ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 3);
      canvas.drawCircle(pos, 2.6 * (1 - t * 0.6), paint);
    }

    // The earth dot itself — small, luminous.
    final pos = Offset(
      center.dx + radius * math.cos(angle),
      center.dy + radius * math.sin(angle),
    );

    final glowPaint = Paint()
      ..color = OnboardingColors.accentBlue.withValues(alpha: 0.55)
      ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 6);
    canvas.drawCircle(pos, 8, glowPaint);

    final dotPaint = Paint()
      ..shader = ui.Gradient.radial(
        pos,
        4.5,
        const [
          Color(0xFFCDE6FF),
          OnboardingColors.accentBlue,
        ],
      );
    canvas.drawCircle(pos, 4.5, dotPaint);
  }

  @override
  bool shouldRepaint(covariant _EarthOrbitPainter oldDelegate) =>
      oldDelegate.angle != angle || oldDelegate.radius != radius;
}

// ---------------------------------------------------------------------------
// Aurora glow — large, soft radial gradients that give the background depth
// without committing to a flat color.
// ---------------------------------------------------------------------------

class _AuroraGlow extends StatelessWidget {
  const _AuroraGlow();

  @override
  Widget build(BuildContext context) {
    return IgnorePointer(
      child: Stack(
        fit: StackFit.expand,
        children: [
          // Warm sun-wash high in the frame.
          DecoratedBox(
            decoration: BoxDecoration(
              gradient: RadialGradient(
                center: const Alignment(0, -0.25),
                radius: 0.95,
                colors: [
                  OnboardingColors.sunWarm.withValues(alpha: 0.12),
                  OnboardingColors.sunWarm.withValues(alpha: 0.02),
                  Colors.transparent,
                ],
                stops: const [0.0, 0.45, 1.0],
              ),
            ),
          ),
          // Cool indigo wash low and to the right — feels like night sky
          // bleeding in from below the horizon.
          DecoratedBox(
            decoration: BoxDecoration(
              gradient: RadialGradient(
                center: const Alignment(0.7, 0.7),
                radius: 0.9,
                colors: [
                  OnboardingColors.nightIndigo.withValues(alpha: 0.55),
                  OnboardingColors.nightIndigo.withValues(alpha: 0.1),
                  Colors.transparent,
                ],
                stops: const [0.0, 0.5, 1.0],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Horizon glow — a sunrise-colored band at the bottom edge of the screen,
// grounding the composition.
// ---------------------------------------------------------------------------

class _HorizonGlow extends StatelessWidget {
  const _HorizonGlow();

  @override
  Widget build(BuildContext context) {
    return IgnorePointer(
      child: Align(
        alignment: Alignment.bottomCenter,
        child: FractionallySizedBox(
          widthFactor: 1.0,
          heightFactor: 0.32,
          child: DecoratedBox(
            decoration: BoxDecoration(
              gradient: LinearGradient(
                begin: Alignment.bottomCenter,
                end: Alignment.topCenter,
                colors: [
                  OnboardingColors.sunWarm.withValues(alpha: 0.08),
                  OnboardingColors.sunWarm.withValues(alpha: 0.02),
                  Colors.transparent,
                ],
                stops: const [0.0, 0.4, 1.0],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Starfield — fixed-seed point field with slow per-star twinkle. Drawn at
// low alpha so it reads as depth, not decoration.
// ---------------------------------------------------------------------------

class _StarfieldPainter extends CustomPainter {
  final double t;
  static const _seed = 11;
  static const _count = 110;

  _StarfieldPainter({required this.t});

  @override
  void paint(Canvas canvas, Size size) {
    final rng = math.Random(_seed);
    for (var i = 0; i < _count; i++) {
      final x = rng.nextDouble() * size.width;
      final y = rng.nextDouble() * size.height;
      final base = 0.18 + rng.nextDouble() * 0.5;
      final phase = rng.nextDouble() * math.pi * 2;
      final twinkle =
          (math.sin(t * 2 * math.pi + phase) + 1) / 2; // 0..1
      final radius = 0.5 + rng.nextDouble() * 1.4;
      final alpha = (base * (0.5 + 0.5 * twinkle)).clamp(0.0, 0.85);

      // A handful of stars get the warm sun tint, the rest stay cold white.
      final tint = i % 17 == 0
          ? OnboardingColors.sunWarm
          : (i % 11 == 0 ? OnboardingColors.moonGlow : Colors.white);

      final paint = Paint()
        ..color = tint.withValues(alpha: alpha)
        ..maskFilter = radius > 1.2
            ? const MaskFilter.blur(BlurStyle.normal, 1.2)
            : null;
      canvas.drawCircle(Offset(x, y), radius, paint);
    }
  }

  @override
  bool shouldRepaint(covariant _StarfieldPainter oldDelegate) =>
      oldDelegate.t != t;
}
