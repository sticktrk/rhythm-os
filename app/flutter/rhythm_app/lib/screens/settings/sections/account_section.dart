import 'package:flutter/material.dart';
import '../../../config/feature_flags.dart';
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
    // Hide entire section when login is disabled
    if (!FeatureFlags.loginEnabled) {
      return const SizedBox.shrink();
    }

    final isSignedIn = user != null && !user!.isAnonymous;
    final isAnonymous = user != null && user!.isAnonymous;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const SettingsSectionHeader(title: 'Account'),
        if (isSignedIn)
          SettingsGroup(
            children: [
              SettingsRow(
                icon: Icons.person_outline,
                iconColor: CelestialColors.accentBlue,
                label: 'Profile',
                value: user?.email ?? 'Signed in',
                showChevron: false,
              ),
            ],
          )
        else if (isAnonymous)
          _buildUpgradeAccountCard(context)
        else
          _buildSignInCard(context),
      ],
    );
  }

  Widget _buildSignInCard(BuildContext context) {
    return SettingsGroup(
      children: [
        SettingsRow(
          icon: Icons.cloud_outlined,
          iconColor: CelestialColors.accentBlue,
          label: 'Sign In',
          value: 'Sync settings',
          onTap: () => SignInModal.show(context),
        ),
      ],
    );
  }

  Widget _buildUpgradeAccountCard(BuildContext context) {
    return GestureDetector(
      onTap: () => SignInModal.show(context),
      child: Container(
        padding: const EdgeInsets.all(16),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(14),
          gradient: LinearGradient(
            begin: Alignment.topLeft,
            end: Alignment.bottomRight,
            colors: [
              const Color(0xFFFFC107).withValues(alpha: 0.2),
              CelestialColors.backgroundCard,
            ],
          ),
        ),
        child: Row(
          children: [
            // Solid amber icon
            Container(
              width: 44,
              height: 44,
              decoration: const BoxDecoration(
                shape: BoxShape.circle,
                color: Color(0xFFFFC107),
              ),
              child: const Icon(
                Icons.person_add_outlined,
                color: Color(0xFF1A1A1A),
                size: 22,
              ),
            ),
            const SizedBox(width: 14),
            // Text content
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  const Text(
                    'Create Account',
                    style: TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 17,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    'Sync settings across devices',
                    style: TextStyle(
                      color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                      fontSize: 14,
                    ),
                  ),
                ],
              ),
            ),
            Icon(
              Icons.chevron_right,
              color: CelestialColors.textSecondary.withValues(alpha: 0.5),
              size: 22,
            ),
          ],
        ),
      ),
    );
  }
}
