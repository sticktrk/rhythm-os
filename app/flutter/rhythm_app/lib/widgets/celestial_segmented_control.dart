import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'solar_orbit.dart' show CelestialColors;

/// One choice in a [CelestialSegmentedControl].
class CelestialSegment<T> {
  const CelestialSegment({
    required this.value,
    required this.label,
    required this.accent,
    this.icon,
    this.key,
    this.semanticLabel,
  });

  final T value;
  final String label;

  /// Tint used for the active (selected or busy) state. Segments may carry
  /// distinct accents (Wake amber vs Sleep indigo) or share one.
  final Color accent;
  final IconData? icon;
  final Key? key;
  final String? semanticLabel;
}

/// Density presets for [CelestialSegmentedControl].
enum CelestialSegmentedDensity {
  /// Icon + label segments (mode toggles, source pickers, action rows).
  regular,

  /// Tight text-only segments (the 4-way behavior rows).
  compact,
}

/// The app's segmented control: a recessed track holding mutually exclusive
/// segments, with the active one tinted in its accent. This single widget
/// backs the Presets screen's manual Wake/Sleep toggle, the room Schedule
/// tab's source + test toggles, and the behavior rows — so "segmented"
/// renders identically everywhere.
class CelestialSegmentedControl<T> extends StatelessWidget {
  const CelestialSegmentedControl({
    super.key,
    required this.segments,
    required this.selected,
    required this.onTap,
    this.busyValue,
    this.enabled = true,
    this.allowReselect = false,
    this.density = CelestialSegmentedDensity.regular,
    this.trackColor,
    this.tintUnselectedIcons = false,
    this.mediumHaptic = false,
  });

  final List<CelestialSegment<T>> segments;

  /// The currently active value, or null when nothing is selected.
  final T? selected;
  final ValueChanged<T> onTap;

  /// Segment whose icon slot shows a spinner (an in-flight action). Renders
  /// with active styling regardless of [selected].
  final T? busyValue;
  final bool enabled;

  /// Whether tapping the already-selected segment still fires [onTap] —
  /// true for action rows (re-triggering a test) and idempotent saves.
  final bool allowReselect;
  final CelestialSegmentedDensity density;

  /// Track fill behind the segments. Defaults to the recessed dark wash used
  /// inside cards; screens sitting on the dark background pass a card color.
  final Color? trackColor;

  /// Keep unselected segment icons in their accent (action rows keep their
  /// sun/moon color as an affordance) instead of the neutral grey.
  final bool tintUnselectedIcons;

  /// Use a medium impact on tap (mode switches, tests) instead of the
  /// selection click.
  final bool mediumHaptic;

  @override
  Widget build(BuildContext context) {
    final compact = density == CelestialSegmentedDensity.compact;
    return Container(
      padding: EdgeInsets.all(compact ? 3 : 4),
      decoration: BoxDecoration(
        color: trackColor ??
            CelestialColors.backgroundDark.withValues(alpha: 0.55),
        borderRadius: BorderRadius.circular(compact ? 10 : 12),
      ),
      child: Row(
        children: [for (final segment in segments) _buildSegment(segment)],
      ),
    );
  }

  Widget _buildSegment(CelestialSegment<T> segment) {
    final compact = density == CelestialSegmentedDensity.compact;
    final isSelected = segment.value == selected;
    final isBusy = busyValue != null && segment.value == busyValue;
    final active = isSelected || isBusy;
    final accent = segment.accent;
    final canTap = enabled && (allowReselect || !isSelected);

    return Expanded(
      child: Semantics(
        key: segment.key,
        button: true,
        selected: isSelected,
        enabled: enabled,
        label: segment.semanticLabel ?? segment.label,
        excludeSemantics: true,
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTap: canTap
              ? () {
                  if (mediumHaptic) {
                    HapticFeedback.mediumImpact();
                  } else {
                    HapticFeedback.selectionClick();
                  }
                  onTap(segment.value);
                }
              : null,
          child: AnimatedOpacity(
            duration: const Duration(milliseconds: 150),
            opacity: enabled || isBusy ? 1.0 : 0.55,
            child: AnimatedContainer(
              duration: const Duration(milliseconds: 250),
              curve: Curves.easeOut,
              constraints: const BoxConstraints(minHeight: 44),
              padding: EdgeInsets.symmetric(vertical: compact ? 8 : 11),
              decoration: BoxDecoration(
                borderRadius: BorderRadius.circular(compact ? 8 : 9),
                color: active
                    ? accent.withValues(alpha: 0.16)
                    : Colors.transparent,
                border: Border.all(
                  color: active
                      ? accent.withValues(alpha: 0.5)
                      : Colors.transparent,
                  width: 1,
                ),
              ),
              child: Row(
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  if (isBusy)
                    SizedBox.square(
                      dimension: 16,
                      child: CircularProgressIndicator(
                        strokeWidth: 2,
                        color: accent,
                      ),
                    )
                  else if (segment.icon != null)
                    Icon(
                      segment.icon,
                      size: compact ? 14 : 16,
                      color: active
                          ? accent
                          : tintUnselectedIcons
                              ? accent.withValues(alpha: 0.85)
                              : CelestialColors.textSecondary
                                  .withValues(alpha: 0.5),
                    ),
                  if (isBusy || segment.icon != null)
                    SizedBox(width: compact ? 6 : 8),
                  Flexible(
                    child: Text(
                      segment.label,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        color: active
                            ? accent
                            : CelestialColors.textSecondary
                                .withValues(alpha: 0.7),
                        fontSize: compact ? 11.5 : 13.5,
                        fontWeight: FontWeight.w600,
                        letterSpacing: 0.2,
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}
