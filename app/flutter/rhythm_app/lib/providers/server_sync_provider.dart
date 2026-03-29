/// Server sync provider — bridges SDK connection with app providers.
///
/// Handles:
/// - Auto-connect to server when hub exists
/// - Hello reconciliation (accept server rooms + config, push location)
/// - Server-driven room sync (server discovers rooms from hub)
/// - rhythm_state events → update RoomProvider
/// - Location pushes from app → server
library;

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter_timezone/flutter_timezone.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../services/hue/hue_service_locator.dart';
import 'home_provider.dart';
import 'room_provider.dart';

/// Whether a timezone string looks like a proper IANA name (contains '/').
/// Abbreviations like "EST", "PST" don't handle DST transitions.
bool _isIanaTimezone(String? tz) => tz != null && tz.contains('/');

/// Syncs app state with a server (ESP32, rhythm-server, addon) via
/// [RhythmConnection] from the SDK.
///
/// This provider is the glue between [RhythmConnection], [RoomProvider],
/// and [HomeProvider]. It manages:
///
/// 1. **Auto-connect**: When a server hub exists, connects the HTTP client.
/// 2. **Hello reconciliation**: On connect, accepts server's rooms and config
///    as authoritative and pushes any location differences.
/// 3. **Server-driven room sync**: When hub pairing changes, triggers
///    server-side re-discovery via `POST /api/sync`.
/// 4. **Rhythm state sync**: Listens for server rhythm_state events and updates
///    RoomProvider (server is authoritative for rhythm state).
class ServerSyncProvider extends ChangeNotifier {
  final RhythmConnection _connection;
  final RoomProvider _roomProvider;
  final HomeProvider _homeProvider;

  StreamSubscription<RhythmHello>? _helloSub;
  StreamSubscription<RhythmRoomState>? _rhythmStateSub;
  StreamSubscription<String>? _hubEventSub;
  StreamSubscription<RoomSourceDto>? _sourceChangedSub;
  StreamSubscription<RhythmMotionTimer>? _motionTimerSub;
  StreamSubscription<void>? _newRoomsSub;
  StreamSubscription<Map<String, dynamic>>? _triageChangedSub;
  StreamSubscription<RhythmConnectionState>? _connectionStateSub;

  /// Suppresses push-back when receiving rhythm_state from server.
  bool _receivingFromServer = false;

  /// Prevents redundant sync triggers during hello processing.
  bool _isProcessingHello = false;

  /// Suppresses the next source-change sync after hello populates rooms.
  ///
  /// The `_isProcessingHello` guard doesn't catch source-change events because
  /// the broadcast stream delivers them asynchronously (after the flag resets).
  /// This flag bridges that gap — set in `_onHello`, cleared in
  /// `_onSourceRoomsChanged`.
  bool _suppressNextSourceSync = false;

  /// Last known server hub for auto-connect.
  Hub? _serverHub;

  /// All hub infos from the last server hello: [{type, address, connected}, ...].
  List<Map<String, dynamic>> _lastHubInfos = [];

  /// Cached rooms from the last RhythmHello (includes typed devices from registry).
  List<RhythmRoom> _helloRooms = [];

  /// Previous connection state for detecting transitions.
  RhythmConnectionState _previousConnectionState = RhythmConnectionState.disconnected;

  /// Tracks the last poll time for debouncing [fullRefresh] and [pollNow].
  DateTime _lastPollTime = DateTime.fromMillisecondsSinceEpoch(0);

  /// Firmware version reported by server in the hello message.
  String _firmwareVersion = '0.0.0';

  /// Platform type reported by server ("desktop" or "embedded").
  String _serverPlatformType = 'desktop';

  /// Deployment context reported by server ("ha_addon", "server", "embedded", etc.).
  String _serverPlatformContext = 'server';

  /// Whether power-save mode is active on the server.
  bool _powerSave = false;

  /// Soft-off brightness percentage (1-50) from server settings.
  int _softOffBrightness = 1;

  /// Rhythm update interval in seconds from server settings.
  int _rhythmIntervalSecs = 60;

  /// Pending triage counts from SSE triage_changed events.
  int _triagePendingCount = 0;
  int _triagePendingDevices = 0;
  int _triagePendingRooms = 0;

  /// Whether we're currently connected and synced.
  bool get synced => _connection.connected;

  /// Whether the server can dispatch room actions.
  bool get canDispatchActions => _connection.connected || HueServiceLocator.isDemoMode;

  /// Connection state of the underlying connection.
  RhythmConnectionState get connectionState => _connection.connectionState;

  /// Firmware version reported by server.
  String get firmwareVersion => _firmwareVersion;

  /// Platform type reported by server ("desktop" or "embedded").
  String get serverPlatformType => _serverPlatformType;

  /// Deployment context reported by server ("ha_addon", "server", "embedded", etc.).
  String get serverPlatformContext => _serverPlatformContext;

  /// Whether the connected server is embedded (constrained sockets/memory).
  bool get isEmbeddedServer => _serverPlatformType == 'embedded';

  /// Whether power-save mode is active on the server.
  bool get powerSave => _powerSave;

  /// Soft-off brightness percentage (1-50).
  int get softOffBrightness => _softOffBrightness;
  set softOffBrightness(int value) {
    if (_softOffBrightness != value) {
      _softOffBrightness = value;
      notifyListeners();
    }
  }

  /// Rhythm update interval in seconds.
  int get rhythmIntervalSecs => _rhythmIntervalSecs;

  /// Total pending triage entries (devices + rooms).
  int get triagePendingCount => _triagePendingCount;

  /// Pending device merge entries.
  int get triagePendingDevices => _triagePendingDevices;

  /// Pending room binding entries.
  int get triagePendingRooms => _triagePendingRooms;

  /// Primary hub info from last server hello (backward compat).
  Map<String, dynamic> get serverHubInfo =>
      _lastHubInfos.isNotEmpty ? _lastHubInfos.first : {};

  /// All hub infos from last server hello.
  List<Map<String, dynamic>> get serverHubInfos => _lastHubInfos;

  /// Whether no hubs are configured on the server.
  bool get hasNoHubConfigured =>
      _lastHubInfos.isEmpty ||
      _lastHubInfos.every((h) => h['type'] == 'none' || h['type'] == null);

  /// Hub types currently connected on the server.
  Set<String> get connectedHubTypes => _lastHubInfos
      .where((h) => h['connected'] == true && h['type'] != 'none')
      .map((h) => h['type'] as String)
      .toSet();

  /// Hub types configured on the server (connected or not).
  Set<String> get configuredHubTypes => _lastHubInfos
      .where((h) => h['type'] != null && h['type'] != 'none')
      .map((h) => h['type'] as String)
      .toSet();

  /// Whether the server needs a location (has a Hue hub and runs as HA addon).
  bool get serverNeedsLocation {
    if (!configuredHubTypes.contains('hue')) return false;
    return _serverPlatformContext == 'ha_addon';
  }

  /// Rooms from the last server hello (with typed devices from the registry).
  List<RhythmRoom> get helloRooms => _helloRooms;

  /// Number of typed lights for a room (0 until lights are typed in the backend).
  int lightCountForRoom(String roomId) {
    return _helloRooms.where((r) => r.id == roomId).firstOrNull?.lightCount ?? 0;
  }

  /// All typed devices for a room (lights, buttons, motion sensors).
  List<RhythmDevice> devicesForRoom(String roomId) {
    return _helloRooms.where((r) => r.id == roomId).firstOrNull?.devices ?? [];
  }

  /// Human-readable device summary for a room (e.g. "4 lights, 2 buttons").
  String deviceSummaryForRoom(String roomId) {
    return _helloRooms.where((r) => r.id == roomId).firstOrNull?.deviceSummary ?? '';
  }

  /// All rooms grouped by hub type (for hub debug sections).
  Map<String, List<RhythmRoom>> get roomsByHubType {
    final result = <String, List<RhythmRoom>>{};
    for (final room in _helloRooms) {
      final key = room.hubType ?? 'unknown';
      (result[key] ??= []).add(room);
    }
    return result;
  }

  /// All unique devices for a hub type, de-duplicated and sorted lights->buttons->motion.
  List<RhythmDevice> devicesForHub(String hubType) {
    final seen = <String>{};
    final devices = <RhythmDevice>[];
    for (final room in _helloRooms.where((r) => r.hubType == hubType)) {
      for (final device in room.devices) {
        if (seen.add(device.id)) {
          devices.add(device);
        }
      }
    }
    devices.sort((a, b) {
      const order = {RhythmDeviceType.light: 0, RhythmDeviceType.button: 1, RhythmDeviceType.motion: 2};
      return (order[a.type] ?? 3).compareTo(order[b.type] ?? 3);
    });
    return devices;
  }

  /// Summary string for a hub type (e.g. "12 lights, 4 buttons across 5 rooms").
  String deviceSummaryForHub(String hubType) {
    final devices = devicesForHub(hubType);
    final rooms = roomsByHubType[hubType] ?? [];
    final l = devices.where((d) => d.type == RhythmDeviceType.light).length;
    final b = devices.where((d) => d.type == RhythmDeviceType.button).length;
    final m = devices.where((d) => d.type == RhythmDeviceType.motion).length;
    final parts = <String>[];
    if (l > 0) parts.add('$l light${l > 1 ? 's' : ''}');
    if (b > 0) parts.add('$b button${b > 1 ? 's' : ''}');
    if (m > 0) parts.add('$m sensor${m > 1 ? 's' : ''}');
    if (parts.isEmpty) return 'No devices';
    final roomCount = rooms.length;
    return '${parts.join(', ')} across $roomCount room${roomCount > 1 ? 's' : ''}';
  }

  /// The underlying SDK connection (for SSE debug props, ping, etc.).
  RhythmConnection get connection => _connection;

  /// The server API client (available after connect).
  RhythmServerApi get api => _connection.api;

  ServerSyncProvider({
    required RhythmConnection connection,
    required RoomProvider roomProvider,
    required HomeProvider homeProvider,
  })  : _connection = connection,
        _roomProvider = roomProvider,
        _homeProvider = homeProvider {
    // Listen for connection events
    _helloSub = _connection.helloEvents.listen(_onHello);
    _rhythmStateSub = _connection.rhythmStateEvents.listen(_onRhythmState);
    _hubEventSub = _connection.hubEvents.listen(_onHubEvent);
    _motionTimerSub = _connection.motionTimerEvents.listen(_onMotionTimer);
    _newRoomsSub = _connection.newRoomsDetected.listen(_onNewRoomsDetected);
    _triageChangedSub = _connection.triageChangedEvents.listen(_onTriageChanged);

    // Listen for room source changes (Hue pairing, re-sync, disconnect)
    _sourceChangedSub =
        _roomProvider.onSourceRoomsChanged.listen(_onSourceRoomsChanged);

    // Listen for connection state changes
    _connectionStateSub =
        _connection.connectionStateStream.listen(_onConnectionStateChanged);
  }

  /// Connect to server if a hub is available.
  ///
  /// Called from the ProxyProvider update. Since ProxyProvider fires on
  /// every dependency change (including frequent RoomProvider notifies),
  /// this method is guarded to only act when the hub config actually changes.
  void connectIfAvailable() {
    // Demo mode: set the server hub reference so UI sees a hub,
    // but don't actually connect to the fake 127.0.0.1 host.
    if (HueServiceLocator.isDemoMode) {
      final hubs = _homeProvider.currentHomeHubs;
      final serverHub = hubs
          .where((h) => h.type == HubType.server)
          .firstOrNull;
      if (serverHub != null && _serverHub?.id != serverHub.id) {
        _serverHub = serverHub;
        notifyListeners();
      }
      return;
    }

    final hubs = _homeProvider.currentHomeHubs;
    final serverHub = hubs
        .where((h) => h.type == HubType.server)
        .firstOrNull;

    if (serverHub != null) {
      if (serverHub.id == _serverHub?.id &&
          serverHub.endpoint.host == _serverHub?.endpoint.host &&
          serverHub.endpoint.port == _serverHub?.endpoint.port) {
        return; // Same hub, no change
      }
      debugPrint('ServerSync: connectIfAvailable — connecting to ${serverHub.endpoint.host}:${serverHub.endpoint.port}');
      _serverHub = serverHub;
      // Defer all side-effects to avoid notifyListeners during ProxyProvider build phase
      final host = serverHub.endpoint.host;
      final port = serverHub.endpoint.port;
      Future.microtask(() async {
        _roomProvider.clearTransientState();
        _connection.connect(host, port: port);
      });
    } else if (_serverHub != null) {
      debugPrint('ServerSync: connectIfAvailable — hub removed, disconnecting');
      _serverHub = null;
      Future.microtask(() async {
        await _roomProvider.clearAllRooms();
        _connection.disconnect();
      });
    }
  }

  /// Trigger a full reconnect (for pull-to-refresh).
  ///
  /// Re-fetches `GET /api/state` and emits a fresh hello so the UI gets the
  /// complete picture (devices, config, rooms, sensors, settings).
  ///
  /// Skips if called within 2 seconds of the last refresh to protect the server
  /// from rapid pull-refreshes.
  Future<void> fullRefresh() async {
    final now = DateTime.now();
    if (now.difference(_lastPollTime).inSeconds < 2) return;
    _lastPollTime = now;
    await _connection.reconnect();
  }

  /// Trigger an immediate lightweight poll (rooms/state only).
  Future<void> pollNow() async {
    final now = DateTime.now();
    if (now.difference(_lastPollTime).inSeconds < 2) return;
    _lastPollTime = now;
    await _connection.pollNow();
  }

  // ============================================================================
  // Event handlers (Server → App)
  // ============================================================================

  /// Handle hello from server — accept rooms and reconcile config.
  void _onHello(RhythmHello hello) {
    debugPrint('ServerSync: Hello received with ${hello.rooms.length} rooms, version=${hello.version}');
    debugPrint('ServerSync: Server global config: ${hello.config}');
    debugPrint('ServerSync: Server location: ${hello.location}');
    for (final r in hello.rooms) {
      debugPrint('ServerSync: Server room "${r.name}" rhythm=${r.rhythmEnabled} offset=${r.timeOffset} softOff=${r.softOff}');
    }
    _firmwareVersion = hello.version;
    _serverPlatformType = hello.platformType;
    _serverPlatformContext = hello.platformContext;
    _powerSave = hello.settings?.powerSave ?? false;
    _softOffBrightness = hello.settings?.softOffBrightness ?? 1;
    _rhythmIntervalSecs = hello.settings?.rhythmIntervalSecs ?? 60;
    _helloRooms = hello.rooms;
    _lastHubInfos = hello.hubs;

    // Bootstrap countdown timer from server's last tick timestamp
    if (hello.lastTickEpochMs != null) {
      final lastTick = DateTime.fromMillisecondsSinceEpoch(hello.lastTickEpochMs!);
      for (final room in hello.rooms) {
        if (room.id.isNotEmpty && room.rhythmEnabled) {
          _roomProvider.setLastTickTime(room.id, lastTick);
        }
      }
    }

    _isProcessingHello = true;
    // Only suppress the next source-change event if we're actually going to
    // add rooms (which triggers addRoomsFromSource → onSourceRoomsChanged).
    // If the server has 0 rooms, no event fires, and a stale suppress flag
    // would eat the next real event (e.g. Hue pairing).
    _suppressNextSourceSync = hello.rooms.isNotEmpty;
    try {
      // 1. Accept server rooms as authoritative
      _acceptServerRooms(hello.rooms, hello.hub);

      // 2. Reconcile motion sensors — mark rooms that have sensors,
      //    unmark rooms that lost their sensors since last hello
      final serverSensorRooms = <String>{};
      for (final room in hello.rooms) {
        if (room.id.isNotEmpty && room.hasMotionSensor) {
          _roomProvider.markRoomHasSensor(room.id);
          serverSensorRooms.add(room.id);
        }
      }
      _roomProvider.reconcileMotionSensors(serverSensorRooms);

      // 3. Accept server config as authoritative, push location if different
      _acceptServerConfig(hello.config);
      _pushLocationIfUnset(hello.location);
    } finally {
      _isProcessingHello = false;
    }

    // Fetch initial triage count (non-blocking).
    _connection.api.getTriageCount().then((data) {
      if (data != null) _onTriageChanged(data);
    });

    notifyListeners();
  }

  /// Accept rooms from the server as the authoritative source.
  ///
  /// The server discovers rooms from connected hubs. Each room carries its
  /// own `hub_type`, so we group by source and add each group atomically.
  void _acceptServerRooms(List<RhythmRoom> serverRooms, Map<String, dynamic> hubInfo) {
    // Filter out empty rooms (e.g. from stale server-side rooms.json)
    final validRooms = serverRooms.where((r) => r.id.isNotEmpty).toList();
    if (validRooms.isEmpty) {
      debugPrint('ServerSync: Server has no rooms — clearing local rooms');
      _roomProvider.clearAllRooms();
      return;
    }

    // Fallback source from the primary hub (backward compat for rooms
    // without per-room hub_type).
    final fallbackHubType = hubInfo['type'] as String?;
    final fallbackSource = switch (fallbackHubType) {
      'hue' => RoomSourceDto.hue,
      'homeassistant' || 'home_assistant' => RoomSourceDto.homeAssistant,
      'esp32' => RoomSourceDto.esp32,
      _ => RoomSourceDto.unknown,
    };

    // Group rooms by their per-room hub_type (multi-hub aware).
    final grouped = <RoomSourceDto, List<RhythmRoom>>{};
    for (final sr in validRooms) {
      final source = switch (sr.hubType) {
        'hue' => RoomSourceDto.hue,
        'homeassistant' || 'home_assistant' => RoomSourceDto.homeAssistant,
        'esp32' => RoomSourceDto.esp32,
        _ => fallbackSource,
      };
      (grouped[source] ??= []).add(sr);
    }
    debugPrint('ServerSync: Accepting ${validRooms.length} rooms across ${grouped.length} source(s): ${grouped.entries.map((e) => '${e.key}=${e.value.length}').join(', ')}');

    // The server is authoritative for ALL rooms. Remove any local rooms
    // from sources not present in the server's list.
    final serverRoomIds = validRooms.map((r) => r.id).toSet();
    for (final otherSource in RoomSourceDto.values) {
      if (grouped.containsKey(otherSource)) continue;
      final stale = _roomProvider.getRoomsBySource(otherSource)
          .where((r) => !serverRoomIds.contains(r.id))
          .toList();
      if (stale.isNotEmpty) {
        debugPrint('ServerSync: Removing ${stale.length} stale room(s) from source=$otherSource');
        for (final room in stale) {
          _roomProvider.removeRoom(room.id);
        }
      }
    }

    // Add each source group atomically (preserves user state like
    // rhythmEnabled, timeOffset, curveConfig for rooms that already exist).
    for (final entry in grouped.entries) {
      final source = entry.key;
      final rooms = <RoomDto>[];
      for (final sr in entry.value) {
        rooms.add(RoomDto.raw(
          id: sr.id,
          name: sr.name,
          source: source,
          deviceIds: sr.devices
              .where((d) => d.type == RhythmDeviceType.light)
              .map((d) => d.id)
              .toList(),
          rhythmEnabled: sr.rhythmEnabled,
          disabled: sr.disabled,
          lightsOn: sr.lightsOn ?? false,
          timeOffsetMinutes: sr.timeOffset,
          brightnessOffset: sr.brightnessOffset,
        ));
      }
      _roomProvider.addRoomsFromSource(source, rooms);
    }

    // Apply runtime state from server atomically (rhythmEnabled, timeOffset,
    // brightnessOffset, softOff) — single save + notify per room.
    _receivingFromServer = true;
    try {
      for (final sr in validRooms) {
        if (_roomProvider.getRoom(sr.id) != null) {
          _roomProvider.applyServerRoomState(
            sr.id,
            rhythmEnabled: sr.rhythmEnabled,
            timeOffset: sr.timeOffset,
            brightnessOffset: sr.brightnessOffset,
            softOff: sr.softOff,
            lightsOn: sr.lightsOn,
            brightness: sr.brightness,
            kelvin: sr.kelvin,
          );
        }
      }
    } finally {
      _receivingFromServer = false;
    }
  }

  /// Handle source rooms changed (Hue pairing, re-sync, disconnect).
  ///
  /// Hub credentials are only pushed via explicit user action (hub picker,
  /// configurator screens, server settings) — not automatically here.
  void _onSourceRoomsChanged(RoomSourceDto source) {
    debugPrint('ServerSync: onSourceRoomsChanged($source), connected=${_connection.connected}, processingHello=$_isProcessingHello, suppressSync=$_suppressNextSourceSync');
    if (!_connection.connected || _isProcessingHello) return;
    if (_suppressNextSourceSync) {
      _suppressNextSourceSync = false;
      debugPrint('ServerSync: Suppressing source sync (hello just populated rooms)');
      return;
    }
  }

  /// Handle rhythm_state from server (button event, tick, poll diff).
  void _onRhythmState(RhythmRoomState state) {
    _receivingFromServer = true;
    try {
      if (_roomProvider.getRoom(state.roomId) == null) return;
      _roomProvider.applyServerRoomState(
        state.roomId,
        rhythmEnabled: state.rhythmEnabled,
        timeOffset: state.timeOffset,
        brightnessOffset: state.brightnessOffset,
        softOff: state.softOff,
        lightsOn: state.lightsOn,
        brightness: state.brightness,
        kelvin: state.kelvin,
        tick: state.tick,
      );
    } finally {
      _receivingFromServer = false;
    }
  }

  /// Handle motion timer updates from server.
  void _onMotionTimer(RhythmMotionTimer event) {
    // Any motion event (even clearing) means this room has a sensor.
    _roomProvider.markRoomHasSensor(event.roomId);

    // Idle sensor (no active motion, no countdown) — just mark presence.
    final isIdle = !event.motionActive && event.remainingSecs == null;
    if (event.isCleared || isIdle) {
      _roomProvider.clearMotionTimer(event.roomId);
    } else {
      _roomProvider.updateMotionTimer(
        event.roomId,
        MotionTimerInfo(
          motionActive: event.motionActive,
          motionOwned: event.motionOwned,
          remainingSecs: event.remainingSecs,
          timeoutSecs: event.timeoutSecs,
          receivedAt: DateTime.now(),
        ),
      );
    }
  }

  /// Handle new rooms detected in poll — trigger a full re-hello.
  void _onNewRoomsDetected(void _) {
    debugPrint('ServerSync: New rooms detected in poll — triggering re-hello');
    _connection.reconnect();
  }

  /// Handle hub lifecycle events from server.
  ///
  /// When a hub connects, trigger a re-hello to pick up newly discovered
  /// rooms. When a hub disconnects, just update the UI.
  void _onHubEvent(String event) {
    debugPrint('ServerSync: hub_status=$event');
    // Update the primary hub's connected state for backward compat.
    if (_lastHubInfos.isNotEmpty) {
      _lastHubInfos = [
        {..._lastHubInfos.first, 'connected': event == 'connected'},
        ..._lastHubInfos.skip(1),
      ];
    }
    notifyListeners();

    // A hub just connected — re-fetch full state to pick up new rooms.
    if (event == 'connected') {
      debugPrint('ServerSync: Hub connected — triggering re-hello for room sync');
      _connection.reconnect();
    }
  }

  /// Handle connection state transitions — reset metadata but keep rooms.
  ///
  /// Rooms are preserved so the UI doesn't flicker during reconnect.
  /// They'll be re-synced via hello on reconnect. Widgets can check
  /// [connectionState] to show a reconnecting indicator.
  void _onTriageChanged(Map<String, dynamic> data) {
    final count = (data['total'] as num?)?.toInt() ??
        (data['pending_count'] as num?)?.toInt() ?? 0;
    final devices = (data['devices'] as num?)?.toInt() ??
        (data['pending_devices'] as num?)?.toInt() ?? 0;
    final rooms = (data['rooms'] as num?)?.toInt() ??
        (data['pending_rooms'] as num?)?.toInt() ?? 0;
    if (_triagePendingCount != count ||
        _triagePendingDevices != devices ||
        _triagePendingRooms != rooms) {
      _triagePendingCount = count;
      _triagePendingDevices = devices;
      _triagePendingRooms = rooms;
      notifyListeners();
    }
  }

  void _onConnectionStateChanged(RhythmConnectionState current) {
    final previous = _previousConnectionState;
    _previousConnectionState = current;

    if (current != previous &&
        (current == RhythmConnectionState.disconnected ||
         current == RhythmConnectionState.reconnecting)) {
      debugPrint('ServerSync: Connection lost ($previous → $current) — resetting metadata, keeping rooms');
      _firmwareVersion = '0.0.0';
      _serverPlatformType = 'desktop';
      _serverPlatformContext = 'server';
      _powerSave = false;
      _helloRooms = [];
      _lastHubInfos = [];
      _softOffBrightness = 1;
      _rhythmIntervalSecs = 60;
      _triagePendingCount = 0;
      _triagePendingDevices = 0;
      _triagePendingRooms = 0;
    }

    notifyListeners();
  }

  // ============================================================================
  // Push triggers (App → Server)
  // ============================================================================

  /// Dispatch a room action through the server (server controls lights).
  ///
  /// Returns true if dispatched to server, false if not connected.
  /// On success, applies the server's response immediately for fast convergence.
  bool dispatchAction(String roomId, String action) {
    if (HueServiceLocator.isDemoMode) return true; // optimistic UI already applied
    if (!_connection.connected) return false;
    _connection.api.roomAction(roomId: roomId, action: action).then((serverState) {
      if (serverState != null) _onRhythmState(serverState);
    });
    return true;
  }

  /// Dispatch multiple room actions in a single batch request.
  ///
  /// Returns true if dispatched to server, false if not connected.
  /// On success, applies each returned state for fast convergence.
  Future<bool> dispatchBatchActions(List<({String roomId, String action})> actions) async {
    if (!_connection.connected || actions.isEmpty) return false;
    final states = await _connection.api.roomActionBatch(actions);
    for (final state in states) {
      _onRhythmState(state);
    }
    return true;
  }

  /// Set room brightness through the server.
  ///
  /// Returns true if dispatched to server, false if not connected.
  bool dispatchBrightness(String roomId, int brightness) {
    if (HueServiceLocator.isDemoMode) {
      _roomProvider.applyServerRoomState(
        roomId,
        rhythmEnabled: _roomProvider.getRoom(roomId)?.rhythmEnabled ?? true,
        timeOffset: 0,
        brightnessOffset: 0,
        softOff: false,
        lightsOn: true,
        brightness: brightness,
        kelvin: _roomProvider.getKelvin(roomId),
      );
      return true;
    }
    if (!_connection.connected) return false;
    _connection.api.roomBrightness(roomId: roomId, brightness: brightness);
    return true;
  }

  /// Push room preferences to the server (user-state only, no topology).
  void pushRoomPreferences(String roomId, {bool? rhythmEnabled, bool? disabled, bool? softOff}) {
    if (HueServiceLocator.isDemoMode) return; // optimistic UI already applied
    if (!_connection.connected || _receivingFromServer) return;
    debugPrint('ServerSync: pushRoomPreferences $roomId rhythmEnabled=$rhythmEnabled disabled=$disabled softOff=$softOff');
    _connection.api.roomPreferencesSet(
      roomId: roomId,
      rhythmEnabled: rhythmEnabled,
      disabled: disabled,
      softOff: softOff,
    );
  }

  /// Push room preferences for multiple rooms in a single batch request.
  ///
  /// Avoids per-room socket overhead on constrained servers (ESP32).
  void pushBatchRoomPreferences(List<Map<String, dynamic>> items) {
    if (!_connection.connected || _receivingFromServer || items.isEmpty) return;
    debugPrint('ServerSync: pushBatchRoomPreferences (${items.length} rooms)');
    _connection.api.roomPreferencesBatchSet(items);
  }

  /// Reset all on-rooms to their current adaptive curve position via server.
  ///
  /// Returns room states for immediate UI convergence.
  Future<List<RhythmRoomState>> dispatchFixMyLights() async {
    if (HueServiceLocator.isDemoMode) {
      // Reset all on-rooms locally
      for (final room in _roomProvider.rooms) {
        if (room.lightsOn) {
          await _roomProvider.applyServerRoomState(
            room.id,
            rhythmEnabled: true,
            timeOffset: 0,
            brightnessOffset: 0,
            softOff: false,
            lightsOn: true,
            brightness: 75,
            kelvin: 3200,
          );
        }
      }
      _roomProvider.bumpResetGeneration();
      return [];
    }
    if (!_connection.connected) return [];
    final states = await _connection.api.fixMyLights();
    for (final state in states) {
      _onRhythmState(state);
    }
    if (states.isNotEmpty) {
      _roomProvider.bumpResetGeneration();
    }
    return states;
  }

  /// Trigger server-side room discovery from the connected hub.
  ///
  /// After sync completes, does a full refresh to pick up new rooms/devices.
  Future<void> triggerServerSync() async {
    if (!_connection.connected) return;
    debugPrint('ServerSync: Triggering server-side sync');
    await _connection.api.triggerSync();
    await _connection.reconnect();
  }

  /// Push location to the server.
  void pushLocation({
    required double lat,
    required double lon,
    double? utcOffset,
    String? timezoneName,
  }) {
    if (!_connection.connected) return;
    _connection.api.locationSet(lat: lat, lon: lon, utcOffset: utcOffset, timezoneName: timezoneName);
  }

  /// Push per-room motion timeout to the server.
  void pushMotionTimeout(String roomId, int timeoutSecs) {
    if (!_connection.connected) return;
    _connection.api.motionTimeoutSet(roomId: roomId, timeoutSecs: timeoutSecs);
  }

  // ============================================================================
  // Config/location reconciliation
  // ============================================================================

  /// Push hub credentials for a specific room source.
  ///
  /// Called after Hue pairing or other hub configuration changes so the
  /// server gets the credentials it needs to connect to the hub.
  void pushHubCredentials(RoomSourceDto source) {
    if (!_connection.connected) return;
    _pushHubCredentialsForSource(source);
  }

  /// Tell the addon to auto-configure HA using its SUPERVISOR_TOKEN.
  /// Sends empty credentials — server fills them from its environment.
  Future<bool> configureAddonHaHub() async {
    if (!_connection.connected) return false;
    await _connection.api.hubCredentials(
      hubType: 'homeassistant',
      address: '',
      credentials: {},
    );
    _connection.reconnect(); // Re-fetch state with new rooms
    return true;
  }

  /// Tell the server to disconnect ALL hubs — clears all credentials, runtimes,
  /// and rooms.
  Future<void> disconnectHub() async {
    if (!_connection.connected) return;
    debugPrint('ServerSync: Sending hub disconnect to server');
    await _connection.api.hubDisconnect();
  }

  /// Disconnect a single hub by type + address.
  Future<void> disconnectOneHub(String hubType, String address) async {
    if (!_connection.connected) return;
    debugPrint('ServerSync: Disconnecting hub $hubType @ $address');
    await _connection.api.hubDisconnectOne(hubType: hubType, address: address);
  }

  void _pushHubCredentialsForSource(RoomSourceDto source) {
    final hubType = _hubTypeForSource(source);
    if (hubType == null) return;

    final hub = _homeProvider.getFirstHubOfType(hubType);
    if (hub == null || !hub.hasCredentials) return;

    // HA expects {"token": "..."}, Hue expects {"username": "..."}
    final credentials = hubType == HubType.homeAssistant
        ? {'token': hub.token}
        : {'username': hub.token};

    debugPrint('ServerSync: Pushing ${hub.typeName} credentials after source change');
    _connection.api.hubCredentials(
      hubType: _hubTypeWireName(hubType),
      address: '${hub.endpoint.host}:${hub.endpoint.port}',
      credentials: credentials,
    );
  }

  /// Accept server config as authoritative — update app's Home if different.
  ///
  /// The server (addon) owns the curve config. On hello, if the server's
  /// config differs from the app's cached copy, we update the app to match.
  void _acceptServerConfig(Map<String, dynamic> serverConfig) {
    final srvMinBri = serverConfig['min_brightness'] as int?;
    final srvMaxBri = serverConfig['max_brightness'] as int?;
    final srvMinCct = serverConfig['min_color_temp'] as int?;
    final srvMaxCct = serverConfig['max_color_temp'] as int?;
    final srvWlBri = (serverConfig['width_left_bri'] as num?)?.toDouble();
    final srvWrBri = (serverConfig['width_right_bri'] as num?)?.toDouble();
    final srvWlCct = (serverConfig['width_left_cct'] as num?)?.toDouble();
    final srvWrCct = (serverConfig['width_right_cct'] as num?)?.toDouble();
    final srvShapeP = (serverConfig['shape_p'] as num?)?.toDouble();
    final srvMaxDim = serverConfig['max_dim_steps'] as int?;

    if (srvMinBri == null || srvMaxBri == null ||
        srvMinCct == null || srvMaxCct == null ||
        srvWlBri == null || srvWrBri == null ||
        srvWlCct == null || srvWrCct == null ||
        srvShapeP == null || srvMaxDim == null) {
      debugPrint('ServerSync: CONFIG ACCEPT skipped — server config incomplete');
      return;
    }

    final serverCurve = CurveConfigDto(
      minBrightness: srvMinBri,
      maxBrightness: srvMaxBri,
      minColorTemp: srvMinCct,
      maxColorTemp: srvMaxCct,
      widthLeftBri: srvWlBri,
      widthRightBri: srvWrBri,
      widthLeftCct: srvWlCct,
      widthRightCct: srvWrCct,
      shapeP: srvShapeP,
      maxDimSteps: srvMaxDim,
    );

    final appConfig = _homeProvider.currentHome?.curveConfig;

    debugPrint('ServerSync: CONFIG COMPARE — '
        'Server: bri=$srvMinBri-$srvMaxBri cct=$srvMinCct-$srvMaxCct '
        'wBri=$srvWlBri/$srvWrBri wCct=$srvWlCct/$srvWrCct shapeP=$srvShapeP | '
        'App: bri=${appConfig?.minBrightness}-${appConfig?.maxBrightness} '
        'cct=${appConfig?.minColorTemp}-${appConfig?.maxColorTemp} '
        'wBri=${appConfig?.widthLeftBri}/${appConfig?.widthRightBri} '
        'wCct=${appConfig?.widthLeftCct}/${appConfig?.widthRightCct} '
        'shapeP=${appConfig?.shapeP}');

    if (appConfig != serverCurve) {
      debugPrint('ServerSync: Config mismatch — accepting server config into app');
      _homeProvider.updateCurrentHomeCurveConfig(serverCurve);
    }
  }

  void _pushLocationIfUnset(Map<String, dynamic> serverLocation) {
    final home = _homeProvider.currentHome;
    final loc = home?.location;
    if (loc == null) return;

    final srvLat = (serverLocation['latitude'] as num?)?.toDouble();
    final srvLon = (serverLocation['longitude'] as num?)?.toDouble();

    // Only push when the server has no location at all. Location is the
    // *house* location (fixed, tied to the server/hub) — once set, it should
    // only change via an explicit user action in location settings.
    if (srvLat == null || srvLon == null) {
      debugPrint('ServerSync: Server has no location — pushing app location');
      _pushLocationWithIanaTimezone(loc);
    }
  }

  /// Push location to server, ensuring a proper IANA timezone name.
  ///
  /// If the Home's timezone is a non-IANA abbreviation (e.g. "EST"),
  /// falls back to the device's local timezone via [FlutterTimezone].
  void _pushLocationWithIanaTimezone(HomeLocation loc) {
    final homeTz = _homeProvider.currentHome?.timezone;
    if (_isIanaTimezone(homeTz)) {
      _connection.api.locationSet(lat: loc.latitude, lon: loc.longitude, timezoneName: homeTz);
    } else {
      // Resolve proper IANA name asynchronously.
      FlutterTimezone.getLocalTimezone().then((tz) {
        final ianaTz = tz.identifier;
        debugPrint('ServerSync: Resolved IANA timezone: $ianaTz (was: $homeTz)');
        _connection.api.locationSet(lat: loc.latitude, lon: loc.longitude, timezoneName: ianaTz);

        // Also fix the Home model so future syncs don't need this fallback.
        final home = _homeProvider.currentHome;
        if (home != null && home.timezone != ianaTz) {
          _homeProvider.updateCurrentHome(
            home.copyWith(timezone: ianaTz, updatedAt: DateTime.now(), pendingSync: true),
          );
        }
      }).catchError((e) {
        debugPrint('ServerSync: FlutterTimezone failed: $e, using home timezone');
        _connection.api.locationSet(lat: loc.latitude, lon: loc.longitude, timezoneName: homeTz);
      });
    }
  }

  // ============================================================================
  // Hub-agnostic helpers
  // ============================================================================

  HubType? _hubTypeForSource(RoomSourceDto source) {
    switch (source) {
      case RoomSourceDto.hue:
        return HubType.hue;
      case RoomSourceDto.homeAssistant:
        return HubType.homeAssistant;
      default:
        return null;
    }
  }

  /// Map Dart HubType to the Rust wire string.
  ///
  /// Dart enum names are camelCase (`homeAssistant`) but the Rust server
  /// expects lowercase (`homeassistant`).
  String _hubTypeWireName(HubType type) {
    switch (type) {
      case HubType.homeAssistant:
        return 'homeassistant';
      case HubType.hue:
        return 'hue';
      case HubType.server:
        return 'server';
    }
  }

  @override
  void dispose() {
    _helloSub?.cancel();
    _rhythmStateSub?.cancel();
    _hubEventSub?.cancel();
    _sourceChangedSub?.cancel();
    _motionTimerSub?.cancel();
    _newRoomsSub?.cancel();
    _triageChangedSub?.cancel();
    _connectionStateSub?.cancel();
    super.dispose();
  }
}
