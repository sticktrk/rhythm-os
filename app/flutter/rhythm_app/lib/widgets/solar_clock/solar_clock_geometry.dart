import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:rhythm_core/rhythm_core.dart';

enum SolarClockOrientation {
  solarAligned,
  standard24,
}

class SolarClockGeometry {
  final Offset center;
  final double radius;
  final double solarNoon;
  final SolarClockOrientation orientation;

  const SolarClockGeometry({
    required this.center,
    required this.radius,
    required this.solarNoon,
    this.orientation = SolarClockOrientation.solarAligned,
  });

  Rect get arcRect => Rect.fromCircle(center: center, radius: radius);

  double angleForHour(double hour) {
    return switch (orientation) {
      SolarClockOrientation.solarAligned =>
        SolarUtils.hourToAngle(hour, solarNoon),
      SolarClockOrientation.standard24 => -math.pi / 2 +
          (SolarUtils.normalizeHour(hour - 12.0) / 24.0) * 2 * math.pi,
    };
  }

  double hourForAngle(double angle) {
    return switch (orientation) {
      SolarClockOrientation.solarAligned =>
        SolarUtils.angleToHour(angle, solarNoon),
      SolarClockOrientation.standard24 => SolarUtils.normalizeHour(
          12.0 + (angle + math.pi / 2) * 24.0 / (2 * math.pi),
        ),
    };
  }

  Offset positionForHour(double hour) {
    final angle = angleForHour(hour);
    return Offset(
      center.dx + radius * math.cos(angle),
      center.dy + radius * math.sin(angle),
    );
  }

  double hourFromPosition(Offset position) {
    final angle = math.atan2(position.dy - center.dy, position.dx - center.dx);
    return hourForAngle(angle);
  }

  factory SolarClockGeometry.fromConstraints(
    BoxConstraints constraints, {
    required double solarNoon,
    SolarClockOrientation orientation = SolarClockOrientation.solarAligned,
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
      orientation: orientation,
    );
  }
}
