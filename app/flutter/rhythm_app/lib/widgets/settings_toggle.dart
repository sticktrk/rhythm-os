import 'package:flutter/material.dart';
import 'solar_orbit.dart'; // For CelestialColors

/// A reusable toggle switch widget styled for the settings screen.
class SettingsToggle extends StatelessWidget {
  final bool value;

  const SettingsToggle({
    super.key,
    required this.value,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      width: 50,
      height: 28,
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(14),
        color: value
            ? CelestialColors.accentBlue.withValues(alpha: 0.3)
            : CelestialColors.orbitRing.withValues(alpha: 0.5),
      ),
      child: Stack(
        children: [
          AnimatedPositioned(
            duration: const Duration(milliseconds: 200),
            curve: Curves.easeOut,
            left: value ? 24 : 2,
            top: 2,
            child: Container(
              width: 24,
              height: 24,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: value
                    ? CelestialColors.accentBlue
                    : CelestialColors.textSecondary,
              ),
            ),
          ),
        ],
      ),
    );
  }
}
