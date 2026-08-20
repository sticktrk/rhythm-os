import 'package:flutter/material.dart';

/// A mode-tinted summary chip: accent-washed fill and hairline, an icon +
/// label header row, and a caller-supplied value area beneath it. Shared by
/// the Alarm Schedule screen's Day/Sleep Start chips (value = trigger
/// summary text) and the room Schedule tab's Wake/Sleep chips (value =
/// time + stepper row), so the "mode chip" reads identically at both scopes.
class ModeSummaryChip extends StatelessWidget {
  const ModeSummaryChip({
    super.key,
    required this.icon,
    required this.label,
    required this.accent,
    required this.child,
    this.centered = false,
    this.padding = const EdgeInsets.fromLTRB(12, 8, 12, 10),
    this.headerGap = 4,
  });

  final IconData icon;
  final String label;
  final Color accent;

  /// The value area under the header — a summary line, a time with
  /// steppers, whatever the host needs.
  final Widget child;

  /// Center the header and value (room time chips) instead of the default
  /// start alignment (alarm trigger chips).
  final bool centered;
  final EdgeInsetsGeometry padding;
  final double headerGap;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: padding,
      decoration: BoxDecoration(
        color: accent.withValues(alpha: 0.08),
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: accent.withValues(alpha: 0.18)),
      ),
      child: Column(
        crossAxisAlignment:
            centered ? CrossAxisAlignment.center : CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Row(
            mainAxisSize: centered ? MainAxisSize.min : MainAxisSize.max,
            mainAxisAlignment:
                centered ? MainAxisAlignment.center : MainAxisAlignment.start,
            children: [
              Icon(icon, size: 14, color: accent),
              const SizedBox(width: 6),
              Flexible(
                child: Text(
                  label,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: accent,
                    fontSize: 12,
                    fontWeight: FontWeight.w700,
                    letterSpacing: 0.4,
                  ),
                ),
              ),
            ],
          ),
          SizedBox(height: headerGap),
          child,
        ],
      ),
    );
  }
}
