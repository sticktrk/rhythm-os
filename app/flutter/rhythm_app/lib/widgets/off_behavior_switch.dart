import 'package:flutter/material.dart';
import 'solar_orbit.dart' show CelestialColors;

/// A two-segment switch for a node's off behavior — both options stay visible
/// ("Off" | "Dim") with the active one tinted. Tapping a segment selects
/// that state.
///
/// Shared between the room settings sheet and the individual-light (device)
/// detail sheet so a bulb gets the same standby control a room does.
class OffBehaviorSwitch extends StatelessWidget {
  final bool standby;
  final ValueChanged<bool> onChanged;

  const OffBehaviorSwitch({
    super.key,
    required this.standby,
    required this.onChanged,
  });

  static const Color _standbyColor = Color(0xFF7C83FF);

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.all(3),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundDark.withValues(alpha: 0.55),
        borderRadius: BorderRadius.circular(999),
        border: Border.all(
          color: CelestialColors.orbitRing.withValues(alpha: 0.4),
        ),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          _segment(
            label: 'Off',
            selected: !standby,
            color: CelestialColors.textSecondary,
            onTap: () => onChanged(false),
          ),
          _segment(
            label: 'Dim',
            selected: standby,
            color: _standbyColor,
            onTap: () => onChanged(true),
          ),
        ],
      ),
    );
  }

  Widget _segment({
    required String label,
    required bool selected,
    required Color color,
    required VoidCallback onTap,
  }) {
    return GestureDetector(
      onTap: onTap,
      behavior: HitTestBehavior.opaque,
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 180),
        curve: Curves.easeOut,
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 5),
        decoration: BoxDecoration(
          color: selected ? color.withValues(alpha: 0.18) : Colors.transparent,
          borderRadius: BorderRadius.circular(999),
        ),
        child: Text(
          label,
          style: TextStyle(
            color: selected
                ? color
                : CelestialColors.textSecondary.withValues(alpha: 0.5),
            fontSize: 13,
            fontWeight: FontWeight.w600,
            letterSpacing: 0.2,
          ),
        ),
      ),
    );
  }
}
