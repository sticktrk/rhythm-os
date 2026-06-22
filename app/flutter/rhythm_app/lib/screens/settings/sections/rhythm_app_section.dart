import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmConnectionState;
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
import '../../../services/account_session_service.dart';
import '../../../services/auth_service.dart';
import '../../../services/cloud_backup_service.dart';
import '../../../services/employee_mode_service.dart';
import '../dialogs/feedback_dialog.dart';
import '../../location_settings_screen.dart';
import 'backup_restore_screen.dart';

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
                      final employeeMode =
                          EmployeeModeService.instance.isActive;
                      final currentUser = AuthService().currentUser;
                      final hasAccountSession =
                          caps.hasAccounts && currentUser != null;
                      final hasPermanentAccount =
                          currentUser != null && !currentUser.isAnonymous;
                      return Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          const SizedBox(height: 8),
                          if (hasPermanentAccount) ...[
                            SettingsGroup(
                              children: [
                                SettingsRow(
                                  icon: Icons.person_outline,
                                  iconColor: CelestialColors.accentBlue,
                                  label: 'Profile',
                                  value: currentUser.email ?? 'Signed in',
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
                              if (!employeeMode)
                                SettingsRow(
                                  icon: Icons.cloud_sync_outlined,
                                  iconColor: const Color(0xFF26A69A),
                                  label: 'Backup & Restore',
                                  value: serverConnected
                                      ? (canUseCloudBackups
                                          ? null
                                          : 'Sign in required')
                                      : 'Server not connected',
                                  onTap: () =>
                                      BackupRestoreScreen.show(context),
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
                          if (hasAccountSession)
                            SettingsGroup(
                              children: [
                                SettingsRow(
                                  icon: Icons.logout_rounded,
                                  iconColor: const Color(0xFFFFC857),
                                  label: 'Log Out',
                                  showChevron: false,
                                  onTap: () => _logOut(context),
                                ),
                                if (hasPermanentAccount && !employeeMode)
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

  Future<void> _logOut(BuildContext context) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: const Text(
          'Log Out',
          style: TextStyle(color: CelestialColors.textPrimary),
        ),
        content: const Text(
          'This will remove this device\'s local setup and return to onboarding. Your account and cloud data will not be deleted.',
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
              'Log Out',
              style: TextStyle(color: Color(0xFFFFC857)),
            ),
          ),
        ],
      ),
    );

    if (confirmed != true || !context.mounted) return;

    try {
      await AccountSessionService.instance.logOutAndReset(
        serverSyncProvider: context.read<ServerSyncProvider>(),
        homeProvider: context.read<HomeProvider>(),
        hubProvider: context.read<HubConnectionProvider>(),
        roomProvider: context.read<RoomProvider>(),
        preserveEmployeeMode: EmployeeModeService.instance.isActive,
      );
    } catch (error, stackTrace) {
      debugPrint('Log Out: Failed: $error');
      debugPrint('$stackTrace');
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Could not log out: $error')),
        );
      }
      return;
    }

    if (!context.mounted) return;
    _returnToOnboarding(context);
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
            debugPrint('Delete Account: Homes deleted via local fallback');
          } catch (e2) {
            debugPrint('Delete Account: Local fallback also failed: $e2');
          }
          await authService.signOut();
        }
      }

      if (!context.mounted) return;

      await AccountSessionService.instance.resetLocalSessionState(
        serverSyncProvider: context.read<ServerSyncProvider>(),
        homeProvider: homeProvider,
        hubProvider: context.read<HubConnectionProvider>(),
        roomProvider: context.read<RoomProvider>(),
        debugLabel: 'Delete Account',
      );

      AnalyticsService().logAccountDeleted();
      AnalyticsService().logSignOut();
      AnalyticsService().resetUser();

      debugPrint('Delete Account: Complete! Returning to onboarding...');

      if (!context.mounted) return;
      _returnToOnboarding(context);
    }
  }

  void _returnToOnboarding(BuildContext context) {
    if (!context.mounted) return;
    Navigator.of(context, rootNavigator: true)
        .popUntil((route) => route.isFirst);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      AuthGate.resetToOnboarding();
    });
  }
}
