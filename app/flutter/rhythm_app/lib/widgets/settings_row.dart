import 'package:flutter/material.dart';
import 'solar_orbit.dart'; // For CelestialColors

/// Warm amber color for settings icons (like Streaks app)
const Color _settingsAmber = Color(0xFFFFC107);

/// A reusable settings row widget with icon, label, value, and optional chevron.
///
/// Redesigned for a cleaner, more spacious look inspired by Streaks app:
/// - Solid amber circular icons (not translucent)
/// - No individual card backgrounds (used inside SettingsGroup cards)
/// - Clean typography with proper hierarchy
class SettingsRow extends StatelessWidget {
  final IconData icon;
  final Color? iconColor;
  final String label;
  final String? value;
  final Widget? trailing;
  final bool showChevron;
  final VoidCallback? onTap;

  const SettingsRow({
    super.key,
    required this.icon,
    this.iconColor,
    required this.label,
    this.value,
    this.trailing,
    this.showChevron = true,
    this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final effectiveIconColor = iconColor ?? _settingsAmber;
    // Use dark icon on light backgrounds, light icon on dark/colored backgrounds
    final iconForeground = _isDarkColor(effectiveIconColor)
        ? Colors.white
        : const Color(0xFF1A1A1A);

    return GestureDetector(
      onTap: onTap,
      behavior: HitTestBehavior.opaque,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
        child: Row(
          children: [
            // Solid colored circular icon
            Container(
              width: 36,
              height: 36,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: effectiveIconColor,
              ),
              child: Icon(
                icon,
                color: iconForeground,
                size: 18,
              ),
            ),
            const SizedBox(width: 14),
            // Label only (value moved to trailing area)
            Expanded(
              child: Text(
                label,
                style: const TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 16,
                  fontWeight: FontWeight.w400,
                  letterSpacing: -0.2,
                ),
              ),
            ),
            // Value text (if present, shown before trailing)
            if (value != null) ...[
              Flexible(
                child: Text(
                  value!,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.7),
                    fontSize: 15,
                  ),
                ),
              ),
              const SizedBox(width: 6),
            ],
            // Trailing widget or chevron
            if (trailing != null)
              trailing!
            else if (showChevron)
              Icon(
                Icons.chevron_right,
                color: CelestialColors.textSecondary.withValues(alpha: 0.5),
                size: 22,
              ),
          ],
        ),
      ),
    );
  }

  /// Check if a color is dark (for determining icon foreground color)
  bool _isDarkColor(Color color) {
    return color.computeLuminance() < 0.5;
  }
}

/// A settings section header with a title.
class SettingsSectionHeader extends StatelessWidget {
  final String title;

  const SettingsSectionHeader({
    super.key,
    required this.title,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(left: 8, bottom: 12, top: 28),
      child: Text(
        title.toUpperCase(),
        style: TextStyle(
          color: CelestialColors.textSecondary.withValues(alpha: 0.6),
          fontSize: 13,
          fontWeight: FontWeight.w500,
          letterSpacing: 0.8,
        ),
      ),
    );
  }
}

/// A group of settings rows contained in a single card with dividers.
/// This creates the clean, grouped look like the Streaks app.
class SettingsGroup extends StatelessWidget {
  final List<Widget> children;

  const SettingsGroup({
    super.key,
    required this.children,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(14),
      ),
      clipBehavior: Clip.antiAlias,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: MainAxisSize.min,
        children: [
          for (int i = 0; i < children.length; i++) ...[
            children[i],
            if (i < children.length - 1)
              Padding(
                padding: const EdgeInsets.only(left: 66),
                child: Container(
                  height: 0.5,
                  color: CelestialColors.orbitRing.withValues(alpha: 0.2),
                ),
              ),
          ],
        ],
      ),
    );
  }
}
