import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../widgets/onboarding_orbit.dart';
import '../widgets/sun_glow_button.dart';
import '../providers/onboarding_provider.dart';
import '../../services/analytics_service.dart';

/// Welcome screen with animated orbit and Get Started/Sign In options.
class WelcomeScreen extends StatefulWidget {
  const WelcomeScreen({super.key});

  @override
  State<WelcomeScreen> createState() => _WelcomeScreenState();
}

class _WelcomeScreenState extends State<WelcomeScreen> {
  @override
  void initState() {
    super.initState();
    AnalyticsService().logOnboardingStarted();
  }

  @override
  Widget build(BuildContext context) {
    return Container(
      decoration: BoxDecoration(
        color: OnboardingColors.backgroundDark,
        gradient: RadialGradient(
          center: Alignment.center,
          radius: 1.2,
          colors: [
            OnboardingColors.sunWarm.withValues(alpha: 0.05),
            OnboardingColors.backgroundDark,
          ],
        ),
      ),
      child: SafeArea(
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 32),
          child: Column(
            children: [
              const Spacer(flex: 2),
              // Animated orbit
              const OnboardingOrbit(size: 220),
              const SizedBox(height: 48),
              // Title with warm gradient
              ShaderMask(
                shaderCallback: (bounds) => LinearGradient(
                  colors: [
                    OnboardingColors.sunWarm,
                    OnboardingColors.sunWarm.withValues(alpha: 0.8),
                    Colors.white.withValues(alpha: 0.9),
                  ],
                  begin: Alignment.topLeft,
                  end: Alignment.bottomRight,
                ).createShader(bounds),
                child: const Text(
                  'Rhythm',
                  style: TextStyle(
                    fontSize: 48,
                    fontWeight: FontWeight.bold,
                    color: Colors.white,
                    letterSpacing: 2,
                  ),
                ),
              ),
              const SizedBox(height: 12),
              // Tagline
              const Text(
                'Light that moves with the sun',
                style: TextStyle(
                  color: OnboardingColors.textSecondary,
                  fontSize: 18,
                  fontWeight: FontWeight.w400,
                  letterSpacing: 0.5,
                ),
              ),
              const Spacer(flex: 3),
              // Get Started button
              SunGlowButton(
                text: 'Get Started',
                onPressed: () {
                  context.read<OnboardingProvider>().nextPage();
                },
              ),
              const Spacer(),
            ],
          ),
        ),
      ),
    );
  }
}
