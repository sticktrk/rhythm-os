import 'package:flutter/material.dart';

import 'solar_orbit.dart' show CelestialColors;

/// Shared loading surface used from app bootstrap through hub synchronization.
///
/// Keeping this presentation identical prevents startup, auth restoration, and
/// the first hub hello from looking like three separate loading screens.
class HubConnectionLoadingScreen extends StatelessWidget {
  final VoidCallback? onChooseHome;

  const HubConnectionLoadingScreen({
    super.key,
    this.onChooseHome,
  });

  @override
  Widget build(BuildContext context) {
    return ColoredBox(
      key: const Key('hub_connection_loading'),
      color: CelestialColors.backgroundDark,
      child: SafeArea(
        bottom: false,
        child: Stack(
          children: [
            if (onChooseHome != null)
              Positioned(
                top: 8,
                left: 12,
                child: IconButton(
                  tooltip: 'Choose Home',
                  onPressed: onChooseHome,
                  icon: Icon(
                    Icons.home_rounded,
                    color: CelestialColors.textSecondary.withValues(
                      alpha: 0.82,
                    ),
                  ),
                ),
              ),
            Center(
              child: Padding(
                padding: const EdgeInsets.symmetric(horizontal: 40),
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    const _PulsingHubIcon(),
                    const SizedBox(height: 24),
                    const Text(
                      'Setting up...',
                      style: TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 22,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                    const SizedBox(height: 12),
                    Text(
                      'Connecting to your lights',
                      textAlign: TextAlign.center,
                      style: TextStyle(
                        color: CelestialColors.textSecondary.withValues(
                          alpha: 0.7,
                        ),
                        fontSize: 15,
                        height: 1.5,
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _PulsingHubIcon extends StatefulWidget {
  const _PulsingHubIcon();

  @override
  State<_PulsingHubIcon> createState() => _PulsingHubIconState();
}

class _PulsingHubIconState extends State<_PulsingHubIcon>
    with SingleTickerProviderStateMixin {
  late final AnimationController _controller;

  @override
  void initState() {
    super.initState();
    _controller = AnimationController(
      vsync: this,
      duration: const Duration(seconds: 2),
    )..repeat(reverse: true);
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: _controller,
      builder: (context, child) {
        final opacity = 0.3 + 0.7 * _controller.value;
        return Icon(
          Icons.hub,
          size: 64,
          color: CelestialColors.accentBlue.withValues(alpha: opacity),
        );
      },
    );
  }
}
