import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../../../config/feature_flags.dart';
import '../../../widgets/solar_orbit.dart'; // For CelestialColors
import '../../../onboarding/screens/account_screen.dart';
import '../../../onboarding/providers/onboarding_provider.dart';
import '../../../onboarding/providers/auth_provider.dart' as onboarding;
import '../../../onboarding/widgets/onboarding_orbit.dart';
import '../../../services/app_state_refresh.dart';
import '../../../services/analytics_service.dart';
import '../../../widgets/connect_hub_screen.dart';

/// Full-screen sign-in modal.
class SignInModal {
  /// Show the sign-in modal as a full-screen slide-up sheet.
  static void show(BuildContext context) {
    // Guard: don't show modal when auxiliary sign-in is disabled
    if (!FeatureFlags.auxSignIn) return;
    AnalyticsService().logScreenView('sign_in');
    final hostContext = context;

    Navigator.of(hostContext).push(
      PageRouteBuilder(
        opaque: false,
        barrierColor: Colors.black54,
        pageBuilder: (context, animation, secondaryAnimation) {
          return MultiProvider(
            providers: [
              ChangeNotifierProvider(create: (_) => OnboardingProvider()),
              ChangeNotifierProvider(create: (_) => onboarding.AuthProvider()),
            ],
            child: Builder(
              builder: (context) {
                return Scaffold(
                  backgroundColor: OnboardingColors.backgroundDark,
                  body: SafeArea(
                    child: Column(
                      children: [
                        // Modal header with close button
                        Container(
                          padding: const EdgeInsets.symmetric(
                              horizontal: 16, vertical: 12),
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
                                    color: CelestialColors.accentBlue
                                        .withValues(alpha: 0.15),
                                    border: Border.all(
                                      color: CelestialColors.accentBlue
                                          .withValues(alpha: 0.3),
                                      width: 1,
                                    ),
                                  ),
                                  child: const Icon(
                                    Icons.close,
                                    color: CelestialColors.accentBlue,
                                    size: 20,
                                  ),
                                ),
                              ),
                              const Expanded(
                                child: Text(
                                  'Sign In',
                                  textAlign: TextAlign.center,
                                  style: TextStyle(
                                    color: CelestialColors.textPrimary,
                                    fontSize: 18,
                                    fontWeight: FontWeight.w600,
                                    letterSpacing: 0.3,
                                  ),
                                ),
                              ),
                              // Spacer to balance close button
                              const SizedBox(width: 40),
                            ],
                          ),
                        ),
                        // Account screen content
                        Expanded(
                          child: AccountScreen(
                            onComplete: () => _finishSignIn(hostContext,
                                openHomeChooser: false),
                            onSignedInComplete: () => _finishSignIn(hostContext,
                                openHomeChooser: true),
                          ),
                        ),
                      ],
                    ),
                  ),
                );
              },
            ),
          );
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

  static Future<void> _finishSignIn(
    BuildContext context, {
    required bool openHomeChooser,
  }) async {
    if (!context.mounted) return;

    await AppStateRefresh.sync(context);
    if (!context.mounted) return;

    Navigator.of(context).popUntil((route) => route.isFirst);

    if (!openHomeChooser) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!context.mounted) return;
      ConnectHubScreen.show(context, mode: ConnectHubMode.rhythmServer);
    });
  }
}
