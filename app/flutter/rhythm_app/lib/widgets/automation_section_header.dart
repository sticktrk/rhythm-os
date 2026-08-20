import 'package:flutter/material.dart';

import 'solar_orbit.dart' show CelestialColors;

/// A numbered, two-line section header for schedule/automation flows. The
/// tinted step badge plus subtitle turns loose lists into a clear sequence:
/// ① decide how things switch → ② define what each mode does → ③ try it.
///
/// Shared by the whole-house Presets screen and the per-room Schedule tab so
/// both read as the same feature at two scopes.
class AutomationSectionHeader extends StatelessWidget {
  final int step;
  final String title;
  final String subtitle;
  final Color accent;

  const AutomationSectionHeader({
    super.key,
    required this.step,
    required this.title,
    required this.subtitle,
    required this.accent,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(left: 2, right: 8, top: 30, bottom: 14),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // Tinted numeral badge — conveys "first, then" ordering.
          Container(
            width: 26,
            height: 26,
            alignment: Alignment.center,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: accent.withValues(alpha: 0.16),
              border: Border.all(
                color: accent.withValues(alpha: 0.4),
                width: 1,
              ),
            ),
            child: Text(
              '$step',
              style: TextStyle(
                color: accent,
                fontSize: 13,
                fontWeight: FontWeight.w700,
                height: 1,
              ),
            ),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  title.toUpperCase(),
                  style: TextStyle(
                    color: accent,
                    fontSize: 12.5,
                    fontWeight: FontWeight.w700,
                    letterSpacing: 0.9,
                  ),
                ),
                const SizedBox(height: 3),
                Text(
                  subtitle,
                  style: TextStyle(
                    color:
                        CelestialColors.textSecondary.withValues(alpha: 0.85),
                    fontSize: 13,
                    height: 1.35,
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}
