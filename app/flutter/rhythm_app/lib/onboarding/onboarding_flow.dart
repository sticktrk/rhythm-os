import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../config/feature_flags.dart';
import 'screens/welcome_screen.dart';
import 'screens/location_screen.dart';
import 'screens/account_screen.dart';
import 'widgets/orbit_progress.dart';
import 'widgets/onboarding_orbit.dart';
import 'providers/onboarding_provider.dart';
import 'providers/auth_provider.dart';

/// Main onboarding flow orchestrator with PageView navigation.
class OnboardingFlow extends StatefulWidget {
  final VoidCallback onComplete;

  const OnboardingFlow({super.key, required this.onComplete});

  @override
  State<OnboardingFlow> createState() => _OnboardingFlowState();
}

class _OnboardingFlowState extends State<OnboardingFlow> {
  late PageController _pageController;

  @override
  void initState() {
    super.initState();
    _pageController = PageController();
  }

  @override
  void dispose() {
    _pageController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return MultiProvider(
      providers: [
        ChangeNotifierProvider(create: (_) => OnboardingProvider()),
        ChangeNotifierProvider(create: (_) => AuthProvider()),
      ],
      child: Consumer2<OnboardingProvider, AuthProvider>(
        builder: (context, onboardingProvider, authProvider, child) {
          // Sync page controller with provider state
          if (_pageController.hasClients &&
              _pageController.page?.round() != onboardingProvider.currentPage) {
            _pageController.animateToPage(
              onboardingProvider.currentPage,
              duration: const Duration(milliseconds: 500),
              curve: Curves.easeOutCubic,
            );
          }

          return Scaffold(
            backgroundColor: OnboardingColors.backgroundDark,
            body: Stack(
              children: [
                // Page content
                PageView(
                  controller: _pageController,
                  physics: const NeverScrollableScrollPhysics(),
                  onPageChanged: (index) {
                    // Keep provider in sync if somehow page changes
                    if (onboardingProvider.currentPage != index) {
                      onboardingProvider.goToPage(index);
                    }
                  },
                  children: [
                    const WelcomeScreen(),
                    LocationScreen(
                      onComplete: FeatureFlags.onboardingSignIn ? null : widget.onComplete,
                    ),
                    if (FeatureFlags.onboardingSignIn)
                      AccountScreen(onComplete: widget.onComplete),
                  ],
                ),
                // Progress indicator (not shown on welcome screen)
                if (onboardingProvider.currentPage > 0)
                  Positioned(
                    left: 0,
                    right: 0,
                    bottom: MediaQuery.of(context).padding.bottom + 16,
                    child: AnimatedOrbitProgress(
                      totalSteps: onboardingProvider.totalPages,
                      currentStep: onboardingProvider.currentPage,
                    ),
                  ),
                // No back button in onboarding flow
              ],
            ),
          );
        },
      ),
    );
  }
}
