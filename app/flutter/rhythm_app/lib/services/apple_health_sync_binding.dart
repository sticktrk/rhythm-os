import '../backend/backend.dart';
import '../providers/home_provider.dart';
import '../providers/server_sync_provider.dart';
import 'apple_health_history_source.dart';
import 'apple_health_service.dart';
import 'apple_health_sync_controller.dart';
import 'auth_service.dart';
import 'hue/hue_service_locator.dart';
import 'server_identity.dart';
import 'settings_service.dart';

AppleHealthSyncController createAppleHealthSyncController(
    HomeProvider homes, ServerSyncProvider server) {
  AppleHealthScope? environment() {
    if (HueServiceLocator.isDemoMode) return null;
    final home = homes.currentHome;
    final hub = homes.activeServerHub;
    if (home == null || hub == null || hub.homeId != home.id) return null;
    final identity = normalizeServerIdentity(hub.serverInstanceId);
    if (serverIdentityKind(identity) != ServerIdentityKind.durable) return null;
    return AppleHealthScope(
        accountId: AuthService().currentUserId ?? 'local',
        homeId: home.id,
        serverId: identity!,
        nodeId: 'global');
  }

  final controller = AppleHealthSyncController(
    isConnected: () => server.connection.connected,
    writer: AppleHealthService(),
    currentEnvironment: environment,
    readPreference: () => SettingsService.instance.appleHealthPreference,
    writePreference: SettingsService.instance.setAppleHealthPreference,
    loadHistory: (scope, cutoff) async {
      if (!scope.sameEnvironment(environment())) {
        throw StateError('Home changed');
      }
      final authBackend =
          BackendProvider.isInitialized ? BackendProvider.instance.auth : null;
      final cloudPage = authBackend is SupabaseAuthBackend &&
              authBackend.client.auth.currentUser?.id == scope.accountId
          ? AppleHealthHistorySource.supabasePage(authBackend.client)
          : null;
      final local = server.connection.connected &&
              server.connectedServerInstanceId == scope.serverId &&
              server.connectedServerHub?.homeId == scope.homeId
          ? server.connection.runtimeApi
          : null;
      return AppleHealthHistorySource(
        cloudPage: cloudPage,
        localHistory: local == null
            ? null
            : (_) => local.getHistoryOrThrow(
                limit: 2000,
                source: 'switch'),
      ).load(scope, cutoff);
    },
  );
  controller.start([homes, server]);
  return controller;
}
