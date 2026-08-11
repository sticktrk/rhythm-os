import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart' show Hub, HubType;
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmConnectionState;

import '../../../providers/home_provider.dart';
import '../../../providers/room_page_provider.dart';
import '../../../providers/server_sync_provider.dart';
import '../../../services/analytics_service.dart';
import '../../../services/cloud_backup_service.dart';
import '../../../services/server_endpoint_resolver.dart';
import '../../../services/settings_service.dart';
import '../../triage_screen.dart';
import '../../../widgets/settings_row.dart';
import '../../../widgets/solar_orbit.dart';

class BackupRestoreScreen extends StatefulWidget {
  const BackupRestoreScreen({super.key});

  static Future<void> show(BuildContext context) {
    AnalyticsService().logScreenView('backup_restore');
    return Navigator.of(context).push(
      MaterialPageRoute(builder: (_) => const BackupRestoreScreen()),
    );
  }

  @override
  State<BackupRestoreScreen> createState() => _BackupRestoreScreenState();
}

class _BackupRestoreScreenState extends State<BackupRestoreScreen> {
  CloudBackupSnapshot? _latestSnapshot;
  bool _isLoadingSnapshot = false;

  @override
  void initState() {
    super.initState();
    _refreshSnapshotAvailability();
  }

  Future<void> _refreshSnapshotAvailability() async {
    final cloudBackups = CloudBackupService.instance;
    if (!cloudBackups.canUseCloudBackups) {
      if (!mounted) return;
      setState(() {
        _latestSnapshot = null;
        _isLoadingSnapshot = false;
      });
      return;
    }

    setState(() {
      _isLoadingSnapshot = true;
    });

    CloudBackupSnapshot? snapshot;
    try {
      snapshot = await cloudBackups.getSnapshotForCurrentUser();
    } catch (error) {
      debugPrint(
          'BackupRestoreScreen: failed to load snapshot metadata: $error');
    }

    if (!mounted) return;
    setState(() {
      _latestSnapshot = snapshot;
      _isLoadingSnapshot = false;
    });
  }

  @override
  Widget build(BuildContext context) {
    final serverSync = context.watch<ServerSyncProvider>();
    final serverConnected =
        serverSync.connectionState == RhythmConnectionState.connected;
    final canUseCloudBackups = CloudBackupService.instance.canUseCloudBackups;
    final backupAvailabilityLabel = !serverConnected
        ? 'Server not connected'
        : (canUseCloudBackups ? null : 'Sign in required');
    final restoreAvailabilityLabel = !serverConnected
        ? 'Server not connected'
        : !canUseCloudBackups
            ? 'Sign in required'
            : _isLoadingSnapshot
                ? 'Checking cloud backup...'
                : (_latestSnapshot == null ? 'No cloud backup' : null);
    final backupEnabled = serverConnected && canUseCloudBackups;
    final restoreEnabled =
        backupEnabled && !_isLoadingSnapshot && _latestSnapshot != null;

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
                        'Save the current Rhythm Server state and All Rooms layout to your cloud backup, or restore the latest cloud backup back onto the connected server.',
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
                          value: backupAvailabilityLabel,
                          showChevron: backupEnabled,
                          onTap:
                              backupEnabled ? () => _backupNow(context) : null,
                        ),
                        SettingsRow(
                          icon: Icons.settings_backup_restore_rounded,
                          iconColor: const Color(0xFF26A69A),
                          label: 'Restore From Backup',
                          value: restoreAvailabilityLabel,
                          showChevron: restoreEnabled,
                          onTap: restoreEnabled
                              ? () => _restoreFromBackup(context)
                              : null,
                        ),
                      ],
                    ),
                    const SizedBox(height: 16),
                    _buildReviewStatusCard(
                      context,
                      serverSync: serverSync,
                      serverConnected: serverConnected,
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

  Widget _buildReviewStatusCard(
    BuildContext context, {
    required ServerSyncProvider serverSync,
    required bool serverConnected,
  }) {
    final review = serverSync.review;
    final hasAttention = serverSync.hasReviewAttention;
    final lines = <String>[
      if (review.pending.total > 0)
        '${review.pending.total} pending review item${review.pending.total == 1 ? '' : 's'} still need confirmation.',
      if (review.disconnectedHubs.isNotEmpty)
        'Reconnect ${review.disconnectedHubs.length} hub${review.disconnectedHubs.length == 1 ? '' : 's'}: ${review.disconnectedHubs.take(2).map((hub) => hub.label).join(', ')}${review.disconnectedHubs.length > 2 ? '...' : ''}',
      if (review.preferredEndpoints.isNotEmpty)
        '${review.preferredEndpoints.length} device${review.preferredEndpoints.length == 1 ? '' : 's'} kept a preferred endpoint selection.',
      if (review.hubConfiguredConflicts.isNotEmpty)
        '${review.hubConfiguredConflicts.length} native hub conflict${review.hubConfiguredConflicts.length == 1 ? '' : 's'} still need a recheck.',
    ];

    return Container(
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(
          color: const Color(0xFF64B5F6).withValues(alpha: 0.2),
        ),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Row(
            children: [
              Icon(
                Icons.fact_check_outlined,
                color: Color(0xFF64B5F6),
                size: 18,
              ),
              SizedBox(width: 10),
              Text(
                'After Restore',
                style: TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 15,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ],
          ),
          const SizedBox(height: 10),
          Text(
            !serverConnected
                ? 'Connect a Rhythm Server to inspect restore status.'
                : hasAttention
                    ? 'Reconnect hubs, confirm prior review decisions, and clear conflicts before calling the restore complete.'
                    : 'Current server state is clear. If you restore a backup, any reconnect or review follow-up will appear here.',
            style: const TextStyle(
              color: CelestialColors.textSecondary,
              fontSize: 13,
              height: 1.35,
            ),
          ),
          for (final line in lines) ...[
            const SizedBox(height: 8),
            Text(
              line,
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.72),
                fontSize: 12,
                height: 1.3,
              ),
            ),
          ],
          if (review.preferredEndpoints.isNotEmpty) ...[
            const SizedBox(height: 8),
            Text(
              'Preferred route: ${review.preferredEndpoints.first.name} via ${review.preferredEndpoints.first.hubLabel}',
              style: TextStyle(
                color: const Color(0xFF9FD3FF).withValues(alpha: 0.92),
                fontSize: 12,
              ),
            ),
          ],
          const SizedBox(height: 14),
          Row(
            children: [
              Expanded(
                child: _InlineActionButton(
                  label: 'Refresh Status',
                  color: const Color(0xFF64B5F6),
                  enabled: serverConnected,
                  onTap: () async {
                    await serverSync.fullRefresh();
                    if (!mounted) return;
                    setState(() {});
                  },
                ),
              ),
              const SizedBox(width: 10),
              Expanded(
                child: _InlineActionButton(
                  label: 'Device Review',
                  color: const Color(0xFFFF9800),
                  enabled: serverConnected,
                  onTap: () => Navigator.of(context).push(
                    MaterialPageRoute(builder: (_) => const TriageScreen()),
                  ),
                ),
              ),
            ],
          ),
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
        setState(() {
          _latestSnapshot = snapshot;
          _isLoadingSnapshot = false;
        });
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

    final snapshot =
        _latestSnapshot ?? await cloudBackups.getSnapshotForCurrentUser();
    if (snapshot == null) {
      if (context.mounted) {
        setState(() {
          _latestSnapshot = null;
        });
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(
            content: Text('No cloud backup is available to restore.'),
          ),
        );
      }
      return;
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
      final resolved = await ServerEndpointResolver.resolve(
        serverHub,
        syncProvider: serverSync,
      );
      debugPrint(
        'BackupRestoreScreen: manual restore using ${resolved.baseUrl}',
      );
      final api = resolved.bundleApi();
      await api.putBackupBundle(snapshot.backupBundle);
      final scopeKey = RoomPageProvider.layoutScopeFor(
        home: homeProvider.currentHome,
        hubs: homeProvider.currentHomeHubs,
      );
      final scopeKeyAliases = RoomPageProvider.layoutScopeAliasesFor(
        home: homeProvider.currentHome,
        hubs: homeProvider.currentHomeHubs,
      );
      final restoredLayout =
          await SettingsService.instance.applyCloudSettingsBundle(
        snapshot.appSettingsBundle,
        roomLayoutScopeKey: scopeKey,
        roomLayoutHubKey: RoomPageProvider.hubLayoutKey(serverHub),
        roomLayoutHubKeyAliases:
            RoomPageProvider.hubLayoutKeyAliases(serverHub),
      );
      if (restoredLayout && context.mounted) {
        final roomPages = context.read<RoomPageProvider>();
        roomPages.setLayoutScope(
          scopeKey,
          scopeKeyAliases: scopeKeyAliases,
        );
        roomPages.reloadLayout();
      }
      await serverSync.fullRefresh();

      if (context.mounted) {
        Navigator.of(context, rootNavigator: true).pop();
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(
            content: Text(
              'Backup restored. Reconnect hubs and review any remaining items below.',
            ),
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

class _InlineActionButton extends StatelessWidget {
  const _InlineActionButton({
    required this.label,
    required this.color,
    required this.enabled,
    required this.onTap,
  });

  final String label;
  final Color color;
  final bool enabled;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: enabled ? onTap : null,
      child: Opacity(
        opacity: enabled ? 1 : 0.45,
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
          decoration: BoxDecoration(
            color: color.withValues(alpha: 0.12),
            borderRadius: BorderRadius.circular(10),
            border: Border.all(color: color.withValues(alpha: 0.28)),
          ),
          child: Center(
            child: Text(
              label,
              style: TextStyle(
                color: color,
                fontSize: 13,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
        ),
      ),
    );
  }
}
