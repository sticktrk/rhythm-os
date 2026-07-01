import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmConnectionState;
import '../../backend/auth/auth_user.dart';
import '../../config/platform_capabilities.dart';
import '../../providers/server_sync_provider.dart';
import '../../widgets/header_close_button.dart';
import '../../widgets/solar_orbit.dart'; // For CelestialColors
import '../../providers/settings_provider.dart';
import '../../providers/home_provider.dart';
import '../../services/auth_service.dart';
import '../../widgets/settings_row.dart';
import '../power_usage_screen.dart';
import 'light_screen.dart';
import 'sections/account_section.dart';
import 'sections/lights_devices_section.dart';
// import 'sections/sleep_section.dart'; // TODO: Re-enable when sleep schedule is implemented
import 'sections/rhythm_server_section.dart';
import 'sections/rhythm_app_section.dart';

/// Full-screen settings modal with slide-up animation.
///
/// Layout:
/// - Close button (X) in circular blue container, top-left
/// - "Settings" title centered
/// - Scrollable content with grouped card rows
class SettingsScreen extends StatelessWidget {
  const SettingsScreen({super.key, this.onClose});

  /// Dismisses this menu back to Home (shown as a ✕ in the header).
  final VoidCallback? onClose;

  @override
  Widget build(BuildContext context) {
    return ChangeNotifierProxyProvider<HomeProvider, SettingsProvider>(
      create: (_) => SettingsProvider(),
      update: (_, homeProvider, settingsProvider) {
        settingsProvider?.updateFromHome(
          homeProvider.currentHome,
          homeProvider.currentHomeHubs,
        );
        return settingsProvider!;
      },
      child: Scaffold(
        backgroundColor: CelestialColors.backgroundDark,
        body: SafeArea(
          child: Column(
            children: [
              _buildHeader(context),
              Expanded(
                child: SingleChildScrollView(
                  padding: const EdgeInsets.symmetric(horizontal: 20),
                  child: Builder(
                    builder: (context) {
                      final caps = context.read<PlatformCapabilities>();
                      if (!caps.hasAccounts) {
                        return _buildSettingsContent(context);
                      }
                      return StreamBuilder<AuthUser?>(
                        stream: AuthService().authStateChanges,
                        initialData: AuthService().currentUser,
                        builder: (context, snapshot) {
                          return _buildSettingsContent(
                            context,
                            user: snapshot.data,
                          );
                        },
                      );
                    },
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _buildSettingsContent(BuildContext context, {AuthUser? user}) {
    final caps = context.read<PlatformCapabilities>();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (caps.hasAccounts) AccountSection(user: user),
        // SleepSection(), // TODO: Re-enable when sleep schedule is implemented

        // "Your Light" — the *experience*: how the light looks and feels. Kept
        // deliberately separate from the Hardware group below so it's never
        // confused with the physical devices that produce it.
        const SettingsSectionHeader(
          title: 'Your Light',
          icon: Icons.light_mode_rounded,
          subtitle: 'How your light looks and feels through the day.',
        ),
        SettingsGroup(
          children: [
            _buildLightRow(context),
          ],
        ),

        // "Hardware" — the *physical devices* and their connections. The icon,
        // subtitle, and cool-tech accent make it unmistakable that these are
        // hardware settings, not light-appearance settings.
        SettingsSectionHeader(
          title: 'Hardware',
          icon: Icons.memory_rounded,
          subtitle: 'Your physical Rhythm devices and connections.',
          accent: const Color(0xFF00BCD4).withValues(alpha: 0.85),
        ),
        SettingsGroup(
          children: [
            _buildDevicesRow(context),
            _buildRhythmOsServerRow(),
          ],
        ),
        const SettingsSectionHeader(
          title: 'Energy',
          icon: Icons.bolt_rounded,
          subtitle: 'How much power your lights are using.',
        ),
        SettingsGroup(
          children: [
            _buildPowerUsageRow(context),
          ],
        ),
        const SettingsSectionHeader(
          title: 'App',
          icon: Icons.tune_rounded,
          subtitle: 'Location, backups, help, and version.',
        ),
        SettingsGroup(
          children: [
            SettingsRow(
              icon: Icons.apps_rounded,
              iconColor: CelestialColors.accentBlue,
              label: 'App Settings',
              onTap: () => RhythmAppDetailScreen.show(context),
            ),
          ],
        ),
        const SizedBox(height: 40),
      ],
    );
  }

  Widget _buildRhythmOsServerRow() {
    return Consumer2<HomeProvider, ServerSyncProvider>(
      builder: (context, homeProvider, serverSync, child) {
        final serverHub = homeProvider.activeServerHub;
        final serverState = serverSync.connectionState;
        final isOnline = serverState == RhythmConnectionState.connected;
        final isConnecting = serverState == RhythmConnectionState.connecting ||
            serverState == RhythmConnectionState.reconnecting;

        String? statusText;
        Color? statusColor;
        if (serverHub != null) {
          if (isOnline) {
            statusText = 'Online';
            statusColor = const Color(0xFF22C55E);
          } else if (isConnecting) {
            statusText = 'Connecting...';
            statusColor = const Color(0xFFE8A54B);
          } else {
            statusText = 'Offline';
            statusColor = Colors.red.shade400;
          }
        }

        return SettingsRow(
          icon: Icons.developer_board,
          iconColor: const Color(0xFF00BCD4),
          label: 'LightBox',
          trailing: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (serverHub != null &&
                  statusText != null &&
                  statusColor != null) ...[
                Container(
                  width: 8,
                  height: 8,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: statusColor,
                  ),
                ),
                const SizedBox(width: 6),
                Text(
                  statusText,
                  style: TextStyle(
                    color: statusColor,
                    fontSize: 13,
                    fontWeight: FontWeight.w500,
                  ),
                ),
                const SizedBox(width: 8),
              ],
              Icon(
                Icons.chevron_right,
                color: CelestialColors.textSecondary.withValues(alpha: 0.7),
                size: 24,
              ),
            ],
          ),
          showChevron: false,
          onTap: () => RhythmServerDetailScreen.show(context),
        );
      },
    );
  }

  Widget _buildDevicesRow(BuildContext context) {
    return SettingsRow(
      icon: Icons.lightbulb_rounded,
      iconColor: const Color(0xFFFFB300),
      label: 'Devices',
      onTap: () => DevicesListScreen.show(context),
    );
  }

  Widget _buildLightRow(BuildContext context) {
    return SettingsRow(
      icon: Icons.light_mode_rounded,
      iconColor: const Color(0xFFFFB74D),
      label: 'Light',
      value: 'Day · Sleep',
      onTap: () => LightScreen.show(context),
    );
  }

  Widget _buildPowerUsageRow(BuildContext context) {
    return SettingsRow(
      icon: Icons.bolt_rounded,
      iconColor: const Color(0xFF4ADE80),
      label: 'Power Usage',
      onTap: () => PowerUsageScreen.show(context),
    );
  }

  Widget _buildHeader(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 16),
      child: Row(
        children: [
          if (onClose != null)
            HeaderCloseButton(onTap: onClose!)
          else
            const SizedBox(width: 40),
          const Expanded(
            child: Text(
              'Settings',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 18,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
          const SizedBox(width: 40),
        ],
      ),
    );
  }
}
