import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'solar_orbit.dart' show CelestialColors;

/// Circular ✕ button for the top-level menu headers (Presets, Settings).
///
/// These screens are reached by fanning out of the gear button and have no
/// Home entry to return through, so each carries its own close affordance that
/// dismisses back to Home.
class HeaderCloseButton extends StatelessWidget {
  const HeaderCloseButton({super.key, required this.onTap});

  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return Semantics(
      button: true,
      label: 'Close',
      child: Material(
        color: Colors.transparent,
        shape: const CircleBorder(),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: () {
            HapticFeedback.selectionClick();
            onTap();
          },
          child: Container(
            width: 40,
            height: 40,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: CelestialColors.backgroundCard.withValues(alpha: 0.85),
              border: Border.all(
                color: CelestialColors.orbitRing.withValues(alpha: 0.5),
              ),
            ),
            child: Icon(
              Icons.close_rounded,
              color: CelestialColors.textSecondary.withValues(alpha: 0.9),
              size: 22,
            ),
          ),
        ),
      ),
    );
  }
}
