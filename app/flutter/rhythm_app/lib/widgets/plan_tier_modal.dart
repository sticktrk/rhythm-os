import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';

import '../models/plan_tier.dart';
import '../providers/subscription_provider.dart';
import '../screens/settings/dialogs/sign_in_modal.dart';
import '../services/auth_service.dart';
import 'solar_orbit.dart' show CelestialColors;

const String _kProPriceLabel = 'Coming soon';
const String _kProPriceCadence = '';

const Color _kProAmber = Color(0xFFFFB900);
const Color _kProAmberDeep = Color(0xFFFF8C00);
const Color _kBasicBlue = Color(0xFF58A6FF);
const Color _kFreeMoon = Color(0xFF7C8EBF);

/// Visible UI tier — distinct from [PlanTier] because the wire enum collapses
/// "not signed in" and "signed-in free" into a single `basic`. The modal needs
/// to render all three columns, so we split them here.
enum _UiTier { free, basic, pro }

/// Reusable upsell modal that compares Free / Basic / Pro side-by-side.
///
/// Open from anywhere a tier-gated feature is touched:
///
/// ```dart
/// PlanTierModal.show(context, highlightFeature: Entitlement.standby);
/// ```
///
/// The user's current tier is detected automatically from
/// [SubscriptionProvider] + [AuthService] and visually emphasised. When
/// [highlightFeature] is supplied, the matching row in the tier that
/// unlocks it pulses softly so the user can see exactly what they'd gain.
class PlanTierModal extends StatefulWidget {
  const PlanTierModal({
    super.key,
    this.highlightFeature,
  });

  final Entitlement? highlightFeature;

  static Future<void> show(
    BuildContext context, {
    Entitlement? highlightFeature,
  }) {
    HapticFeedback.lightImpact();
    return showModalBottomSheet<void>(
      context: context,
      isScrollControlled: true,
      useSafeArea: true,
      backgroundColor: Colors.transparent,
      barrierColor: Colors.black.withValues(alpha: 0.72),
      builder: (_) => PlanTierModal(highlightFeature: highlightFeature),
    );
  }

  @override
  State<PlanTierModal> createState() => _PlanTierModalState();
}

class _PlanTierModalState extends State<PlanTierModal>
    with TickerProviderStateMixin {
  late final AnimationController _shimmer;
  late final AnimationController _pulse;
  late final AnimationController _stagger;

  @override
  void initState() {
    super.initState();
    _shimmer = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 3200),
    )..repeat();
    _pulse = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 1600),
    )..repeat(reverse: true);
    _stagger = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 700),
    )..forward();
  }

  @override
  void dispose() {
    _shimmer.dispose();
    _pulse.dispose();
    _stagger.dispose();
    super.dispose();
  }

  bool _mutating = false;

  _UiTier _resolveCurrentTier(SubscriptionProvider subscription) {
    if (subscription.tier == PlanTier.pro) return _UiTier.pro;
    final auth = AuthService();
    if (auth.isSignedIn && !auth.isAnonymous) return _UiTier.basic;
    return _UiTier.free;
  }

  /// Whether the user can toggle between Basic and Pro from this modal.
  /// Signed-in non-anon accounts hit the edge function; demo mode flips a
  /// local override. Signed-out users see the sign-in CTA instead.
  bool _canMutateTier(SubscriptionProvider subscription) {
    if (subscription.isDemoOverrideActive) return true;
    final auth = AuthService();
    return auth.isSignedIn && !auth.isAnonymous;
  }

  Future<void> _onTierTapped(_UiTier tier, _UiTier current) async {
    if (tier == current || _mutating) return;
    switch (tier) {
      case _UiTier.pro:
        await _setTier(PlanTier.pro);
      case _UiTier.basic:
        await _setTier(PlanTier.basic);
      case _UiTier.free:
        // "Free" maps to signed-out, not a subscription tier — no-op.
        break;
    }
  }

  Future<void> _setTier(PlanTier tier) async {
    final subscription = context.read<SubscriptionProvider>();
    HapticFeedback.mediumImpact();
    if (subscription.isDemoOverrideActive) {
      await subscription.setDemoOverride(tier);
      return;
    }
    setState(() => _mutating = true);
    try {
      await subscription.changePlan(tier);
    } catch (error) {
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          behavior: SnackBarBehavior.floating,
          backgroundColor: CelestialColors.backgroundCard,
          content: Text(
            "Couldn't update plan: $error",
            style: const TextStyle(color: CelestialColors.textPrimary),
          ),
        ),
      );
    } finally {
      if (mounted) setState(() => _mutating = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final subscription = context.watch<SubscriptionProvider>();
    final current = _resolveCurrentTier(subscription);
    final tapEnabled = _canMutateTier(subscription) && !_mutating;

    return ClipRRect(
      borderRadius: const BorderRadius.vertical(top: Radius.circular(28)),
      child: DecoratedBox(
        decoration: const BoxDecoration(
          color: CelestialColors.backgroundDark,
        ),
        child: Stack(
          children: [
            const Positioned.fill(
              child: IgnorePointer(child: _StarfieldBackdrop()),
            ),
            Positioned.fill(
              child: IgnorePointer(
                child: DecoratedBox(
                  decoration: BoxDecoration(
                    gradient: RadialGradient(
                      center: const Alignment(0, -0.85),
                      radius: 1.2,
                      colors: [
                        _kProAmber.withValues(alpha: 0.10),
                        Colors.transparent,
                      ],
                    ),
                  ),
                ),
              ),
            ),
            Column(
              children: [
                _Handle(),
                _Header(onClose: () => Navigator.of(context).pop()),
                Expanded(
                  child: SingleChildScrollView(
                    physics: const BouncingScrollPhysics(),
                    padding: const EdgeInsets.fromLTRB(20, 4, 20, 24),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        _StaggeredEntry(
                          controller: _stagger,
                          startFraction: 0.00,
                          child: _TierCard(
                            tier: _UiTier.pro,
                            isCurrent: current == _UiTier.pro,
                            highlightFeature: widget.highlightFeature,
                            pulse: _pulse,
                            shimmer: _shimmer,
                            onTap: tapEnabled
                                ? () => _onTierTapped(_UiTier.pro, current)
                                : null,
                          ),
                        ),
                        const SizedBox(height: 14),
                        _StaggeredEntry(
                          controller: _stagger,
                          startFraction: 0.18,
                          child: _TierCard(
                            tier: _UiTier.basic,
                            isCurrent: current == _UiTier.basic,
                            highlightFeature: widget.highlightFeature,
                            pulse: _pulse,
                            shimmer: _shimmer,
                            onTap: tapEnabled
                                ? () => _onTierTapped(_UiTier.basic, current)
                                : null,
                          ),
                        ),
                        const SizedBox(height: 14),
                        _StaggeredEntry(
                          controller: _stagger,
                          startFraction: 0.36,
                          child: _TierCard(
                            tier: _UiTier.free,
                            isCurrent: current == _UiTier.free,
                            highlightFeature: widget.highlightFeature,
                            pulse: _pulse,
                            shimmer: _shimmer,
                            onTap: null,
                          ),
                        ),
                        const SizedBox(height: 22),
                        _StaggeredEntry(
                          controller: _stagger,
                          startFraction: 0.55,
                          child: _Footnote(),
                        ),
                      ],
                    ),
                  ),
                ),
                _StaggeredEntry(
                  controller: _stagger,
                  startFraction: 0.45,
                  child: _StickyCta(
                    currentTier: current,
                    busy: _mutating,
                    onSubscribe: () => _setTier(PlanTier.pro),
                    onCancel: () => _setTier(PlanTier.basic),
                  ),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Header & sticky chrome
// ---------------------------------------------------------------------------

class _Handle extends StatelessWidget {
  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(top: 10, bottom: 6),
      child: Container(
        width: 40,
        height: 4,
        decoration: BoxDecoration(
          color: CelestialColors.textSecondary.withValues(alpha: 0.35),
          borderRadius: BorderRadius.circular(2),
        ),
      ),
    );
  }
}

class _Header extends StatelessWidget {
  const _Header({required this.onClose});
  final VoidCallback onClose;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 8, 12, 18),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  'RHYTHM PLANS',
                  style: TextStyle(
                    fontSize: 11,
                    fontWeight: FontWeight.w700,
                    letterSpacing: 2.4,
                    color: _kProAmber.withValues(alpha: 0.85),
                  ),
                ),
                const SizedBox(height: 8),
                const Text(
                  'Choose your light.',
                  style: TextStyle(
                    fontSize: 28,
                    height: 1.05,
                    fontWeight: FontWeight.w700,
                    color: CelestialColors.textPrimary,
                    letterSpacing: -0.4,
                  ),
                ),
                const SizedBox(height: 6),
                Text(
                  'Three tiers, one Rhythm. Move up whenever you like.',
                  style: TextStyle(
                    fontSize: 13,
                    height: 1.4,
                    color: CelestialColors.textSecondary,
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(width: 12),
          _CloseButton(onTap: onClose),
        ],
      ),
    );
  }
}

class _CloseButton extends StatelessWidget {
  const _CloseButton({required this.onTap});
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: () {
        HapticFeedback.selectionClick();
        onTap();
      },
      child: Container(
        width: 36,
        height: 36,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          color: CelestialColors.backgroundCard,
          border: Border.all(
            color: CelestialColors.orbitRing.withValues(alpha: 0.7),
          ),
        ),
        alignment: Alignment.center,
        child: Icon(
          Icons.close_rounded,
          size: 18,
          color: CelestialColors.textSecondary,
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Tier card
// ---------------------------------------------------------------------------

class _TierCard extends StatelessWidget {
  const _TierCard({
    required this.tier,
    required this.isCurrent,
    required this.highlightFeature,
    required this.pulse,
    required this.shimmer,
    this.onTap,
  });

  final _UiTier tier;
  final bool isCurrent;
  final Entitlement? highlightFeature;
  final Animation<double> pulse;
  final Animation<double> shimmer;
  final VoidCallback? onTap;

  _TierVisuals get _v => _visualsFor(tier);

  @override
  Widget build(BuildContext context) {
    final features = _featuresFor(tier);
    final accent = _v.accent;
    final isPro = tier == _UiTier.pro;

    final card = Container(
      padding: const EdgeInsets.fromLTRB(18, 18, 18, 16),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(20),
        gradient: LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: isPro
              ? [
                  const Color(0xFF1F1A12),
                  const Color(0xFF15110C),
                ]
              : [
                  CelestialColors.backgroundCard,
                  const Color(0xFF11151B),
                ],
        ),
        border: Border.all(
          color: isCurrent
              ? accent.withValues(alpha: 0.55)
              : CelestialColors.orbitRing.withValues(alpha: 0.55),
          width: isCurrent ? 1.4 : 1,
        ),
        boxShadow: [
          if (isCurrent)
            BoxShadow(
              color: accent.withValues(alpha: isPro ? 0.30 : 0.18),
              blurRadius: 28,
              spreadRadius: -2,
            )
          else if (isPro)
            BoxShadow(
              color: _kProAmber.withValues(alpha: 0.10),
              blurRadius: 20,
              spreadRadius: -6,
            ),
        ],
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _CelestialGlyph(tier: tier, pulse: pulse),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      children: [
                        Flexible(
                          child: Text(
                            _v.title,
                            style: TextStyle(
                              fontSize: 20,
                              fontWeight: FontWeight.w700,
                              color: CelestialColors.textPrimary,
                              letterSpacing: -0.3,
                            ),
                          ),
                        ),
                        if (isCurrent) ...[
                          const SizedBox(width: 8),
                          _CurrentChip(color: accent),
                        ],
                      ],
                    ),
                    const SizedBox(height: 4),
                    Text(
                      _v.tagline,
                      style: TextStyle(
                        fontSize: 12.5,
                        height: 1.35,
                        color: CelestialColors.textSecondary,
                      ),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 8),
              _PriceBadge(tier: tier),
            ],
          ),
          const SizedBox(height: 16),
          _Divider(color: accent),
          const SizedBox(height: 12),
          for (final feature in features)
            _FeatureRow(
              feature: feature,
              accent: accent,
              highlight: highlightFeature == feature.entitlement,
              pulse: pulse,
            ),
        ],
      ),
    );

    final framed = isPro
        ? AnimatedBuilder(
            animation: shimmer,
            builder: (context, child) {
              final t = shimmer.value;
              return ShaderMask(
                shaderCallback: (rect) {
                  return LinearGradient(
                    begin: const Alignment(-1.4, -1),
                    end: const Alignment(1.4, 1),
                    stops: [
                      (t - 0.18).clamp(0.0, 1.0),
                      t.clamp(0.0, 1.0),
                      (t + 0.18).clamp(0.0, 1.0),
                    ],
                    colors: [
                      Colors.white.withValues(alpha: 0.0),
                      Colors.white.withValues(alpha: 0.06),
                      Colors.white.withValues(alpha: 0.0),
                    ],
                  ).createShader(rect);
                },
                blendMode: BlendMode.plus,
                child: child!,
              );
            },
            child: card,
          )
        : card;

    if (onTap == null) return framed;

    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: onTap,
      child: framed,
    );
  }
}

class _CurrentChip extends StatelessWidget {
  const _CurrentChip({required this.color});
  final Color color;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 3),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.16),
        borderRadius: BorderRadius.circular(999),
        border: Border.all(color: color.withValues(alpha: 0.45)),
      ),
      child: Text(
        'YOU',
        style: TextStyle(
          fontSize: 9.5,
          fontWeight: FontWeight.w800,
          letterSpacing: 1.2,
          color: color,
        ),
      ),
    );
  }
}

class _PriceBadge extends StatelessWidget {
  const _PriceBadge({required this.tier});
  final _UiTier tier;

  @override
  Widget build(BuildContext context) {
    switch (tier) {
      case _UiTier.free:
        return _PriceText(primary: 'Free', secondary: 'forever');
      case _UiTier.basic:
        return _PriceText(primary: 'Free', secondary: 'with account');
      case _UiTier.pro:
        if (_kProPriceCadence.isEmpty) {
          return _PriceText(
            primary: _kProPriceLabel,
            secondary: '',
            warm: true,
          );
        }
        return _PriceText(
          primary: _kProPriceLabel,
          secondary: _kProPriceCadence,
          warm: true,
        );
    }
  }
}

class _PriceText extends StatelessWidget {
  const _PriceText({
    required this.primary,
    required this.secondary,
    this.warm = false,
  });

  final String primary;
  final String secondary;
  final bool warm;

  @override
  Widget build(BuildContext context) {
    final primaryColor = warm
        ? _kProAmber
        : CelestialColors.textPrimary.withValues(alpha: 0.92);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.end,
      children: [
        Text(
          primary,
          style: TextStyle(
            fontSize: 14,
            fontWeight: FontWeight.w700,
            color: primaryColor,
            letterSpacing: -0.1,
          ),
        ),
        if (secondary.isNotEmpty)
          Padding(
            padding: const EdgeInsets.only(top: 2),
            child: Text(
              secondary,
              style: TextStyle(
                fontSize: 10.5,
                fontWeight: FontWeight.w500,
                color: CelestialColors.textSecondary.withValues(alpha: 0.85),
                letterSpacing: 0.2,
              ),
            ),
          ),
      ],
    );
  }
}

class _Divider extends StatelessWidget {
  const _Divider({required this.color});
  final Color color;

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      height: 1,
      child: DecoratedBox(
        decoration: BoxDecoration(
          gradient: LinearGradient(
            colors: [
              color.withValues(alpha: 0),
              color.withValues(alpha: 0.35),
              color.withValues(alpha: 0),
            ],
          ),
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Feature rows
// ---------------------------------------------------------------------------

class _FeatureRow extends StatelessWidget {
  const _FeatureRow({
    required this.feature,
    required this.accent,
    required this.highlight,
    required this.pulse,
  });

  final _Feature feature;
  final Color accent;
  final bool highlight;
  final Animation<double> pulse;

  @override
  Widget build(BuildContext context) {
    final body = Padding(
      padding: const EdgeInsets.symmetric(vertical: 7),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.center,
        children: [
          _StatusGlyph(status: feature.status, accent: accent),
          const SizedBox(width: 12),
          Expanded(
            child: Text(
              feature.label,
              style: TextStyle(
                fontSize: 13.5,
                height: 1.3,
                fontWeight: FontWeight.w500,
                color: feature.status == _FeatureStatus.absent
                    ? CelestialColors.textSecondary.withValues(alpha: 0.55)
                    : CelestialColors.textPrimary.withValues(alpha: 0.94),
              ),
            ),
          ),
          if (feature.status == _FeatureStatus.comingSoon)
            _MiniTag(label: 'Soon', color: CelestialColors.textSecondary),
        ],
      ),
    );

    if (!highlight) return body;

    return AnimatedBuilder(
      animation: pulse,
      builder: (context, child) {
        final t = Curves.easeInOut.transform(pulse.value);
        return Container(
          margin: const EdgeInsets.symmetric(vertical: 1),
          decoration: BoxDecoration(
            color: accent.withValues(alpha: 0.06 + 0.10 * t),
            borderRadius: BorderRadius.circular(10),
            border: Border.all(
              color: accent.withValues(alpha: 0.25 + 0.30 * t),
            ),
          ),
          padding: const EdgeInsets.symmetric(horizontal: 8),
          child: child,
        );
      },
      child: body,
    );
  }
}

class _StatusGlyph extends StatelessWidget {
  const _StatusGlyph({required this.status, required this.accent});
  final _FeatureStatus status;
  final Color accent;

  @override
  Widget build(BuildContext context) {
    switch (status) {
      case _FeatureStatus.included:
        return Container(
          width: 18,
          height: 18,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: accent.withValues(alpha: 0.18),
            border: Border.all(color: accent.withValues(alpha: 0.6)),
          ),
          alignment: Alignment.center,
          child: Icon(Icons.check_rounded, size: 12, color: accent),
        );
      case _FeatureStatus.comingSoon:
        return Container(
          width: 18,
          height: 18,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: accent.withValues(alpha: 0.10),
            border: Border.all(color: accent.withValues(alpha: 0.35)),
          ),
          alignment: Alignment.center,
          child: Icon(Icons.schedule_rounded,
              size: 11, color: accent.withValues(alpha: 0.85)),
        );
      case _FeatureStatus.absent:
        return Container(
          width: 18,
          height: 18,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: Colors.transparent,
            border: Border.all(
              color: CelestialColors.orbitRing.withValues(alpha: 0.7),
            ),
          ),
          alignment: Alignment.center,
          child: Container(
            width: 6,
            height: 1.2,
            color: CelestialColors.textSecondary.withValues(alpha: 0.5),
          ),
        );
    }
  }
}

class _MiniTag extends StatelessWidget {
  const _MiniTag({required this.label, required this.color});
  final String label;
  final Color color;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.10),
        borderRadius: BorderRadius.circular(4),
      ),
      child: Text(
        label,
        style: TextStyle(
          fontSize: 9.5,
          fontWeight: FontWeight.w700,
          letterSpacing: 0.6,
          color: color.withValues(alpha: 0.9),
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Sticky CTA
// ---------------------------------------------------------------------------

class _StickyCta extends StatelessWidget {
  const _StickyCta({
    required this.currentTier,
    required this.busy,
    required this.onSubscribe,
    required this.onCancel,
  });

  final _UiTier currentTier;
  final bool busy;
  final VoidCallback onSubscribe;
  final VoidCallback onCancel;

  @override
  Widget build(BuildContext context) {
    return Container(
      decoration: BoxDecoration(
        color: CelestialColors.backgroundDark,
        border: Border(
          top: BorderSide(
            color: CelestialColors.orbitRing.withValues(alpha: 0.5),
          ),
        ),
      ),
      padding: EdgeInsets.fromLTRB(
        20,
        14,
        20,
        14 + MediaQuery.of(context).padding.bottom * 0.5,
      ),
      child: switch (currentTier) {
        _UiTier.free => _PrimaryButton(
            label: 'Create your free account',
            sublabel: 'Unlock cloud backup & sync',
            gradient: const [_kBasicBlue, Color(0xFF3B7DD8)],
            icon: Icons.cloud_outlined,
            busy: false,
            onTap: () {
              Navigator.of(context).pop();
              SignInModal.show(context);
            },
          ),
        _UiTier.basic => _PrimaryButton(
            label: 'Subscribe to Pro',
            sublabel: 'Standby, sleep & advanced controls',
            gradient: const [_kProAmber, _kProAmberDeep],
            icon: Icons.auto_awesome_rounded,
            busy: busy,
            onTap: busy ? null : onSubscribe,
          ),
        _UiTier.pro => _GhostButton(
            label: busy ? 'Updating…' : "You're on Pro — tap to cancel",
            onTap: busy ? null : onCancel,
          ),
      },
    );
  }
}

class _PrimaryButton extends StatelessWidget {
  const _PrimaryButton({
    required this.label,
    required this.sublabel,
    required this.gradient,
    required this.icon,
    required this.onTap,
    this.busy = false,
  });

  final String label;
  final String sublabel;
  final List<Color> gradient;
  final IconData icon;
  final VoidCallback? onTap;
  final bool busy;

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onTap,
      child: Opacity(
        opacity: onTap == null && !busy ? 0.55 : 1,
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 14),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(16),
            gradient: LinearGradient(
              begin: Alignment.topLeft,
              end: Alignment.bottomRight,
              colors: gradient,
            ),
            boxShadow: [
              BoxShadow(
                color: gradient.first.withValues(alpha: 0.35),
                blurRadius: 18,
                offset: const Offset(0, 8),
              ),
            ],
          ),
          child: Row(
            children: [
              Container(
                width: 36,
                height: 36,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: Colors.white.withValues(alpha: 0.18),
                ),
                alignment: Alignment.center,
                child: busy
                    ? const SizedBox(
                        width: 16,
                        height: 16,
                        child: CircularProgressIndicator(
                          strokeWidth: 2,
                          valueColor:
                              AlwaysStoppedAnimation<Color>(Colors.white),
                        ),
                      )
                    : Icon(icon, color: Colors.white, size: 18),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      busy ? 'Updating…' : label,
                      style: const TextStyle(
                        color: Colors.white,
                        fontSize: 15,
                        fontWeight: FontWeight.w700,
                        letterSpacing: -0.2,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      sublabel,
                      style: TextStyle(
                        color: Colors.white.withValues(alpha: 0.85),
                        fontSize: 12,
                        height: 1.3,
                      ),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 8),
              if (!busy)
                Icon(Icons.arrow_forward_rounded,
                    color: Colors.white.withValues(alpha: 0.95)),
            ],
          ),
        ),
      ),
    );
  }
}

class _GhostButton extends StatelessWidget {
  const _GhostButton({required this.label, required this.onTap});
  final String label;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onTap,
      child: Container(
        padding: const EdgeInsets.symmetric(vertical: 14),
        alignment: Alignment.center,
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(16),
          border: Border.all(
            color: _kProAmber.withValues(alpha: 0.45),
          ),
          color: _kProAmber.withValues(alpha: 0.06),
        ),
        child: Text(
          label,
          style: TextStyle(
            color: _kProAmber,
            fontSize: 14,
            fontWeight: FontWeight.w700,
            letterSpacing: 0.1,
          ),
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Stagger helper
// ---------------------------------------------------------------------------

class _StaggeredEntry extends StatelessWidget {
  const _StaggeredEntry({
    required this.controller,
    required this.startFraction,
    required this.child,
  });

  final AnimationController controller;
  final double startFraction;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final anim = CurvedAnimation(
      parent: controller,
      curve: Interval(startFraction, 1.0, curve: Curves.easeOutCubic),
    );
    return AnimatedBuilder(
      animation: anim,
      builder: (context, _) {
        final t = anim.value;
        return Opacity(
          opacity: t,
          child: Transform.translate(
            offset: Offset(0, (1 - t) * 18),
            child: child,
          ),
        );
      },
    );
  }
}

class _Footnote extends StatelessWidget {
  @override
  Widget build(BuildContext context) {
    return Center(
      child: Text(
        'Plans can change. Cancel anytime.',
        style: TextStyle(
          fontSize: 11.5,
          color: CelestialColors.textSecondary.withValues(alpha: 0.65),
          letterSpacing: 0.2,
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Celestial glyphs (moon / star / sun)
// ---------------------------------------------------------------------------

class _CelestialGlyph extends StatelessWidget {
  const _CelestialGlyph({required this.tier, required this.pulse});
  final _UiTier tier;
  final Animation<double> pulse;

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: 48,
      height: 48,
      child: AnimatedBuilder(
        animation: pulse,
        builder: (context, _) {
          return CustomPaint(
            painter: _GlyphPainter(
              tier: tier,
              pulse: Curves.easeInOut.transform(pulse.value),
            ),
          );
        },
      ),
    );
  }
}

class _GlyphPainter extends CustomPainter {
  _GlyphPainter({required this.tier, required this.pulse});
  final _UiTier tier;
  final double pulse;

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);
    switch (tier) {
      case _UiTier.free:
        _paintMoon(canvas, center, size);
      case _UiTier.basic:
        _paintStar(canvas, center, size);
      case _UiTier.pro:
        _paintSun(canvas, center, size);
    }
  }

  void _paintMoon(Canvas canvas, Offset c, Size s) {
    final r = s.width * 0.34;
    final disk = Paint()
      ..shader = RadialGradient(
        colors: [
          _kFreeMoon.withValues(alpha: 0.35),
          _kFreeMoon.withValues(alpha: 0.08),
        ],
      ).createShader(Rect.fromCircle(center: c, radius: r));
    canvas.drawCircle(c, r, disk);
    final ring = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.1
      ..color = _kFreeMoon.withValues(alpha: 0.55);
    canvas.drawCircle(c, r, ring);
    final crescent = Paint()
      ..color = CelestialColors.backgroundCard;
    canvas.drawCircle(c.translate(r * 0.32, -r * 0.05), r * 0.85, crescent);
  }

  void _paintStar(Canvas canvas, Offset c, Size s) {
    final r = s.width * 0.30;
    final glow = Paint()
      ..shader = RadialGradient(
        colors: [
          _kBasicBlue.withValues(alpha: 0.45),
          _kBasicBlue.withValues(alpha: 0.0),
        ],
      ).createShader(Rect.fromCircle(center: c, radius: r * 1.6));
    canvas.drawCircle(c, r * 1.6, glow);

    final path = Path();
    const points = 4;
    for (int i = 0; i < points * 2; i++) {
      final isOuter = i.isEven;
      final radius = isOuter ? r : r * 0.34;
      final angle = -math.pi / 2 + i * math.pi / points;
      final p = Offset(
        c.dx + radius * math.cos(angle),
        c.dy + radius * math.sin(angle),
      );
      if (i == 0) {
        path.moveTo(p.dx, p.dy);
      } else {
        path.lineTo(p.dx, p.dy);
      }
    }
    path.close();
    final fill = Paint()
      ..shader = LinearGradient(
        begin: Alignment.topLeft,
        end: Alignment.bottomRight,
        colors: [_kBasicBlue, const Color(0xFF3B7DD8)],
      ).createShader(Rect.fromCircle(center: c, radius: r));
    canvas.drawPath(path, fill);
  }

  void _paintSun(Canvas canvas, Offset c, Size s) {
    final coreR = s.width * 0.22;
    final haloR = s.width * 0.46;

    final halo = Paint()
      ..shader = RadialGradient(
        colors: [
          _kProAmber.withValues(alpha: 0.55 + 0.18 * pulse),
          _kProAmber.withValues(alpha: 0.0),
        ],
      ).createShader(Rect.fromCircle(center: c, radius: haloR));
    canvas.drawCircle(c, haloR, halo);

    final rayPaint = Paint()
      ..color = _kProAmber.withValues(alpha: 0.85)
      ..strokeWidth = 1.4
      ..strokeCap = StrokeCap.round;
    const rays = 8;
    for (int i = 0; i < rays; i++) {
      final angle = i * (2 * math.pi / rays) + pulse * 0.08;
      final inner = coreR * 1.35;
      final outer = coreR * 1.85 + pulse * 1.4;
      final p1 = Offset(
        c.dx + inner * math.cos(angle),
        c.dy + inner * math.sin(angle),
      );
      final p2 = Offset(
        c.dx + outer * math.cos(angle),
        c.dy + outer * math.sin(angle),
      );
      canvas.drawLine(p1, p2, rayPaint);
    }

    final core = Paint()
      ..shader = RadialGradient(
        colors: [
          const Color(0xFFFFE08A),
          _kProAmber,
          _kProAmberDeep,
        ],
      ).createShader(Rect.fromCircle(center: c, radius: coreR));
    canvas.drawCircle(c, coreR, core);
  }

  @override
  bool shouldRepaint(covariant _GlyphPainter oldDelegate) =>
      oldDelegate.pulse != pulse || oldDelegate.tier != tier;
}

// ---------------------------------------------------------------------------
// Starfield backdrop
// ---------------------------------------------------------------------------

class _StarfieldBackdrop extends StatelessWidget {
  const _StarfieldBackdrop();

  @override
  Widget build(BuildContext context) {
    return CustomPaint(painter: _StarfieldPainter());
  }
}

class _StarfieldPainter extends CustomPainter {
  static final List<_Star> _stars = _generate();

  static List<_Star> _generate() {
    final rng = math.Random(7);
    return List.generate(60, (_) {
      return _Star(
        Offset(rng.nextDouble(), rng.nextDouble()),
        0.3 + rng.nextDouble() * 1.2,
        0.10 + rng.nextDouble() * 0.5,
      );
    });
  }

  @override
  void paint(Canvas canvas, Size size) {
    final paint = Paint();
    for (final star in _stars) {
      paint.color = Colors.white.withValues(alpha: star.alpha);
      canvas.drawCircle(
        Offset(star.pos.dx * size.width, star.pos.dy * size.height),
        star.r,
        paint,
      );
    }
  }

  @override
  bool shouldRepaint(covariant CustomPainter oldDelegate) => false;
}

class _Star {
  _Star(this.pos, this.r, this.alpha);
  final Offset pos;
  final double r;
  final double alpha;
}

// ---------------------------------------------------------------------------
// Tier content
// ---------------------------------------------------------------------------

class _TierVisuals {
  const _TierVisuals({
    required this.title,
    required this.tagline,
    required this.accent,
  });
  final String title;
  final String tagline;
  final Color accent;
}

_TierVisuals _visualsFor(_UiTier tier) {
  switch (tier) {
    case _UiTier.free:
      return const _TierVisuals(
        title: 'Free',
        tagline: 'Adaptive lighting, on this device.',
        accent: _kFreeMoon,
      );
    case _UiTier.basic:
      return const _TierVisuals(
        title: 'Basic',
        tagline: 'Sign in to keep your setup in the cloud.',
        accent: _kBasicBlue,
      );
    case _UiTier.pro:
      return const _TierVisuals(
        title: 'Pro',
        tagline: 'Every dial, every mode, fully tunable.',
        accent: _kProAmber,
      );
  }
}

enum _FeatureStatus { absent, included, comingSoon }

class _Feature {
  const _Feature({
    required this.label,
    required this.status,
    this.entitlement,
  });

  final String label;
  final _FeatureStatus status;
  final Entitlement? entitlement;
}

List<_Feature> _featuresFor(_UiTier tier) {
  switch (tier) {
    case _UiTier.free:
      return const [
        _Feature(label: 'Adaptive lighting on this device',
            status: _FeatureStatus.included),
        _Feature(label: 'All Rooms control',
            status: _FeatureStatus.included),
        _Feature(label: 'Manual scenes & light profiles',
            status: _FeatureStatus.included),
      ];
    case _UiTier.basic:
      return const [
        _Feature(
          label: 'Cloud backup & restore',
          status: _FeatureStatus.included,
          entitlement: Entitlement.cloudBackupRestore,
        ),
        _Feature(
          label: 'All Rooms sync across devices',
          status: _FeatureStatus.included,
          entitlement: Entitlement.multiDeviceSync,
        ),
        _Feature(
          label: 'Multi-user access',
          status: _FeatureStatus.comingSoon,
          entitlement: Entitlement.multiUserAccess,
        ),
      ];
    case _UiTier.pro:
      return const [
        _Feature(
          label: 'Standby & sleep idle scenes',
          status: _FeatureStatus.included,
          entitlement: Entitlement.standby,
        ),
        _Feature(
          label: 'Custom motion timeout, fade & interval',
          status: _FeatureStatus.included,
          entitlement: Entitlement.advancedDayControls,
        ),
        _Feature(
          label: 'Sleep brightness & color overrides',
          status: _FeatureStatus.included,
          entitlement: Entitlement.sleepPrimarySettings,
        ),
        _Feature(
          label: 'Physical button trigger for Day ⇄ Sleep',
          status: _FeatureStatus.included,
          entitlement: Entitlement.transitionButton,
        ),
        _Feature(
          label: 'Time simulator — preview any hour',
          status: _FeatureStatus.included,
          entitlement: Entitlement.timeSimulator,
        ),
        _Feature(
          label: 'Remote access from anywhere',
          status: _FeatureStatus.comingSoon,
          entitlement: Entitlement.remoteAccess,
        ),
      ];
  }
}
