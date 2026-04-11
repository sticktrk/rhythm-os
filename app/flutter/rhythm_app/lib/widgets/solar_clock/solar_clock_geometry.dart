import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:rhythm_core/rhythm_core.dart';

class SolarClockGeometry {
  final Offset center;
  final double radius;
  final double solarNoon;

  const SolarClockGeometry({
    required this.center,
    required this.radius,
    required this.solarNoon,
  });

  Rect get arcRect => Rect.fromCircle(center: center, radius: radius);

  Offset positionForHour(double hour) {
    final angle = SolarUtils.hourToAngle(hour, solarNoon);
    return Offset(
      center.dx + radius * math.cos(angle),
      center.dy + radius * math.sin(angle),
    );
  }

  double hourFromPosition(Offset position) {
    final angle = math.atan2(position.dy - center.dy, position.dx - center.dx);
    return SolarUtils.angleToHour(angle, solarNoon);
  }

  factory SolarClockGeometry.fromConstraints(
    BoxConstraints constraints, {
    required double solarNoon,
    double horizonFactor = 0.52,
    double radiusWidthFactor = 0.42,
    double radiusHeightFactor = 0.55,
  }) {
    assert(
      constraints.hasBoundedWidth && constraints.hasBoundedHeight,
      'SolarClockGeometry requires bounded width and height.',
    );
    final size = constraints.biggest;
    final horizonY = size.height * horizonFactor;
    return SolarClockGeometry(
      center: Offset(size.width / 2, horizonY),
      radius: math.min(
          size.width * radiusWidthFactor, horizonY * radiusHeightFactor),
      solarNoon: solarNoon,
    );
  }
}
