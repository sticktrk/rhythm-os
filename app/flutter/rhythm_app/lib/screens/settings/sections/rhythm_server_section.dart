import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmConnectionState;
import '../../../widgets/settings_row.dart';
import '../../../widgets/solar_orbit.dart';
import '../../../providers/home_provider.dart';
import '../../../providers/server_sync_provider.dart';
import '../../hubs/rhythmserver_settings_screen.dart';
import '../../triage_screen.dart';
import '../../power_usage_screen.dart';
import '../../../widgets/connect_hub_screen.dart';

/// Detail screen for the Rhythm Server settings group.
class RhythmServerDetailScreen extends StatelessWidget {
  const RhythmServerDetailScreen({super.key});

  static Future<void> show(BuildContext context) {
    return Navigator.of(context).push(
      MaterialPageRoute(builder: (_) => const RhythmServerDetailScreen()),
    );
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _buildHeader(context),
            Expanded(
              child: SingleChildScrollView(
                padding: const EdgeInsets.symmetric(horizontal: 20),
                child: Consumer2<HomeProvider, ServerSyncProvider>(
                  builder: (context, homeProvider, serverSync, child) {
                    return Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        const SizedBox(height: 8),
                        SettingsGroup(
                          children: [
                            // ── RhythmServer ──
                            Builder(
                              builder: (context) {
                                final esp32Hub = homeProvider
                                    .getFirstHubOfType(HubType.server);
                                final serverState =
                                    serverSync.connectionState;
                                final isOnline = serverState ==
                                    RhythmConnectionState.connected;
                                final isConnecting = serverState ==
                                        RhythmConnectionState.connecting ||
                                    serverState ==
                                        RhythmConnectionState.reconnecting;

                                String? statusText;
                                Color? statusColor;
                                if (esp32Hub != null) {
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
                                  label: 'RhythmServer',
                                  trailing: Row(
                                    mainAxisSize: MainAxisSize.min,
                                    children: [
                                      if (esp32Hub != null &&
                                          statusText != null) ...[
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
                                        color: CelestialColors.textSecondary
                                            .withValues(alpha: 0.7),
                                        size: 24,
                                      ),
                                    ],
                                  ),
                                  showChevron: false,
                                  onTap: esp32Hub != null
                                      ? () =>
                                          RhythmServerSettingsScreen.show(
                                              context,
                                              hub: esp32Hub)
                                      : () => ConnectHubScreen.show(context,
                                          mode:
                                              ConnectHubMode.rhythmServer),
                                );
                              },
                            ),
                            // ── Device Review ──
                            SettingsRow(
                              icon: Icons.devices_other,
                              iconColor: const Color(0xFFFF9800),
                              label: 'Device Review',
                              trailing: Row(
                                mainAxisSize: MainAxisSize.min,
                                children: [
                                  if (serverSync.triagePendingCount > 0)
                                    Container(
                                      padding: const EdgeInsets.symmetric(
                                          horizontal: 7, vertical: 2),
                                      decoration: BoxDecoration(
                                        color: const Color(0xFFFF9800),
                                        borderRadius:
                                            BorderRadius.circular(10),
                                      ),
                                      child: Text(
                                        '${serverSync.triagePendingCount}',
                                        style: const TextStyle(
                                          color: Color(0xFF1A1A1A),
                                          fontSize: 12,
                                          fontWeight: FontWeight.w700,
                                        ),
                                      ),
                                    ),
                                  if (serverSync.triagePendingCount > 0)
                                    const SizedBox(width: 8),
                                  Icon(
                                    Icons.chevron_right,
                                    color: CelestialColors.textSecondary
                                        .withValues(alpha: 0.5),
                                    size: 22,
                                  ),
                                ],
                              ),
                              showChevron: false,
                              onTap: () => Navigator.of(context).push(
                                MaterialPageRoute(
                                    builder: (_) => const TriageScreen()),
                              ),
                            ),
                            // ── Power Usage ──
                            SettingsRow(
                              icon: Icons.bolt_rounded,
                              iconColor: const Color(0xFF4ADE80),
                              label: 'Power Usage',
                              onTap: () => PowerUsageScreen.show(context),
                            ),
                          ],
                        ),
                        const SizedBox(height: 40),
                      ],
                    );
                  },
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildHeader(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      child: Row(
        children: [
          GestureDetector(
            onTap: () => Navigator.of(context).pop(),
            child: Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: CelestialColors.accentBlue.withValues(alpha: 0.2),
              ),
              child: const Icon(
                Icons.chevron_left,
                color: CelestialColors.accentBlue,
                size: 24,
              ),
            ),
          ),
          const Expanded(
            child: Text(
              'Rhythm Server',
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
