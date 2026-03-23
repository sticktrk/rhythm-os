import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../../../main.dart';
import '../../../widgets/solar_orbit.dart'; // For CelestialColors
import '../../../widgets/settings_row.dart';
import '../../../providers/settings_provider.dart';
import '../../../providers/room_provider.dart';
import '../../../providers/home_provider.dart';
import '../../../providers/hub_connection_provider.dart';
import '../../../services/analytics_service.dart';
import '../../../services/auth_service.dart';
import '../../../services/settings_service.dart';
import '../../../services/hue_sse_storage.dart';
import '../dialogs/feedback_dialog.dart';
/// About section showing help and version info.
class AboutSection extends StatelessWidget {
  const AboutSection({super.key});

  @override
  Widget build(BuildContext context) {
    return Consumer<SettingsProvider>(
      builder: (context, settings, child) {
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            const SettingsSectionHeader(title: 'About'),
            SettingsGroup(
              children: [
                SettingsRow(
                  icon: Icons.chat_bubble_outline_rounded,
                  iconColor: const Color(0xFFFFC857),
                  label: 'Help & Feedback',
                  onTap: () => FeedbackDialog.show(context),
                ),
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
          ],
        );
      },
    );
  }
}

/// Standalone Delete Account section - always displayed at the very bottom.
class DeleteAccountSection extends StatelessWidget {
  const DeleteAccountSection({super.key});

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
      debugPrint('Delete Account: Starting account deletion...');

      // 1. Delete from server FIRST (before clearing local state)
      final authService = AuthService();
      if (authService.currentUser != null) {
        debugPrint('Delete Account: Deleting account from server...');
        try {
          await authService.deleteAccount();
          debugPrint('Delete Account: Server account deleted');
        } catch (e) {
          debugPrint('Delete Account: Edge Function failed: $e');
          // Fallback: delete homes directly via PostgREST (RLS allows owner delete).
          // CASCADE will remove hubs. The auth user remains but has no data,
          // so re-signing in gives a clean slate.
          try {
            final homeProvider = context.read<HomeProvider>();
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

      // 2. Disconnect hub connections
      try {
        final hubProvider = context.read<HubConnectionProvider>();
        hubProvider.disconnect();
        debugPrint('Delete Account: Hub connections disconnected');
      } catch (e) {
        debugPrint('Delete Account: Hub disconnect error (may not be provided): $e');
      }

      // 4. Clear in-memory RoomProvider state
      try {
        final roomProvider = context.read<RoomProvider>();
        await roomProvider.clearAllRooms();
        debugPrint('Delete Account: Room provider cleared');
      } catch (e) {
        debugPrint('Delete Account: Room clear error: $e');
      }

      // 5. Clear SSE storage
      try {
        await HueSseStorage.clearAll();
        debugPrint('Delete Account: SSE storage cleared');
      } catch (e) {
        debugPrint('Delete Account: SSE storage clear error: $e');
      }

      // 6. Clear all local settings via SettingsService
      await SettingsService.instance.clearAll();
      debugPrint('Delete Account: Settings service cleared');

      // 7. Clear HomeProvider state (homes and hubs)
      try {
        final homeProvider = context.read<HomeProvider>();
        await homeProvider.onUserSignOut();
        debugPrint('Delete Account: Home provider signed out (all local data cleared)');
      } catch (e) {
        debugPrint('Delete Account: Home provider error: $e');
      }

      // 8. Reset analytics identity
      AnalyticsService().logSignOut();
      AnalyticsService().resetUser();

      debugPrint('Delete Account: Complete! Returning to onboarding...');

      // 9. Pop ALL routes and trigger onboarding reset
      if (context.mounted) {
        Navigator.of(context, rootNavigator: true).popUntil((route) => route.isFirst);
        WidgetsBinding.instance.addPostFrameCallback((_) {
          AuthGate.resetToOnboarding();
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const SizedBox(height: 20),
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
      ],
    );
  }
}
