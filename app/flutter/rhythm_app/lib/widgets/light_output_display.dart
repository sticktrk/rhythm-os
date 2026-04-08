import 'package:flutter/material.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'solar_orbit.dart';

/// Displays the current light output values (brightness and color temperature).
///
/// Shows:
/// - Brightness percentage
/// - Color temperature in Kelvin
/// - A colored indicator showing the actual CCT color
/// - Optional time display
class LightOutputDisplay extends StatelessWidget {
  final int brightness;
  final int kelvin;
  final double? selectedHour;

  const LightOutputDisplay({
    super.key,
    required this.brightness,
    required this.kelvin,
    this.selectedHour,
  });

  String _formatTime(double hour, {required bool use12Hour}) {
    final totalMinutes = (hour * 60).round();
    final hours = totalMinutes ~/ 60;
    final minutes = totalMinutes % 60;

    if (use12Hour) {
      final period = hours >= 12 ? 'PM' : 'AM';
      final displayHours = hours == 0 ? 12 : (hours > 12 ? hours - 12 : hours);
      return '${displayHours.toString().padLeft(2, '0')}:${minutes.toString().padLeft(2, '0')} $period';
    } else {
      return '${hours.toString().padLeft(2, '0')}:${minutes.toString().padLeft(2, '0')}';
    }
  }

  String _getTemperatureLabel(int kelvin) {
    if (kelvin <= 2000) return 'Candlelight';
    if (kelvin <= 2700) return 'Warm';
    if (kelvin <= 3500) return 'Soft White';
    if (kelvin <= 4500) return 'Neutral';
    if (kelvin <= 5500) return 'Cool White';
    return 'Daylight';
  }

  @override
  Widget build(BuildContext context) {
    final cctColor = ColorUtils.cctToColor(kelvin);

    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 16),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard.withValues(alpha: 0.8),
        borderRadius: BorderRadius.circular(16),
        border: Border.all(
          color: CelestialColors.orbitRing.withValues(alpha: 0.3),
        ),
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          // Time display (if provided)
          if (selectedHour != null) ...[
            Text(
              _formatTime(selectedHour!, use12Hour: !MediaQuery.alwaysUse24HourFormatOf(context)),
              style: const TextStyle(
                color: CelestialColors.textSecondary,
                fontSize: 14,
                fontWeight: FontWeight.w500,
              ),
            ),
            const SizedBox(height: 8),
          ],
          // Main values row
          Row(
            mainAxisAlignment: MainAxisAlignment.center,
            mainAxisSize: MainAxisSize.min,
            children: [
              // Brightness
              _buildValueDisplay(
                value: '$brightness%',
                label: 'Brightness',
                icon: Icons.brightness_6,
              ),
              const SizedBox(width: 24),
              // Color indicator
              Container(
                width: 12,
                height: 12,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: cctColor,
                  boxShadow: [
                    BoxShadow(
                      color: cctColor.withValues(alpha: 0.5),
                      blurRadius: 6,
                      spreadRadius: 1,
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 24),
              // Color temperature
              _buildValueDisplay(
                value: '${kelvin}K',
                label: _getTemperatureLabel(kelvin),
                icon: Icons.thermostat,
              ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _buildValueDisplay({
    required String value,
    required String label,
    required IconData icon,
  }) {
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              icon,
              size: 16,
              color: CelestialColors.textSecondary,
            ),
            const SizedBox(width: 4),
            Text(
              value,
              style: const TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 20,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
        ),
        const SizedBox(height: 2),
        Text(
          label,
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.7),
            fontSize: 11,
          ),
        ),
      ],
    );
  }
}

/// A compact version of the light output display for inline use.
class LightOutputCompact extends StatelessWidget {
  final int brightness;
  final int kelvin;
  final Color? directColor;

  const LightOutputCompact({
    super.key,
    required this.brightness,
    required this.kelvin,
    this.directColor,
  });

  @override
  Widget build(BuildContext context) {
    final hasDirectColor = directColor != null;
    final dotColor = hasDirectColor
        ? directColor!
        : ColorUtils.cctToColor(kelvin);

    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Text(
          '$brightness%',
          style: const TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 16,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(width: 12),
        Container(
          width: 10,
          height: 10,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: dotColor,
            boxShadow: [
              BoxShadow(
                color: dotColor.withValues(alpha: 0.5),
                blurRadius: 4,
              ),
            ],
          ),
        ),
        if (!hasDirectColor) ...[
          const SizedBox(width: 12),
          Text(
            '${kelvin}K',
            style: const TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 16,
              fontWeight: FontWeight.w600,
            ),
          ),
        ],
      ],
    );
  }
}
