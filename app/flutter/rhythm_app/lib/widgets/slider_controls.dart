import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import '../models/config_model.dart';
import '../services/analytics_service.dart';

/// Control panel with sliders for adjusting curve parameters.
/// Uses super-Gaussian curve with width multipliers for ramp speed.
class SliderControls extends StatefulWidget {
  final CurveConfigDto config;
  final ValueChanged<CurveConfigDto> onConfigChanged;
  final double? solarNoonHour;

  const SliderControls({
    super.key,
    required this.config,
    required this.onConfigChanged,
    this.solarNoonHour,
  });

  @override
  State<SliderControls> createState() => _SliderControlsState();
}

class _SliderControlsState extends State<SliderControls> {
  // UI-only mirror state (not persisted)
  // null = not yet initialized, will be set based on config values
  bool? _mirrorMorning;
  bool? _mirrorEvening;

  /// Initialize mirror state based on whether bri and cct widths match
  void _initMirrorState() {
    _mirrorMorning ??=
        (widget.config.widthLeftBri - widget.config.widthLeftCct).abs() < 0.01;
    _mirrorEvening ??=
        (widget.config.widthRightBri - widget.config.widthRightCct).abs() <
            0.01;
  }

  @override
  Widget build(BuildContext context) {
    // Initialize mirror state on first build based on config values
    _initMirrorState();
    return Consumer<ConfigModel>(
      builder: (context, model, child) {
        return SingleChildScrollView(
          padding: const EdgeInsets.fromLTRB(16, 4, 16, 12),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              // Morning / Evening side by side
              _buildCurveSections(model),
              const SizedBox(height: 8),
              // Shape and Ranges section
              _buildShapeSection(),
              const SizedBox(height: 8),
              _buildRangesSection(),
              const SizedBox(height: 8),
              // Solar context toggle at bottom
              _buildSolarContextToggle(model),
            ],
          ),
        );
      },
    );
  }

  Widget _buildCurveSections(ConfigModel model) {
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        // Morning section
        Expanded(
          child: _buildCurveSection(
            title: 'MORNING',
            isMorning: true,
            widthBri: widget.config.widthLeftBri,
            widthCct: widget.config.widthLeftCct,
            mirror: _mirrorMorning!,
            onWidthBriChanged: (v) {
              var newConfig = widget.config.copyWith(widthLeftBri: v);
              // If mirror is on, sync CCT to brightness
              if (_mirrorMorning!) {
                newConfig = newConfig.copyWith(widthLeftCct: v);
              }
              widget.onConfigChanged(newConfig);
            },
            onWidthCctChanged: (v) =>
                widget.onConfigChanged(widget.config.copyWith(widthLeftCct: v)),
            onMirrorChanged: (v) {
              setState(() => _mirrorMorning = v);
              // Track mirror toggle
              AnalyticsService().logMirrorToggle(enabled: v, side: 'morning');
              // If turning on, sync CCT to brightness
              if (v) {
                widget.onConfigChanged(widget.config.copyWith(
                  widthLeftCct: widget.config.widthLeftBri,
                ));
              }
            },
            onReset: () {
              final d = CurveConfigDto.default_();
              widget.onConfigChanged(widget.config.copyWith(
                widthLeftBri: d.widthLeftBri,
                widthLeftCct: d.widthLeftCct,
              ));
            },
          ),
        ),
        const SizedBox(width: 8),
        // Evening section
        Expanded(
          child: _buildCurveSection(
            title: 'EVENING',
            isMorning: false,
            widthBri: widget.config.widthRightBri,
            widthCct: widget.config.widthRightCct,
            mirror: _mirrorEvening!,
            onWidthBriChanged: (v) {
              var newConfig = widget.config.copyWith(widthRightBri: v);
              // If mirror is on, sync CCT to brightness
              if (_mirrorEvening!) {
                newConfig = newConfig.copyWith(widthRightCct: v);
              }
              widget.onConfigChanged(newConfig);
            },
            onWidthCctChanged: (v) => widget
                .onConfigChanged(widget.config.copyWith(widthRightCct: v)),
            onMirrorChanged: (v) {
              setState(() => _mirrorEvening = v);
              // Track mirror toggle
              AnalyticsService().logMirrorToggle(enabled: v, side: 'evening');
              // If turning on, sync CCT to brightness
              if (v) {
                widget.onConfigChanged(widget.config.copyWith(
                  widthRightCct: widget.config.widthRightBri,
                ));
              }
            },
            onReset: () {
              final d = CurveConfigDto.default_();
              widget.onConfigChanged(widget.config.copyWith(
                widthRightBri: d.widthRightBri,
                widthRightCct: d.widthRightCct,
              ));
            },
          ),
        ),
      ],
    );
  }

  Widget _buildCurveSection({
    required String title,
    required bool isMorning,
    required double widthBri,
    required double widthCct,
    required bool mirror,
    required ValueChanged<double> onWidthBriChanged,
    required ValueChanged<double> onWidthCctChanged,
    required ValueChanged<bool> onMirrorChanged,
    required VoidCallback onReset,
  }) {
    return Container(
      padding: const EdgeInsets.all(10),
      decoration: BoxDecoration(
        color: const Color(0xFF1E1E2E),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: Colors.white.withValues(alpha: 0.1)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // Header with Reset button
          Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            children: [
              Text(
                title,
                style: const TextStyle(
                  color: Colors.white,
                  fontSize: 13,
                  fontWeight: FontWeight.bold,
                  letterSpacing: 1,
                ),
              ),
              _buildResetButton(onReset),
            ],
          ),
          const SizedBox(height: 8),

          // Brightness width
          const Text(
            'Brightness Ramp',
            style: TextStyle(
              color: Colors.white70,
              fontSize: 12,
              fontWeight: FontWeight.w500,
            ),
          ),
          const SizedBox(height: 4),
          _buildSliderRow(
            label: 'Width',
            value: widthBri,
            min: 0.2,
            max: 2.0,
            divisions: 36, // 0.05 increments
            displayValue: _formatWidth(widthBri),
            onChanged: onWidthBriChanged,
            enabled: true,
          ),
          const SizedBox(height: 6),

          // Color Temperature width with mirror checkbox
          Row(
            children: [
              const Text(
                'CCT Ramp',
                style: TextStyle(
                  color: Colors.white70,
                  fontSize: 12,
                  fontWeight: FontWeight.w500,
                ),
              ),
              const SizedBox(width: 8),
              _buildMirrorCheckbox(mirror, onMirrorChanged),
            ],
          ),
          const SizedBox(height: 4),
          _buildSliderRow(
            label: 'Width',
            value: mirror ? widthBri : widthCct,
            min: 0.2,
            max: 2.0,
            divisions: 36, // 0.05 increments
            displayValue: _formatWidth(mirror ? widthBri : widthCct),
            onChanged: onWidthCctChanged,
            enabled: !mirror,
          ),
        ],
      ),
    );
  }

  /// Format width value with speed indicator
  /// width < 1 = larger sigma = gentler slope = slower ramp
  /// width > 1 = smaller sigma = steeper slope = faster ramp
  String _formatWidth(double width) {
    String speed;
    if (width < 0.7) {
      speed = 'slow';
    } else if (width > 1.3) {
      speed = 'fast';
    } else {
      speed = 'normal';
    }
    return '${width.toStringAsFixed(2)} ($speed)';
  }

  Widget _buildResetButton(VoidCallback onPressed) {
    return TextButton(
      onPressed: onPressed,
      style: TextButton.styleFrom(
        backgroundColor: const Color(0xFF2A2A3E),
        foregroundColor: Colors.white70,
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
        minimumSize: Size.zero,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(4),
        ),
      ),
      child: const Text(
        'Reset',
        style: TextStyle(fontSize: 12),
      ),
    );
  }

  Widget _buildMirrorCheckbox(bool value, ValueChanged<bool> onChanged) {
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        SizedBox(
          width: 18,
          height: 18,
          child: Checkbox(
            value: value,
            onChanged: (v) => onChanged(v ?? true),
            fillColor: WidgetStateProperty.resolveWith((states) {
              if (states.contains(WidgetState.selected)) {
                return const Color(0xFF1E90FF);
              }
              return Colors.transparent;
            }),
            side: BorderSide(color: Colors.white.withValues(alpha: 0.5)),
            materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
          ),
        ),
        const SizedBox(width: 4),
        GestureDetector(
          onTap: () => onChanged(!value),
          child: const Text(
            'mirror brightness',
            style: TextStyle(
              color: Colors.white54,
              fontSize: 11,
            ),
          ),
        ),
      ],
    );
  }

  Widget _buildSliderRow({
    required String label,
    required double value,
    required double min,
    required double max,
    required String displayValue,
    required ValueChanged<double> onChanged,
    required bool enabled,
    int? divisions,
    VoidCallback? onChangeEnd,
    String? trackingParameter,
  }) {
    final effectiveColor = enabled ? Colors.white70 : Colors.white30;
    final sliderColor = enabled ? const Color(0xFF1E90FF) : Colors.white24;
    final trackColor = enabled ? Colors.white24 : Colors.white12;

    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2),
      child: Row(
        children: [
          SizedBox(
            width: 50,
            child: Text(
              label,
              style: TextStyle(
                color: effectiveColor,
                fontSize: 12,
              ),
            ),
          ),
          Expanded(
            child: SliderTheme(
              data: SliderThemeData(
                activeTrackColor: sliderColor,
                inactiveTrackColor: trackColor,
                thumbColor: enabled ? Colors.white : Colors.white54,
                overlayColor: sliderColor.withValues(alpha: 0.2),
                trackHeight: 4,
                thumbShape: const RoundSliderThumbShape(
                  enabledThumbRadius: 6,
                  disabledThumbRadius: 5,
                ),
              ),
              child: Slider(
                value: value.clamp(min, max),
                min: min,
                max: max,
                divisions: divisions,
                onChanged: enabled ? onChanged : null,
                onChangeEnd: enabled
                    ? (v) {
                        onChangeEnd?.call();
                        if (trackingParameter != null) {
                          AnalyticsService()
                              .logCurveSliderChange(trackingParameter, v);
                        }
                      }
                    : null,
              ),
            ),
          ),
          SizedBox(
            width: 90,
            child: Text(
              displayValue,
              style: TextStyle(
                color: effectiveColor,
                fontSize: 12,
                fontWeight: FontWeight.w500,
              ),
              textAlign: TextAlign.right,
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildShapeSection() {
    return Container(
      padding: const EdgeInsets.all(10),
      decoration: BoxDecoration(
        color: const Color(0xFF1E1E2E),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: Colors.white.withValues(alpha: 0.1)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text(
            'CURVE SHAPE',
            style: TextStyle(
              color: Colors.white,
              fontSize: 13,
              fontWeight: FontWeight.bold,
              letterSpacing: 1,
            ),
          ),
          const SizedBox(height: 6),
          _buildSliderRow(
            label: 'Shape',
            value: widget.config.shapeP,
            min: 2.0,
            max: 10.0,
            divisions: 8, // whole numbers: 2, 3, 4, 5, 6, 7, 8, 9, 10
            displayValue: _formatShape(widget.config.shapeP),
            onChanged: (v) =>
                widget.onConfigChanged(widget.config.copyWith(shapeP: v)),
            enabled: true,
            trackingParameter: 'shape',
          ),
          Text(
            'Lower = round peak, Higher = flat plateau',
            style: TextStyle(
              color: Colors.white.withValues(alpha: 0.5),
              fontSize: 10,
            ),
          ),
        ],
      ),
    );
  }

  String _formatShape(double p) {
    String shape;
    if (p <= 3) {
      shape = 'round';
    } else if (p >= 5) {
      shape = 'flat';
    } else {
      shape = 'moderate';
    }
    return '${p.toStringAsFixed(1)} ($shape)';
  }

  Widget _buildRangesSection() {
    return Container(
      padding: const EdgeInsets.all(10),
      decoration: BoxDecoration(
        color: const Color(0xFF1E1E2E),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: Colors.white.withValues(alpha: 0.1)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text(
            'RANGES',
            style: TextStyle(
              color: Colors.white,
              fontSize: 13,
              fontWeight: FontWeight.bold,
              letterSpacing: 1,
            ),
          ),
          const SizedBox(height: 6),
          _buildRangeSlider(
            label: 'Brightness (min-\nmax)',
            minValue: widget.config.minBrightness.toDouble(),
            maxValue: widget.config.maxBrightness.toDouble(),
            rangeMin: 1,
            rangeMax: 100,
            minLabel: widget.config.minBrightness.toString(),
            maxLabel: widget.config.maxBrightness.toString(),
            onChanged: (min, max) {
              widget.onConfigChanged(widget.config.copyWith(
                minBrightness: min.round(),
                maxBrightness: max.round(),
              ));
            },
          ),
          const SizedBox(height: 6),
          _buildRangeSlider(
            label: 'Color Temperature\n(K, min-max)',
            minValue: widget.config.minColorTemp.toDouble(),
            maxValue: widget.config.maxColorTemp.toDouble(),
            rangeMin: 500,
            rangeMax: 6500,
            minLabel: widget.config.minColorTemp.toString(),
            maxLabel: widget.config.maxColorTemp.toString(),
            onChanged: (min, max) {
              widget.onConfigChanged(widget.config.copyWith(
                minColorTemp: min.round(),
                maxColorTemp: max.round(),
              ));
            },
          ),
        ],
      ),
    );
  }

  Widget _buildSolarContextToggle(ConfigModel model) {
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        SizedBox(
          height: 24,
          width: 24,
          child: Checkbox(
            value: model.showSolarContext,
            onChanged: (value) {
              model.setShowSolarContext(value ?? true);
            },
            fillColor: WidgetStateProperty.resolveWith((states) {
              if (states.contains(WidgetState.selected)) {
                return const Color(0xFF1E90FF);
              }
              return Colors.transparent;
            }),
            side: const BorderSide(color: Colors.white54),
          ),
        ),
        const SizedBox(width: 6),
        const Text(
          'Solar Context',
          style: TextStyle(color: Colors.white70, fontSize: 13),
        ),
      ],
    );
  }

  Widget _buildRangeSlider({
    required String label,
    required double minValue,
    required double maxValue,
    required double rangeMin,
    required double rangeMax,
    required String minLabel,
    required String maxLabel,
    required void Function(double min, double max) onChanged,
  }) {
    return Row(
      children: [
        SizedBox(
          width: 120,
          child: Text(
            label,
            style: const TextStyle(
              color: Colors.white70,
              fontSize: 12,
              height: 1.3,
            ),
          ),
        ),
        Text(
          minLabel,
          style: const TextStyle(
            color: Colors.white54,
            fontSize: 12,
          ),
        ),
        Expanded(
          child: SliderTheme(
            data: SliderThemeData(
              activeTrackColor: const Color(0xFF1E90FF),
              inactiveTrackColor: Colors.white24,
              thumbColor: Colors.white,
              overlayColor: const Color(0xFF1E90FF).withValues(alpha: 0.2),
              trackHeight: 4,
              rangeThumbShape: const RoundRangeSliderThumbShape(
                enabledThumbRadius: 6,
              ),
            ),
            child: RangeSlider(
              values: RangeValues(
                minValue.clamp(rangeMin, rangeMax),
                maxValue.clamp(rangeMin, rangeMax),
              ),
              min: rangeMin,
              max: rangeMax,
              onChanged: (values) {
                onChanged(values.start, values.end);
              },
            ),
          ),
        ),
        Text(
          maxLabel,
          style: const TextStyle(
            color: Colors.white54,
            fontSize: 12,
          ),
        ),
      ],
    );
  }
}
