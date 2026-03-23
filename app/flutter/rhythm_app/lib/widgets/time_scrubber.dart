import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'solar_orbit.dart';

/// Horizontal timeline bar with CCT gradient and time scrubbing.
///
/// Shows a full 24-hour timeline with:
/// - CCT gradient background representing color temperature throughout the day
/// - Time markers at 6 AM, 12 PM, 6 PM, 12 AM
/// - Current position indicator synced with the orbit
/// - Drag/tap to scrub through time
class TimeScrubber extends StatefulWidget {
  final CurveData? curveData;
  final double selectedHour;
  final ValueChanged<double> onHourChanged;

  const TimeScrubber({
    super.key,
    required this.curveData,
    required this.selectedHour,
    required this.onHourChanged,
  });

  @override
  State<TimeScrubber> createState() => _TimeScrubberState();
}

class _TimeScrubberState extends State<TimeScrubber> {
  double? _lastFeedbackHour;

  void _handleDragStart(DragStartDetails details) {
    _lastFeedbackHour = widget.selectedHour;
    HapticFeedback.lightImpact();
  }

  void _handleDragUpdate(DragUpdateDetails details, double width) {
    final x = details.localPosition.dx.clamp(0.0, width);
    final hour = (x / width) * 24;
    final snappedHour = (hour * 4).round() / 4; // Snap to 15-minute intervals

    // Haptic feedback at hour boundaries
    if (_lastFeedbackHour != null &&
        snappedHour.floor() != _lastFeedbackHour!.floor()) {
      HapticFeedback.selectionClick();
      _lastFeedbackHour = snappedHour;
    }

    widget.onHourChanged(snappedHour.clamp(0.0, 23.99));
  }

  void _handleDragEnd(DragEndDetails details) {
    _lastFeedbackHour = null;
  }

  void _handleTap(TapUpDetails details, double width) {
    final x = details.localPosition.dx.clamp(0.0, width);
    final hour = (x / width) * 24;
    final snappedHour = (hour * 4).round() / 4;
    HapticFeedback.mediumImpact();
    widget.onHourChanged(snappedHour.clamp(0.0, 23.99));
  }

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        final width = constraints.maxWidth;
        const height = 56.0;

        return SizedBox(
          height: height,
          child: Column(
            children: [
              // Time labels
              SizedBox(
                height: 16,
                child: Row(
                  mainAxisAlignment: MainAxisAlignment.spaceBetween,
                  children: [
                    _buildTimeLabel('12 AM'),
                    _buildTimeLabel('6 AM'),
                    _buildTimeLabel('Noon'),
                    _buildTimeLabel('6 PM'),
                    _buildTimeLabel('12 AM'),
                  ],
                ),
              ),
              const SizedBox(height: 4),
              // Gradient bar with position indicator
              Expanded(
                child: GestureDetector(
                  onHorizontalDragStart: _handleDragStart,
                  onHorizontalDragUpdate: (d) => _handleDragUpdate(d, width),
                  onHorizontalDragEnd: _handleDragEnd,
                  onTapUp: (d) => _handleTap(d, width),
                  child: Stack(
                    clipBehavior: Clip.none,
                    children: [
                      // Gradient background
                      CustomPaint(
                        size: Size(width, height - 20),
                        painter:
                            _GradientBarPainter(curveData: widget.curveData),
                      ),
                      // Position indicator
                      _buildPositionIndicator(width, height - 20),
                    ],
                  ),
                ),
              ),
            ],
          ),
        );
      },
    );
  }

  Widget _buildTimeLabel(String label) {
    return Text(
      label,
      style: TextStyle(
        color: CelestialColors.textSecondary.withValues(alpha: 0.8),
        fontSize: 10,
        fontWeight: FontWeight.w500,
      ),
    );
  }

  Widget _buildPositionIndicator(double width, double height) {
    final x = (widget.selectedHour / 24) * width;

    return Positioned(
      left: x - 1.5,
      top: -4,
      child: Container(
        width: 3,
        height: height + 8,
        decoration: BoxDecoration(
          color: CelestialColors.accentBlue,
          borderRadius: BorderRadius.circular(1.5),
          boxShadow: [
            BoxShadow(
              color: CelestialColors.accentBlue.withValues(alpha: 0.5),
              blurRadius: 4,
              spreadRadius: 1,
            ),
          ],
        ),
        child: Column(
          mainAxisAlignment: MainAxisAlignment.start,
          children: [
            Container(
              width: 8,
              height: 8,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: CelestialColors.accentBlue,
                boxShadow: [
                  BoxShadow(
                    color: CelestialColors.accentBlue.withValues(alpha: 0.5),
                    blurRadius: 4,
                    spreadRadius: 1,
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// Custom painter for the CCT gradient bar.
class _GradientBarPainter extends CustomPainter {
  final CurveData? curveData;

  _GradientBarPainter({required this.curveData});

  @override
  void paint(Canvas canvas, Size size) {
    final rect = RRect.fromRectAndRadius(
      Rect.fromLTWH(0, 0, size.width, size.height),
      const Radius.circular(8),
    );

    // Clip to rounded rect
    canvas.clipRRect(rect);

    if (curveData == null || curveData!.hours.isEmpty) {
      // Default gradient if no data
      final gradient = LinearGradient(
        colors: [
          ColorUtils.cctToColor(2700),
          ColorUtils.cctToColor(6500),
          ColorUtils.cctToColor(6500),
          ColorUtils.cctToColor(2700),
        ],
        stops: const [0.0, 0.4, 0.6, 1.0],
      );
      final paint = Paint()..shader = gradient.createShader(rect.outerRect);
      canvas.drawRect(rect.outerRect, paint);
      return;
    }

    // Draw segments based on curve data
    const segments = 48; // One segment per 30 minutes
    final segmentWidth = size.width / segments;

    for (int i = 0; i < segments; i++) {
      final hour = (i / segments) * 24;
      final brightness =
          _interpolateValue(curveData!.hours, curveData!.brightness, hour);
      final kelvin =
          _interpolateValue(curveData!.hours, curveData!.kelvin, hour);

      final color = ColorUtils.cctToColor(kelvin.toInt());
      final opacity = 0.4 + (brightness / 100) * 0.6;

      final paint = Paint()..color = color.withValues(alpha: opacity);
      final segmentRect = Rect.fromLTWH(
        i * segmentWidth,
        0,
        segmentWidth + 1, // +1 to avoid gaps
        size.height,
      );
      canvas.drawRect(segmentRect, paint);
    }

    // Draw border
    final borderPaint = Paint()
      ..style = PaintingStyle.stroke
      ..color = CelestialColors.orbitRing.withValues(alpha: 0.5)
      ..strokeWidth = 1;
    canvas.drawRRect(rect, borderPaint);
  }

  double _interpolateValue(
      List<double> hours, List<int> values, double targetHour) {
    if (hours.isEmpty) return 50.0;
    if (hours.length == 1) return values[0].toDouble();

    // Find surrounding points
    int lowerIdx = 0;
    int upperIdx = hours.length - 1;

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
  bool shouldRepaint(covariant _GradientBarPainter oldDelegate) {
    return curveData != oldDelegate.curveData;
  }
}
