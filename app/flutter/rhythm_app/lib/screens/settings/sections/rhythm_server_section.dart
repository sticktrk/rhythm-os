import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../../../providers/home_provider.dart';
import '../../../services/analytics_service.dart';
import '../../../services/employee_mode_service.dart';
import '../../../widgets/connect_hub_screen.dart';
import '../../../widgets/settings_row.dart';
import '../../../widgets/solar_orbit.dart';
import '../../hubs/rhythmserver_settings_screen.dart';

/// Detail screen for the RhythmOS Server settings group.
class RhythmServerDetailScreen extends StatelessWidget {
  const RhythmServerDetailScreen({super.key});

  static Future<void> show(BuildContext context) {
    AnalyticsService().logScreenView('rhythm_server_detail');
    return Navigator.of(context).push(
      MaterialPageRoute(builder: (_) => const RhythmServerDetailScreen()),
    );
  }

  @override
  Widget build(BuildContext context) {
    final bridgeHub = context.watch<HomeProvider>().activeServerHub;
    final employeeMode = EmployeeModeService.instance.isActive;

    if (bridgeHub != null) {
      return RhythmServerSettingsScreen(
        hub: bridgeHub,
        headerTitleOverride: 'RhythmOS Server',
        useBackButton: true,
      );
    }

    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _buildHeader(context),
            Expanded(
              child: SingleChildScrollView(
                padding: const EdgeInsets.symmetric(horizontal: 20),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    const SizedBox(height: 8),
                    SettingsGroup(
                      children: [
                        SettingsRow(
                          icon: Icons.developer_board,
                          iconColor: const Color(0xFF00BCD4),
                          label: employeeMode
                              ? 'Support server unavailable'
                              : 'Connect RhythmOS Server',
                          value: employeeMode ? 'Waiting for grant' : null,
                          onTap: employeeMode
                              ? null
                              : () => ConnectHubScreen.show(
                                    context,
                                    mode: ConnectHubMode.rhythmServer,
                                  ),
                        ),
                      ],
                    ),
                    const SizedBox(height: 40),
                  ],
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
              'RhythmOS Server',
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
