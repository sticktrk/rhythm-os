import 'package:flutter/material.dart';

/// Warm coral used for physical light-delivery warnings on celestial surfaces.
const lightDeliveryWarningColor = Color(0xFFFF6159);

/// Compact warning signal shared by room cards, bulb cards, and bulb rows.
///
/// A count appears only when more than one target is affected, keeping the
/// common single-bulb state visually quiet while making a room fan-out failure
/// immediately distinct.
class LightDeliveryWarningSignal extends StatelessWidget {
  const LightDeliveryWarningSignal({
    super.key,
    this.count = 1,
    this.size = 19,
    this.glow = false,
  });

  final int count;
  final double size;
  final bool glow;

  @override
  Widget build(BuildContext context) {
    final showCount = count > 1;
    return SizedBox(
      width: showCount ? size + 8 : size,
      height: size,
      child: Stack(
        clipBehavior: Clip.none,
        children: [
          if (glow)
            Positioned.fill(
              child: DecoratedBox(
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  boxShadow: [
                    BoxShadow(
                      color: lightDeliveryWarningColor.withValues(alpha: 0.38),
                      blurRadius: 10,
                    ),
                  ],
                ),
              ),
            ),
          Icon(
            Icons.warning_amber_rounded,
            size: size,
            color: lightDeliveryWarningColor,
          ),
          if (showCount)
            Positioned(
              right: -1,
              top: -4,
              child: Container(
                constraints: const BoxConstraints(minWidth: 14, minHeight: 14),
                padding: const EdgeInsets.symmetric(horizontal: 3),
                decoration: BoxDecoration(
                  color: lightDeliveryWarningColor,
                  borderRadius: BorderRadius.circular(8),
                  border: Border.all(color: const Color(0xFF161D25), width: 1),
                ),
                alignment: Alignment.center,
                child: Text(
                  count > 9 ? '9+' : '$count',
                  style: const TextStyle(
                    color: Colors.white,
                    fontSize: 8,
                    fontWeight: FontWeight.w800,
                    height: 1,
                  ),
                ),
              ),
            ),
        ],
      ),
    );
  }
}
