import 'package:flutter/material.dart';

import 'info_tooltip.dart';

/// Generic numerical setting row with an Auto/value toggle and slider.
///
/// Tap behaviors:
/// - Tap anywhere on the header row while in Auto to open the slider.
/// - Tap the Auto chip to collapse the slider.
/// - Tap the value chip to open the slider.
class AutoSliderSettingRow extends StatelessWidget {
  static const Color textSecondary = Color(0xFF8A919C);

  final IconData icon;
  final String title;
  final Color color;
  final bool isAuto;
  final double sliderValue;
  final double effectiveValue;
  final double sliderMin;
  final double sliderMax;
  final int divisions;
  final String Function(double) format;
  final ValueChanged<double> onSliderChanged;
  final ValueChanged<double>? onSliderChangeEnd;
  final VoidCallback onAuto;
  final VoidCallback onManual;
  final String? tooltip;

  const AutoSliderSettingRow({
    super.key,
    required this.icon,
    required this.title,
    required this.color,
    required this.isAuto,
    required this.sliderValue,
    required this.effectiveValue,
    required this.sliderMin,
    required this.sliderMax,
    required this.divisions,
    required this.format,
    required this.onSliderChanged,
    this.onSliderChangeEnd,
    required this.onAuto,
    required this.onManual,
    this.tooltip,
  });

  @override
  Widget build(BuildContext context) {
    final displayValue = isAuto ? effectiveValue : sliderValue;
    return Column(
      children: [
        GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTap: isAuto ? onManual : null,
          child: Row(
            children: [
              Container(
                width: 30,
                height: 30,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: color.withValues(alpha: 0.1),
                ),
                child: Icon(icon, color: color, size: 15),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Row(
                  children: [
                    Flexible(
                      child: Text(
                        title,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(
                          color: textSecondary,
                          fontSize: 14,
                          fontWeight: FontWeight.w500,
                        ),
                      ),
                    ),
                    if (tooltip != null) ...[
                      const SizedBox(width: 2),
                      InfoTooltip(message: tooltip!, iconSize: 13),
                    ],
                  ],
                ),
              ),
              _TimingAutoToggle(
                color: color,
                isAuto: isAuto,
                valueLabel: format(displayValue),
                onAuto: onAuto,
                onManual: onManual,
              ),
            ],
          ),
        ),
        AnimatedSize(
          duration: const Duration(milliseconds: 250),
          curve: Curves.easeOutCubic,
          alignment: Alignment.topCenter,
          child: isAuto
              ? const SizedBox.shrink()
              : Padding(
                  padding: const EdgeInsets.only(top: 8),
                  child: Column(
                    children: [
                      SliderTheme(
                        data: SliderThemeData(
                          activeTrackColor: color,
                          inactiveTrackColor: color.withValues(alpha: 0.12),
                          thumbColor: color,
                          overlayColor: color.withValues(alpha: 0.12),
                          trackHeight: 4,
                          thumbShape: const RoundSliderThumbShape(
                            enabledThumbRadius: 8,
                          ),
                          overlayShape: const RoundSliderOverlayShape(
                            overlayRadius: 18,
                          ),
                        ),
                        child: Slider(
                          value: sliderValue.clamp(sliderMin, sliderMax),
                          min: sliderMin,
                          max: sliderMax,
                          divisions: divisions,
                          onChanged: onSliderChanged,
                          onChangeEnd: onSliderChangeEnd,
                        ),
                      ),
                      Padding(
                        padding: const EdgeInsets.symmetric(horizontal: 6),
                        child: Row(
                          mainAxisAlignment: MainAxisAlignment.spaceBetween,
                          children: [
                            Text(
                              format(sliderMin),
                              style: TextStyle(
                                color: textSecondary.withValues(alpha: 0.4),
                                fontSize: 11,
                              ),
                            ),
                            Text(
                              format(sliderMax),
                              style: TextStyle(
                                color: textSecondary.withValues(alpha: 0.4),
                                fontSize: 11,
                              ),
                            ),
                          ],
                        ),
                      ),
                    ],
                  ),
                ),
        ),
      ],
    );
  }
}

class _TimingAutoToggle extends StatelessWidget {
  final Color color;
  final bool isAuto;
  final String valueLabel;
  final VoidCallback onAuto;
  final VoidCallback onManual;

  const _TimingAutoToggle({
    required this.color,
    required this.isAuto,
    required this.valueLabel,
    required this.onAuto,
    required this.onManual,
  });

  @override
  Widget build(BuildContext context) {
    Widget chipFrame({
      required bool active,
      required Widget child,
      required VoidCallback onTap,
    }) {
      return GestureDetector(
        onTap: onTap,
        behavior: HitTestBehavior.opaque,
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
          decoration: BoxDecoration(
            color: active ? color.withValues(alpha: 0.14) : Colors.transparent,
            borderRadius: BorderRadius.circular(6),
            border: Border.all(
              color: active
                  ? color.withValues(alpha: 0.30)
                  : color.withValues(alpha: 0.14),
            ),
          ),
          child: child,
        ),
      );
    }

    Widget textBody(String label, bool active) {
      return Text(
        label,
        style: TextStyle(
          color: active ? color : color.withValues(alpha: 0.45),
          fontSize: 11,
          fontWeight: FontWeight.w600,
          fontFeatures: const [FontFeature.tabularFigures()],
        ),
      );
    }

    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        chipFrame(
          active: isAuto,
          child: textBody('Auto', isAuto),
          onTap: isAuto ? () {} : onAuto,
        ),
        const SizedBox(width: 6),
        chipFrame(
          active: !isAuto,
          child: textBody(valueLabel, !isAuto),
          onTap: !isAuto ? () {} : onManual,
        ),
      ],
    );
  }
}
