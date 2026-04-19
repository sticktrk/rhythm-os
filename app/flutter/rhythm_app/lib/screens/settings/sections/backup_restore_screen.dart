import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart' show Hub, HubType;
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show RhythmBundleApi, RhythmConnectionState;

import '../../../providers/home_provider.dart';
import '../../../providers/server_sync_provider.dart';
import '../../../services/analytics_service.dart';
import '../../../services/cloud_backup_service.dart';
import '../../../widgets/settings_row.dart';
import '../../../widgets/solar_orbit.dart';

class BackupRestoreScreen extends StatelessWidget {
  const BackupRestoreScreen({super.key});

  static Future<void> show(BuildContext context) {
    AnalyticsService().logScreenView('backup_restore');
    return Navigator.of(context).push(
      MaterialPageRoute(builder: (_) => const BackupRestoreScreen()),
    );
  }

  @override
  Widget build(BuildContext context) {
    final serverSync = context.watch<ServerSyncProvider>();
    final serverConnected =
        serverSync.connectionState == RhythmConnectionState.connected;
    final canUseCloudBackups = CloudBackupService.instance.canUseCloudBackups;
    final availabilityLabel = !serverConnected
        ? 'Server not connected'
        : (canUseCloudBackups ? null : 'Sign in required');
    final actionsEnabled = serverConnected && canUseCloudBackups;

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
                    Container(
                      padding: const EdgeInsets.all(16),
                      decoration: BoxDecoration(
                        color: CelestialColors.backgroundCard,
                        borderRadius: BorderRadius.circular(14),
                      ),
                      child: const Text(
                        'Save the current Rhythm Server state to your cloud backup, or restore the latest cloud backup back onto the connected server.',
                        style: TextStyle(
                          color: CelestialColors.textSecondary,
                          fontSize: 14,
                          height: 1.4,
                        ),
                      ),
                    ),
                    const SizedBox(height: 12),
                    SettingsGroup(
                      children: [
                        SettingsRow(
                          icon: Icons.cloud_upload_rounded,
                          iconColor: const Color(0xFF42A5F5),
                          label: 'Back Up Now',
                          value: availabilityLabel,
                          showChevron: actionsEnabled,
                          onTap:
                              actionsEnabled ? () => _backupNow(context) : null,
                        ),
                        SettingsRow(
                          icon: Icons.settings_backup_restore_rounded,
                          iconColor: const Color(0xFF26A69A),
                          label: 'Restore From Backup',
                          value: availabilityLabel,
                          showChevron: actionsEnabled,
                          onTap: actionsEnabled
                              ? () => _restoreFromBackup(context)
                              : null,
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
              'Backup & Restore',
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

  Future<void> _backupNow(BuildContext context) async {
    final serverSync = context.read<ServerSyncProvider>();
    final homeProvider = context.read<HomeProvider>();
    final serverHub = _resolveConnectedServer(
      serverSync: serverSync,
      homeProvider: homeProvider,
    );

    if (serverHub == null ||
        serverSync.connectionState != RhythmConnectionState.connected) {
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(
            content: Text('Connect a Rhythm Server to back up now.'),
          ),
        );
      }
      return;
    }

    final cloudBackups = CloudBackupService.instance;
    if (!cloudBackups.canUseCloudBackups) {
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(
            content: Text('Sign in to save a cloud backup.'),
          ),
        );
      }
      return;
    }

    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: const Text(
          'Back Up Now',
          style: TextStyle(color: CelestialColors.textPrimary),
        ),
        content: const Text(
          'This will capture the current settings from the connected Rhythm Server and save them as your latest cloud backup.',
          style: TextStyle(color: CelestialColors.textSecondary),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: Text(
              'Cancel',
              style: TextStyle(color: CelestialColors.textSecondary),
            ),
          ),
          TextButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text(
              'Back Up',
              style: TextStyle(color: CelestialColors.accentBlue),
            ),
          ),
        ],
      ),
    );

    if (confirmed != true || !context.mounted) return;

    AnalyticsService().logEvent('backup_capture_started', {
      'source_hub_id': serverHub.id,
      'source_hub_type': serverHub.type.name,
    });

    _showProgressDialog(context, message: 'Saving cloud backup...');
    try {
      debugPrint(
        'BackupRestoreScreen: manual backup using ${serverHub.endpoint.baseUrl}',
      );
      final snapshot = await cloudBackups.captureNow(
        serverHub: serverHub,
        home: homeProvider.currentHome,
        reason: 'manual_backup',
      );

      if (context.mounted) {
        Navigator.of(context, rootNavigator: true).pop();
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(
            content: Text('Cloud backup saved from the connected server.'),
          ),
        );
      }

      AnalyticsService().logEvent('backup_capture_succeeded', {
        'source_hub_id': serverHub.id,
        'source_hub_type': snapshot.sourceHubType,
      });
    } catch (error) {
      if (context.mounted) {
        Navigator.of(context, rootNavigator: true).pop();
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text('Backup failed: $error'),
            backgroundColor: Colors.red.shade800,
          ),
        );
      }

      AnalyticsService().logEvent('backup_capture_failed', {
        'source_hub_id': serverHub.id,
        'error': _trimAnalyticsError(error),
      });
    }
  }

  Future<void> _restoreFromBackup(BuildContext context) async {
    final serverSync = context.read<ServerSyncProvider>();
    final homeProvider = context.read<HomeProvider>();
    final serverHub = _resolveConnectedServer(
      serverSync: serverSync,
      homeProvider: homeProvider,
    );

    if (serverHub == null ||
        serverSync.connectionState != RhythmConnectionState.connected) {
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(
            content: Text('Connect a Rhythm Server to restore a backup.'),
          ),
        );
      }
      return;
    }

    final cloudBackups = CloudBackupService.instance;
    if (!cloudBackups.canUseCloudBackups) {
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(
            content: Text('Sign in to restore from your cloud backup.'),
          ),
        );
      }
      return;
    }

    var snapshot = await cloudBackups.getSnapshotForCurrentUser();
    if (snapshot == null) {
      try {
        snapshot = await cloudBackups.captureNow(
          serverHub: serverHub,
          home: homeProvider.currentHome,
          reason: 'restore_prefetch',
        );
      } catch (error) {
        if (context.mounted) {
          ScaffoldMessenger.of(context).showSnackBar(
            SnackBar(
              content: Text('Could not load a cloud backup: $error'),
              backgroundColor: Colors.red.shade800,
            ),
          );
        }
        return;
      }
    }
    if (!context.mounted) return;

    final sourceLabel = snapshot.sourceHubName.isNotEmpty
        ? snapshot.sourceHubName
        : snapshot.sourceHubHost;
    final capturedAtLabel =
        _formatBackupTimestamp(snapshot.capturedAt ?? snapshot.updatedAt);

    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: const Text(
          'Restore From Backup',
          style: TextStyle(color: CelestialColors.textPrimary),
        ),
        content: Text(
          capturedAtLabel == null
              ? 'This will restore your latest cloud backup to the connected Rhythm Server and overwrite any existing settings on it.'
              : 'This will restore your latest cloud backup from $sourceLabel ($capturedAtLabel) to the connected Rhythm Server and overwrite any existing settings on it.',
          style: const TextStyle(color: CelestialColors.textSecondary),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: Text(
              'Cancel',
              style: TextStyle(color: CelestialColors.textSecondary),
            ),
          ),
          TextButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text(
              'Restore',
              style: TextStyle(color: CelestialColors.accentBlue),
            ),
          ),
        ],
      ),
    );

    if (confirmed != true || !context.mounted) return;

    AnalyticsService().logEvent('backup_restore_started', {
      'source_hub_type': snapshot.sourceHubType,
      'target_hub_id': serverHub.id,
    });

    _showProgressDialog(context, message: 'Restoring backup...');
    try {
      debugPrint(
        'BackupRestoreScreen: manual restore using ${serverHub.endpoint.baseUrl}',
      );
      final api = RhythmBundleApi(baseUrl: serverHub.endpoint.baseUrl);
      await api.putBackupBundle(snapshot.backupBundle);
      await serverSync.fullRefresh();
      cloudBackups.scheduleCapture(
        serverHub: serverHub,
        home: homeProvider.currentHome,
        delay: const Duration(seconds: 3),
        reason: 'manual_restore',
      );

      if (context.mounted) {
        Navigator.of(context, rootNavigator: true).pop();
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(
            content: Text('Backup restored to the connected Rhythm Server.'),
          ),
        );
      }

      AnalyticsService().logEvent('backup_restore_succeeded', {
        'source_hub_type': snapshot.sourceHubType,
        'target_hub_id': serverHub.id,
      });
    } catch (error) {
      if (context.mounted) {
        Navigator.of(context, rootNavigator: true).pop();
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text('Restore failed: $error'),
            backgroundColor: Colors.red.shade800,
          ),
        );
      }

      AnalyticsService().logEvent('backup_restore_failed', {
        'target_hub_id': serverHub.id,
        'error': _trimAnalyticsError(error),
      });
    }
  }

  void _showProgressDialog(
    BuildContext context, {
    required String message,
  }) {
    showDialog<void>(
      context: context,
      barrierDismissible: false,
      builder: (context) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        content: Row(
          children: [
            const SizedBox(
              width: 20,
              height: 20,
              child: CircularProgressIndicator(strokeWidth: 2),
            ),
            const SizedBox(width: 16),
            Expanded(
              child: Text(
                message,
                style: const TextStyle(color: CelestialColors.textPrimary),
              ),
            ),
          ],
        ),
      ),
    );
  }

  String? _formatBackupTimestamp(DateTime? timestamp) {
    if (timestamp == null) return null;
    final local = timestamp.toLocal();
    final month = local.month.toString().padLeft(2, '0');
    final day = local.day.toString().padLeft(2, '0');
    final hour = local.hour.toString().padLeft(2, '0');
    final minute = local.minute.toString().padLeft(2, '0');
    return '${local.year}-$month-$day $hour:$minute';
  }

  Hub? _resolveConnectedServer({
    required ServerSyncProvider serverSync,
    required HomeProvider homeProvider,
  }) {
    return serverSync.connectedServerHub ??
        homeProvider.getFirstHubOfType(HubType.server);
  }

  String _trimAnalyticsError(Object error) {
    final value = error.toString();
    return value.substring(0, value.length > 120 ? 120 : value.length);
  }
}
