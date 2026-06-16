import 'package:flutter/material.dart';

class BetaBadge extends StatelessWidget {
  final double fontSize;
  final EdgeInsets padding;

  const BetaBadge({
    super.key,
    this.fontSize = 9,
    this.padding = const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
  });

  @override
  Widget build(BuildContext context) {
    const badgeColor = Color(0xFFE8A54B);

    return Container(
      padding: padding,
      decoration: BoxDecoration(
        color: badgeColor.withValues(alpha: 0.15),
        borderRadius: BorderRadius.circular(999),
        border: Border.all(
          color: badgeColor.withValues(alpha: 0.4),
        ),
      ),
      child: Text(
        'BETA',
        style: TextStyle(
          fontSize: fontSize,
          fontWeight: FontWeight.w700,
          letterSpacing: 1,
          color: badgeColor,
        ),
      ),
    );
  }
}

class BetaLabel extends StatelessWidget {
  final String label;
  final TextStyle style;
  final double spacing;
  final double badgeFontSize;
  final EdgeInsets badgePadding;

  const BetaLabel({
    super.key,
    required this.label,
    required this.style,
    this.spacing = 8,
    this.badgeFontSize = 8,
    this.badgePadding = const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
  });

  @override
  Widget build(BuildContext context) {
    return Wrap(
      spacing: spacing,
      runSpacing: 4,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        Text(label, style: style),
        BetaBadge(
          fontSize: badgeFontSize,
          padding: badgePadding,
        ),
      ],
    );
  }
}
