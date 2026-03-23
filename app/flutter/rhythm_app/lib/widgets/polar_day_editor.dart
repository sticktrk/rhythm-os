import 'dart:math' as math;
import 'dart:ui' as ui;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:rhythm_core/rhythm_core.dart';

/// Polar Day Editor - Edit Mode for shaping sunrise/sunset transitions.
///
/// A meditative, clock-like interface where:
/// - Angle = time of day (circular, no start/end)
/// - Distance from center = brightness
/// - Ring color = color temperature
///
/// The editor focuses on the two important moments: sunrise and sunset.
/// Users shape how light wakes them up and winds them down.
class PolarDayEditor extends StatefulWidget {
  final CurveData? curveData;
  final CurveConfigDto config;
  final ValueChanged<CurveConfigDto>? onConfigChanged;
  final VoidCallback? onConfigChangeEnd;

  const PolarDayEditor({
    super.key,
    required this.curveData,
    required this.config,
    this.onConfigChanged,
    this.onConfigChangeEnd,
  });

  /// Get solar info from curve data
  SolarInfo? get solarInfo => curveData?.solar;

  @override
  State<PolarDayEditor> createState() => _PolarDayEditorState();
}

class _PolarDayEditorState extends State<PolarDayEditor>
    with TickerProviderStateMixin {
  // Animation controllers
  late AnimationController _pulseController;
  late AnimationController _entryController;
  late Animation<double> _entryAnimation;

  // Drag state
  _DragTarget? _activeDrag;
  Offset? _dragStart;
  double? _dragStartValue;

  // Current time indicator
  double get _nowHour {
    final now = DateTime.now();
    return now.hour + now.minute / 60.0;
  }

  @override
  void initState() {
    super.initState();

    // Subtle pulse for the sun
    _pulseController = AnimationController(
      duration: const Duration(milliseconds: 4000),
      vsync: this,
    )..repeat(reverse: true);

    // Entry animation
    _entryController = AnimationController(
      duration: const Duration(milliseconds: 1200),
      vsync: this,
    );
    _entryAnimation = CurvedAnimation(
      parent: _entryController,
      curve: Curves.easeOutExpo,
    );
    _entryController.forward();
  }

  @override
  void dispose() {
    _pulseController.dispose();
    _entryController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        final size = math.min(constraints.maxWidth, constraints.maxHeight);

        return AnimatedBuilder(
          animation: Listenable.merge([_pulseController, _entryAnimation]),
          builder: (context, child) {
            return GestureDetector(
              behavior: HitTestBehavior.opaque,
              onPanStart: (details) => _onPanStart(details, size),
              onPanUpdate: (details) => _onPanUpdate(details, size),
              onPanEnd: _onPanEnd,
              child: CustomPaint(
                size: Size(size, size),
                painter: _PolarDayPainter(
                  curveData: widget.curveData,
                  config: widget.config,
                  solarInfo: widget.solarInfo,
                  nowHour: _nowHour,
                  pulseValue: _pulseController.value,
                  entryValue: _entryAnimation.value,
                  activeDrag: _activeDrag,
                  currentBrightness: _getBrightnessAtHour(_nowHour),
                  currentKelvin: _getKelvinAtHour(_nowHour),
                ),
              ),
            );
          },
        );
      },
    );
  }

  void _onPanStart(DragStartDetails details, double size) {
    final center = Offset(size / 2, size / 2);
    final localPos = details.localPosition;
    final target = _hitTest(localPos, center, size);

    if (target != null) {
      HapticFeedback.mediumImpact();
      setState(() {
        _activeDrag = target;
        _dragStart = localPos;
        _dragStartValue = _getValueForTarget(target);
      });
    }
  }

  void _onPanUpdate(DragUpdateDetails details, double size) {
    if (_activeDrag == null || _dragStart == null || _dragStartValue == null) {
      return;
    }

    final center = Offset(size / 2, size / 2);
    final localPos = details.localPosition;

    // Calculate radial delta (distance change from center)
    final startDist = (_dragStart! - center).distance;
    final currentDist = (localPos - center).distance;
    final radialDelta = (currentDist - startDist) / size;

    // Map to parameter change based on drag target
    CurveConfigDto? newConfig;

    switch (_activeDrag!) {
      case _DragTarget.sunriseTransition:
        // Radial drag adjusts width (transition speed)
        // Outward = slower/gentler transition (more gradual wake-up)
        // Inward = faster/steeper transition (quicker ramp)
        final newWidth =
            (_dragStartValue! + radialDelta * 3.0).clamp(0.3, 2.0);
        newConfig = widget.config.copyWith(widthLeftBri: newWidth);
        break;

      case _DragTarget.sunsetTransition:
        final newWidth =
            (_dragStartValue! + radialDelta * 3.0).clamp(0.3, 2.0);
        newConfig = widget.config.copyWith(widthRightBri: newWidth);
        break;

      case _DragTarget.peakShape:
        // Vertical drag adjusts shape (peak flatness)
        // Up = flatter plateau, Down = rounder peak
        final verticalDelta = (_dragStart!.dy - localPos.dy) / size;
        final newShape =
            (_dragStartValue! + verticalDelta * 8.0).clamp(2.0, 10.0);
        newConfig = widget.config.copyWith(shapeP: newShape);
        break;
    }

    if (newConfig != null) {
      HapticFeedback.selectionClick();
      widget.onConfigChanged?.call(newConfig);
    }
  }

  void _onPanEnd(DragEndDetails details) {
    if (_activeDrag != null) {
      widget.onConfigChangeEnd?.call();
      setState(() {
        _activeDrag = null;
        _dragStart = null;
        _dragStartValue = null;
      });
    }
  }

  _DragTarget? _hitTest(Offset position, Offset center, double size) {
    final maxRadius = size / 2 * 0.85;
    final ringRadius = maxRadius * 0.75;
    final minHandleRadius = maxRadius * 0.35;
    final maxHandleRadius = ringRadius - maxRadius * 0.02;

    // Get solar times
    final sunrise = widget.solarInfo?.sunrise ?? 6.0;
    final sunset = widget.solarInfo?.sunset ?? 18.0;
    final solarNoon = widget.solarInfo?.solarNoon ?? 12.0;

    // Helper to get handle position
    Offset getHandlePos(double hour, double paramValue, bool isPeak) {
      double normalizedParam;
      if (isPeak) {
        normalizedParam = ((paramValue - 2.0) / 8.0).clamp(0.0, 1.0);
      } else {
        normalizedParam = ((paramValue - 0.3) / 1.7).clamp(0.0, 1.0);
      }
      final handleRadius = minHandleRadius + (maxHandleRadius - minHandleRadius) * normalizedParam;
      final angle = _hourToAngle(hour);
      return Offset(
        center.dx + handleRadius * math.cos(angle),
        center.dy + handleRadius * math.sin(angle),
      );
    }

    final hitRadius = maxRadius * 0.08; // Touch target size

    // Check each handle
    final morningPos = getHandlePos(sunrise, widget.config.widthLeftBri, false);
    if ((position - morningPos).distance < hitRadius) {
      return _DragTarget.sunriseTransition;
    }

    final eveningPos = getHandlePos(sunset, widget.config.widthRightBri, false);
    if ((position - eveningPos).distance < hitRadius) {
      return _DragTarget.sunsetTransition;
    }

    final peakPos = getHandlePos(solarNoon, widget.config.shapeP, true);
    if ((position - peakPos).distance < hitRadius) {
      return _DragTarget.peakShape;
    }

    return null;
  }

  bool _isInAngularZone(double hour, double start, double end) {
    // Handle wrap-around at midnight
    if (start < 0) start += 24;
    if (end > 24) end -= 24;

    if (start < end) {
      return hour >= start && hour <= end;
    } else {
      // Wraps around midnight
      return hour >= start || hour <= end;
    }
  }

  double _getValueForTarget(_DragTarget target) {
    switch (target) {
      case _DragTarget.sunriseTransition:
        return widget.config.widthLeftBri;
      case _DragTarget.sunsetTransition:
        return widget.config.widthRightBri;
      case _DragTarget.peakShape:
        return widget.config.shapeP;
    }
  }

  double _angleToHour(double angle) {
    // Add PI/2 to shift so top is 0, then normalize to 0-24
    double normalizedAngle = angle + math.pi / 2;
    if (normalizedAngle < 0) normalizedAngle += 2 * math.pi;
    return (normalizedAngle / (2 * math.pi)) * 24;
  }

  double _hourToAngle(double hour) {
    // Map 0-24 hours to 0-2*PI, starting from top (-PI/2)
    return (hour / 24) * 2 * math.pi - math.pi / 2;
  }

  int _getBrightnessAtHour(double hour) {
    if (widget.curveData == null) return 50;
    final data = widget.curveData!;
    if (data.hours.isEmpty) return 50;

    int idx = 0;
    double minDiff = double.infinity;
    for (int i = 0; i < data.hours.length; i++) {
      final diff = (data.hours[i] - hour).abs();
      if (diff < minDiff) {
        minDiff = diff;
        idx = i;
      }
    }
    return data.brightness[idx];
  }

  int _getKelvinAtHour(double hour) {
    if (widget.curveData == null) return 4000;
    final data = widget.curveData!;
    if (data.hours.isEmpty) return 4000;

    int idx = 0;
    double minDiff = double.infinity;
    for (int i = 0; i < data.hours.length; i++) {
      final diff = (data.hours[i] - hour).abs();
      if (diff < minDiff) {
        minDiff = diff;
        idx = i;
      }
    }
    return data.kelvin[idx];
  }
}

/// Drag targets for the editor
enum _DragTarget {
  sunriseTransition,
  sunsetTransition,
  peakShape,
}

/// Custom painter for the polar day visualization
class _PolarDayPainter extends CustomPainter {
  final CurveData? curveData;
  final CurveConfigDto config;
  final SolarInfo? solarInfo;
  final double nowHour;
  final double pulseValue;
  final double entryValue;
  final _DragTarget? activeDrag;
  final int currentBrightness;
  final int currentKelvin;

  // Color palette
  static const _voidColor = Color(0xFF0A0B14);
  static const _nightCore = Color(0xFF13152A);
  static const _nightDeep = Color(0xFF2A2D4E);
  static const _twilight = Color(0xFF7B6B99);
  static const _sunriseWarm = Color(0xFFFF9B6A);
  static const _sunsetAmber = Color(0xFFE8734A);
  static const _sunGlow = Color(0xFFFFBE5C);
  static const _sunCore = Color(0xFFFFF8E7);
  static const _textMuted = Color(0x66FFF8E7);
  static const _textHint = Color(0x33FFF8E7);

  _PolarDayPainter({
    required this.curveData,
    required this.config,
    this.solarInfo,
    required this.nowHour,
    required this.pulseValue,
    required this.entryValue,
    this.activeDrag,
    required this.currentBrightness,
    required this.currentKelvin,
  });

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);
    final maxRadius = size.width / 2 * 0.85;

    // Apply entry animation
    canvas.save();
    canvas.translate(center.dx, center.dy);
    canvas.scale(0.9 + 0.1 * entryValue);
    canvas.translate(-center.dx, -center.dy);

    // Draw layers
    _drawAtmosphere(canvas, center, maxRadius);
    _drawBrightnessRing(canvas, center, maxRadius);
    _drawCentralSun(canvas, center, maxRadius);
    _drawTimeMarkers(canvas, center, maxRadius);
    _drawTransitionHandles(canvas, center, maxRadius);
    _drawNowIndicator(canvas, center, maxRadius);

    canvas.restore();
  }

  void _drawAtmosphere(Canvas canvas, Offset center, double radius) {
    // Subtle radial gradient for atmosphere
    final paint = Paint()
      ..shader = ui.Gradient.radial(
        center,
        radius * 1.2,
        [
          _nightCore.withValues(alpha: 0.3 * entryValue),
          _voidColor.withValues(alpha: 0.0),
        ],
        [0.3, 1.0],
      );

    canvas.drawCircle(center, radius * 1.2, paint);
  }

  void _drawBrightnessRing(Canvas canvas, Offset center, double maxRadius) {
    if (curveData == null || curveData!.hours.isEmpty) {
      // Draw placeholder ring
      final paint = Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = maxRadius * 0.08
        ..color = _nightDeep.withValues(alpha: 0.5 * entryValue);
      canvas.drawCircle(center, maxRadius * 0.75, paint);
      return;
    }

    final data = curveData!;

    // Fixed ring radius (like SolarOrbit's orbital ring)
    final ringRadius = maxRadius * 0.75;
    final ringWidth = maxRadius * 0.08;

    const segments = 72;
    const sweepAngle = (2 * math.pi) / segments;

    // First pass: Draw glow extending inward based on brightness
    for (int i = 0; i < segments; i++) {
      final hour = (i / segments) * 24;
      final brightness = _interpolateValue(data.hours, data.brightness, hour);
      final kelvin = _interpolateValue(data.hours, data.kelvin, hour);

      final normBri = (brightness - config.minBrightness) /
          (config.maxBrightness - config.minBrightness);
      final clampedBri = normBri.clamp(0.0, 1.0);

      final cctColor = ColorUtils.curveColorForCCT(kelvin.round());
      final angle = _hourToAngle(hour);

      // Glow extends inward from ring - more brightness = longer glow reaching center
      // At max brightness, glow reaches close to center
      final glowLength = maxRadius * 0.5 * clampedBri; // Max 50% of radius
      final glowOpacity = 0.15 + clampedBri * 0.35; // 0.15 to 0.5

      if (glowLength > 0) {
        // Draw radial glow beam from ring toward center
        final outerX = center.dx + ringRadius * math.cos(angle);
        final outerY = center.dy + ringRadius * math.sin(angle);
        final innerRadius = ringRadius - glowLength;
        final innerX = center.dx + innerRadius * math.cos(angle);
        final innerY = center.dy + innerRadius * math.sin(angle);

        // Create gradient paint for the glow beam
        final glowPaint = Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = ringWidth * 1.5
          ..strokeCap = StrokeCap.round
          ..shader = ui.Gradient.linear(
            Offset(outerX, outerY),
            Offset(innerX, innerY),
            [
              cctColor.withValues(alpha: glowOpacity * entryValue),
              cctColor.withValues(alpha: 0.0),
            ],
          )
          ..maskFilter = MaskFilter.blur(BlurStyle.normal, ringWidth * 0.8);

        canvas.drawLine(
          Offset(outerX, outerY),
          Offset(innerX, innerY),
          glowPaint,
        );
      }
    }

    // Second pass: Draw the main CCT ring (like SolarOrbit's orbital ring)
    final ringPaint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = ringWidth
      ..strokeCap = StrokeCap.butt;

    final ringRect = Rect.fromCircle(center: center, radius: ringRadius);

    for (int i = 0; i < segments; i++) {
      final hour = (i / segments) * 24;
      final brightness = _interpolateValue(data.hours, data.brightness, hour);
      final kelvin = _interpolateValue(data.hours, data.kelvin, hour);

      final cctColor = ColorUtils.curveColorForCCT(kelvin.round());
      // Ring opacity based on brightness (matching SolarOrbit style)
      final opacity = (0.3 + (brightness / 100) * 0.7) * entryValue;

      ringPaint.color = cctColor.withValues(alpha: opacity);

      final startAngle = (i / segments) * 2 * math.pi - math.pi / 2;
      canvas.drawArc(ringRect, startAngle, sweepAngle + 0.02, false, ringPaint);
    }

    // Draw subtle outer glow on the ring
    final outerGlowPaint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = ringWidth * 0.5
      ..color = _sunGlow.withValues(alpha: 0.15 * entryValue)
      ..maskFilter = MaskFilter.blur(BlurStyle.normal, ringWidth * 0.5);
    canvas.drawCircle(center, ringRadius, outerGlowPaint);

    // Draw tick marks at hours (outside the ring)
    final tickRadius = ringRadius + ringWidth / 2 + maxRadius * 0.02;
    for (int h = 0; h < 24; h++) {
      final angle = _hourToAngle(h.toDouble());
      final isMajor = h % 6 == 0;
      final tickLength = isMajor ? maxRadius * 0.04 : maxRadius * 0.02;

      final tickPaint = Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = isMajor ? 2 : 1
        ..color = _textMuted.withValues(alpha: (isMajor ? 0.5 : 0.25) * entryValue);

      canvas.drawLine(
        Offset(center.dx + tickRadius * math.cos(angle), center.dy + tickRadius * math.sin(angle)),
        Offset(center.dx + (tickRadius + tickLength) * math.cos(angle), center.dy + (tickRadius + tickLength) * math.sin(angle)),
        tickPaint,
      );
    }
  }


  void _drawCentralSun(Canvas canvas, Offset center, double maxRadius) {
    // Get CCT color for current time
    final sunColor = ColorUtils.curveColorForCCT(currentKelvin);

    // Size scales with brightness (matching SolarOrbit proportions)
    // SolarOrbit: minSunSize = size * 0.15, maxSunSize = size * 0.35
    final minSize = maxRadius * 0.25;
    final maxSize = maxRadius * 0.55;
    final brightnessScale = currentBrightness / 100.0;
    final dynamicRadius = minSize + (maxSize - minSize) * brightnessScale;

    // Pulsing effect
    final pulseScale = 1.0 + 0.05 * pulseValue;
    final radius = dynamicRadius * pulseScale * entryValue;

    // Glow intensity based on brightness
    final glowOpacity = 0.3 + brightnessScale * 0.4;

    // Layer 1: Outermost atmospheric glow (very large, very diffuse)
    final atmospherePaint = Paint()
      ..shader = ui.Gradient.radial(
        center,
        radius * 4.0,
        [
          sunColor.withValues(alpha: glowOpacity * 0.4 * entryValue),
          sunColor.withValues(alpha: glowOpacity * 0.15 * entryValue),
          sunColor.withValues(alpha: 0.0),
        ],
        [0.0, 0.4, 1.0],
      );
    canvas.drawCircle(center, radius * 4.0, atmospherePaint);

    // Layer 2: Outer glow with blur
    final outerGlowPaint = Paint()
      ..shader = ui.Gradient.radial(
        center,
        radius * 2.5,
        [
          sunColor.withValues(alpha: glowOpacity * 0.5 * entryValue),
          sunColor.withValues(alpha: glowOpacity * 0.2 * entryValue),
          Colors.transparent,
        ],
        [0.0, 0.5, 1.0],
      )
      ..maskFilter = MaskFilter.blur(BlurStyle.normal, radius * 0.6);
    canvas.drawCircle(center, radius * 2.5, outerGlowPaint);

    // Layer 3: Inner bright glow
    final innerGlowPaint = Paint()
      ..shader = ui.Gradient.radial(
        center,
        radius * 1.5,
        [
          sunColor.withValues(alpha: glowOpacity * 0.8 * entryValue),
          sunColor.withValues(alpha: glowOpacity * 0.3 * entryValue),
          Colors.transparent,
        ],
        [0.0, 0.6, 1.0],
      )
      ..maskFilter = MaskFilter.blur(BlurStyle.normal, radius * 0.4);
    canvas.drawCircle(center, radius * 1.5, innerGlowPaint);

    // Layer 4: Sun body with white-hot center (matching SolarOrbit gradient)
    final sunPaint = Paint()
      ..shader = ui.Gradient.radial(
        center,
        radius,
        [
          Colors.white,
          sunColor,
          sunColor.withValues(alpha: 0.6),
          sunColor.withValues(alpha: 0.0),
        ],
        [0.0, 0.3, 0.6, 1.0],
      );
    canvas.drawCircle(center, radius, sunPaint);
  }

  void _drawTimeMarkers(Canvas canvas, Offset center, double radius) {
    final textPainter = TextPainter(
      textDirection: TextDirection.ltr,
    );

    // Cardinal times
    final markers = [
      (0.0, '12 AM'),
      (6.0, '6 AM'),
      (12.0, '12 PM'),
      (18.0, '6 PM'),
    ];

    for (final (hour, label) in markers) {
      final angle = _hourToAngle(hour);
      final labelRadius = radius * 1.08;
      final x = center.dx + labelRadius * math.cos(angle);
      final y = center.dy + labelRadius * math.sin(angle);

      textPainter.text = TextSpan(
        text: label,
        style: TextStyle(
          color: _textMuted.withValues(alpha: entryValue),
          fontSize: radius * 0.07,
          fontWeight: FontWeight.w400,
          letterSpacing: 0.5,
        ),
      );
      textPainter.layout();

      textPainter.paint(
        canvas,
        Offset(x - textPainter.width / 2, y - textPainter.height / 2),
      );
    }
  }

  void _drawTransitionHandles(Canvas canvas, Offset center, double maxRadius) {
    final sunrise = solarInfo?.sunrise ?? 6.0;
    final sunset = solarInfo?.sunset ?? 18.0;
    final solarNoon = solarInfo?.solarNoon ?? 12.0;

    // Draw handles at transition zones
    _drawHandle(
      canvas,
      center,
      maxRadius,
      sunrise,
      'morning',
      activeDrag == _DragTarget.sunriseTransition,
    );

    _drawHandle(
      canvas,
      center,
      maxRadius,
      sunset,
      'evening',
      activeDrag == _DragTarget.sunsetTransition,
    );

    // Peak handle (smaller, at solar noon)
    _drawHandle(
      canvas,
      center,
      maxRadius,
      solarNoon,
      'peak',
      activeDrag == _DragTarget.peakShape,
    );
  }

  void _drawHandle(
    Canvas canvas,
    Offset center,
    double maxRadius,
    double hour,
    String type,
    bool isActive,
  ) {
    final ringRadius = maxRadius * 0.75;
    final angle = _hourToAngle(hour);

    // Handle position varies based on parameter value
    // Range: from ring inward toward center
    final minHandleRadius = maxRadius * 0.35; // Closest to center (fastest)
    final maxHandleRadius = ringRadius - maxRadius * 0.02; // Near ring (slowest)

    double paramValue;
    if (type == 'morning') {
      paramValue = config.widthLeftBri;
    } else if (type == 'evening') {
      paramValue = config.widthRightBri;
    } else {
      paramValue = config.shapeP;
    }

    // Normalize parameter to 0-1 range
    double normalizedParam;
    if (type == 'peak') {
      // shapeP: 2.0 (round) to 10.0 (flat)
      normalizedParam = (paramValue - 2.0) / 8.0;
    } else {
      // width: 0.3 (fast) to 2.0 (slow)
      normalizedParam = (paramValue - 0.3) / 1.7;
    }
    normalizedParam = normalizedParam.clamp(0.0, 1.0);

    // Map to radius: higher value = further from center
    final handleRadius = minHandleRadius + (maxHandleRadius - minHandleRadius) * normalizedParam;

    final x = center.dx + handleRadius * math.cos(angle);
    final y = center.dy + handleRadius * math.sin(angle);

    // Draw guide line from center to ring
    final linePaint = Paint()
      ..color = _sunGlow.withValues(alpha: (isActive ? 0.4 : 0.2) * entryValue)
      ..strokeWidth = 2;
    canvas.drawLine(
      Offset(center.dx + minHandleRadius * 0.8 * math.cos(angle),
             center.dy + minHandleRadius * 0.8 * math.sin(angle)),
      Offset(center.dx + ringRadius * math.cos(angle),
             center.dy + ringRadius * math.sin(angle)),
      linePaint,
    );

    // Handle size varies by type and state
    final baseSize = type == 'peak' ? maxRadius * 0.04 : maxRadius * 0.055;
    final size = isActive ? baseSize * 1.3 : baseSize;

    // Handle glow
    final glowPaint = Paint()
      ..color = _sunCore.withValues(alpha: isActive ? 0.9 : 0.5)
      ..maskFilter = MaskFilter.blur(BlurStyle.normal, size * 2.5);
    canvas.drawCircle(Offset(x, y), size * 2, glowPaint);

    // Handle body with gradient
    final handlePaint = Paint()
      ..shader = ui.Gradient.radial(
        Offset(x - size * 0.2, y - size * 0.2),
        size * 1.2,
        [
          _sunCore,
          isActive ? _sunGlow : _sunsetAmber,
        ],
      );
    canvas.drawCircle(Offset(x, y), size, handlePaint);

    // Inner highlight
    final highlightPaint = Paint()
      ..color = Colors.white.withValues(alpha: 0.6);
    canvas.drawCircle(Offset(x - size * 0.2, y - size * 0.2), size * 0.3, highlightPaint);

    // Label at the ring edge
    if (!isActive && type != 'peak') {
      final textPainter = TextPainter(
        textDirection: TextDirection.ltr,
        text: TextSpan(
          text: type,
          style: TextStyle(
            color: _sunCore.withValues(alpha: entryValue * 0.85),
            fontSize: maxRadius * 0.05,
            fontWeight: FontWeight.w500,
            letterSpacing: 0.5,
          ),
        ),
      );
      textPainter.layout();

      // Position label at the ring
      final labelRadius = ringRadius + maxRadius * 0.06;
      final lx = center.dx + labelRadius * math.cos(angle);
      final ly = center.dy + labelRadius * math.sin(angle);

      textPainter.paint(
        canvas,
        Offset(lx - textPainter.width / 2, ly - textPainter.height / 2),
      );
    }
  }

  void _drawNowIndicator(Canvas canvas, Offset center, double maxRadius) {
    // Fixed ring radius (matching _drawBrightnessRing)
    final ringRadius = maxRadius * 0.75;
    final ringWidth = maxRadius * 0.08;
    final tickRadius = ringRadius + ringWidth / 2 + maxRadius * 0.02;

    final angle = _hourToAngle(nowHour);

    // Glowing line from center to ring
    final linePaint = Paint()
      ..strokeWidth = 2
      ..shader = ui.Gradient.linear(
        center,
        Offset(center.dx + ringRadius * math.cos(angle), center.dy + ringRadius * math.sin(angle)),
        [
          _sunCore.withValues(alpha: 0.0),
          _sunCore.withValues(alpha: 0.3 * entryValue),
          _sunCore.withValues(alpha: 0.7 * entryValue),
        ],
        [0.0, 0.5, 1.0],
      );

    canvas.drawLine(
      Offset(center.dx + maxRadius * 0.2 * math.cos(angle), center.dy + maxRadius * 0.2 * math.sin(angle)),
      Offset(center.dx + ringRadius * math.cos(angle), center.dy + ringRadius * math.sin(angle)),
      linePaint,
    );

    // Now dot on the ring
    final x = center.dx + ringRadius * math.cos(angle);
    final y = center.dy + ringRadius * math.sin(angle);

    // Glow around dot
    final glowPaint = Paint()
      ..color = _sunCore.withValues(alpha: 0.6 * entryValue)
      ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 10);
    canvas.drawCircle(Offset(x, y), maxRadius * 0.04, glowPaint);

    // Dot
    final dotPaint = Paint()..color = _sunCore.withValues(alpha: entryValue);
    canvas.drawCircle(Offset(x, y), maxRadius * 0.025, dotPaint);

    // Time label - position outside the tick marks
    final hours = nowHour.floor();
    final minutes = ((nowHour - hours) * 60).round();
    final h12 = hours > 12 ? hours - 12 : (hours == 0 ? 12 : hours);
    final timeStr = '$h12:${minutes.toString().padLeft(2, '0')}';

    final textPainter = TextPainter(
      textDirection: TextDirection.ltr,
      text: TextSpan(
        text: timeStr,
        style: TextStyle(
          color: _sunCore.withValues(alpha: entryValue * 0.9),
          fontSize: maxRadius * 0.055,
          fontWeight: FontWeight.w600,
          letterSpacing: 0.5,
        ),
      ),
    );
    textPainter.layout();

    // Position label outside the ticks
    final labelRadius = tickRadius + maxRadius * 0.08;
    final lx = center.dx + labelRadius * math.cos(angle);
    final ly = center.dy + labelRadius * math.sin(angle);

    textPainter.paint(
      canvas,
      Offset(lx - textPainter.width / 2, ly - textPainter.height / 2),
    );
  }

  double _hourToAngle(double hour) {
    // Map 0-24 hours to 0-2*PI, starting from top (-PI/2)
    return (hour / 24) * 2 * math.pi - math.pi / 2;
  }

  double _interpolateValue(
      List<double> hours, List<int> values, double targetHour) {
    if (hours.isEmpty) return 50.0;
    if (hours.length == 1) return values[0].toDouble();

    // Find surrounding points
    var lowerIdx = 0;
    var upperIdx = hours.length - 1;

    for (int i = 0; i < hours.length - 1; i++) {
      if (hours[i] <= targetHour && hours[i + 1] >= targetHour) {
        lowerIdx = i;
        upperIdx = i + 1;
        break;
      }
    }

    // Handle wrap-around at midnight
    if (targetHour < hours.first) {
      lowerIdx = hours.length - 1;
      upperIdx = 0;
    } else if (targetHour > hours.last) {
      lowerIdx = hours.length - 1;
      upperIdx = 0;
    }

    final lowerHour = hours[lowerIdx];
    final upperHour = hours[upperIdx];
    final lowerValue = values[lowerIdx];
    final upperValue = values[upperIdx];

    if (lowerHour == upperHour) return lowerValue.toDouble();

    double t;
    if (upperIdx == 0 && lowerIdx == hours.length - 1) {
      final totalSpan = (24 - lowerHour) + upperHour;
      final position = targetHour >= lowerHour
          ? targetHour - lowerHour
          : (24 - lowerHour) + targetHour;
      t = position / totalSpan;
    } else {
      t = (targetHour - lowerHour) / (upperHour - lowerHour);
    }

    return lowerValue + (upperValue - lowerValue) * t;
  }

  @override
  bool shouldRepaint(covariant _PolarDayPainter oldDelegate) {
    return curveData != oldDelegate.curveData ||
        config != oldDelegate.config ||
        nowHour != oldDelegate.nowHour ||
        pulseValue != oldDelegate.pulseValue ||
        entryValue != oldDelegate.entryValue ||
        activeDrag != oldDelegate.activeDrag ||
        currentBrightness != oldDelegate.currentBrightness ||
        currentKelvin != oldDelegate.currentKelvin;
  }
}
