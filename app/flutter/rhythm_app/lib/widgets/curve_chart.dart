import 'package:flutter/material.dart';
import 'package:fl_chart/fl_chart.dart';
import 'package:rhythm_core/rhythm_core.dart';

import '../utils/app_color_temperature.dart';

/// Interactive chart showing brightness and color temperature curves.
class CurveChart extends StatefulWidget {
  final CurveData? data;
  final double? currentHour;
  final double? selectedHour;
  final bool showNowMarker;
  final bool showSolarContext;
  final ValueChanged<double>? onHourSelected;

  const CurveChart({
    super.key,
    this.data,
    this.currentHour,
    this.selectedHour,
    this.showNowMarker = true,
    this.showSolarContext = true,
    this.onHourSelected,
  });

  @override
  State<CurveChart> createState() => _CurveChartState();
}

class _CurveChartState extends State<CurveChart>
    with SingleTickerProviderStateMixin {
  // Chart area key for measuring
  final GlobalKey _chartKey = GlobalKey();

  // Y-axis max value (brightness can exceed 100% with solar boost)
  static const double _maxY = 125;

  // Chart margins (must match fl_chart reservedSize values)
  static const double _leftMargin = 40;
  static const double _rightMargin = 50;
  static const double _topMargin = 10;
  static const double _bottomMargin = 30;

  // Animation for pulsing now marker
  late AnimationController _pulseController;
  late Animation<double> _pulseAnimation;

  @override
  void initState() {
    super.initState();
    _pulseController = AnimationController(
      duration: const Duration(seconds: 2),
      vsync: this,
    )..repeat(reverse: true);

    _pulseAnimation = Tween<double>(begin: 1.0, end: 1.4).animate(
      CurvedAnimation(parent: _pulseController, curve: Curves.easeInOut),
    );
  }

  @override
  void dispose() {
    _pulseController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (widget.data == null) {
      return const Center(
        child: CircularProgressIndicator(),
      );
    }

    return LayoutBuilder(
      builder: (context, constraints) {
        return GestureDetector(
          onTapDown: (details) =>
              _handleTap(details.localPosition, constraints),
          onPanUpdate: (details) =>
              _handlePointer(details.localPosition, constraints),
          child: Stack(
            children: [
              // The chart
              SizedBox(
                key: _chartKey,
                width: constraints.maxWidth,
                height: constraints.maxHeight,
                child: _buildChart(),
              ),
              // Now marker with pulsing animation
              if (widget.showNowMarker && widget.currentHour != null)
                _buildNowMarker(constraints),
              // Selection annotation overlay
              if (widget.selectedHour != null)
                _buildSelectionAnnotation(constraints),
            ],
          ),
        );
      },
    );
  }

  void _handlePointer(Offset localPosition, BoxConstraints constraints) {
    if (widget.onHourSelected == null) return;

    // Calculate chart area
    final chartWidth = constraints.maxWidth - _leftMargin - _rightMargin;
    final xInChart = localPosition.dx - _leftMargin;

    // Convert to hour (0-24)
    final hour = (xInChart / chartWidth) * 24;
    final clampedHour = hour.clamp(0.0, 24.0);

    widget.onHourSelected!(clampedHour);
  }

  void _handleTap(Offset localPosition, BoxConstraints constraints) {
    _handlePointer(localPosition, constraints);
  }

  Widget _buildChart() {
    final solarLines = <VerticalLine>[];

    if (widget.showSolarContext && widget.data != null) {
      final solar = widget.data!.solar;
      if (solar.solarMidnight >= 0 && solar.solarMidnight <= 24) {
        solarLines.add(
          VerticalLine(
            x: solar.solarMidnight,
            color: Colors.blueGrey.withValues(alpha: 0.2),
            strokeWidth: 1,
            dashArray: [3, 3],
            label: VerticalLineLabel(
              show: true,
              alignment: Alignment.topCenter,
              padding: const EdgeInsets.only(top: 4),
              style: TextStyle(
                color: Colors.blueGrey.withValues(alpha: 0.6),
                fontSize: 8,
              ),
              labelResolver: (line) => 'midnight',
            ),
          ),
        );
      }

      if (solar.sunrise != null && solar.sunrise! > 0) {
        solarLines.add(
          VerticalLine(
            x: solar.sunrise!,
            color: Colors.orange.withValues(alpha: 0.3),
            strokeWidth: 1,
            label: VerticalLineLabel(
              show: true,
              alignment: Alignment.bottomRight,
              padding: const EdgeInsets.only(bottom: 4, left: 4),
              style: TextStyle(
                color: Colors.orange.withValues(alpha: 0.7),
                fontSize: 8,
              ),
              labelResolver: (line) => 'sunrise ${_formatTime(solar.sunrise!)}',
            ),
          ),
        );
      }

      if (solar.solarNoon > 0) {
        solarLines.add(
          VerticalLine(
            x: solar.solarNoon,
            color: Colors.amber.withValues(alpha: 0.4),
            strokeWidth: 2,
            label: VerticalLineLabel(
              show: true,
              alignment: Alignment.topCenter,
              padding: const EdgeInsets.only(top: 4),
              style: TextStyle(
                color: Colors.amber.withValues(alpha: 0.9),
                fontSize: 8,
                fontWeight: FontWeight.bold,
              ),
              labelResolver: (line) => 'noon',
            ),
          ),
        );
      }

      if (solar.sunset != null && solar.sunset! > 0) {
        solarLines.add(
          VerticalLine(
            x: solar.sunset!,
            color: Colors.deepOrange.withValues(alpha: 0.3),
            strokeWidth: 1,
            label: VerticalLineLabel(
              show: true,
              alignment: Alignment.bottomLeft,
              padding: const EdgeInsets.only(bottom: 4, right: 4),
              style: TextStyle(
                color: Colors.deepOrange.withValues(alpha: 0.7),
                fontSize: 8,
              ),
              labelResolver: (line) => 'sunset ${_formatTime(solar.sunset!)}',
            ),
          ),
        );
      }

      _addTwilightLines(
        solarLines,
        solar.dawn,
        labelPrefix: 'dawn',
        civilColor: Colors.lightBlueAccent,
        nauticalColor: Colors.blueGrey,
        astronomicalColor: Colors.blue,
        alignment: Alignment.topRight,
      );
      _addTwilightLines(
        solarLines,
        solar.dusk,
        labelPrefix: 'dusk',
        civilColor: Colors.deepOrangeAccent,
        nauticalColor: Colors.brown,
        astronomicalColor: Colors.redAccent,
        alignment: Alignment.topLeft,
      );
    }

    return LineChart(
      LineChartData(
        minX: 0,
        maxX: 24,
        minY: 0,
        maxY: _maxY,
        clipData: const FlClipData.all(),
        gridData: _buildStandardGrid(),
        titlesData: _buildStandardTitles(),
        borderData: FlBorderData(
          show: true,
          border: Border.all(color: Colors.white10),
        ),
        lineBarsData: [
          // Brightness curve with CCT-based color gradient
          LineChartBarData(
            spots: _brightnessSpots(),
            isCurved: true,
            gradient: _buildBrightnessGradient(),
            barWidth: 5,
            dotData: const FlDotData(show: false),
            belowBarData: BarAreaData(
              show: true,
              gradient: _buildBrightnessGradient(opacity: 0.15),
            ),
          ),
        ],
        extraLinesData: ExtraLinesData(
          verticalLines: [
            ...solarLines,
            // Current time marker (dashed)
            if (widget.currentHour != null)
              VerticalLine(
                x: widget.currentHour!,
                color: Colors.white54,
                strokeWidth: 1,
                dashArray: [5, 5],
              ),
            // Selection cursor line
            if (widget.selectedHour != null)
              VerticalLine(
                x: widget.selectedHour!,
                color: const Color(0xFF1E90FF),
                strokeWidth: 2,
              ),
          ],
        ),
        lineTouchData: const LineTouchData(
          enabled: false, // We handle touch ourselves
        ),
      ),
    );
  }

  void _addTwilightLines(
    List<VerticalLine> lines,
    TwilightPhase? phase, {
    required String labelPrefix,
    required Color civilColor,
    required Color nauticalColor,
    required Color astronomicalColor,
    required Alignment alignment,
  }) {
    if (phase == null) return;

    // Stagger vertically: civil at top, nautical middle, astronomical bottom
    _addTwilightLine(
      lines,
      phase.civil,
      civilColor,
      '$labelPrefix civil',
      alignment,
      verticalOffset: 4,
    );
    _addTwilightLine(
      lines,
      phase.nautical,
      nauticalColor,
      '$labelPrefix nautical',
      alignment,
      verticalOffset: 18,
    );
    _addTwilightLine(
      lines,
      phase.astronomical,
      astronomicalColor,
      '$labelPrefix astro',
      alignment,
      verticalOffset: 32,
    );
  }

  void _addTwilightLine(
    List<VerticalLine> lines,
    double? hour,
    Color color,
    String label,
    Alignment alignment, {
    double verticalOffset = 4,
  }) {
    if (hour == null || hour <= 0) return;
    // Use appropriate padding based on alignment, with vertical staggering
    final padding = alignment == Alignment.topLeft
        ? EdgeInsets.only(top: verticalOffset, right: 4)
        : EdgeInsets.only(top: verticalOffset, left: 4);
    lines.add(
      VerticalLine(
        x: hour,
        color: color.withValues(alpha: 0.25),
        strokeWidth: 1,
        dashArray: [2, 4],
        label: VerticalLineLabel(
          show: true,
          alignment: alignment,
          padding: padding,
          style: TextStyle(
            color: color.withValues(alpha: 0.7),
            fontSize: 8,
          ),
          labelResolver: (line) => '$label ${_formatTime(hour)}',
        ),
      ),
    );
  }

  FlGridData _buildStandardGrid() {
    return FlGridData(
      show: true,
      drawVerticalLine: true,
      horizontalInterval: 50,
      verticalInterval: 6,
      getDrawingHorizontalLine: (value) => const FlLine(
        color: Colors.white10,
        strokeWidth: 1,
      ),
      getDrawingVerticalLine: (value) => const FlLine(
        color: Colors.white10,
        strokeWidth: 1,
      ),
    );
  }

  FlTitlesData _buildStandardTitles() {
    return FlTitlesData(
      leftTitles: AxisTitles(
        sideTitles: SideTitles(
          showTitles: true,
          reservedSize: _leftMargin,
          getTitlesWidget: (value, meta) {
            return Text(
              '${value.toInt()}%',
              style: const TextStyle(
                color: Colors.white54,
                fontSize: 10,
              ),
            );
          },
        ),
      ),
      bottomTitles: AxisTitles(
        sideTitles: SideTitles(
          showTitles: true,
          reservedSize: _bottomMargin,
          interval: 3,
          getTitlesWidget: (value, meta) {
            final hour = value.toInt();
            if (hour % 3 != 0 || hour > 24) return const SizedBox.shrink();

            String label;
            if (!MediaQuery.alwaysUse24HourFormatOf(context)) {
              final h = hour % 12 == 0 ? 12 : hour % 12;
              final suffix = hour < 12 || hour == 24 ? 'a' : 'p';
              label = '$h$suffix';
            } else {
              label = '${hour.toString().padLeft(2, '0')}:00';
            }
            return Padding(
              padding: const EdgeInsets.only(top: 8),
              child: Text(
                label,
                style: const TextStyle(
                  color: Colors.white54,
                  fontSize: 10,
                ),
              ),
            );
          },
        ),
      ),
      rightTitles: AxisTitles(
        sideTitles: SideTitles(
          showTitles: true,
          reservedSize: _rightMargin,
          getTitlesWidget: (value, meta) {
            final kelvin = _percentToKelvin(value);
            return Text(
              '${kelvin.toInt()}K',
              style: const TextStyle(
                color: Colors.white54,
                fontSize: 10,
              ),
            );
          },
        ),
      ),
      topTitles: const AxisTitles(
        sideTitles: SideTitles(showTitles: false),
      ),
    );
  }

  Widget _buildNowMarker(BoxConstraints constraints) {
    if (widget.currentHour == null || widget.data == null) {
      return const SizedBox.shrink();
    }

    final hour = widget.currentHour!;
    final brightness = _interpolateAtHour(widget.data!.brightness, hour);
    final kelvin = _interpolateAtHour(widget.data!.kelvin, hour);

    // Calculate position
    final chartWidth = constraints.maxWidth - _leftMargin - _rightMargin;
    final chartHeight = constraints.maxHeight - _topMargin - _bottomMargin;

    final xPos = _leftMargin + (hour / 24) * chartWidth;
    final yPos = _topMargin + (1 - brightness / _maxY) * chartHeight;

    // Format time
    final timeStr = _formatTime(hour);
    final cctColor = _cctToColor(kelvin.round());

    return Stack(
      children: [
        // Pulsing now marker
        Positioned(
          left: xPos - 8,
          top: yPos - 8,
          child: AnimatedBuilder(
            animation: _pulseAnimation,
            builder: (context, child) {
              return Transform.scale(
                scale: _pulseAnimation.value,
                child: Opacity(
                  opacity: 1.5 - _pulseAnimation.value * 0.5,
                  child: Container(
                    width: 16,
                    height: 16,
                    decoration: BoxDecoration(
                      color: cctColor,
                      shape: BoxShape.circle,
                      border: Border.all(
                        color: const Color(0xFF1E90FF),
                        width: 3,
                      ),
                      boxShadow: [
                        BoxShadow(
                          color: const Color(0xFF1E90FF).withValues(alpha: 0.5),
                          blurRadius: 8,
                          spreadRadius: 2,
                        ),
                      ],
                    ),
                  ),
                ),
              );
            },
          ),
        ),
        // "now" label below chart
        Positioned(
          left: xPos - 25,
          bottom: 2,
          child: Container(
            padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 2),
            decoration: BoxDecoration(
              color: Colors.black.withValues(alpha: 0.7),
              borderRadius: BorderRadius.circular(2),
            ),
            child: Text(
              'now $timeStr',
              style: const TextStyle(
                color: Colors.white,
                fontSize: 9,
                fontWeight: FontWeight.bold,
              ),
            ),
          ),
        ),
      ],
    );
  }

  Widget _buildSelectionAnnotation(BoxConstraints constraints) {
    if (widget.selectedHour == null || widget.data == null) {
      return const SizedBox.shrink();
    }

    final hour = widget.selectedHour!;
    final brightness = _interpolateAtHour(widget.data!.brightness, hour);
    final kelvin = _interpolateAtHour(widget.data!.kelvin, hour);

    // Calculate position
    final chartWidth = constraints.maxWidth - _leftMargin - _rightMargin;
    final chartHeight = constraints.maxHeight - _topMargin - _bottomMargin;

    final xPos = _leftMargin + (hour / 24) * chartWidth;
    final yPos = _topMargin + (1 - brightness / _maxY) * chartHeight;

    // Format time
    final timeStr = _formatTime(hour);

    return Stack(
      children: [
        // Annotation box
        Positioned(
          left: (xPos - 60).clamp(0, constraints.maxWidth - 120),
          top: (yPos - 60).clamp(0, constraints.maxHeight - 50),
          child: Container(
            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
            decoration: BoxDecoration(
              color: Colors.black.withValues(alpha: 0.8),
              borderRadius: BorderRadius.circular(4),
              border: Border.all(color: Colors.white30),
            ),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(
                  timeStr,
                  style: const TextStyle(
                    color: Colors.white,
                    fontSize: 12,
                    fontWeight: FontWeight.bold,
                  ),
                ),
                Text(
                  '${brightness.round()}% • ${kelvin.round()}K',
                  style: const TextStyle(
                    color: Colors.white70,
                    fontSize: 11,
                  ),
                ),
              ],
            ),
          ),
        ),
      ],
    );
  }

  String _formatTime(double hour) {
    final h = hour.floor();
    final m = ((hour - h) * 60).round();

    if (!MediaQuery.alwaysUse24HourFormatOf(context)) {
      final period = h < 12 ? 'AM' : 'PM';
      final h12 = h == 0 ? 12 : (h > 12 ? h - 12 : h);
      return '$h12:${m.toString().padLeft(2, '0')} $period';
    } else {
      return '${h.toString().padLeft(2, '0')}:${m.toString().padLeft(2, '0')}';
    }
  }

  double _interpolateAtHour(List<int> values, double hour) {
    if (widget.data == null || values.isEmpty) return 0;

    final hours = widget.data!.hours;
    if (hour <= hours.first) return values.first.toDouble();
    if (hour >= hours.last) return values.last.toDouble();

    for (int i = 1; i < hours.length; i++) {
      if (hour <= hours[i]) {
        final t = (hour - hours[i - 1]) / (hours[i] - hours[i - 1]);
        return values[i - 1] + t * (values[i] - values[i - 1]);
      }
    }
    return values.last.toDouble();
  }

  LinearGradient? _buildBrightnessGradient({double opacity = 1.0}) {
    if (widget.data == null || widget.data!.kelvin.isEmpty) return null;

    // Sample CCT values at key points for gradient
    final kelvinValues = widget.data!.kelvin;
    final n = kelvinValues.length;
    if (n < 2) return null;

    // Create gradient stops based on CCT values
    final colors = <Color>[];
    final stops = <double>[];

    // Sample at regular intervals
    const numSamples = 8;
    for (int i = 0; i <= numSamples; i++) {
      final idx = (i * (n - 1) / numSamples).round().clamp(0, n - 1);
      final color = AppColorTemperature.curveColor(kelvinValues[idx]);
      colors.add(color.withValues(alpha: opacity));
      stops.add(i / numSamples);
    }

    return LinearGradient(
      colors: colors,
      stops: stops,
      begin: Alignment.centerLeft,
      end: Alignment.centerRight,
    );
  }

  List<FlSpot> _brightnessSpots() {
    if (widget.data == null) return [];

    final spots = List.generate(widget.data!.hours.length, (i) {
      return FlSpot(
          widget.data!.hours[i], widget.data!.brightness[i].toDouble());
    });

    // Add wrap-around point at hour 24 (same as hour 0) to complete the curve
    if (spots.isNotEmpty && widget.data!.brightness.isNotEmpty) {
      spots.add(FlSpot(24, widget.data!.brightness[0].toDouble()));
    }

    return spots;
  }

  double _percentToKelvin(double percent) {
    if (widget.data == null) return 4000;
    final minK = widget.data!.kelvin.reduce((a, b) => a < b ? a : b).toDouble();
    final maxK = widget.data!.kelvin.reduce((a, b) => a > b ? a : b).toDouble();
    final range = maxK - minK;
    return minK + (percent / 100) * range;
  }

  /// Convert color temperature to RGB color using proper algorithm
  Color _cctToColor(int kelvin) {
    return AppColorTemperature.curveColor(kelvin);
  }
}
