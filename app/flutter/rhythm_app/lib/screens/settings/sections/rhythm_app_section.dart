import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart' show HubType;
import '../../../config/platform_capabilities.dart';
import '../../../main.dart';
import '../../../widgets/solar_orbit.dart';
import '../../../widgets/settings_row.dart';
import '../../../providers/settings_provider.dart';
import '../../../providers/room_provider.dart';
import '../../../providers/home_provider.dart';
import '../../../providers/hub_connection_provider.dart';
import '../../../providers/server_sync_provider.dart';
import '../../../services/analytics_service.dart';
import '../../../services/auth_service.dart';
import '../../../services/cloud_backup_service.dart';
import '../../../services/settings_service.dart';
import '../../../services/hue_sse_storage.dart';
import '../../../services/server_bundle_api.dart';
import '../dialogs/feedback_dialog.dart';
import '../../location_settings_screen.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmConnectionState;

/// Detail screen for the Rhythm App settings group.
class RhythmAppDetailScreen extends StatelessWidget {
  const RhythmAppDetailScreen({super.key});

  static Future<void> show(BuildContext context) {
    AnalyticsService().logScreenView('rhythm_app_detail');
    return Navigator.of(context).push(
      MaterialPageRoute(builder: (_) => const RhythmAppDetailScreen()),
    );
  }

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
                  child: Consumer<SettingsProvider>(
                    builder: (context, settings, child) {
                      final caps = context.read<PlatformCapabilities>();
                      final serverSync = context.watch<ServerSyncProvider>();
                      final serverConnected = serverSync.connectionState ==
                          RhythmConnectionState.connected;
                      final canUseCloudBackups =
                          CloudBackupService.instance.canUseCloudBackups;
                      return Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          const SizedBox(height: 8),
                          if (AuthService().currentUser != null &&
                              !AuthService().currentUser!.isAnonymous) ...[
                            SettingsGroup(
                              children: [
                                SettingsRow(
                                  icon: Icons.person_outline,
                                  iconColor: CelestialColors.accentBlue,
                                  label: 'Profile',
                                  value: AuthService().currentUser?.email ??
                                      'Signed in',
                                  showChevron: false,
                                ),
                              ],
                            ),
                            const SizedBox(height: 12),
                          ],
                          if (caps.hasLocationSetup) ...[
                            SettingsGroup(
                              children: [
                                SettingsRow(
                                  icon: Icons.location_on_outlined,
                                  iconColor: const Color(0xFF4CAF50),
                                  label: 'Location',
                                  onTap: () =>
                                      _showLocationSettings(context, settings),
                                ),
                              ],
                            ),
                            const SizedBox(height: 12),
                          ],
                          SettingsGroup(
                            children: [
                              SettingsRow(
                                icon: Icons.chat_bubble_outline_rounded,
                                iconColor: const Color(0xFFFFC857),
                                label: 'Help & Feedback',
                                onTap: () {
                                  AnalyticsService().logFeedbackOpened();
                                  FeedbackDialog.show(context);
                                },
                              ),
                              SettingsRow(
                                icon: Icons.settings_backup_restore_rounded,
                                iconColor: const Color(0xFF26A69A),
                                label: 'Restore From Backup',
                                value: serverConnected
                                    ? (canUseCloudBackups
                                        ? null
                                        : 'Sign in required')
                                    : 'Server not connected',
                                showChevron: serverConnected,
                                onTap: serverConnected
                                    ? () => _restoreFromBackup(context)
                                    : null,
                              ),
                            ],
                          ),
                          const SizedBox(height: 12),
                          SettingsGroup(
                            children: [
                              SettingsRow(
                                icon: Icons.info_outline,
                                iconColor: const Color(0xFF607D8B),
                                label: 'Version',
                                value: settings.appVersion,
                                showChevron: false,
                                onTap: null,
                              ),
                            ],
                          ),
                          const SizedBox(height: 12),
                          SettingsGroup(
                            children: [
                              SettingsRow(
                                icon: Icons.delete_forever_rounded,
                                iconColor: Colors.red,
                                label: 'Delete Account',
                                showChevron: false,
                                onTap: () => _deleteAccount(context),
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
              'Rhythm App',
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

  Future<void> _showLocationSettings(
      BuildContext context, SettingsProvider settings) async {
    final result = await LocationSettingsScreen.show(context);
    if (result == true) {
      settings.loadSettings();
    }
  }

  Future<void> _restoreFromBackup(BuildContext context) async {
    final serverSync = context.read<ServerSyncProvider>();
    final homeProvider = context.read<HomeProvider>();
    final serverHub = homeProvider.getFirstHubOfType(HubType.server);

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
    snapshot ??= await cloudBackups.captureNow(
      serverHub: serverHub,
      home: homeProvider.currentHome,
      reason: 'restore_prefetch',
    );
    if (!context.mounted) return;

    if (snapshot == null) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text('No cloud backup found yet for this account.'),
        ),
      );
      return;
    }

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

    _showRestoreProgressDialog(context);
    try {
      final api = ServerBundleApi(endpoint: serverHub.endpoint);
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
        'error': error.toString().substring(
            0, error.toString().length > 120 ? 120 : error.toString().length),
      });
    }
  }

  void _showRestoreProgressDialog(BuildContext context) {
    showDialog<void>(
      context: context,
      barrierDismissible: false,
      builder: (context) => const AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        content: Row(
          children: [
            SizedBox(
              width: 20,
              height: 20,
              child: CircularProgressIndicator(strokeWidth: 2),
            ),
            SizedBox(width: 16),
            Expanded(
              child: Text(
                'Restoring backup...',
                style: TextStyle(color: CelestialColors.textPrimary),
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

  Future<void> _deleteAccount(BuildContext context) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: const Text(
          'Delete Account',
          style: TextStyle(color: CelestialColors.textPrimary),
        ),
        content: const Text(
          'This will permanently delete your account and all associated data including homes, hubs, rooms, and preferences. This action cannot be undone.',
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
              'Delete',
              style: TextStyle(color: Colors.red),
            ),
          ),
        ],
      ),
    );

    if (confirmed == true && context.mounted) {
      final homeProvider = context.read<HomeProvider>();
      final hubProvider = context.read<HubConnectionProvider>();
      final roomProvider = context.read<RoomProvider>();
      debugPrint('Delete Account: Starting account deletion...');

      final authService = AuthService();
      if (authService.currentUser != null) {
        debugPrint('Delete Account: Deleting account from server...');
        try {
          await authService.deleteAccount();
          debugPrint('Delete Account: Server account deleted');
        } catch (e) {
          debugPrint('Delete Account: Edge Function failed: $e');
          try {
            for (final home in homeProvider.homes) {
              await homeProvider.repository.deleteHome(home.id);
            }
            debugPrint('Delete Account: Homes deleted via PostgREST fallback');
          } catch (e2) {
            debugPrint('Delete Account: PostgREST fallback also failed: $e2');
          }
          await authService.signOut();
        }
      }

      if (!context.mounted) return;

      try {
        hubProvider.disconnect();
        debugPrint('Delete Account: Hub connections disconnected');
      } catch (e) {
        debugPrint(
            'Delete Account: Hub disconnect error (may not be provided): $e');
      }

      try {
        await roomProvider.clearAllRooms();
        debugPrint('Delete Account: Room provider cleared');
      } catch (e) {
        debugPrint('Delete Account: Room clear error: $e');
      }

      try {
        await HueSseStorage.clearAll();
        debugPrint('Delete Account: SSE storage cleared');
      } catch (e) {
        debugPrint('Delete Account: SSE storage clear error: $e');
      }

      await SettingsService.instance.clearAll();
      debugPrint('Delete Account: Settings service cleared');

      try {
        await homeProvider.onUserSignOut();
        debugPrint(
            'Delete Account: Home provider signed out (all local data cleared)');
      } catch (e) {
        debugPrint('Delete Account: Home provider error: $e');
      }

      AnalyticsService().logAccountDeleted();
      AnalyticsService().logSignOut();
      AnalyticsService().resetUser();

      debugPrint('Delete Account: Complete! Returning to onboarding...');

      if (context.mounted) {
        Navigator.of(context, rootNavigator: true)
            .popUntil((route) => route.isFirst);
        WidgetsBinding.instance.addPostFrameCallback((_) {
          AuthGate.resetToOnboarding();
        });
      }
    }
  }
}
