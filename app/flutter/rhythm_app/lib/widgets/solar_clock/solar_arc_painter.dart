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
  final bool showUpperArc;
  final bool showEventMarkers;
  final bool showHourLabels;
  final bool showLowerArc;
  final bool showFullLowerArc;

  const SolarArcPainter({
    required this.data,
    required this.geometry,
    required this.use24,
    this.curveData,
    this.arcStrokeWidth = 4.0,
    this.showUpperArc = true,
    this.showEventMarkers = true,
    this.showHourLabels = true,
    this.showLowerArc = true,
    this.showFullLowerArc = false,
  });

  @override
  void paint(Canvas canvas, Size size) {
    if (showUpperArc) _drawUpperArc(canvas);
    if (showLowerArc) _drawLowerArc(canvas);
    if (showHourLabels) _drawHourLabels(canvas);
    if (showEventMarkers) {
      _drawEventMarkers(canvas);
    }
  }

  void _drawUpperArc(Canvas canvas) {
    if (geometry.orientation == SolarClockOrientation.standard24) {
      _drawStandardDayArc(canvas);
      return;
    }

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
        final midHour = geometry.hourForAngle(startAngle + segSweep / 2);
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
    if (geometry.orientation == SolarClockOrientation.standard24) {
      _drawStandardNightArc(canvas);
      return;
    }

    final rect = geometry.arcRect;
    final dashPaint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 3.0
      ..strokeCap = StrokeCap.round;

    if (showFullLowerArc) {
      _drawFullLowerArc(canvas, rect, dashPaint);
      return;
    }

    if (data.isPolarDay || data.isPolarNight) {
      _drawFullLowerArc(canvas, rect, dashPaint);
      return;
    }

    final astronomicalDusk =
        data.twilightTimes.dusk.astronomical ?? data.sunset;
    final astronomicalDawn =
        data.twilightTimes.dawn.astronomical ?? data.sunrise;

    final duskAngle = geometry.angleForHour(astronomicalDusk);
    if (duskAngle > 0.01) {
      final segments =
          (geometry.radius * duskAngle / 8.0).round().clamp(1, 100);
      final dashSweep = duskAngle / segments;
      for (int i = 0; i < segments; i++) {
        final dashStart = (i / segments) * duskAngle;
        final midHour = geometry.hourForAngle(dashStart + dashSweep * 0.2);
        final (color, opacity) = _curveStyleAt(midHour);
        dashPaint.color = color.withValues(alpha: opacity * 0.5);
        canvas.drawArc(rect, dashStart, dashSweep * 0.4, false, dashPaint);
      }
    }

    var dawnAngle = geometry.angleForHour(astronomicalDawn);
    if (dawnAngle < 0) dawnAngle += 2 * math.pi;
    final dawnSweep = math.pi - dawnAngle;
    if (dawnSweep > 0.01) {
      final segments =
          (geometry.radius * dawnSweep / 8.0).round().clamp(1, 100);
      final dashSweep = dawnSweep / segments;
      for (int i = 0; i < segments; i++) {
        final dashStart = dawnAngle + (i / segments) * dawnSweep;
        final midHour = geometry.hourForAngle(dashStart + dashSweep * 0.2);
        final (color, opacity) = _curveStyleAt(midHour);
        dashPaint.color = color.withValues(alpha: opacity * 0.5);
        canvas.drawArc(rect, dashStart, dashSweep * 0.4, false, dashPaint);
      }
    }
  }

  void _drawFullLowerArc(Canvas canvas, Rect rect, Paint dashPaint) {
    final segments = (geometry.radius * math.pi / 8.0).round().clamp(1, 140);
    final dashSweep = math.pi / segments;

    for (int i = 0; i < segments; i++) {
      final dashStart = (i / segments) * math.pi;
      final midHour = geometry.hourForAngle(dashStart + dashSweep * 0.2);
      final (color, opacity) = _curveStyleAt(midHour);
      dashPaint.color = curveData == null || curveData!.isEmpty
          ? Colors.white.withValues(alpha: 0.20)
          : color.withValues(alpha: opacity * 0.5);
      canvas.drawArc(rect, dashStart, dashSweep * 0.4, false, dashPaint);
    }
  }

  void _drawStandardDayArc(Canvas canvas) {
    if (data.isPolarNight) return;

    _drawHourRange(
      canvas,
      startHour: data.isPolarDay ? 0.0 : data.sunrise,
      endHour: data.isPolarDay ? 24.0 : data.sunset,
      fullCircle: data.isPolarDay,
      dashed: false,
      strokeWidth: arcStrokeWidth,
    );
  }

  void _drawStandardNightArc(Canvas canvas) {
    if (data.isPolarDay) return;

    final astronomicalDusk =
        data.twilightTimes.dusk.astronomical ?? data.sunset;
    final astronomicalDawn =
        data.twilightTimes.dawn.astronomical ?? data.sunrise;

    _drawHourRange(
      canvas,
      startHour: data.isPolarNight
          ? 0.0
          : showFullLowerArc
              ? data.sunset
              : astronomicalDusk,
      endHour: data.isPolarNight
          ? 24.0
          : showFullLowerArc
              ? data.sunrise
              : astronomicalDawn,
      fullCircle: data.isPolarNight,
      dashed: true,
      strokeWidth: 3.0,
      opacityMultiplier: 0.5,
      noCurveColor: Colors.white.withValues(alpha: 0.20),
    );
  }

  void _drawHourRange(
    Canvas canvas, {
    required double startHour,
    required double endHour,
    required bool dashed,
    required double strokeWidth,
    bool fullCircle = false,
    double opacityMultiplier = 1.0,
    Color? noCurveColor,
  }) {
    final hourSpan = fullCircle ? 24.0 : _clockwiseHourSpan(startHour, endHour);
    if (hourSpan <= 0.01) return;

    final rect = geometry.arcRect;
    final segments = dashed
        ? (geometry.radius * (hourSpan / 24.0) * 2 * math.pi / 8.0)
            .round()
            .clamp(1, 140)
        : (48 * (hourSpan / 24.0)).round().clamp(1, 96);
    final hourStep = hourSpan / segments;
    final sweep = (2 * math.pi * hourStep) / 24.0;
    final paint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = strokeWidth
      ..strokeCap = dashed ? StrokeCap.round : StrokeCap.butt;

    for (int i = 0; i < segments; i++) {
      final segmentStartHour = fullCircle
          ? i * hourStep
          : SolarUtils.normalizeHour(startHour + i * hourStep);
      final midHour = fullCircle
          ? (i + 0.5) * hourStep
          : SolarUtils.normalizeHour(startHour + (i + 0.5) * hourStep);
      final (color, opacity) = _curveStyleAt(midHour);
      paint.color = curveData == null || curveData!.isEmpty
          ? noCurveColor ?? color.withValues(alpha: opacity * opacityMultiplier)
          : color.withValues(alpha: opacity * opacityMultiplier);
      canvas.drawArc(
        rect,
        geometry.angleForHour(segmentStartHour),
        dashed ? sweep * 0.4 : sweep + 0.02,
        false,
        paint,
      );
    }
  }

  double _clockwiseHourSpan(double startHour, double endHour) {
    final span = SolarUtils.normalizeHour(endHour - startHour);
    if (span == 0.0 && startHour != endHour) return 24.0;
    return span;
  }

  void _drawHourLabels(Canvas canvas) {
    const labels = [0, 3, 6, 9, 12, 15, 18, 21];

    for (final hour in labels) {
      final angle = geometry.angleForHour(hour.toDouble());
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

    if (!data.isPolarDay && !data.isPolarNight) {
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
    final angle = geometry.angleForHour(hour);
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
    final angle = geometry.angleForHour(data.solarNoon);
    final cosValue = math.cos(angle);
    final sinValue = math.sin(angle);
    final extent =
        geometry.orientation == SolarClockOrientation.standard24 ? 12.0 : 10.0;
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
        showUpperArc != oldDelegate.showUpperArc ||
        showEventMarkers != oldDelegate.showEventMarkers ||
        showHourLabels != oldDelegate.showHourLabels ||
        showLowerArc != oldDelegate.showLowerArc ||
        showFullLowerArc != oldDelegate.showFullLowerArc;
  }
}
