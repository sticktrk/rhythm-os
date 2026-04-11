import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:rhythm_core/rhythm_core.dart';

import 'solar_clock_data.dart';
import 'solar_clock_geometry.dart';

class SolarArcPainter extends CustomPainter {
  final SolarClockData data;
  final SolarClockGeometry geometry;
  final bool use24;
  final SolarCurveSamples? curveData;
  final double arcStrokeWidth;
  final bool showEventMarkers;
  final bool showHourLabels;
  final bool showLowerArc;

  const SolarArcPainter({
    required this.data,
    required this.geometry,
    required this.use24,
    this.curveData,
    this.arcStrokeWidth = 4.0,
    this.showEventMarkers = true,
    this.showHourLabels = true,
    this.showLowerArc = true,
  });

  @override
  void paint(Canvas canvas, Size size) {
    _drawUpperArc(canvas);
    if (showLowerArc) _drawLowerArc(canvas);
    if (showHourLabels) _drawHourLabels(canvas);
    if (showEventMarkers && !data.isPolarDay && !data.isPolarNight) {
      _drawEventMarkers(canvas);
    }
  }

  void _drawUpperArc(Canvas canvas) {
    final rect = geometry.arcRect;
    final samples = curveData;

    if (samples != null && !samples.isEmpty) {
      const segments = 48;
      const segSweep = math.pi / segments;
      final paint = Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = arcStrokeWidth
        ..strokeCap = StrokeCap.butt;

      for (int i = 0; i < segments; i++) {
        final startAngle = -math.pi + i * segSweep;
        final midHour = SolarUtils.angleToHour(
          startAngle + segSweep / 2,
          data.solarNoon,
        );
        final (color, opacity) = _curveStyleAt(midHour);
        paint.color = color.withValues(alpha: opacity);
        canvas.drawArc(rect, startAngle, segSweep + 0.02, false, paint);
      }
      return;
    }

    canvas.drawArc(
      rect,
      -math.pi,
      math.pi,
      false,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = arcStrokeWidth
        ..shader = LinearGradient(
          begin: Alignment.centerLeft,
          end: Alignment.centerRight,
          colors: [
            const Color(0xFFF0A830).withValues(alpha: 0.20),
            const Color(0xFFF0A830).withValues(alpha: 0.55),
            const Color(0xFFF0A830).withValues(alpha: 0.20),
          ],
        ).createShader(rect),
    );
  }

  void _drawLowerArc(Canvas canvas) {
    final rect = geometry.arcRect;
    final dashPaint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 3.0
      ..strokeCap = StrokeCap.round;

    if (data.isPolarDay || data.isPolarNight) {
      final segments = (geometry.radius * math.pi / 8.0).round();
      for (int i = 0; i < segments; i++) {
        dashPaint.color = Colors.white.withValues(alpha: 0.20);
        canvas.drawArc(
          rect,
          (i / segments) * math.pi,
          math.pi / segments * 0.4,
          false,
          dashPaint,
        );
      }
      return;
    }

    final astronomicalDusk =
        data.twilightTimes.dusk.astronomical ?? data.sunset;
    final astronomicalDawn =
        data.twilightTimes.dawn.astronomical ?? data.sunrise;

    final duskAngle = SolarUtils.hourToAngle(astronomicalDusk, data.solarNoon);
    if (duskAngle > 0.01) {
      final segments =
          (geometry.radius * duskAngle / 8.0).round().clamp(1, 100);
      final dashSweep = duskAngle / segments;
      for (int i = 0; i < segments; i++) {
        final dashStart = (i / segments) * duskAngle;
        final midHour = SolarUtils.angleToHour(
          dashStart + dashSweep * 0.2,
          data.solarNoon,
        );
        final (color, opacity) = _curveStyleAt(midHour);
        dashPaint.color = color.withValues(alpha: opacity * 0.5);
        canvas.drawArc(rect, dashStart, dashSweep * 0.4, false, dashPaint);
      }
    }

    var dawnAngle = SolarUtils.hourToAngle(astronomicalDawn, data.solarNoon);
    if (dawnAngle < 0) dawnAngle += 2 * math.pi;
    final dawnSweep = math.pi - dawnAngle;
    if (dawnSweep > 0.01) {
      final segments =
          (geometry.radius * dawnSweep / 8.0).round().clamp(1, 100);
      final dashSweep = dawnSweep / segments;
      for (int i = 0; i < segments; i++) {
        final dashStart = dawnAngle + (i / segments) * dawnSweep;
        final midHour = SolarUtils.angleToHour(
          dashStart + dashSweep * 0.2,
          data.solarNoon,
        );
        final (color, opacity) = _curveStyleAt(midHour);
        dashPaint.color = color.withValues(alpha: opacity * 0.5);
        canvas.drawArc(rect, dashStart, dashSweep * 0.4, false, dashPaint);
      }
    }
  }

  void _drawHourLabels(Canvas canvas) {
    const labels = [0, 3, 6, 9, 12, 15, 18, 21];

    for (final hour in labels) {
      final angle = SolarUtils.hourToAngle(hour.toDouble(), data.solarNoon);
      final cosValue = math.cos(angle);
      final sinValue = math.sin(angle);
      final isUpperArc = sinValue < 0;
      final label = use24
          ? hour.toString().padLeft(2, '0')
          : hour == 0
              ? '12a'
              : hour == 12
                  ? '12p'
                  : hour < 12
                      ? '${hour}a'
                      : '${hour - 12}p';
      final textPainter = TextPainter(
        text: TextSpan(
          text: label,
          style: TextStyle(
            color: Colors.white.withValues(alpha: isUpperArc ? 0.30 : 0.18),
            fontSize: 9,
            fontWeight: FontWeight.w500,
            letterSpacing: 0.5,
          ),
        ),
        textDirection: TextDirection.ltr,
      )..layout();
      final distance = geometry.radius + 18;
      final position = Offset(
        geometry.center.dx + distance * cosValue - textPainter.width / 2,
        geometry.center.dy + distance * sinValue - textPainter.height / 2,
      );
      textPainter.paint(canvas, position);
    }
  }

  void _drawEventMarkers(Canvas canvas) {
    const fallbackDawn = Color(0xFFF0A830);
    const fallbackDusk = Color(0xFF7898D0);
    final tw = data.twilightTimes;

    Color tickColor(double hour, Color fallback) {
      final samples = curveData;
      if (samples == null || samples.isEmpty) return fallback;
      return SolarUtils.curveColorAt(
        hour,
        hours: samples.hours,
        kelvin: samples.kelvin,
        fallback: fallback,
      );
    }

    if (tw.dawn.astronomical != null) {
      _drawTwilightTick(
        canvas,
        tw.dawn.astronomical!,
        tickColor(tw.dawn.astronomical!, fallbackDawn),
        0.25,
        8,
        1.2,
      );
    }
    if (tw.dusk.astronomical != null) {
      _drawTwilightTick(
        canvas,
        tw.dusk.astronomical!,
        tickColor(tw.dusk.astronomical!, fallbackDusk),
        0.25,
        8,
        1.2,
      );
    }
    if (tw.dawn.nautical != null) {
      _drawTwilightTick(
        canvas,
        tw.dawn.nautical!,
        tickColor(tw.dawn.nautical!, fallbackDawn),
        0.45,
        10,
        1.5,
      );
    }
    if (tw.dusk.nautical != null) {
      _drawTwilightTick(
        canvas,
        tw.dusk.nautical!,
        tickColor(tw.dusk.nautical!, fallbackDusk),
        0.45,
        10,
        1.5,
      );
    }
    if (tw.dawn.civil != null) {
      _drawTwilightTick(
        canvas,
        tw.dawn.civil!,
        tickColor(tw.dawn.civil!, fallbackDawn),
        0.70,
        12,
        2.0,
      );
    }
    if (tw.dusk.civil != null) {
      _drawTwilightTick(
        canvas,
        tw.dusk.civil!,
        tickColor(tw.dusk.civil!, fallbackDusk),
        0.70,
        12,
        2.0,
      );
    }

    _drawSolarNoonMarker(canvas);
  }

  void _drawTwilightTick(
    Canvas canvas,
    double hour,
    Color color,
    double alpha,
    double extent,
    double stroke,
  ) {
    final angle = SolarUtils.hourToAngle(hour, data.solarNoon);
    final cosValue = math.cos(angle);
    final sinValue = math.sin(angle);
    final inner = Offset(
      geometry.center.dx + (geometry.radius - extent) * cosValue,
      geometry.center.dy + (geometry.radius - extent) * sinValue,
    );
    final outer = Offset(
      geometry.center.dx + (geometry.radius + extent) * cosValue,
      geometry.center.dy + (geometry.radius + extent) * sinValue,
    );

    canvas.drawLine(
      inner,
      outer,
      Paint()
        ..color = color.withValues(alpha: alpha)
        ..strokeWidth = stroke
        ..strokeCap = StrokeCap.round,
    );
    canvas.drawLine(
      inner,
      outer,
      Paint()
        ..color = color.withValues(alpha: alpha * 0.3)
        ..strokeWidth = stroke * 3.0
        ..strokeCap = StrokeCap.round
        ..maskFilter = MaskFilter.blur(BlurStyle.normal, stroke * 2.0),
    );
  }

  void _drawSolarNoonMarker(Canvas canvas) {
    final angle = SolarUtils.hourToAngle(data.solarNoon, data.solarNoon);
    final cosValue = math.cos(angle);
    final sinValue = math.sin(angle);
    const extent = 10.0;
    final inner = Offset(
      geometry.center.dx + (geometry.radius - extent) * cosValue,
      geometry.center.dy + (geometry.radius - extent) * sinValue,
    );
    final outer = Offset(
      geometry.center.dx + (geometry.radius + extent) * cosValue,
      geometry.center.dy + (geometry.radius + extent) * sinValue,
    );

    const color = Color(0xFFE8D5A8);
    canvas.drawLine(
      inner,
      outer,
      Paint()
        ..color = color
        ..strokeWidth = 2.0
        ..strokeCap = StrokeCap.round,
    );
    canvas.drawLine(
      inner,
      outer,
      Paint()
        ..color = color.withValues(alpha: 0.20)
        ..strokeWidth = 6.0
        ..strokeCap = StrokeCap.round
        ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 4),
    );
  }

  (Color, double) _curveStyleAt(double hour) {
    final samples = curveData;
    if (samples == null || samples.isEmpty) {
      return (const Color(0xFFF0A830), 0.55);
    }

    final brightness = SolarUtils.interpolateValue(
      samples.hours,
      samples.brightness,
      hour,
    );
    final color = SolarUtils.curveColorAt(
      hour,
      hours: samples.hours,
      kelvin: samples.kelvin,
    );
    final opacity = 0.20 + (brightness / 100) * 0.55;
    return (color, opacity);
  }

  @override
  bool shouldRepaint(covariant SolarArcPainter oldDelegate) {
    return data != oldDelegate.data ||
        geometry != oldDelegate.geometry ||
        use24 != oldDelegate.use24 ||
        curveData != oldDelegate.curveData ||
        arcStrokeWidth != oldDelegate.arcStrokeWidth ||
        showEventMarkers != oldDelegate.showEventMarkers ||
        showHourLabels != oldDelegate.showHourLabels ||
        showLowerArc != oldDelegate.showLowerArc;
  }
}
