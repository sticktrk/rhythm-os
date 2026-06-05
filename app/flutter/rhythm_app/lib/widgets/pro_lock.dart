import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../config/feature_flags.dart';
import '../models/plan_tier.dart';
import 'plan_tier_modal.dart';

// Amber palette shared by the Pro-lock surfaces.
const Color _amber = Color(0xFFF9A825);
const Color _amberWarm = Color(0xFFFFB900);
const Color _amberDeep = Color(0xFFFF8C00);

/// Small "PRO" / "N locked" badge. Renders nothing when entitlements are off.
class ProBadge extends StatelessWidget {
  const ProBadge({super.key, required this.label, this.solid = false});

  final String label;
  final bool solid;

  @override
  Widget build(BuildContext context) {
    if (!FeatureFlags.entitlementsEnabled) return const SizedBox.shrink();
    if (solid) {
      return Container(
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
        decoration: BoxDecoration(
          gradient: const LinearGradient(
            begin: Alignment.topLeft,
            end: Alignment.bottomRight,
            colors: [_amberWarm, _amberDeep],
          ),
          borderRadius: BorderRadius.circular(6),
        ),
        child: Text(
          label.toUpperCase(),
          style: const TextStyle(
            color: Colors.white,
            fontSize: 10,
            fontWeight: FontWeight.w800,
            letterSpacing: 1.0,
          ),
        ),
      );
    }
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
      decoration: BoxDecoration(
        color: _amber.withValues(alpha: 0.12),
        borderRadius: BorderRadius.circular(6),
        border: Border.all(color: _amber.withValues(alpha: 0.30)),
      ),
      child: Text(
        label,
        style: TextStyle(
          color: _amber.withValues(alpha: 0.92),
          fontSize: 11,
          fontWeight: FontWeight.w700,
          letterSpacing: 0.1,
        ),
      ),
    );
  }
}

/// Wraps a Pro-only card so it always renders, but blocks interaction and
/// reveals an upsell tap target when [unlocked] is false. The visual
/// treatment — dimmed body, amber gloss, a floating PRO chip — is meant to
/// read as "you can see what you're missing" rather than "this is disabled".
class ProLockWrap extends StatelessWidget {
  const ProLockWrap({
    super.key,
    required this.child,
    required this.unlocked,
    required this.entitlement,
  });

  final Widget child;
  final bool unlocked;
  final Entitlement entitlement;

  @override
  Widget build(BuildContext context) {
    if (unlocked) return child;
    return _LockedCard(entitlement: entitlement, child: child);
  }
}

class _LockedCard extends StatefulWidget {
  const _LockedCard({required this.child, required this.entitlement});

  final Widget child;
  final Entitlement entitlement;

  @override
  State<_LockedCard> createState() => _LockedCardState();
}

class _LockedCardState extends State<_LockedCard>
    with SingleTickerProviderStateMixin {
  late final AnimationController _shimmer;

  @override
  void initState() {
    super.initState();
    _shimmer = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 3400),
    )..repeat();
  }

  @override
  void dispose() {
    _shimmer.dispose();
    super.dispose();
  }

  void _openUpsell() {
    HapticFeedback.lightImpact();
    PlanTierModal.show(context, highlightFeature: widget.entitlement);
  }

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: _openUpsell,
      child: ClipRRect(
        borderRadius: BorderRadius.circular(18),
        child: Stack(
          children: [
            // 1. The real card, painted but inert.
            IgnorePointer(
              ignoring: true,
              child: ColorFiltered(
                colorFilter: const ColorFilter.matrix(<double>[
                  // De-saturate ~55% so cool greens/teals don't fight the
                  // warm amber lock veil.
                  0.55, 0.35, 0.10, 0, 0,
                  0.20, 0.65, 0.15, 0, 0,
                  0.20, 0.35, 0.45, 0, 0,
                  0, 0, 0, 0.55, 0,
                ]),
                child: child(),
              ),
            ),

            // 2. Diagonal amber veil — top-right glow.
            Positioned.fill(
              child: IgnorePointer(
                child: DecoratedBox(
                  decoration: BoxDecoration(
                    gradient: LinearGradient(
                      begin: Alignment.bottomLeft,
                      end: Alignment.topRight,
                      colors: [
                        _amberWarm.withValues(alpha: 0.02),
                        _amberWarm.withValues(alpha: 0.08),
                      ],
                    ),
                  ),
                ),
              ),
            ),

            // 3. Slow shimmer sweep that signals "tap me".
            Positioned.fill(
              child: IgnorePointer(
                child: AnimatedBuilder(
                  animation: _shimmer,
                  builder: (context, _) {
                    final t = _shimmer.value;
                    return ShaderMask(
                      shaderCallback: (rect) => LinearGradient(
                        begin: const Alignment(-1.4, -1),
                        end: const Alignment(1.4, 1),
                        stops: [
                          (t - 0.20).clamp(0.0, 1.0),
                          t.clamp(0.0, 1.0),
                          (t + 0.20).clamp(0.0, 1.0),
                        ],
                        colors: [
                          Colors.white.withValues(alpha: 0.00),
                          Colors.white.withValues(alpha: 0.04),
                          Colors.white.withValues(alpha: 0.00),
                        ],
                      ).createShader(rect),
                      blendMode: BlendMode.plus,
                      child: const ColoredBox(color: Colors.transparent),
                    );
                  },
                ),
              ),
            ),

            // 4. Floating PRO chip in the top-right.
            const Positioned(
              top: 10,
              right: 12,
              child: ProBadge(label: 'Pro', solid: true),
            ),

            // 5. Bottom-center upsell hint.
            Positioned(
              left: 0,
              right: 0,
              bottom: 10,
              child: Center(
                child: Container(
                  padding:
                      const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
                  decoration: BoxDecoration(
                    color: Colors.black.withValues(alpha: 0.38),
                    borderRadius: BorderRadius.circular(999),
                    border: Border.all(
                      color: _amber.withValues(alpha: 0.35),
                    ),
                  ),
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      const Icon(
                        Icons.lock_open_rounded,
                        size: 12,
                        color: _amberWarm,
                      ),
                      const SizedBox(width: 6),
                      Text(
                        'Tap to unlock',
                        style: TextStyle(
                          color: Colors.white.withValues(alpha: 0.95),
                          fontSize: 11,
                          fontWeight: FontWeight.w700,
                          letterSpacing: 0.2,
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget child() => widget.child;
}
