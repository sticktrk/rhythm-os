import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../../../providers/server_sync_provider.dart';
import '../../../services/analytics_service.dart';
import '../../../widgets/settings_row.dart';
import '../../../widgets/solar_orbit.dart';
import '../../hubs/rhythmserver_settings_screen.dart';
import '../../triage_screen.dart';

/// Detail screen for light hub management and device review.
class LightsDevicesDetailScreen extends StatelessWidget {
  const LightsDevicesDetailScreen({super.key});

  static Future<void> show(BuildContext context) {
    AnalyticsService().logScreenView('lights_devices_detail');
    return Navigator.of(context).push(
      MaterialPageRoute(builder: (_) => const LightsDevicesDetailScreen()),
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
                child: Consumer<ServerSyncProvider>(
                  builder: (context, serverSync, child) {
                    return Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        const SizedBox(height: 8),
                        const RhythmServerHubManagementSection(),
                        const SizedBox(height: 12),
                        SettingsGroup(
                          children: [
                            SettingsRow(
                              icon: Icons.devices_other,
                              iconColor: const Color(0xFFFF9800),
                              label: 'Device Review',
                              value: serverSync
                                      .hubConfiguredConflicts.isNotEmpty
                                  ? '${serverSync.hubConfiguredConflicts.length} conflict${serverSync.hubConfiguredConflicts.length == 1 ? '' : 's'}'
                                  : serverSync.triagePendingCount > 0
                                      ? '${serverSync.triagePendingCount} pending'
                                      : 'Clear',
                              trailing: Row(
                                mainAxisSize: MainAxisSize.min,
                                children: [
                                  if (serverSync.triagePendingCount > 0)
                                    Container(
                                      padding: const EdgeInsets.symmetric(
                                          horizontal: 7, vertical: 2),
                                      decoration: BoxDecoration(
                                        color: const Color(0xFFFF9800),
                                        borderRadius: BorderRadius.circular(10),
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
                                  builder: (_) => const TriageScreen(),
                                ),
                              ),
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
              'Lights & Devices',
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
