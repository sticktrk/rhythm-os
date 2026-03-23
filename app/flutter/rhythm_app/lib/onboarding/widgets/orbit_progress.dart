import 'package:flutter/material.dart';
import 'onboarding_orbit.dart';

/// Progress indicator styled as orbit path with glowing dots.
class OrbitProgress extends StatelessWidget {
  final int totalSteps;
  final int currentStep;

  const OrbitProgress({
    super.key,
    required this.totalSteps,
    required this.currentStep,
  });

  @override
  Widget build(BuildContext context) {
    return Row(
      mainAxisAlignment: MainAxisAlignment.center,
      children: List.generate(totalSteps, (index) {
        final isActive = index == currentStep;
        final isCompleted = index < currentStep;

        return Padding(
          padding: const EdgeInsets.symmetric(horizontal: 4),
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 300),
            curve: Curves.easeOutCubic,
            width: isActive ? 24 : 10,
            height: 10,
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(5),
              color: isActive
                  ? OnboardingColors.sunWarm
                  : isCompleted
                      ? OnboardingColors.accentBlue.withValues(alpha: 0.8)
                      : OnboardingColors.orbitRing,
              boxShadow: isActive
                  ? [
                      BoxShadow(
                        color: OnboardingColors.sunWarm.withValues(alpha: 0.5),
                        blurRadius: 8,
                        spreadRadius: 1,
                      ),
                    ]
                  : null,
            ),
          ),
        );
      }),
    );
  }
}

/// Animated version with smoother transitions between steps.
class AnimatedOrbitProgress extends StatefulWidget {
  final int totalSteps;
  final int currentStep;

  const AnimatedOrbitProgress({
    super.key,
    required this.totalSteps,
    required this.currentStep,
  });

  @override
  State<AnimatedOrbitProgress> createState() => _AnimatedOrbitProgressState();
}

class _AnimatedOrbitProgressState extends State<AnimatedOrbitProgress>
    with SingleTickerProviderStateMixin {
  late AnimationController _pulseController;
  late Animation<double> _pulseAnimation;

  @override
  void initState() {
    super.initState();
    _pulseController = AnimationController(
      duration: const Duration(milliseconds: 1500),
      vsync: this,
    )..repeat(reverse: true);

    _pulseAnimation = Tween<double>(begin: 0.5, end: 1.0).animate(
      CurvedAnimation(parent: _pulseController, curve: Curves.easeInOut),
    );
  }

  @override
  void dispose() {
    _pulseController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Row(
      mainAxisAlignment: MainAxisAlignment.center,
      children: List.generate(widget.totalSteps, (index) {
        final isActive = index == widget.currentStep;
        final isCompleted = index < widget.currentStep;

        return Padding(
          padding: const EdgeInsets.symmetric(horizontal: 4),
          child: AnimatedBuilder(
            animation: _pulseAnimation,
            builder: (context, child) {
              return AnimatedContainer(
                duration: const Duration(milliseconds: 300),
                curve: Curves.easeOutCubic,
                width: isActive ? 24 : 10,
                height: 10,
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(5),
                  color: isActive
                      ? OnboardingColors.sunWarm
                      : isCompleted
                          ? OnboardingColors.accentBlue.withValues(alpha: 0.8)
                          : OnboardingColors.orbitRing,
                  boxShadow: isActive
                      ? [
                          BoxShadow(
                            color: OnboardingColors.sunWarm
                                .withValues(alpha: _pulseAnimation.value * 0.6),
                            blurRadius: 8,
                            spreadRadius: 1,
                          ),
                        ]
                      : null,
                ),
              );
            },
          ),
        );
      }),
    );
  }
}
