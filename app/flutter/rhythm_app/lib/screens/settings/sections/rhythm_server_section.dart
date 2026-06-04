import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmConnectionState;
import '../../../providers/home_provider.dart';
import '../../../providers/server_sync_provider.dart';
import '../../../services/analytics_service.dart';
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
    return Consumer2<HomeProvider, ServerSyncProvider>(
      builder: (context, homeProvider, serverSync, child) {
        final activeHub = homeProvider.activeServerHub;
        final serverHubs = _sortedServerHubs(
          homeProvider.currentHomeServerHubs,
          activeHub,
        );

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
                        const SettingsSectionHeader(title: 'Saved Servers'),
                        SettingsGroup(
                          children: [
                            for (final hub in serverHubs)
                              _ServerHubRow(
                                hub: hub,
                                isActive: hub.id == activeHub?.id,
                                connectionState: serverSync.connectionState,
                                onTap: () => _openOrSwitchServer(
                                  context,
                                  hub,
                                  isActive: hub.id == activeHub?.id,
                                ),
                              ),
                            SettingsRow(
                              icon: Icons.add_circle_outline,
                              iconColor: const Color(0xFF00BCD4),
                              label: serverHubs.isEmpty
                                  ? 'Connect RhythmOS Server'
                                  : 'Connect Another Server',
                              onTap: () => ConnectHubScreen.show(
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
      },
    );
  }

  List<Hub> _sortedServerHubs(List<Hub> hubs, Hub? activeHub) {
    final sorted = hubs.toList();
    sorted.sort((left, right) {
      if (left.id == activeHub?.id) return -1;
      if (right.id == activeHub?.id) return 1;
      final leftRecency = left.lastConnected ?? left.updatedAt;
      final rightRecency = right.lastConnected ?? right.updatedAt;
      return rightRecency.compareTo(leftRecency);
    });
    return sorted;
  }

  Future<void> _openOrSwitchServer(
    BuildContext context,
    Hub hub, {
    required bool isActive,
  }) async {
    if (isActive) {
      return RhythmServerSettingsScreen.show(
        context,
        hub: hub,
        headerTitleOverride: 'RhythmOS Server',
        useBackButton: true,
      );
    }

    final homeProvider = context.read<HomeProvider>();
    final serverSync = context.read<ServerSyncProvider>();
    final activatedHub = await homeProvider.activateServerHub(hub);
    if (!context.mounted) return;

    if (activatedHub == null) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text('Could not switch to ${hub.name}.'),
          backgroundColor: Colors.red.shade400,
        ),
      );
      return;
    }

    serverSync.connectIfAvailable();
    return RhythmServerSettingsScreen.show(
      context,
      hub: activatedHub,
      headerTitleOverride: 'RhythmOS Server',
      useBackButton: true,
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

class _ServerHubRow extends StatelessWidget {
  final Hub hub;
  final bool isActive;
  final RhythmConnectionState connectionState;
  final VoidCallback onTap;

  const _ServerHubRow({
    required this.hub,
    required this.isActive,
    required this.connectionState,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final status = _status();
    return GestureDetector(
      onTap: onTap,
      behavior: HitTestBehavior.opaque,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
        child: Row(
          children: [
            Container(
              width: 36,
              height: 36,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: isActive
                    ? const Color(0xFF00BCD4)
                    : CelestialColors.orbitRing.withValues(alpha: 0.35),
              ),
              child: Icon(
                Icons.developer_board,
                color: isActive ? const Color(0xFF1A1A1A) : Colors.white,
                size: 18,
              ),
            ),
            const SizedBox(width: 14),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    hub.name,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 16,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                  const SizedBox(height: 3),
                  Text(
                    _subtitle(),
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color:
                          CelestialColors.textSecondary.withValues(alpha: 0.7),
                      fontSize: 13,
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(width: 12),
            Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Container(
                  width: 8,
                  height: 8,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: status.color,
                  ),
                ),
                const SizedBox(width: 6),
                Text(
                  status.label,
                  style: TextStyle(
                    color: status.color,
                    fontSize: 13,
                    fontWeight: FontWeight.w500,
                  ),
                ),
                const SizedBox(width: 8),
                Icon(
                  isActive ? Icons.chevron_right : Icons.swap_horiz_rounded,
                  color: CelestialColors.textSecondary.withValues(alpha: 0.7),
                  size: 22,
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }

  String _subtitle() {
    final port = hub.endpoint.port;
    final portSuffix = port == 0 ? '' : ':$port';
    final token = hub.token?.trim();
    final tokenText =
        token != null && token.isNotEmpty ? ' · owner token saved' : '';
    return '${hub.endpoint.host}$portSuffix$tokenText';
  }

  ({String label, Color color}) _status() {
    if (!isActive) {
      return (label: 'Saved', color: CelestialColors.textSecondary);
    }
    switch (connectionState) {
      case RhythmConnectionState.connected:
        return (label: 'Active', color: const Color(0xFF22C55E));
      case RhythmConnectionState.connecting:
      case RhythmConnectionState.reconnecting:
        return (label: 'Connecting', color: const Color(0xFFE8A54B));
      case RhythmConnectionState.disconnected:
        return (label: 'Offline', color: Colors.red.shade400);
    }
  }
}
