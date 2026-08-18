import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'solar_orbit.dart' show CelestialColors;

/// One tab in a [SegmentedTabBar].
class SegmentedTab<T> {
  final String label;
  final T value;
  final Key? key;
  const SegmentedTab(this.label, this.value, {this.key});
}

/// A segmented (pill) tab selector — the shared chrome used by the room
/// settings sheet and the device (bulb) detail sheet so they read as the same
/// kind of surface. The selected tab is tinted warm; tapping switches.
class SegmentedTabBar<T> extends StatelessWidget {
  final List<SegmentedTab<T>> tabs;
  final T selected;
  final ValueChanged<T> onChanged;

  const SegmentedTabBar({
    super.key,
    required this.tabs,
    required this.selected,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      height: 36,
      decoration: BoxDecoration(
        color: CelestialColors.backgroundDark.withValues(alpha: 0.5),
        borderRadius: BorderRadius.circular(18),
        border: Border.all(
          color: CelestialColors.orbitRing.withValues(alpha: 0.3),
          width: 1,
        ),
      ),
      child: Row(
        children: [for (final tab in tabs) _item(tab)],
      ),
    );
  }

  Widget _item(SegmentedTab<T> tab) {
    final isSelected = tab.value == selected;
    return Expanded(
      child: Semantics(
        key: tab.key ?? ValueKey('segmented-tab-${tab.label.toLowerCase()}'),
        button: true,
        selected: isSelected,
        label: '${tab.label} tab',
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTap: () {
            HapticFeedback.selectionClick();
            onChanged(tab.value);
          },
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 200),
            margin: const EdgeInsets.all(3),
            decoration: BoxDecoration(
              color: isSelected
                  ? CelestialColors.sunWarm.withValues(alpha: 0.2)
                  : Colors.transparent,
              borderRadius: BorderRadius.circular(15),
              border: isSelected
                  ? Border.all(
                      color: CelestialColors.sunWarm.withValues(alpha: 0.4),
                      width: 1,
                    )
                  : null,
            ),
            alignment: Alignment.center,
            child: ExcludeSemantics(
              child: Text(
                tab.label,
                style: TextStyle(
                  color: isSelected
                      ? CelestialColors.sunWarm
                      : CelestialColors.textSecondary.withValues(alpha: 0.7),
                  fontSize: 13,
                  fontWeight: isSelected ? FontWeight.w600 : FontWeight.w500,
                  letterSpacing: 0.5,
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}
