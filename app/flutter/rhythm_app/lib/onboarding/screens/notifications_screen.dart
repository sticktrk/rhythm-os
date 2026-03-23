import 'dart:math' as math;
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../widgets/onboarding_orbit.dart';
import '../widgets/sun_glow_button.dart';
import '../providers/onboarding_provider.dart';
import '../../services/analytics_service.dart';

/// Notifications permission screen.
class NotificationsScreen extends StatefulWidget {
  const NotificationsScreen({super.key});

  @override
  State<NotificationsScreen> createState() => _NotificationsScreenState();
}

class _NotificationsScreenState extends State<NotificationsScreen>
    with SingleTickerProviderStateMixin {
  late AnimationController _rayController;
  bool _isLoading = false;

  @override
  void initState() {
    super.initState();
    _rayController = AnimationController(
      duration: const Duration(milliseconds: 2000),
      vsync: this,
    )..repeat();
  }

  @override
  void dispose() {
    _rayController.dispose();
    super.dispose();
  }

  Future<void> _requestNotifications() async {
    setState(() => _isLoading = true);

    AnalyticsService().logOnboardingNotifications(allowed: true);

    // Record preference locally (notifications can be implemented with local notifications later)
    if (mounted) {
      context.read<OnboardingProvider>().setNotificationsEnabled(true);
      context.read<OnboardingProvider>().nextPage();
    }

    if (mounted) {
      setState(() => _isLoading = false);
    }
  }

  void _skipNotifications() {
    AnalyticsService().logOnboardingNotifications(allowed: false);
    context.read<OnboardingProvider>().setNotificationsEnabled(false);
    context.read<OnboardingProvider>().nextPage();
  }

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
                'Stay in Rhythm',
                style: TextStyle(
                  color: OnboardingColors.textPrimary,
                  fontSize: 28,
                  fontWeight: FontWeight.bold,
                  letterSpacing: 0.5,
                ),
              ),
              const SizedBox(height: 12),
              const Text(
                'Get gentle reminders to help train your lighting preferences',
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: OnboardingColors.textSecondary,
                  fontSize: 16,
                  height: 1.5,
                ),
              ),
              const Spacer(),
              // Bell icon with radiating sun rays
              AnimatedBuilder(
                animation: _rayController,
                builder: (context, child) {
                  return CustomPaint(
                    size: const Size(160, 160),
                    painter: _SunRaysPainter(
                      progress: _rayController.value,
                      color: OnboardingColors.sunWarm,
                    ),
                    child: Center(
                      child: Container(
                        width: 80,
                        height: 80,
                        decoration: BoxDecoration(
                          shape: BoxShape.circle,
                          color: OnboardingColors.backgroundCard,
                          border: Border.all(
                            color: OnboardingColors.sunWarm.withValues(alpha: 0.5),
                            width: 2,
                          ),
                          boxShadow: [
                            BoxShadow(
                              color: OnboardingColors.sunWarm.withValues(alpha: 0.3),
                              blurRadius: 20,
                              spreadRadius: 0,
                            ),
                          ],
                        ),
                        child: const Icon(
                          Icons.notifications_rounded,
                          color: OnboardingColors.sunWarm,
                          size: 36,
                        ),
                      ),
                    ),
                  );
                },
              ),
              const Spacer(),
              // Notification types preview
              Container(
                padding: const EdgeInsets.all(20),
                decoration: BoxDecoration(
                  color: OnboardingColors.backgroundCard,
                  borderRadius: BorderRadius.circular(16),
                  border: Border.all(
                    color: OnboardingColors.orbitRing.withValues(alpha: 0.5),
                  ),
                ),
                child: Column(
                  children: [
                    _buildNotificationItem(
                      Icons.wb_twilight,
                      'Sunset alerts',
                      'Know when evening mode begins',
                    ),
                    const SizedBox(height: 16),
                    _buildNotificationItem(
                      Icons.bedtime,
                      'Bedtime reminders',
                      'Wind down with dimming lights',
                    ),
                    const SizedBox(height: 16),
                    _buildNotificationItem(
                      Icons.tips_and_updates,
                      'Smart suggestions',
                      'Personalized lighting tips',
                    ),
                  ],
                ),
              ),
              const SizedBox(height: 32),
              // Primary button
              SunGlowButton(
                text: 'Enable Notifications',
                isLoading: _isLoading,
                onPressed: _requestNotifications,
              ),
              const SizedBox(height: 16),
              // Skip link
              TextLinkButton(
                text: 'Not Now',
                onPressed: _skipNotifications,
              ),
              const SizedBox(height: 48),
            ],
          ),
        ),
      ),
    );
  }

  Widget _buildNotificationItem(IconData icon, String title, String subtitle) {
    return Row(
      children: [
        Container(
          width: 44,
          height: 44,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: OnboardingColors.sunWarm.withValues(alpha: 0.15),
          ),
          child: Icon(
            icon,
            color: OnboardingColors.sunWarm,
            size: 22,
          ),
        ),
        const SizedBox(width: 16),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                title,
                style: const TextStyle(
                  color: OnboardingColors.textPrimary,
                  fontSize: 15,
                  fontWeight: FontWeight.w600,
                ),
              ),
              const SizedBox(height: 2),
              Text(
                subtitle,
                style: const TextStyle(
                  color: OnboardingColors.textSecondary,
                  fontSize: 13,
                ),
              ),
            ],
          ),
        ),
      ],
    );
  }
}

/// Custom painter for sun rays animation.
class _SunRaysPainter extends CustomPainter {
  final double progress;
  final Color color;

  _SunRaysPainter({required this.progress, required this.color});

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);
    final paint = Paint()
      ..color = color.withValues(alpha: 0.3)
      ..strokeWidth = 2
      ..style = PaintingStyle.stroke;

    const rayCount = 8;
    const innerRadius = 50.0;
    const outerRadius = 70.0;

    for (int i = 0; i < rayCount; i++) {
      final angle = (i / rayCount) * 2 * math.pi + progress * 2 * math.pi;
      final startX = center.dx + innerRadius * math.cos(angle);
      final startY = center.dy + innerRadius * math.sin(angle);
      final endX = center.dx + outerRadius * math.cos(angle);
      final endY = center.dy + outerRadius * math.sin(angle);

      final fadeAlpha = (0.2 + 0.3 * math.sin(progress * 2 * math.pi + i * 0.5)).clamp(0.0, 1.0);
      paint.color = color.withValues(alpha: fadeAlpha);

      canvas.drawLine(
        Offset(startX, startY),
        Offset(endX, endY),
        paint,
      );
    }
  }

  @override
  bool shouldRepaint(covariant _SunRaysPainter oldDelegate) {
    return progress != oldDelegate.progress;
  }
}
