import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../../widgets/solar_orbit.dart';
import '../../providers/server_sync_provider.dart';
import '../../services/server_http_client.dart';

/// Simple info screen for the HA addon's Home Assistant connection.
class HaInfoScreen extends StatelessWidget {
  const HaInfoScreen({super.key});

  /// Show as a full-screen modal with slide-up transition.
  static Future<void> show(BuildContext context) {
    return Navigator.of(context).push(
      PageRouteBuilder(
        opaque: false,
        barrierColor: Colors.black54,
        pageBuilder: (context, animation, secondaryAnimation) {
          return const HaInfoScreen();
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
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            // ── Top bar ──
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 8),
              child: Row(
                children: [
                  IconButton(
                    icon: const Icon(Icons.close, color: CelestialColors.textPrimary),
                    onPressed: () => Navigator.of(context).pop(),
                  ),
                  const Spacer(),
                ],
              ),
            ),
            Expanded(
              child: SingleChildScrollView(
                padding: const EdgeInsets.symmetric(horizontal: 24),
                child: Column(
                  children: [
                    const SizedBox(height: 40),
                    // ── Hero icon ──
                    Container(
                      width: 80,
                      height: 80,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        color: const Color(0xFF03A9F4).withValues(alpha: 0.15),
                      ),
                      child: const Icon(
                        Icons.home,
                        size: 40,
                        color: Color(0xFF03A9F4),
                      ),
                    ),
                    const SizedBox(height: 20),
                    const Text(
                      'Home Assistant',
                      style: TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 24,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                    const SizedBox(height: 8),
                    // ── Connection status ──
                    Consumer<ServerSyncProvider>(
                      builder: (context, serverSync, _) {
                        final isConnected =
                            serverSync.connectionState == ServerConnectionState.connected;
                        return Row(
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            Container(
                              width: 8,
                              height: 8,
                              decoration: BoxDecoration(
                                shape: BoxShape.circle,
                                color: isConnected
                                    ? const Color(0xFF22C55E)
                                    : Colors.red.shade400,
                              ),
                            ),
                            const SizedBox(width: 6),
                            Text(
                              isConnected ? 'Connected via Add-on' : 'Disconnected',
                              style: TextStyle(
                                color: isConnected
                                    ? const Color(0xFF22C55E)
                                    : Colors.red.shade400,
                                fontSize: 14,
                                fontWeight: FontWeight.w500,
                              ),
                            ),
                          ],
                        );
                      },
                    ),
                    const SizedBox(height: 32),
                    // ── Info card ──
                    Container(
                      width: double.infinity,
                      padding: const EdgeInsets.all(20),
                      decoration: BoxDecoration(
                        color: CelestialColors.backgroundCard,
                        borderRadius: BorderRadius.circular(14),
                      ),
                      child: const Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(
                            'About',
                            style: TextStyle(
                              color: CelestialColors.textPrimary,
                              fontSize: 16,
                              fontWeight: FontWeight.w600,
                            ),
                          ),
                          SizedBox(height: 10),
                          Text(
                            'Home Assistant manages your lights and rooms through '
                            'the Rhythm Lighting add-on. Rooms and devices are '
                            'automatically imported from your Home Assistant areas.',
                            style: TextStyle(
                              color: CelestialColors.textSecondary,
                              fontSize: 14,
                              height: 1.5,
                            ),
                          ),
                        ],
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
