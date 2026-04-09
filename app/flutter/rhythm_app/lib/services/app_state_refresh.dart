import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' as sdk;
import '../api/hybrid_client.dart';
import '../config/platform_context.dart';
import '../providers/home_provider.dart';
import '../providers/room_provider.dart';
import '../providers/server_sync_provider.dart';
import '../providers/hub_connection_provider.dart';
import 'hue/hue_service_locator.dart';
import 'hue/demo_hue_bridge_service.dart';

/// Options for controlling what gets synced.
class SyncOptions {
  /// Sync rooms from configured hubs (Hue, HA, ESP32).
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
          await api.getCurveData();
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
          'AppStateRefresh: Addon /api/state returned ${hello.rooms.length} rooms');

      if (hello.rooms.isEmpty) return 0;

      final rooms = hello.rooms
          .where((r) => r.id.isNotEmpty && r.name.isNotEmpty)
          .map((r) => RoomDto.withSource(
              id: r.id, name: r.name, source: RoomSourceDto.homeAssistant))
          .toList();

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

    // Fetch mock rooms
    final rooms = await DemoHueBridgeService.instance.fetchRooms();
    if (rooms.isEmpty) return 0;
    await roomProvider.addRoomsFromSource(RoomSourceDto.hue, rooms);

    // Apply initial "on" state so rooms appear alive
    for (final room in rooms) {
      await roomProvider.applyServerRoomState(
        room.id,
        rhythmEnabled: true,
        timeOffset: 0,
        brightnessOffset: 0,
        state: sdk.RoomModeState.active,
        lightsOn: true,
        brightness: 75,
        kelvin: 3200,
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
