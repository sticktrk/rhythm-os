import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../../config/platform_capabilities.dart';
import '../../widgets/solar_orbit.dart'; // For CelestialColors
import '../../providers/settings_provider.dart';
import '../../providers/home_provider.dart';
import '../../services/analytics_service.dart';
import '../../services/auth_service.dart';
import '../../widgets/settings_row.dart';
import 'sections/account_section.dart';
// import 'sections/sleep_section.dart'; // TODO: Re-enable when sleep schedule is implemented
import 'sections/preferences_section.dart';
import 'sections/transitions_section.dart';
import 'sections/rhythm_server_section.dart';
import 'sections/rhythm_app_section.dart';

/// Full-screen settings modal with slide-up animation.
///
/// Layout:
/// - Close button (X) in circular blue container, top-left
/// - "Settings" title centered
/// - Scrollable content with grouped card rows
class SettingsScreen extends StatelessWidget {
  const SettingsScreen({super.key});

  /// Show the settings screen as a full-screen modal.
  static Future<void> show(BuildContext context) {
    // Track screen view
    AnalyticsService().logScreenView('settings');
    return Navigator.of(context).push(
      PageRouteBuilder(
        opaque: false,
        barrierColor: Colors.black54,
        pageBuilder: (context, animation, secondaryAnimation) {
          return const SettingsScreen();
        },
        transitionsBuilder: (context, animation, secondaryAnimation, child) {
          final curve = CurvedAnimation(
            parent: animation,
            curve: Curves.easeOutCubic,
            reverseCurve: Curves.easeInCubic,
          );
          return SlideTransition(
            position: Tween<Offset>(
              begin: const Offset(0, 1),
              end: Offset.zero,
            ).animate(curve),
            child: child,
          );
        },
        transitionDuration: const Duration(milliseconds: 350),
        reverseTransitionDuration: const Duration(milliseconds: 300),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    return ChangeNotifierProxyProvider<HomeProvider, SettingsProvider>(
      create: (_) => SettingsProvider(),
      update: (_, homeProvider, settingsProvider) {
        settingsProvider?.updateFromHome(
          homeProvider.currentHome,
          homeProvider.currentHomeHubs,
        );
        return settingsProvider!;
      },
      child: Scaffold(
        backgroundColor: CelestialColors.backgroundDark,
        body: SafeArea(
          child: Column(
            children: [
              _buildHeader(context),
              Expanded(
                child: SingleChildScrollView(
                  padding: const EdgeInsets.symmetric(horizontal: 20),
                  child: Builder(
                    builder: (context) {
                      final caps = context.read<PlatformCapabilities>();
                      if (!caps.hasAccounts) {
                        return Column(
                          crossAxisAlignment: CrossAxisAlignment.stretch,
                          children: [
                            const PreferencesSection(),
                            const TransitionsSection(),
                            const SettingsSectionHeader(title: 'Settings'),
                            SettingsGroup(
                              children: [
                                SettingsRow(
                                  icon: Icons.developer_board,
                                  iconColor: const Color(0xFF00BCD4),
                                  label: 'Rhythm Server',
                                  onTap: () => RhythmServerDetailScreen.show(context),
                                ),
                                SettingsRow(
                                  icon: Icons.apps_rounded,
                                  iconColor: CelestialColors.accentBlue,
                                  label: 'Rhythm App',
                                  onTap: () => RhythmAppDetailScreen.show(context),
                                ),
                              ],
                            ),
                            const SizedBox(height: 40),
                          ],
                        );
                      }
                      return StreamBuilder(
                        stream: AuthService().authStateChanges,
                        initialData: AuthService().currentUser,
                        builder: (context, snapshot) {
                          final user = snapshot.data;
                          final isAnonymous = user != null && user.isAnonymous;

                          return Column(
                            crossAxisAlignment: CrossAxisAlignment.stretch,
                            children: [
                              // Show Account CTA at top for anonymous users
                              if (isAnonymous) AccountSection(user: user),
                              // SleepSection(), // TODO: Re-enable when sleep schedule is implemented
                              const PreferencesSection(),
                              const TransitionsSection(),
                              const SettingsSectionHeader(title: 'Settings'),
                              SettingsGroup(
                                children: [
                                  SettingsRow(
                                    icon: Icons.developer_board,
                                    iconColor: const Color(0xFF00BCD4),
                                    label: 'Rhythm Server',
                                    onTap: () => RhythmServerDetailScreen.show(context),
                                  ),
                                  SettingsRow(
                                    icon: Icons.apps_rounded,
                                    iconColor: CelestialColors.accentBlue,
                                    label: 'Rhythm App',
                                    onTap: () => RhythmAppDetailScreen.show(context),
                                  ),
                                ],
                              ),
                              // Show Account at bottom for signed-in users
                              if (!isAnonymous) AccountSection(user: user),
                              const SizedBox(height: 40),
                            ],
                          );
                        },
                      );
                    },
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _buildHeader(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      child: Row(
        children: [
          // Close button
          GestureDetector(
            onTap: () => Navigator.of(context).pop(),
            child: Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: CelestialColors.accentBlue.withValues(alpha: 0.2),
              ),
              child: const Icon(
                Icons.close,
                color: CelestialColors.accentBlue,
                size: 20,
              ),
            ),
          ),
          // Title
          const Expanded(
            child: Text(
              'Settings',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 18,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
          // Spacer to balance close button
          const SizedBox(width: 40),
        ],
      ),
    );
  }
}
