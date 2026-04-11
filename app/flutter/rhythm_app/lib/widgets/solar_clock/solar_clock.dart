import 'package:flutter/material.dart';

import 'solar_arc_painter.dart';
import 'solar_clock_data.dart';
import 'solar_clock_geometry.dart';

class SolarClock extends StatelessWidget {
  final SolarClockData data;
  final bool use24;
  final SolarCurveSamples? curveData;
  final double arcStrokeWidth;
  final bool showEventMarkers;
  final bool showHourLabels;
  final bool showLowerArc;
  final double horizonFactor;
  final double radiusWidthFactor;
  final double radiusHeightFactor;
  final Widget Function(BuildContext, SolarClockGeometry)? overlayBuilder;

  const SolarClock({
    super.key,
    required this.data,
    required this.use24,
    this.curveData,
    this.arcStrokeWidth = 4.0,
    this.showEventMarkers = true,
    this.showHourLabels = true,
    this.showLowerArc = true,
    this.horizonFactor = 0.52,
    this.radiusWidthFactor = 0.42,
    this.radiusHeightFactor = 0.55,
    this.overlayBuilder,
  });

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        final geometry = SolarClockGeometry.fromConstraints(
          constraints,
          solarNoon: data.solarNoon,
          horizonFactor: horizonFactor,
          radiusWidthFactor: radiusWidthFactor,
          radiusHeightFactor: radiusHeightFactor,
        );

        return Stack(
          fit: StackFit.expand,
          children: [
            CustomPaint(
              painter: SolarArcPainter(
                data: data,
                geometry: geometry,
                use24: use24,
                curveData: curveData,
                arcStrokeWidth: arcStrokeWidth,
                showEventMarkers: showEventMarkers,
                showHourLabels: showHourLabels,
                showLowerArc: showLowerArc,
              ),
            ),
            if (overlayBuilder != null) overlayBuilder!(context, geometry),
          ],
        );
      },
    );
  }
}
