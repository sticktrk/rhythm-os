import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';

import '../providers/server_sync_provider.dart';
import '../screens/hubs/ha_configurator_screen.dart';
import '../screens/hubs/hue_configurator_screen.dart';
import '../screens/hubs/matter_add_method.dart';
import '../screens/hubs/matter_pairing_flow.dart';
import 'beta_badge.dart';
import 'solar_orbit.dart';
import 'bottom_nav_overlay.dart';

/// Shown when the server is connected but has no hub paired (hub.type == "none").
///
/// Offers Home Assistant, Hue, and Matter pairing options.
class HubPickerScreen extends StatefulWidget {
  final VoidCallback onSettingsTap;
  final VoidCallback onSunPositionTap;
  final VoidCallback? onAddDevicesTap;
  final VoidCallback? onReportBugTap;

  const HubPickerScreen({
    super.key,
    required this.onSettingsTap,
    required this.onSunPositionTap,
    this.onAddDevicesTap,
    this.onReportBugTap,
  });

  @override
  State<HubPickerScreen> createState() => _HubPickerScreenState();
}

class _HubPickerScreenState extends State<HubPickerScreen>
    with SingleTickerProviderStateMixin {
  bool _isConfiguringHa = false;

  late AnimationController _breatheController;
  late Animation<double> _breathe;

  @override
  void initState() {
    super.initState();
    _breatheController = AnimationController(
      duration: const Duration(milliseconds: 3000),
      vsync: this,
    )..repeat(reverse: true);
    _breathe = CurvedAnimation(
      parent: _breatheController,
      curve: Curves.easeInOut,
    );
  }

  @override
  void dispose() {
    _breatheController.dispose();
    super.dispose();
  }

  Future<void> _configureHa() async {
    HapticFeedback.mediumImpact();
    setState(() => _isConfiguringHa = true);
    try {
      final serverSync = context.read<ServerSyncProvider>();
      await serverSync.configureAddonHaHub();
      // Server will reconnect and send hello with rooms — UI transitions automatically.
    } catch (e) {
      debugPrint('HubPicker: HA configure failed: $e');
    } finally {
      if (mounted) setState(() => _isConfiguringHa = false);
    }
  }

  Future<void> _configureHaManual() async {
    HapticFeedback.mediumImpact();
    final result = await HAConfiguratorScreen.show(context);
    if (result == true && mounted) {
      final serverSync = context.read<ServerSyncProvider>();
      await serverSync.pushHubCredentials(RoomSourceDto.homeAssistant);
    }
  }

  Future<void> _configureHue() async {
    HapticFeedback.mediumImpact();
    final result = await HueConfiguratorScreen.show(context);
    if (result == true && mounted) {
      final serverSync = context.read<ServerSyncProvider>();
      await serverSync.pushHubCredentials(RoomSourceDto.hue);
    }
  }

  Future<void> _configureMatter() async {
    HapticFeedback.mediumImpact();
    await startMatterPairingFlow(
      context,
      preferredMethod: MatterAddMethod.automatic,
    );
  }

  String _matterSubtitle(ServerSyncProvider serverSync) {
    final onNetwork = serverSync.canAddMatterOnNetworkDevice;
    final bleWifi = serverSync.canCommissionMatterBleWifi;
    return switch ((onNetwork, bleWifi)) {
      (true, true) => 'Scan a QR code or enter a setup code to add a bulb',
      (true, false) => 'Scan a QR code or enter a setup code to add a bulb',
      (false, true) =>
        'Scan a QR code or enter a setup code to commission a bulb',
      _ => 'Scan a QR code or enter a setup code to add a bulb',
    };
  }

  bool _isHubConnected(ServerSyncProvider serverSync, String hubType) {
    final connectedHubTypes = serverSync.connectedHubTypes;
    return switch (hubType) {
      'homeassistant' => connectedHubTypes.contains('homeassistant') ||
          connectedHubTypes.contains('home_assistant'),
      _ => connectedHubTypes.contains(hubType),
    };
  }

  @override
  Widget build(BuildContext context) {
    final serverSync = context.watch<ServerSyncProvider>();
    final isAddon = serverSync.serverPlatformContext == 'ha_addon';
    final homeAssistantConnected = _isHubConnected(serverSync, 'homeassistant');
    final hueConnected = _isHubConnected(serverSync, 'hue');

    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: Stack(
        children: [
          SafeArea(
            bottom: false,
            child: Center(
              child: SingleChildScrollView(
                padding: const EdgeInsets.symmetric(horizontal: 32),
                child: AnimatedBuilder(
                  animation: _breathe,
                  builder: (context, _) {
                    return Column(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        // Hero icon
                        _buildHeroIcon(),
                        const SizedBox(height: 32),
                        // Title
                        Text(
                          'Add Hubs',
                          style: TextStyle(
                            color: CelestialColors.textPrimary,
                            fontSize: 24,
                            fontWeight: FontWeight.w600,
                            letterSpacing: 0.5,
                          ),
                        ),
                        const SizedBox(height: 10),
                        Text(
                          'Connect Hue or Home Assistant,\nor scan a Matter bulb',
                          textAlign: TextAlign.center,
                          style: TextStyle(
                            color: CelestialColors.textSecondary
                                .withValues(alpha: 0.7),
                            fontSize: 15,
                            height: 1.5,
                          ),
                        ),
                        const SizedBox(height: 36),
                        // HA card — one-tap for addon, manual token for server
                        if (isAddon) ...[
                          _buildHubCard(
                            icon: Icons.home_outlined,
                            title: 'Home Assistant',
                            subtitle: 'Use your existing HA areas and lights',
                            color: const Color(0xFF42A5F5),
                            isConnected: homeAssistantConnected,
                            isLoading: _isConfiguringHa,
                            showBetaBadge: true,
                            onTap: _isConfiguringHa ? null : _configureHa,
                          ),
                          const SizedBox(height: 12),
                        ] else ...[
                          _buildHubCard(
                            icon: Icons.home_outlined,
                            title: 'Home Assistant',
                            subtitle: 'Connect with a long-lived access token',
                            color: const Color(0xFF42A5F5),
                            isConnected: homeAssistantConnected,
                            showBetaBadge: true,
                            onTap: _configureHaManual,
                          ),
                          const SizedBox(height: 12),
                        ],
                        // Hue card (always)
                        _buildHubCard(
                          icon: Icons.lightbulb_outline,
                          title: 'Philips Hue',
                          subtitle: 'Connect via push-link pairing',
                          color: const Color(0xFFFFB900),
                          isConnected: hueConnected,
                          onTap: _configureHue,
                        ),
                        const SizedBox(height: 12),
                        _buildHubCard(
                          icon: Icons.memory_outlined,
                          title: 'Matter',
                          subtitle: _matterSubtitle(serverSync),
                          color: const Color(0xFF26A69A),
                          showBetaBadge: true,
                          onTap: _configureMatter,
                        ),
                        // Space for bottom nav
                        const SizedBox(height: 100),
                      ],
                    );
                  },
                ),
              ),
            ),
          ),
          // Bottom overlay
          Positioned(
            bottom: 0,
            left: 0,
            right: 0,
            child: SafeArea(
              child: BottomNavOverlay(
                currentPage: 0,
                totalPages: 1,
                onSettingsTap: widget.onSettingsTap,
                onSunPositionTap: widget.onSunPositionTap,
                onAddDevicesTap: widget.onAddDevicesTap,
                onReportBugTap: widget.onReportBugTap,
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildHeroIcon() {
    final b = _breathe.value;
    const color = Color(0xFF00BCD4);

    return Container(
      width: 80,
      height: 80,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        gradient: LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [
            color.withValues(alpha: 0.15 + b * 0.08),
            color.withValues(alpha: 0.08 + b * 0.05),
          ],
        ),
        border: Border.all(
          color: color.withValues(alpha: 0.2 + b * 0.15),
          width: 1.5,
        ),
        boxShadow: [
          BoxShadow(
            color: color.withValues(alpha: 0.1 + b * 0.12),
            blurRadius: 32 + b * 16,
            spreadRadius: b * 4,
          ),
        ],
      ),
      child: Icon(
        Icons.hub,
        color: color.withValues(alpha: 0.7 + b * 0.3),
        size: 36,
      ),
    );
  }

  Widget _buildHubCard({
    required IconData icon,
    required String title,
    required String subtitle,
    required Color color,
    bool isConnected = false,
    bool isLoading = false,
    bool showBetaBadge = false,
    VoidCallback? onTap,
  }) {
    return GestureDetector(
      onTap: onTap,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 18),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(16),
          color: color.withValues(alpha: 0.08),
          border: Border.all(
            color: color.withValues(alpha: 0.2),
            width: 1,
          ),
        ),
        child: Row(
          children: [
            Container(
              width: 44,
              height: 44,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: color.withValues(alpha: 0.15),
              ),
              child: Icon(
                icon,
                color: color,
                size: 22,
              ),
            ),
            const SizedBox(width: 16),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  if (showBetaBadge)
                    BetaLabel(
                      label: title,
                      style: const TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 16,
                        fontWeight: FontWeight.w600,
                      ),
                    )
                  else
                    Text(
                      title,
                      style: const TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 16,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  const SizedBox(height: 3),
                  Text(
                    subtitle,
                    style: TextStyle(
                      color:
                          CelestialColors.textSecondary.withValues(alpha: 0.7),
                      fontSize: 13,
                    ),
                  ),
                ],
              ),
            ),
            if (isLoading)
              SizedBox(
                width: 20,
                height: 20,
                child: CircularProgressIndicator(
                  strokeWidth: 2,
                  color: color.withValues(alpha: 0.7),
                ),
              )
            else ...[
              if (isConnected) ...[
                const Icon(
                  Icons.check_circle_rounded,
                  color: Color(0xFF22C55E),
                  size: 20,
                ),
                const SizedBox(width: 8),
              ],
              Icon(
                Icons.chevron_right,
                color: CelestialColors.textSecondary.withValues(alpha: 0.4),
                size: 22,
              ),
            ],
          ],
        ),
      ),
    );
  }
}
