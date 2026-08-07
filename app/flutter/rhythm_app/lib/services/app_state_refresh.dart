import 'dart:async';

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' as sdk;
import '../api/hybrid_client.dart';
import '../config/platform_context.dart';
import '../providers/home_provider.dart';
import '../providers/room_page_provider.dart';
import '../providers/room_provider.dart';
import '../providers/server_sync_provider.dart';
import '../providers/hub_connection_provider.dart';
import 'account_cloud_sync_service.dart';
import 'analytics_service.dart';
import 'auth_service.dart';
import 'cloud_backup_service.dart';
import 'demo_server_api.dart';
import 'hue/hue_service_locator.dart';
import 'hue/demo_hue_bridge_service.dart';
import 'remote_access_service.dart';
import 'settings_service.dart';

/// Options for controlling what gets synced.
class SyncOptions {
  /// Sync rooms from configured hubs (Hue, HA, bridge).
  final bool rooms;

  /// Verify hub connections after sync.
  final bool connections;

  const SyncOptions({
    this.rooms = true,
    this.connections = false,
  });

  /// Full sync: rooms + connection verification.
  static const full = SyncOptions(rooms: true, connections: true);

  /// Rooms only: sync rooms without connection verification.
  static const roomsOnly = SyncOptions(rooms: true, connections: false);
}

/// Result of a sync operation.
class SyncResult {
  /// Whether the sync completed successfully.
  final bool success;

  /// Number of rooms synced (if room sync was performed).
  final int roomsSynced;

  /// Error message if sync failed.
  final String? error;

  const SyncResult({
    required this.success,
    this.roomsSynced = 0,
    this.error,
  });

  /// Successful sync result.
  const SyncResult.success({this.roomsSynced = 0})
      : success = true,
        error = null;

  /// Failed sync result.
  const SyncResult.failure(this.error)
      : success = false,
        roomsSynced = 0;
}

/// Unified app state sync entry point.
///
/// Call this after:
/// - Onboarding completion
/// - Settings sign-in
/// - Session recovery
/// - Hub configuration changes
class AppStateRefresh {
  /// Single entry point for all sync operations.
  static Future<SyncResult> sync(
    BuildContext context, {
    SyncOptions options = const SyncOptions(),
  }) async {
    if (!context.mounted) {
      return const SyncResult.failure('Context not mounted');
    }

    final homeProvider = Provider.of<HomeProvider>(context, listen: false);
    final api = Provider.of<RhythmApi>(context, listen: false);
    final roomProvider = Provider.of<RoomProvider>(context, listen: false);
    final hubConnection =
        Provider.of<HubConnectionProvider>(context, listen: false);
    final serverSync = Provider.of<ServerSyncProvider>(context, listen: false);
    int roomsSynced = 0;

    // Step 1: Ensure homes are loaded and create if needed
    await homeProvider.onUserSignIn();
    if (!context.mounted) {
      return const SyncResult.failure('Context not mounted after home init');
    }

    await AccountCloudSyncService.instance.syncHomeAndServerHubs(
      home: homeProvider.currentHome,
      hubs: homeProvider.currentHomeHubs,
      reason: 'app_state_refresh',
    );
    if (!context.mounted) {
      return const SyncResult.failure('Context not mounted after cloud sync');
    }

    _scheduleRemoteAccessAutoEnableIfAvailable(
      homeProvider: homeProvider,
      serverSync: serverSync,
    );
    await _restoreCloudAppSettingsIfAvailable(context, homeProvider);
    _scheduleCloudBackupIfAvailable(homeProvider);

    // Step 2: Sync location into API client for accurate solar calculations
    final home = homeProvider.currentHome;
    final loc = home?.location;
    if (loc != null) {
      if (api is HybridApiClient) {
        final now = DateTime.now();
        final dayOfYear = now.difference(DateTime(now.year, 1, 1)).inDays + 1;
        // Prefer stored IANA timezone; fall back to longitude-derived offset
        final tz = home?.timezone ?? _timezoneFromLongitude(loc.longitude);
        api.updateSolarConfig(
          latitude: loc.latitude,
          longitude: loc.longitude,
          timezone: tz,
          dayOfYear: dayOfYear,
        );
        // Recalculate solarNoonHour from the real coordinates
        try {
          await api.getCurveData(
            overrides: home?.curveConfig ?? defaultCurveConfig,
          );
        } catch (_) {}
        debugPrint(
            'AppStateRefresh: Synced location lat=${loc.latitude} lon=${loc.longitude} '
            'tz=$tz dayOfYear=$dayOfYear solarNoon=${api.solarNoonHour}');
      }
    }

    // Step 3: Room sync from configured hubs if requested
    if (options.rooms) {
      roomsSynced = await _syncRoomsFromHubs(
        homeProvider: homeProvider,
        roomProvider: roomProvider,
        serverSync: serverSync,
      );
      if (!context.mounted) {
        return SyncResult.success(roomsSynced: roomsSynced);
      }
    }

    // Step 4: Connection verification if requested
    if (options.connections && context.mounted) {
      await hubConnection.verifyConnection();
    }

    return SyncResult.success(roomsSynced: roomsSynced);
  }

  static void _scheduleRemoteAccessAutoEnableIfAvailable({
    required HomeProvider homeProvider,
    required ServerSyncProvider serverSync,
  }) {
    if (HueServiceLocator.isDemoMode) return;

    final home = homeProvider.currentHome;
    final serverHub = homeProvider.getFirstHubOfType(HubType.server);
    if (home == null || serverHub == null) return;

    RemoteAccessService.instance.scheduleAutoEnableForHub(
      home: home,
      serverHub: serverHub,
      saveHub: homeProvider.updateHub,
      resolveLatestHub: (homeId, hubId) =>
          _latestCurrentHomeHub(homeProvider, homeId, hubId),
      onEnabled: (_) => serverSync.connectIfAvailable(),
    );
  }

  static Hub? _latestCurrentHomeHub(
    HomeProvider homeProvider,
    String homeId,
    String hubId,
  ) {
    if (homeProvider.currentHome?.id != homeId) return null;
    for (final hub in homeProvider.currentHomeHubs) {
      if (hub.id == hubId) return hub;
    }
    return null;
  }

  static Future<void> _restoreCloudAppSettingsIfAvailable(
    BuildContext context,
    HomeProvider homeProvider,
  ) async {
    // The demo / Virtual Experience hub is a fake host (`demo.rhythm.local`);
    // hitting the cloud backup service for it would just throw a DNS error.
    if (HueServiceLocator.isDemoMode) return;
    final serverHub = homeProvider.getFirstHubOfType(HubType.server);
    if (serverHub == null || !CloudBackupService.instance.canUseCloudBackups) {
      return;
    }

    final scopeKey = RoomPageProvider.layoutScopeFor(
      home: homeProvider.currentHome,
      hubs: homeProvider.currentHomeHubs,
    );
    final hubKey = RoomPageProvider.hubLayoutKey(serverHub);
    final hubKeyAliases = RoomPageProvider.hubLayoutKeyAliases(serverHub);

    RoomPageProvider? roomPageProvider;
    try {
      roomPageProvider = Provider.of<RoomPageProvider>(
        context,
        listen: false,
      );
    } catch (_) {
      roomPageProvider = null;
    }

    try {
      final userId = AuthService().currentUserId;
      final hasUnsyncedLocalEdit = userId != null &&
          SettingsService.instance.isRoomPageLayoutCloudDirty(
            userId: userId,
            scopeKey: scopeKey,
          );
      if (hasUnsyncedLocalEdit) {
        CloudBackupService.instance.scheduleAppSettingsSync(
          serverHub: serverHub,
          home: homeProvider.currentHome,
          roomLayoutScopeKey: scopeKey,
          delay: Duration.zero,
          reason: 'app_state_refresh_retry',
        );
        return;
      }

      final snapshot =
          await CloudBackupService.instance.getSnapshotForCurrentUser();
      if (snapshot == null) return;

      final restored = await SettingsService.instance.applyCloudSettingsBundle(
        snapshot.appSettingsBundle,
        roomLayoutScopeKey: scopeKey,
        roomLayoutHubKey: hubKey,
        roomLayoutHubKeyAliases: hubKeyAliases,
        overwrite: true,
      );
      if (!restored) return;

      roomPageProvider?.setLayoutScope(scopeKey);
      roomPageProvider?.reloadLayout();
      debugPrint(
        'AppStateRefresh: Restored signed-in All Rooms layout from cloud',
      );
      final pages = SettingsService.instance.getRoomPageLayout(
            scopeKey: scopeKey,
          ) ??
          const <List<String>>[];
      unawaited(
        AnalyticsService().logRoomLayoutCloudSyncCompleted(
          direction: 'restore',
          outcome: 'succeeded',
          pageCount: pages.length,
          roomCount: pages.expand((page) => page).toSet().length,
        ),
      );
    } catch (error) {
      debugPrint('AppStateRefresh: Cloud app settings restore skipped: $error');
      unawaited(
        AnalyticsService().logRoomLayoutCloudSyncCompleted(
          direction: 'restore',
          outcome: 'failed',
          pageCount: 0,
          roomCount: 0,
        ),
      );
    }
  }

  static void _scheduleCloudBackupIfAvailable(HomeProvider homeProvider) {
    // Demo hub host (`demo.rhythm.local`) isn't reachable — skip cloud
    // backup capture entirely while in Virtual Experience / demo mode.
    if (HueServiceLocator.isDemoMode) return;
    final serverHub = homeProvider.getFirstHubOfType(HubType.server);
    if (serverHub == null || !CloudBackupService.instance.canUseCloudBackups) {
      return;
    }

    CloudBackupService.instance.scheduleCapture(
      serverHub: serverHub,
      home: homeProvider.currentHome,
      reason: 'app_state_sync',
    );
  }

  /// Sync rooms from all configured hubs.
  /// Returns the number of rooms synced.
  static Future<int> _syncRoomsFromHubs({
    required HomeProvider homeProvider,
    required RoomProvider roomProvider,
    required ServerSyncProvider serverSync,
  }) async {
    debugPrint('AppStateRefresh: Syncing rooms from hubs...');
    debugPrint('AppStateRefresh: currentHome=${homeProvider.currentHome?.id}');
    debugPrint(
        'AppStateRefresh: currentHomeHubs=${homeProvider.currentHomeHubs.length}');

    // HA addon: fetch rooms from the addon backend (auto-imported from HA areas)
    if (PlatformCtx.isHaAddon) {
      return _syncRoomsFromAddon(roomProvider);
    }

    // Demo mode: create a fake server hub and populate mock rooms locally
    if (HueServiceLocator.isDemoMode) {
      return _syncDemoRooms(
        homeProvider: homeProvider,
        roomProvider: roomProvider,
      );
    }

    // Rooms come from the server via the hello/poll cycle in ServerSyncProvider.
    // No client-side Hue sync needed — server is the single source of truth.
    try {
      if (serverSync.synced) {
        debugPrint(
            'AppStateRefresh: Server connected — rooms managed by hello/poll cycle');
      } else {
        debugPrint(
            'AppStateRefresh: Server not yet connected — rooms will arrive via hello');
      }
    } catch (_) {
      // ServerSyncProvider not available in this context
    }

    return 0;
  }

  /// Fetch rooms from the addon backend (same-origin GET /api/state).
  static Future<int> _syncRoomsFromAddon(RoomProvider roomProvider) async {
    try {
      final baseUrl = Uri.base.toString();
      debugPrint('AppStateRefresh: Addon sync baseUrl=$baseUrl');
      final api = sdk.RhythmConfigApi(baseUrl: baseUrl);
      final hello = await api.getState();
      debugPrint(
          'AppStateRefresh: Addon /api/state returned ${hello.nodes.length} nodes');

      final rooms = hello.nodes
          .where((r) =>
              r.id.isNotEmpty && r.name.isNotEmpty && r.kind.isLightAddressable)
          .map((r) => RoomDto(
                id: r.id,
                name: r.name,
                source: RoomSourceDto.homeAssistant,
                kind: _roomNodeKindFromSdk(r.kind),
                parentId: r.parentId,
                placement: _roomNodePlacementFromSdk(r.placement),
                deviceIds: r.kind.isLightDevice ? [r.id] : r.deviceIds,
                rhythmEnabled: r.rhythmEnabled,
                disabled: r.disabled,
                lightsOn: r.lightsOn ?? false,
                timeOffsetMinutes: r.timeOffset,
                brightnessOffset: r.brightnessOffset,
                curveConfig: null,
              ))
          .toList();

      if (rooms.isEmpty) return 0;

      if (rooms.isNotEmpty) {
        await roomProvider.addRoomsFromSource(
            RoomSourceDto.homeAssistant, rooms);
      }

      debugPrint('AppStateRefresh: Synced ${rooms.length} rooms from addon');
      return rooms.length;
    } catch (e) {
      debugPrint('AppStateRefresh: Addon room sync failed: $e');
      return 0;
    }
  }

  /// Populate demo rooms from the mock Hue bridge service.
  ///
  /// Creates a fake server hub so the UI sees a connected hub, then adds
  /// mock rooms with initial "on" state so demo users see warm room cards.
  static Future<int> _syncDemoRooms({
    required HomeProvider homeProvider,
    required RoomProvider roomProvider,
  }) async {
    debugPrint('AppStateRefresh: Syncing demo rooms');

    // Create a fake server hub if one doesn't exist yet
    final existingHub = homeProvider.getFirstHubOfType(HubType.server);
    if (existingHub == null) {
      await homeProvider.addServerHub(
        name: 'Demo Server',
        host: '127.0.0.1',
        port: 54448,
      );
      debugPrint('AppStateRefresh: Created fake demo server hub');
    }

    DemoServerApi.instance.ensureSeeded();
    final nodesById = {
      for (final node in DemoServerApi.instance.lightAddressableNodes)
        node.id: node,
    };
    final rooms = DemoServerApi.instance.buildRoomDtos();
    if (rooms.isEmpty) return 0;
    DemoHueBridgeService.instance.seedRooms(rooms);
    await roomProvider.addRoomsFromSource(RoomSourceDto.hue, rooms);

    // Apply initial "on" state so rooms appear alive
    for (final room in rooms) {
      final node = nodesById[room.id];
      await roomProvider.applyServerNodeState(
        room.id,
        rhythmEnabled: node?.rhythmEnabled ?? true,
        timeOffset: node?.timeOffset ?? 0,
        brightnessOffset: node?.brightnessOffset ?? 0,
        state: node?.state ?? sdk.RoomModeState.active,
        lightsOn: node?.lightsOn ?? true,
        brightness: node?.brightness ?? 75,
        kelvin: node?.kelvin ?? 3200,
      );
    }

    debugPrint('AppStateRefresh: Synced ${rooms.length} demo rooms');
    return rooms.length;
  }

  /// Derive a standard timezone offset from longitude.
  static String _timezoneFromLongitude(double longitude) {
    final offsetHours = (longitude / 15).round();
    if (offsetHours == 0) return 'Etc/GMT';
    // Etc/GMT uses inverted signs
    return offsetHours > 0 ? 'Etc/GMT-$offsetHours' : 'Etc/GMT+${-offsetHours}';
  }
}

RoomNodeKind _roomNodeKindFromSdk(sdk.RhythmNodeKind kind) => switch (kind) {
      sdk.RhythmNodeKind.lightDevice => RoomNodeKind.lightDevice,
      sdk.RhythmNodeKind.switchDevice => RoomNodeKind.switchDevice,
      sdk.RhythmNodeKind.motionSensor => RoomNodeKind.motionSensor,
      sdk.RhythmNodeKind.sensor => RoomNodeKind.sensor,
      sdk.RhythmNodeKind.button => RoomNodeKind.button,
      sdk.RhythmNodeKind.otherDevice => RoomNodeKind.otherDevice,
      sdk.RhythmNodeKind.room => RoomNodeKind.room,
    };

RoomNodePlacement? _roomNodePlacementFromSdk(
        sdk.RhythmNodePlacement? placement) =>
    switch (placement) {
      sdk.RhythmNodePlacement.hubDefault => RoomNodePlacement.hubDefault,
      sdk.RhythmNodePlacement.userOverride => RoomNodePlacement.userOverride,
      sdk.RhythmNodePlacement.standalone => RoomNodePlacement.standalone,
      null => null,
    };
