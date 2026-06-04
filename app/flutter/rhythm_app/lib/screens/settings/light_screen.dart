import 'package:flutter/material.dart';

import '../../widgets/solar_orbit.dart' show CelestialColors;
import 'light_profile_screen.dart';

/// The "Light" tab — the *look* of each mode.
///
/// Stacks the Day and Sleep light profiles (color temperature + brightness) in
/// one scroll. A mode's look is its identity; *when* a mode engages and what the
/// lights do (on/off/standby) are automations and live on the Automations tab.
class LightScreen extends StatelessWidget {
  const LightScreen({super.key});

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _buildHeader(),
            Expanded(
              child: SingleChildScrollView(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: const [
                    SizedBox(height: 4),
                    _ModeSectionLabel(
                      icon: Icons.wb_sunny_rounded,
                      label: 'Day',
                      color: Color(0xFFFFB74D),
                    ),
                    LightProfileScreen(
                      initialProfile: 'rhythm',
                      embedded: true,
                    ),
                    _ModeSectionDivider(),
                    _ModeSectionLabel(
                      icon: Icons.bedtime_rounded,
                      label: 'Sleep',
                      color: Color(0xFF7C83FF),
                    ),
                    LightProfileScreen(
                      initialProfile: 'sleep',
                      embedded: true,
                    ),
                    SizedBox(height: 24),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildHeader() {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 16),
      alignment: Alignment.center,
      child: const Text(
        'Light',
        style: TextStyle(
          color: CelestialColors.textPrimary,
          fontSize: 18,
          fontWeight: FontWeight.w600,
          letterSpacing: 0.3,
        ),
      ),
    );
  }
}

/// Eyebrow label introducing a mode's section within the Light tab.
class _ModeSectionLabel extends StatelessWidget {
  final IconData icon;
  final String label;
  final Color color;

  const _ModeSectionLabel({
    required this.icon,
    required this.label,
    required this.color,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 16, 20, 0),
      child: Row(
        children: [
          Container(
            width: 30,
            height: 30,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: color.withValues(alpha: 0.12),
              border: Border.all(color: color.withValues(alpha: 0.22)),
            ),
            child: Icon(icon, color: color, size: 16),
          ),
          const SizedBox(width: 12),
          Text(
            label.toUpperCase(),
            style: TextStyle(
              color: color,
              fontSize: 12,
              fontWeight: FontWeight.w700,
              letterSpacing: 1.6,
            ),
          ),
        ],
      ),
    );
  }
}

/// Hairline separating the Day and Sleep sections.
class _ModeSectionDivider extends StatelessWidget {
  const _ModeSectionDivider();

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 12, 20, 0),
      child: Container(
        height: 1,
        decoration: BoxDecoration(
          gradient: LinearGradient(
            colors: [
              CelestialColors.orbitRing.withValues(alpha: 0.0),
              CelestialColors.orbitRing.withValues(alpha: 0.6),
              CelestialColors.orbitRing.withValues(alpha: 0.0),
            ],
          ),
        ),
      ),
    );
  }
}
