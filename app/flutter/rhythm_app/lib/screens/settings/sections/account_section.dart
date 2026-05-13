import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../../../config/feature_flags.dart';
import '../../../models/plan_tier.dart';
import '../../../providers/subscription_provider.dart';
import '../../../widgets/plan_tier_modal.dart';
import '../../../widgets/solar_orbit.dart'; // For CelestialColors
import '../../../widgets/settings_row.dart';
import '../../../backend/auth/auth_user.dart';
import '../dialogs/sign_in_modal.dart';

// Note: Sign out is handled via "Reset Setup" in AboutSection

/// Account section showing sign-in card or profile info.
class AccountSection extends StatelessWidget {
  final AuthUser? user;

  const AccountSection({super.key, this.user});

  @override
  Widget build(BuildContext context) {
    final subscription = context.watch<SubscriptionProvider>();
    final isSignedIn = user != null && !user!.isAnonymous;
    final isAnonymous = user != null && user!.isAnonymous;
    final rows = <Widget>[
      if (FeatureFlags.auxSignIn)
        if (isSignedIn)
          SettingsRow(
            icon: Icons.person_outline,
            iconColor: CelestialColors.accentBlue,
            label: 'Profile',
            value: user?.email ?? 'Signed in',
            showChevron: false,
          )
        else if (isAnonymous)
          SettingsRow(
            icon: Icons.person_add_outlined,
            iconColor: const Color(0xFFFFC107),
            label: 'Create Account',
            value: 'Sync layout & backup',
            onTap: () => SignInModal.show(context),
          )
        else
          SettingsRow(
            icon: Icons.cloud_outlined,
            iconColor: CelestialColors.accentBlue,
            label: 'Sign In',
            value: 'Sync layout & backup',
            onTap: () => SignInModal.show(context),
          ),
      SettingsRow(
        icon: Icons.workspace_premium_outlined,
        iconColor: subscription.isPro
            ? const Color(0xFFFFC107)
            : const Color(0xFF90A4AE),
        label: 'Plan',
        trailing: _PlanPill(tier: subscription.tier),
        onTap: () => PlanTierModal.show(context),
      ),
    ];

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const SettingsSectionHeader(title: 'Account & Plan'),
        SettingsGroup(children: rows),
      ],
    );
  }
}

class _PlanPill extends StatelessWidget {
  const _PlanPill({required this.tier});

  final PlanTier tier;

  @override
  Widget build(BuildContext context) {
    final color = tier.isPaid
        ? const Color(0xFFFFC107)
        : CelestialColors.textSecondary.withValues(alpha: 0.7);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
      decoration: BoxDecoration(
        color: color.withValues(alpha: tier.isPaid ? 0.18 : 0.12),
        borderRadius: BorderRadius.circular(999),
        border: Border.all(color: color.withValues(alpha: 0.35)),
      ),
      child: Text(
        tier.displayName,
        style: TextStyle(
          color: color,
          fontSize: 13,
          fontWeight: FontWeight.w700,
          letterSpacing: 0.1,
        ),
      ),
    );
  }
}
