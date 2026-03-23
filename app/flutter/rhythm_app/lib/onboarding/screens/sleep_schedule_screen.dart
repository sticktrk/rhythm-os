import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../widgets/onboarding_orbit.dart';
import '../widgets/sun_glow_button.dart';
import '../widgets/time_wheel_picker.dart';
import '../providers/onboarding_provider.dart';
import '../../services/analytics_service.dart';

/// Sleep schedule screen with bedtime and wake time pickers.
class SleepScheduleScreen extends StatelessWidget {
  const SleepScheduleScreen({super.key});

  @override
  Widget build(BuildContext context) {
    return Container(
      color: OnboardingColors.backgroundDark,
      child: SafeArea(
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 32),
          child: Column(
            children: [
              const SizedBox(height: 48),
              // Header
              const Text(
                'Your Sleep Schedule',
                style: TextStyle(
                  color: OnboardingColors.textPrimary,
                  fontSize: 28,
                  fontWeight: FontWeight.bold,
                  letterSpacing: 0.5,
                ),
              ),
              const SizedBox(height: 12),
              const Text(
                'Rhythm adjusts lighting to match your natural sleep cycle',
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: OnboardingColors.textSecondary,
                  fontSize: 16,
                  height: 1.5,
                ),
              ),
              const SizedBox(height: 24),
              // Orbit visualization with bed/wake markers
              Consumer<OnboardingProvider>(
                builder: (context, provider, child) {
                  final prefs = provider.preferences;
                  return OnboardingOrbit(
                    size: 220,
                    showTimeMarkers: true,
                    bedtimeHour: prefs.bedtimeHour,
                    bedtimeMinute: prefs.bedtimeMinute,
                    wakeTimeHour: prefs.wakeTimeHour,
                    wakeTimeMinute: prefs.wakeTimeMinute,
                  );
                },
              ),
              const SizedBox(height: 32),
              // Time pickers
              Consumer<OnboardingProvider>(
                builder: (context, provider, child) {
                  final prefs = provider.preferences;
                  return Row(
                    mainAxisAlignment: MainAxisAlignment.spaceEvenly,
                    children: [
                      // Bedtime picker (moon icon)
                      TimeWheelPicker(
                        label: 'Bedtime',
                        icon: Icons.nightlight_round,
                        iconColor: OnboardingColors.moonGlow,
                        hour: prefs.bedtimeHour,
                        minute: prefs.bedtimeMinute,
                        onTimeChanged: (time) {
                          provider.setBedtime(time.$1, time.$2);
                        },
                      ),
                      // Wake time picker (sun icon)
                      TimeWheelPicker(
                        label: 'Wake Time',
                        icon: Icons.wb_sunny_rounded,
                        iconColor: OnboardingColors.sunWarm,
                        hour: prefs.wakeTimeHour,
                        minute: prefs.wakeTimeMinute,
                        onTimeChanged: (time) {
                          provider.setWakeTime(time.$1, time.$2);
                        },
                      ),
                    ],
                  );
                },
              ),
              const SizedBox(height: 48),
              // Continue button
              SunGlowButton(
                text: 'Continue',
                onPressed: () {
                  final prefs = context.read<OnboardingProvider>().preferences;
                  final wake = '${prefs.wakeTimeHour.toString().padLeft(2, '0')}:${prefs.wakeTimeMinute.toString().padLeft(2, '0')}';
                  final sleep = '${prefs.bedtimeHour.toString().padLeft(2, '0')}:${prefs.bedtimeMinute.toString().padLeft(2, '0')}';
                  AnalyticsService().logOnboardingSleepSchedule(wake, sleep);
                  context.read<OnboardingProvider>().nextPage();
                },
              ),
              const SizedBox(height: 48),
            ],
          ),
        ),
      ),
    );
  }
}
