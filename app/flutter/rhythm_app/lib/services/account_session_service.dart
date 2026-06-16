import 'package:flutter/foundation.dart';

import '../providers/home_provider.dart';
import '../providers/hub_connection_provider.dart';
import '../providers/room_provider.dart';
import '../providers/server_sync_provider.dart';
import 'analytics_service.dart';
import 'auth_service.dart';
import 'hue/hue_service_locator.dart';
import 'hue_sse_storage.dart';
import 'settings_service.dart';
import 'virtual_experience_service.dart';

/// Coordinates account exit flows so logout and account deletion leave the app
/// in the same clean local state.
class AccountSessionService {
  AccountSessionService._();

  static final AccountSessionService instance = AccountSessionService._();

  Future<void> logOutAndReset({
    required ServerSyncProvider serverSyncProvider,
    required HomeProvider homeProvider,
    required HubConnectionProvider hubProvider,
    required RoomProvider roomProvider,
  }) async {
    debugPrint('Log Out: Signing out...');
    final authService = AuthService();
    if (authService.currentUser != null) {
      await authService.signOut();
    } else {
      HueServiceLocator.setDemoMode(false);
    }

    await resetLocalSessionState(
      serverSyncProvider: serverSyncProvider,
      homeProvider: homeProvider,
      hubProvider: hubProvider,
      roomProvider: roomProvider,
      debugLabel: 'Log Out',
    );

    AnalyticsService().logSignOut();
    AnalyticsService().resetUser();
  }

  Future<void> resetLocalSessionState({
    required ServerSyncProvider serverSyncProvider,
    required HomeProvider homeProvider,
    required HubConnectionProvider hubProvider,
    required RoomProvider roomProvider,
    required String debugLabel,
  }) async {
    debugPrint('$debugLabel: Resetting local session state...');

    if (VirtualExperienceService.instance.isActive) {
      VirtualExperienceService.instance.exit();
    } else {
      HueServiceLocator.setDemoMode(false);
    }

    try {
      serverSyncProvider.connection.disconnect();
      debugPrint('$debugLabel: Server connection disconnected');
    } catch (error) {
      debugPrint('$debugLabel: Server disconnect error: $error');
    }

    try {
      await hubProvider.disconnect();
      debugPrint('$debugLabel: Hub connections disconnected');
    } catch (error) {
      debugPrint('$debugLabel: Hub disconnect error: $error');
    }

    try {
      await roomProvider.clearAllRooms();
      debugPrint('$debugLabel: Room provider cleared');
    } catch (error) {
      debugPrint('$debugLabel: Room clear error: $error');
    }

    try {
      await HueSseStorage.clearAll();
      debugPrint('$debugLabel: SSE storage cleared');
    } catch (error) {
      debugPrint('$debugLabel: SSE storage clear error: $error');
    }

    try {
      await homeProvider.onUserSignOut();
      debugPrint('$debugLabel: Home provider signed out');
    } catch (error) {
      debugPrint('$debugLabel: Home provider error: $error');
    }

    await SettingsService.instance.clearAll();
    debugPrint('$debugLabel: Settings service cleared');
  }
}
