import 'dart:math' as math;
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:rhythm_core/rhythm_core.dart';

/// Celestial color palette for the solar orbit designer.
class CelestialColors {
  static const backgroundDark = Color(0xFF0D1117);
  static const backgroundCard = Color(0xFF161B22);
  static const orbitRing = Color(0xFF30363D);
  static const accentBlue = Color(0xFF58A6FF);
  static const sunWarm = Color(0xFFF9A825);
  static const sunCool = Color(0xFFBBDEFB);
  static const textPrimary = Color(0xFFE6EDF3);
  static const textSecondary = Color(0xFF8B949E);
}

/// Tune mode gesture types.
enum TuneGesture {
  verticalDrag, // Adjusts brightness range
  pinch, // Adjusts curve steepness
  horizontalDrag, // Shifts midpoint
}

/// The central sun + orbital ring widget.
///
/// Features:
/// - Orbital gradient ring showing CCT at each hour
/// - Draggable position indicator on the ring
/// - Central sun with dynamic size (brightness) and color (kelvin)
/// - Long-press to enter tune mode for curve adjustments
class SolarOrbit extends StatefulWidget {
  final CurveData? curveData;
  final double selectedHour;
  final bool isTuneMode;
  final bool isLightOn;
  final bool isRhythmMode;
  final int brightness;
  final bool showTimeLabels;
  final bool eagerGestures;
  final ValueChanged<double> onHourChanged;
  final VoidCallback? onHourChangeEnd;
  final VoidCallback? onSunTap;
  final ValueChanged<int>? onBrightnessChanged;
  final VoidCallback? onBrightnessChangeEnd;
  final VoidCallback? onTuneModeRequested;
  final Function(TuneGesture, double)? onTuneGesture;
  final VoidCallback? onTuneGestureEnd;

  const SolarOrbit({
    super.key,
    required this.curveData,
    required this.selectedHour,
    this.isTuneMode = false,
    this.isLightOn = true,
    this.isRhythmMode = false,
    this.brightness = 100,
    this.showTimeLabels = true,
    this.eagerGestures = false,
    required this.onHourChanged,
    this.onHourChangeEnd,
    this.onSunTap,
    this.onBrightnessChanged,
    this.onBrightnessChangeEnd,
    this.onTuneModeRequested,
    this.onTuneGesture,
    this.onTuneGestureEnd,
  });

  @override
  State<SolarOrbit> createState() => _SolarOrbitState();
}

class _SolarOrbitState extends State<SolarOrbit>
    with TickerProviderStateMixin {
  late AnimationController _pulseController;
  late Animation<double> _pulseAnimation;
  late AnimationController _glowController;
  late Listenable _sunAnimation;

  // For dot dragging
  bool _isDraggingDot = false;
  // For immediate touch feedback before pan starts
  bool _isTouchingDot = false;

  // For brightness adjustment gesture
  bool _isAdjustingBrightness = false;
  double _brightnessStartY = 0;
  int _brightnessAtStart = 100;

  @override
  void initState() {
    super.initState();
    _pulseController = AnimationController(
      duration: const Duration(milliseconds: 3000),
      vsync: this,
    )..repeat(reverse: true);

    _pulseAnimation = Tween<double>(begin: 1.0, end: 1.05).animate(
      CurvedAnimation(parent: _pulseController, curve: Curves.easeInOut),
    );

    _glowController = AnimationController(
      duration: const Duration(milliseconds: 500),
      vsync: this,
      value: 0.0,
    );
    if (widget.isLightOn) {
      _glowController.forward();
    }

    _sunAnimation = Listenable.merge([_pulseAnimation, _glowController]);
  }

  @override
  void didUpdateWidget(SolarOrbit oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.isLightOn != widget.isLightOn) {
      if (widget.isLightOn) {
        _glowController.forward();
      } else {
        _glowController.reverse();
      }
    }
  }

  @override
  void dispose() {
    _pulseController.dispose();
    _glowController.dispose();
    super.dispose();
  }

  /// Get brightness at a specific hour from curve data.
  int _getBrightnessAtHour(double hour) {
    if (widget.curveData == null) return 50;
    final data = widget.curveData!;
    if (data.hours.isEmpty) return 50;

    // Find the closest hour in data
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

  /// Get kelvin at a specific hour from curve data.
  int _getKelvinAtHour(double hour) {
    if (widget.curveData == null) return 4000;
    final data = widget.curveData!;
    if (data.hours.isEmpty) return 4000;

    // Find the closest hour in data
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

  /// Convert hour to angle (0 = top, clockwise).
  double _hourToAngle(double hour) {
    // Map 0-24 hours to 0-2*PI, starting from top (-PI/2)
    return (hour / 24) * 2 * math.pi - math.pi / 2;
  }

  /// Convert angle to hour.
  double _angleToHour(double angle) {
    // Add PI/2 to shift so top is 0, then normalize to 0-24
    double normalizedAngle = angle + math.pi / 2;
    if (normalizedAngle < 0) normalizedAngle += 2 * math.pi;
    return (normalizedAngle / (2 * math.pi)) * 24;
  }

  /// Convert global position to hour based on angle from center.
  double _globalPositionToHour(
      Offset globalPosition, Offset widgetCenter, RenderBox box) {
    final localPosition = box.globalToLocal(globalPosition);
    final dx = localPosition.dx - widgetCenter.dx;
    final dy = localPosition.dy - widgetCenter.dy;
    final angle = math.atan2(dy, dx);
    return _angleToHour(angle);
  }

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        // Calculate sizes based on available space
        final size = math.min(constraints.maxWidth, constraints.maxHeight);
        final orbitRadius = size * 0.38;
        final ringWidth = size * 0.06;
        final center = Offset(size / 2, size / 2);

        final currentBrightness = _getBrightnessAtHour(widget.selectedHour);
        final currentKelvin = _getKelvinAtHour(widget.selectedHour);

        // Sun size scales with brightness (60px min to 140px max at size=400)
        final minSunSize = size * 0.15;
        final maxSunSize = size * 0.35;
        final displayBrightness =
            widget.isLightOn ? widget.brightness : currentBrightness;
        final sunSize =
            minSunSize + (maxSunSize - minSunSize) * (displayBrightness / 100);

        return SizedBox(
          width: size,
          height: size,
          child: Stack(
            clipBehavior: Clip.none,
            alignment: Alignment.center,
            children: [
              // Orbital gradient ring (visual only, no gestures)
              CustomPaint(
                size: Size(size, size),
                painter: _OrbitalRingPainter(
                  curveData: widget.curveData,
                  ringRadius: orbitRadius,
                  ringWidth: ringWidth,
                ),
              ),
              // Time labels on ring (optional)
              if (widget.showTimeLabels)
                ..._buildTimeLabels(center, orbitRadius + ringWidth + 8, size),
              // Draggable position indicator (dot)
              _buildDraggablePositionIndicator(center, orbitRadius, size),
              // Central sun with glow (tappable + vertical drag for brightness)
              // Use explicit positioning to prevent dot position from affecting sun centering
              Positioned(
                left: center.dx - (maxSunSize * 1.3) / 2,
                top: center.dy - (maxSunSize * 1.3) / 2,
                child: SizedBox(
                  width: maxSunSize * 1.3,
                  height: maxSunSize * 1.3,
                  child: GestureDetector(
                    behavior: HitTestBehavior.opaque,
                    onTap: () {
                      HapticFeedback.mediumImpact();
                      widget.onSunTap?.call();
                    },
                    onVerticalDragStart: (details) {
                      HapticFeedback.heavyImpact();
                      setState(() {
                        _isAdjustingBrightness = true;
                        _brightnessStartY = details.globalPosition.dy;
                        _brightnessAtStart = widget.brightness;
                      });
                    },
                    onVerticalDragUpdate: (details) {
                      if (!_isAdjustingBrightness) return;

                      // Calculate delta: moving up = positive brightness change
                      final deltaY =
                          _brightnessStartY - details.globalPosition.dy;
                      // Scale: 200 pixels = full range (0-100)
                      final brightnessDelta = (deltaY / 2).round();
                      final newBrightness =
                          (_brightnessAtStart + brightnessDelta).clamp(1, 100);

                      if (newBrightness != widget.brightness) {
                        HapticFeedback.selectionClick();
                        widget.onBrightnessChanged?.call(newBrightness);
                      }
                    },
                    onVerticalDragEnd: (details) {
                      setState(() {
                        _isAdjustingBrightness = false;
                      });
                      widget.onBrightnessChangeEnd?.call();
                    },
                    onVerticalDragCancel: () {
                      setState(() {
                        _isAdjustingBrightness = false;
                      });
                    },
                    child: Center(
                      child: AnimatedBuilder(
                        animation: _sunAnimation,
                        builder: (context, child) {
                          final glow = _glowController.value;
                          final effectiveBrightness =
                              (displayBrightness * glow).round();
                          final effectiveSunSize = minSunSize +
                              (maxSunSize - minSunSize) *
                                  (effectiveBrightness / 100);
                          return _buildSun(
                            effectiveSunSize *
                                (widget.isLightOn
                                    ? _pulseAnimation.value
                                    : 1.0),
                            currentKelvin,
                            effectiveBrightness,
                            isOn: widget.isLightOn,
                            isAdjusting: _isAdjustingBrightness,
                            maxSunSize: maxSunSize,
                          );
                        },
                      ),
                    ),
                  ),
                ),
              ),
              // Tune mode indicator
              if (widget.isTuneMode) _buildTuneModeIndicator(sunSize),
            ],
          ),
        );
      },
    );
  }

  List<Widget> _buildTimeLabels(
      Offset center, double labelRadius, double size) {
    final labels = [
      ('12 AM', 0.0),
      ('6 AM', 6.0),
      ('12 PM', 12.0),
      ('6 PM', 18.0),
    ];

    return labels.map((label) {
      final angle = _hourToAngle(label.$2);
      final x = center.dx + labelRadius * math.cos(angle);
      final y = center.dy + labelRadius * math.sin(angle);

      return Positioned(
        left: x - 25,
        top: y - 8,
        child: SizedBox(
          width: 50,
          child: Text(
            label.$1,
            textAlign: TextAlign.center,
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.6),
              fontSize: size * 0.03,
              fontWeight: FontWeight.w500,
            ),
          ),
        ),
      );
    }).toList();
  }

  Widget _buildDraggablePositionIndicator(
      Offset center, double radius, double size) {
    final angle = _hourToAngle(widget.selectedHour);
    final x = center.dx + radius * math.cos(angle);
    final y = center.dy + radius * math.sin(angle);
    final indicatorSize = size * 0.12; // Larger for easier touch
    // Ensure minimum 48px hit area for accessibility
    final hitAreaSize = math.max(size * 0.15, 48.0);

    // Visual feedback when touching or dragging
    final isActive = _isTouchingDot || _isDraggingDot;

    final rhythmPulse = widget.isRhythmMode;

    final indicatorWidget = AnimatedBuilder(
      animation: rhythmPulse ? _pulseAnimation : const AlwaysStoppedAnimation(0),
      builder: (context, _) {
        // Pulse ranges from 1.0 to 1.05, remap to a more visible 0..1 range
        final pulseT = rhythmPulse
            ? ((_pulseAnimation.value - 1.0) / 0.05).clamp(0.0, 1.0)
            : 0.0;
        final pulseScale = rhythmPulse ? 1.0 + pulseT * 0.5 : 1.0;
        final pulseGlow = rhythmPulse ? 0.6 + pulseT * 0.4 : (isActive ? 0.8 : 0.6);
        final pulseBlur = rhythmPulse ? 8.0 + pulseT * 10.0 : (isActive ? 12.0 : 8.0);
        final pulseSpread = rhythmPulse ? 2.0 + pulseT * 5.0 : (isActive ? 4.0 : 2.0);

        final dotSize = isActive ? indicatorSize * 1.3 : indicatorSize * pulseScale;

        return Container(
          width: hitAreaSize,
          height: hitAreaSize,
          alignment: Alignment.center,
          color: Colors.transparent,
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 150),
            width: dotSize,
            height: dotSize,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: CelestialColors.accentBlue,
              boxShadow: [
                BoxShadow(
                  color: CelestialColors.accentBlue.withValues(alpha: pulseGlow),
                  blurRadius: pulseBlur,
                  spreadRadius: pulseSpread,
                ),
              ],
            ),
          ),
        );
      },
    );

    // Gesture handling - use eager recognition when in scroll views
    Widget gestureChild;
    if (widget.eagerGestures) {
      gestureChild = RawGestureDetector(
        gestures: <Type, GestureRecognizerFactory>{
          _EagerPanGestureRecognizer:
              GestureRecognizerFactoryWithHandlers<_EagerPanGestureRecognizer>(
            () => _EagerPanGestureRecognizer(),
            (_EagerPanGestureRecognizer instance) {
              instance
                ..onStart = (details) {
                  HapticFeedback.mediumImpact();
                  setState(() {
                    _isDraggingDot = true;
                  });
                }
                ..onUpdate = (details) {
                  if (!_isDraggingDot) return;

                  final box = context.findRenderObject() as RenderBox?;
                  if (box == null) return;

                  final hour =
                      _globalPositionToHour(details.globalPosition, center, box);
                  final snappedHour = (hour * 4).round() / 4;
                  final clampedHour = snappedHour.clamp(0.0, 23.99);

                  if ((clampedHour - widget.selectedHour).abs() > 0.1) {
                    HapticFeedback.selectionClick();
                  }

                  widget.onHourChanged(clampedHour);
                }
                ..onEnd = (details) {
                  setState(() {
                    _isDraggingDot = false;
                    _isTouchingDot = false;
                  });
                  widget.onHourChangeEnd?.call();
                }
                ..onCancel = () {
                  setState(() {
                    _isDraggingDot = false;
                    _isTouchingDot = false;
                  });
                };
            },
          ),
        },
        behavior: HitTestBehavior.opaque,
        child: indicatorWidget,
      );
    } else {
      gestureChild = GestureDetector(
        behavior: HitTestBehavior.opaque,
        onPanStart: (details) {
          HapticFeedback.mediumImpact();
          setState(() {
            _isDraggingDot = true;
          });
        },
        onPanUpdate: (details) {
          if (!_isDraggingDot) return;

          final box = context.findRenderObject() as RenderBox?;
          if (box == null) return;

          final hour =
              _globalPositionToHour(details.globalPosition, center, box);
          final snappedHour = (hour * 4).round() / 4;
          final clampedHour = snappedHour.clamp(0.0, 23.99);

          if ((clampedHour - widget.selectedHour).abs() > 0.1) {
            HapticFeedback.selectionClick();
          }

          widget.onHourChanged(clampedHour);
        },
        onPanEnd: (details) {
          setState(() {
            _isDraggingDot = false;
            _isTouchingDot = false;
          });
          widget.onHourChangeEnd?.call();
        },
        onPanCancel: () {
          setState(() {
            _isDraggingDot = false;
            _isTouchingDot = false;
          });
        },
        child: indicatorWidget,
      );
    }

    return Positioned(
      left: x - hitAreaSize / 2,
      top: y - hitAreaSize / 2,
      // Wrap with Listener for immediate touch feedback (before pan threshold)
      child: Listener(
        behavior: HitTestBehavior.opaque,
        onPointerDown: (event) {
          HapticFeedback.lightImpact();
          setState(() {
            _isTouchingDot = true;
          });
        },
        onPointerUp: (event) {
          setState(() {
            _isTouchingDot = false;
          });
        },
        onPointerCancel: (event) {
          setState(() {
            _isTouchingDot = false;
          });
        },
        child: gestureChild,
      ),
    );
  }

  Widget _buildSun(
    double size,
    int kelvin,
    int brightness, {
    required bool isOn,
    bool isAdjusting = false,
    required double maxSunSize,
  }) {
    final sunColor = ColorUtils.curveColorForCCT(kelvin);
    final glow = _glowController.value;
    final glowOpacity = (0.3 + (brightness / 100) * 0.4) * glow;

    // Off state uses fixed size (2x max sun size), independent of brightness/orbit
    final offSize = maxSunSize * 2.0;
    final displaySize = isOn ? size : offSize;

    final onColors = [
      Colors.white,
      sunColor,
      sunColor.withValues(alpha: 0.6),
      sunColor.withValues(alpha: 0.0),
    ];
    final offColors = [
      CelestialColors.backgroundCard,
      CelestialColors.orbitRing.withValues(alpha: 0.3),
      CelestialColors.orbitRing.withValues(alpha: 0.1),
      CelestialColors.orbitRing.withValues(alpha: 0.0),
    ];
    final gradientColors = [
      Color.lerp(offColors[0], onColors[0], glow)!,
      Color.lerp(offColors[1], onColors[1], glow)!,
      Color.lerp(offColors[2], onColors[2], glow)!,
      Color.lerp(offColors[3], onColors[3], glow)!,
    ];

    return Stack(
      alignment: Alignment.center,
      children: [
        // Main sun
        AnimatedContainer(
          duration: const Duration(milliseconds: 100),
          curve: Curves.easeInOut,
          width: displaySize,
          height: displaySize,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            gradient: RadialGradient(
              colors: gradientColors,
              stops: const [0.0, 0.3, 0.6, 1.0],
            ),
            boxShadow: [
              BoxShadow(
                color: sunColor.withValues(alpha: glowOpacity),
                blurRadius: displaySize * 0.3,
                spreadRadius: displaySize * 0.1,
              ),
              BoxShadow(
                color: sunColor.withValues(alpha: glowOpacity * 0.5),
                blurRadius: displaySize * 0.6,
                spreadRadius: displaySize * 0.2,
              ),
            ],
          ),
          child: AnimatedOpacity(
            opacity: isOn ? 0.0 : 1.0,
            duration: Duration(milliseconds: isOn ? 0 : 400),
            curve: Curves.easeInOut,
            child: Center(
              child: Container(
                width: offSize * 0.4,
                height: offSize * 0.4,
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: CelestialColors.backgroundCard,
                  border: Border.all(
                    color: CelestialColors.orbitRing,
                    width: 2,
                  ),
                ),
                child: Icon(
                  Icons.play_arrow_rounded,
                  color: CelestialColors.textSecondary,
                  size: offSize * 0.25,
                ),
              ),
            ),
          ),
        ),
        // Brightness percentage label during adjustment
        if (isAdjusting && isOn)
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
            decoration: BoxDecoration(
              color: CelestialColors.backgroundCard.withValues(alpha: 0.9),
              borderRadius: BorderRadius.circular(16),
            ),
            child: Text(
              '$brightness%',
              style: const TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 18,
                fontWeight: FontWeight.bold,
              ),
            ),
          ),
      ],
    );
  }

  Widget _buildTuneModeIndicator(double sunSize) {
    return Container(
      width: sunSize * 1.3,
      height: sunSize * 1.3,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        border: Border.all(
          color: CelestialColors.accentBlue.withValues(alpha: 0.6),
          width: 2,
        ),
      ),
    );
  }
}

/// Custom painter for the orbital gradient ring.
class _OrbitalRingPainter extends CustomPainter {
  final CurveData? curveData;
  final double ringRadius;
  final double ringWidth;

  _OrbitalRingPainter({
    required this.curveData,
    required this.ringRadius,
    required this.ringWidth,
  });

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);
    final rect = Rect.fromCircle(center: center, radius: ringRadius);

    // Paint stroke style
    final paint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = ringWidth
      ..strokeCap = StrokeCap.butt;

    if (curveData == null || curveData!.hours.isEmpty) {
      // Draw simple ring if no data
      paint.color = CelestialColors.orbitRing;
      canvas.drawCircle(center, ringRadius, paint);
      return;
    }

    // Draw gradient ring with CCT colors for each segment
    const segments = 48; // 30-minute segments for smooth gradient
    const sweepAngle = (2 * math.pi) / segments;

    for (int i = 0; i < segments; i++) {
      final hour = (i / segments) * 24;
      final brightness =
          _interpolateValue(curveData!.hours, curveData!.brightness, hour);
      final kelvin =
          _interpolateValue(curveData!.hours, curveData!.kelvin, hour);

      final color = ColorUtils.curveColorForCCT(kelvin.toInt());
      // Opacity based on brightness
      final opacity = 0.3 + (brightness / 100) * 0.7;

      paint.color = color.withValues(alpha: opacity);

      // Start angle: top is -PI/2, going clockwise
      final startAngle = (i / segments) * 2 * math.pi - math.pi / 2;

      canvas.drawArc(rect, startAngle, sweepAngle + 0.02, false, paint);
    }
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

    // Handle same values
    if (lowerHour == upperHour) return lowerValue.toDouble();

    // Linear interpolation
    double t;
    if (upperIdx == 0 && lowerIdx == hours.length - 1) {
      // Wrap-around case
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
  bool shouldRepaint(covariant _OrbitalRingPainter oldDelegate) {
    return curveData != oldDelegate.curveData ||
        ringRadius != oldDelegate.ringRadius ||
        ringWidth != oldDelegate.ringWidth;
  }
}

/// Custom pan gesture recognizer that wins the arena immediately.
/// This prevents parent scroll views from stealing the gesture.
class _EagerPanGestureRecognizer extends PanGestureRecognizer {
  @override
  void addAllowedPointer(PointerDownEvent event) {
    super.addAllowedPointer(event);
    // Immediately win the arena to prevent scroll view from competing
    resolve(GestureDisposition.accepted);
  }
}
