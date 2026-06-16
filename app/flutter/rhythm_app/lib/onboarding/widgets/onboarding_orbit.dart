import 'dart:math' as math;
import 'package:flutter/material.dart';

/// Celestial color palette for onboarding.
class OnboardingColors {
  static const backgroundDark = Color(0xFF0D1117);
  static const backgroundCard = Color(0xFF161B22);
  static const orbitRing = Color(0xFF30363D);
  static const accentBlue = Color(0xFF58A6FF);
  static const sunWarm = Color(0xFFF9A825);
  static const textPrimary = Color(0xFFE6EDF3);
  static const textSecondary = Color(0xFF8B949E);

  // Sleep/night colors
  static const moonGlow = Color(0xFF7C8EBF);
  static const nightIndigo = Color(0xFF2D3A5C);
  static const sleepArc = Color(0xFF1E2642);
}

/// Converts hour:minute to angle on a 24-hour clock face.
/// 0:00 (midnight) = top (-π/2), 6:00 = right, 12:00 = bottom, 18:00 = left
double timeToAngle(int hour, int minute) {
  final totalMinutes = hour * 60 + minute;
  final fraction = totalMinutes / (24 * 60);
  return (fraction * 2 * math.pi) - (math.pi / 2);
}

/// Animated sun with orbiting dot and optional bed/wake time markers.
class OnboardingOrbit extends StatefulWidget {
  final double size;
  final Duration orbitDuration;
  final int? bedtimeHour;
  final int? bedtimeMinute;
  final int? wakeTimeHour;
  final int? wakeTimeMinute;
  final bool showTimeMarkers;

  const OnboardingOrbit({
    super.key,
    this.size = 200,
    this.orbitDuration = const Duration(seconds: 4),
    this.bedtimeHour,
    this.bedtimeMinute,
    this.wakeTimeHour,
    this.wakeTimeMinute,
    this.showTimeMarkers = false,
  });

  @override
  State<OnboardingOrbit> createState() => _OnboardingOrbitState();
}

class _OnboardingOrbitState extends State<OnboardingOrbit>
    with TickerProviderStateMixin {
  late AnimationController _orbitController;
  late AnimationController _pulseController;
  late AnimationController _glowController;
  late Animation<double> _pulseAnimation;
  late Animation<double> _glowAnimation;

  @override
  void initState() {
    super.initState();

    // Orbit animation (blue dot rotating around sun)
    _orbitController = AnimationController(
      duration: widget.orbitDuration,
      vsync: this,
    )..repeat();

    // Pulse animation (sun breathing)
    _pulseController = AnimationController(
      duration: const Duration(milliseconds: 3000),
      vsync: this,
    )..repeat(reverse: true);

    _pulseAnimation = Tween<double>(begin: 1.0, end: 1.05).animate(
      CurvedAnimation(parent: _pulseController, curve: Curves.easeInOut),
    );

    // Glow animation for time markers
    _glowController = AnimationController(
      duration: const Duration(milliseconds: 2500),
      vsync: this,
    )..repeat(reverse: true);

    _glowAnimation = Tween<double>(begin: 0.4, end: 0.8).animate(
      CurvedAnimation(parent: _glowController, curve: Curves.easeInOut),
    );
  }

  @override
  void dispose() {
    _orbitController.dispose();
    _pulseController.dispose();
    _glowController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final sunSize = widget.size * 0.4;
    final orbitRadius = widget.size * 0.4;
    final dotSize = widget.size * 0.08;

    final showMarkers = widget.showTimeMarkers &&
        widget.bedtimeHour != null &&
        widget.bedtimeMinute != null &&
        widget.wakeTimeHour != null &&
        widget.wakeTimeMinute != null;

    return SizedBox(
      width: widget.size,
      height: widget.size,
      child: Stack(
        alignment: Alignment.center,
        children: [
          // Subtle radial gradient background glow
          Container(
            width: widget.size,
            height: widget.size,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              gradient: RadialGradient(
                colors: [
                  OnboardingColors.sunWarm.withValues(alpha: 0.08),
                  OnboardingColors.sunWarm.withValues(alpha: 0.02),
                  Colors.transparent,
                ],
                stops: const [0.0, 0.5, 1.0],
              ),
            ),
          ),

          // Sleep arc (night zone between bedtime and wake time)
          if (showMarkers)
            AnimatedBuilder(
              animation: _glowAnimation,
              builder: (context, child) {
                return CustomPaint(
                  size: Size(widget.size, widget.size),
                  painter: _SleepArcPainter(
                    bedtimeAngle: timeToAngle(widget.bedtimeHour!, widget.bedtimeMinute!),
                    wakeTimeAngle: timeToAngle(widget.wakeTimeHour!, widget.wakeTimeMinute!),
                    orbitRadius: orbitRadius + dotSize / 2,
                    glowIntensity: _glowAnimation.value,
                  ),
                );
              },
            ),

          // Hour markers (subtle tick marks around the orbit)
          if (showMarkers)
            CustomPaint(
              size: Size(widget.size, widget.size),
              painter: _HourMarkersPainter(
                orbitRadius: orbitRadius + dotSize / 2,
              ),
            ),

          // Orbit ring (subtle)
          Container(
            width: orbitRadius * 2 + dotSize,
            height: orbitRadius * 2 + dotSize,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              border: Border.all(
                color: OnboardingColors.orbitRing.withValues(alpha: 0.3),
                width: 1,
              ),
            ),
          ),

          // Bedtime marker (moon)
          if (showMarkers)
            AnimatedBuilder(
              animation: _glowAnimation,
              builder: (context, child) {
                final angle = timeToAngle(widget.bedtimeHour!, widget.bedtimeMinute!);
                final x = orbitRadius * math.cos(angle);
                final y = orbitRadius * math.sin(angle);
                return Transform.translate(
                  offset: Offset(x, y),
                  child: _buildMoonMarker(dotSize * 1.4, _glowAnimation.value),
                );
              },
            ),

          // Wake time marker (sunrise)
          if (showMarkers)
            AnimatedBuilder(
              animation: _glowAnimation,
              builder: (context, child) {
                final angle = timeToAngle(widget.wakeTimeHour!, widget.wakeTimeMinute!);
                final x = orbitRadius * math.cos(angle);
                final y = orbitRadius * math.sin(angle);
                return Transform.translate(
                  offset: Offset(x, y),
                  child: _buildSunriseMarker(dotSize * 1.4, _glowAnimation.value),
                );
              },
            ),

          // Orbiting blue dot (only show if no time markers)
          if (!showMarkers)
            AnimatedBuilder(
              animation: _orbitController,
              builder: (context, child) {
                final angle = _orbitController.value * 2 * math.pi - math.pi / 2;
                final x = orbitRadius * math.cos(angle);
                final y = orbitRadius * math.sin(angle);

                return Transform.translate(
                  offset: Offset(x, y),
                  child: child,
                );
              },
              child: Container(
                width: dotSize,
                height: dotSize,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: OnboardingColors.accentBlue,
                  boxShadow: [
                    BoxShadow(
                      color: OnboardingColors.accentBlue.withValues(alpha: 0.6),
                      blurRadius: 8,
                      spreadRadius: 2,
                    ),
                  ],
                ),
              ),
            ),

          // Central sun with pulsing glow
          AnimatedBuilder(
            animation: _pulseAnimation,
            builder: (context, child) {
              return _buildSun(sunSize * _pulseAnimation.value);
            },
          ),
        ],
      ),
    );
  }

  Widget _buildMoonMarker(double size, double glowIntensity) {
    return Container(
      width: size,
      height: size,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        boxShadow: [
          BoxShadow(
            color: OnboardingColors.moonGlow.withValues(alpha: glowIntensity * 0.6),
            blurRadius: size * 0.8,
            spreadRadius: size * 0.1,
          ),
        ],
      ),
      child: Stack(
        alignment: Alignment.center,
        children: [
          // Moon body
          Container(
            width: size * 0.85,
            height: size * 0.85,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              gradient: RadialGradient(
                center: const Alignment(-0.3, -0.3),
                colors: [
                  const Color(0xFFE8EBF2),
                  OnboardingColors.moonGlow,
                  OnboardingColors.moonGlow.withValues(alpha: 0.8),
                ],
                stops: const [0.0, 0.6, 1.0],
              ),
              boxShadow: [
                BoxShadow(
                  color: Colors.black.withValues(alpha: 0.3),
                  blurRadius: 2,
                  offset: const Offset(1, 1),
                ),
              ],
            ),
          ),
          // Crescent shadow overlay
          Positioned(
            right: size * 0.08,
            child: Container(
              width: size * 0.65,
              height: size * 0.85,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: OnboardingColors.backgroundDark.withValues(alpha: 0.85),
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildSunriseMarker(double size, double glowIntensity) {
    return Container(
      width: size,
      height: size,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        boxShadow: [
          BoxShadow(
            color: OnboardingColors.sunWarm.withValues(alpha: glowIntensity * 0.5),
            blurRadius: size * 0.8,
            spreadRadius: size * 0.1,
          ),
        ],
      ),
      child: CustomPaint(
        size: Size(size, size),
        painter: _SunriseIconPainter(glowIntensity: glowIntensity),
      ),
    );
  }

  Widget _buildSun(double size) {
    return Container(
      width: size,
      height: size,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        gradient: RadialGradient(
          colors: [
            Colors.white,
            OnboardingColors.sunWarm,
            OnboardingColors.sunWarm.withValues(alpha: 0.6),
            OnboardingColors.sunWarm.withValues(alpha: 0.0),
          ],
          stops: const [0.0, 0.3, 0.6, 1.0],
        ),
        boxShadow: [
          // Inner glow
          BoxShadow(
            color: OnboardingColors.sunWarm.withValues(alpha: 0.5),
            blurRadius: size * 0.3,
            spreadRadius: size * 0.1,
          ),
          // Outer atmospheric glow
          BoxShadow(
            color: OnboardingColors.sunWarm.withValues(alpha: 0.25),
            blurRadius: size * 0.6,
            spreadRadius: size * 0.2,
          ),
        ],
      ),
    );
  }
}

/// Paints the sleep arc (night zone) between bedtime and wake time.
class _SleepArcPainter extends CustomPainter {
  final double bedtimeAngle;
  final double wakeTimeAngle;
  final double orbitRadius;
  final double glowIntensity;

  _SleepArcPainter({
    required this.bedtimeAngle,
    required this.wakeTimeAngle,
    required this.orbitRadius,
    required this.glowIntensity,
  });

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);

    // Calculate sweep angle (bedtime to wake time, going clockwise through night)
    double sweepAngle = wakeTimeAngle - bedtimeAngle;
    if (sweepAngle <= 0) {
      sweepAngle += 2 * math.pi;
    }

    // Create gradient for the sleep arc
    final rect = Rect.fromCircle(center: center, radius: orbitRadius);
    final gradient = SweepGradient(
      startAngle: bedtimeAngle + math.pi / 2,
      endAngle: bedtimeAngle + math.pi / 2 + sweepAngle,
      colors: [
        OnboardingColors.nightIndigo.withValues(alpha: 0.7 * glowIntensity),
        OnboardingColors.sleepArc.withValues(alpha: 0.5 * glowIntensity),
        OnboardingColors.nightIndigo.withValues(alpha: 0.7 * glowIntensity),
      ],
      stops: const [0.0, 0.5, 1.0],
    );

    // Draw the arc with a soft glow
    final paint = Paint()
      ..shader = gradient.createShader(rect)
      ..style = PaintingStyle.stroke
      ..strokeWidth = 8
      ..strokeCap = StrokeCap.round;

    canvas.drawArc(
      Rect.fromCircle(center: center, radius: orbitRadius),
      bedtimeAngle,
      sweepAngle,
      false,
      paint,
    );

    // Add subtle outer glow
    final glowPaint = Paint()
      ..color = OnboardingColors.moonGlow.withValues(alpha: 0.15 * glowIntensity)
      ..style = PaintingStyle.stroke
      ..strokeWidth = 16
      ..strokeCap = StrokeCap.round
      ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 8);

    canvas.drawArc(
      Rect.fromCircle(center: center, radius: orbitRadius),
      bedtimeAngle,
      sweepAngle,
      false,
      glowPaint,
    );
  }

  @override
  bool shouldRepaint(covariant _SleepArcPainter oldDelegate) {
    return oldDelegate.glowIntensity != glowIntensity ||
        oldDelegate.bedtimeAngle != bedtimeAngle ||
        oldDelegate.wakeTimeAngle != wakeTimeAngle;
  }
}

/// Paints subtle hour markers around the orbit.
class _HourMarkersPainter extends CustomPainter {
  final double orbitRadius;

  _HourMarkersPainter({required this.orbitRadius});

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);

    // Draw markers for 0, 6, 12, 18 hours (cardinal points)
    final majorHours = [0, 6, 12, 18];

    for (final hour in majorHours) {
      final angle = timeToAngle(hour, 0);
      final innerRadius = orbitRadius + 12;
      final outerRadius = orbitRadius + 18;

      final innerPoint = Offset(
        center.dx + innerRadius * math.cos(angle),
        center.dy + innerRadius * math.sin(angle),
      );
      final outerPoint = Offset(
        center.dx + outerRadius * math.cos(angle),
        center.dy + outerRadius * math.sin(angle),
      );

      final paint = Paint()
        ..color = OnboardingColors.orbitRing.withValues(alpha: 0.5)
        ..strokeWidth = 2
        ..strokeCap = StrokeCap.round;

      canvas.drawLine(innerPoint, outerPoint, paint);
    }

    // Draw minor tick marks for every 3 hours
    final minorHours = [3, 9, 15, 21];

    for (final hour in minorHours) {
      final angle = timeToAngle(hour, 0);
      final innerRadius = orbitRadius + 12;
      final outerRadius = orbitRadius + 15;

      final innerPoint = Offset(
        center.dx + innerRadius * math.cos(angle),
        center.dy + innerRadius * math.sin(angle),
      );
      final outerPoint = Offset(
        center.dx + outerRadius * math.cos(angle),
        center.dy + outerRadius * math.sin(angle),
      );

      final paint = Paint()
        ..color = OnboardingColors.orbitRing.withValues(alpha: 0.3)
        ..strokeWidth = 1.5
        ..strokeCap = StrokeCap.round;

      canvas.drawLine(innerPoint, outerPoint, paint);
    }
  }

  @override
  bool shouldRepaint(covariant _HourMarkersPainter oldDelegate) => false;
}

/// Paints a stylized sunrise icon (half sun on horizon).
class _SunriseIconPainter extends CustomPainter {
  final double glowIntensity;

  _SunriseIconPainter({required this.glowIntensity});

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);
    final sunRadius = size.width * 0.32;

    // Clip to show only top half of sun (sunrise effect)
    canvas.save();
    canvas.clipRect(Rect.fromLTWH(0, 0, size.width, size.height / 2 + 2));

    // Sun body gradient
    final sunGradient = RadialGradient(
      center: const Alignment(0, 0.5),
      colors: [
        Colors.white,
        OnboardingColors.sunWarm,
        const Color(0xFFFF8F00),
      ],
      stops: const [0.0, 0.5, 1.0],
    );

    final sunPaint = Paint()
      ..shader = sunGradient.createShader(
        Rect.fromCircle(center: Offset(center.dx, center.dy + sunRadius * 0.3), radius: sunRadius),
      );

    canvas.drawCircle(
      Offset(center.dx, center.dy + sunRadius * 0.3),
      sunRadius,
      sunPaint,
    );

    canvas.restore();

    // Horizon line
    final horizonPaint = Paint()
      ..color = OnboardingColors.sunWarm.withValues(alpha: 0.8)
      ..strokeWidth = 2
      ..strokeCap = StrokeCap.round;

    canvas.drawLine(
      Offset(center.dx - size.width * 0.35, center.dy),
      Offset(center.dx + size.width * 0.35, center.dy),
      horizonPaint,
    );

    // Sun rays
    final rayPaint = Paint()
      ..color = OnboardingColors.sunWarm.withValues(alpha: 0.6 * glowIntensity)
      ..strokeWidth = 1.5
      ..strokeCap = StrokeCap.round;

    final rayAngles = [-math.pi * 0.75, -math.pi * 0.5, -math.pi * 0.25];
    final rayInnerRadius = sunRadius + 2;
    final rayOuterRadius = sunRadius + 6;

    for (final angle in rayAngles) {
      final innerPoint = Offset(
        center.dx + rayInnerRadius * math.cos(angle),
        center.dy + sunRadius * 0.3 + rayInnerRadius * math.sin(angle),
      );
      final outerPoint = Offset(
        center.dx + rayOuterRadius * math.cos(angle),
        center.dy + sunRadius * 0.3 + rayOuterRadius * math.sin(angle),
      );

      if (innerPoint.dy < center.dy && outerPoint.dy < center.dy) {
        canvas.drawLine(innerPoint, outerPoint, rayPaint);
      }
    }
  }

  @override
  bool shouldRepaint(covariant _SunriseIconPainter oldDelegate) {
    return oldDelegate.glowIntensity != glowIntensity;
  }
}
