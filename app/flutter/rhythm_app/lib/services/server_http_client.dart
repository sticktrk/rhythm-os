/// HTTP client for server communication (ESP32, rhythm-server, addon).
///
/// Uses Dio for all HTTP calls with a 15-second poll loop for state changes.
/// The app controls lights directly through Hue — this client only manages
/// room configuration and rhythm state sync.
///
/// ## Design
///
/// - `connect()` fetches `GET /api/state` and starts a 15s poll timer
/// - `helloEvents` fires after successful `GET /api/state`
/// - `rhythmStateEvents` fires from poll diffs (cached vs. new)
/// - Exponential backoff on HTTP errors
/// - Config pushes are fire-and-forget (log errors internally)
library;

import 'dart:async';
import 'dart:convert';

import 'package:dio/dio.dart';
import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

/// Connection state for the server client.
enum ServerConnectionState {
  disconnected,
  connecting,
  connected,
  reconnecting,
}

/// Device type as reported by the server's typed device model.
enum ServerDeviceType {
  light, button, motion;

  static ServerDeviceType fromString(String value) => switch (value) {
    'light' => ServerDeviceType.light,
    'motion' => ServerDeviceType.motion,
    _ => ServerDeviceType.button,
  };
}

/// A typed device from the server's device registry.
class TypedDevice {
  final String id;
  final ServerDeviceType type;
  final String? name;
  final String? manufacturer;
  final String? model;

  const TypedDevice({
    required this.id,
    required this.type,
    this.name,
    this.manufacturer,
    this.model,
  });

  /// Display name: canonical name or truncated ID.
  String get displayName => name ?? id.substring(0, id.length.clamp(0, 8));

  /// Manufacturer + model summary (e.g. "Signify · LCA001").
  String? get productInfo {
    if (manufacturer == null && model == null) return null;
    final parts = [manufacturer, model].whereType<String>();
    return parts.join(' · ');
  }

  factory TypedDevice.fromJson(Map<String, dynamic> json) => TypedDevice(
    id: json['id'] as String? ?? '',
    type: ServerDeviceType.fromString(json['type'] as String? ?? 'button'),
    name: json['name'] as String?,
    manufacturer: json['manufacturer'] as String?,
    model: json['model'] as String?,
  );
}

/// A room as reported by the server in the state response.
class ServerRoom {
  final String id;
  final String name;
  final String groupedLightId;
  final bool rhythmEnabled;
  final bool disabled;
  final double timeOffset;
  final double brightnessOffset;
  final bool softOff;

  /// Which hub this room belongs to (e.g. "hue", "homeassistant").
  final String? hubType;

  /// All device IDs for this room (lights + buttons + motion sensors).
  final List<String> deviceIds;

  /// Typed devices (lights, buttons, motion sensors — enriched with canonical data).
  final List<TypedDevice> devices;

  /// Whether lights are currently on (server-tracked).
  final bool? lightsOn;

  /// Effective brightness percentage (1-100) after offsets.
  final int? brightness;

  /// Effective color temperature in Kelvin.
  final int? kelvin;

  const ServerRoom({
    required this.id,
    required this.name,
    required this.groupedLightId,
    required this.rhythmEnabled,
    required this.disabled,
    required this.timeOffset,
    required this.brightnessOffset,
    required this.softOff,
    this.hubType,
    this.deviceIds = const [],
    this.devices = const [],
    this.lightsOn,
    this.brightness,
    this.kelvin,
  });

  /// Whether this room has at least one motion sensor.
  bool get hasMotionSensor => devices.any((d) => d.type == ServerDeviceType.motion);

  /// All light devices in this room.
  List<TypedDevice> get lights =>
      devices.where((d) => d.type == ServerDeviceType.light).toList();

  /// All button devices in this room.
  List<TypedDevice> get buttons =>
      devices.where((d) => d.type == ServerDeviceType.button).toList();

  /// All motion sensors in this room.
  List<TypedDevice> get motionSensors =>
      devices.where((d) => d.type == ServerDeviceType.motion).toList();

  /// Number of lights in this room.
  ///
  /// Prefers typed light count from [devices]. Falls back to inferring from
  /// [deviceIds] minus typed non-light devices for backward compat.
  int get lightCount {
    final typed = lights.length;
    if (typed > 0) return typed;
    final inferred = deviceIds.length - devices.length;
    return inferred > 0 ? inferred : 0;
  }

  /// Total number of typed devices in this room.
  int get deviceCount => devices.length;

  /// Human-readable device summary (e.g. "4 lights, 2 buttons, 1 sensor").
  String get deviceSummary {
    final parts = <String>[];
    final l = lights.length;
    final b = buttons.length;
    final m = motionSensors.length;
    if (l > 0) parts.add('$l light${l > 1 ? 's' : ''}');
    if (b > 0) parts.add('$b button${b > 1 ? 's' : ''}');
    if (m > 0) parts.add('$m sensor${m > 1 ? 's' : ''}');
    return parts.isEmpty ? 'No devices' : parts.join(', ');
  }

  factory ServerRoom.fromJson(Map<String, dynamic> json) {
    return ServerRoom(
      id: json['id'] as String? ?? '',
      name: json['name'] as String? ?? '',
      groupedLightId: json['grouped_light_id'] as String? ?? '',
      rhythmEnabled: json['rhythm_enabled'] as bool? ?? false,
      disabled: json['disabled'] as bool? ?? false,
      timeOffset: (json['time_offset'] as num?)?.toDouble() ?? 0.0,
      brightnessOffset:
          (json['brightness_offset'] as num?)?.toDouble() ?? 0.0,
      softOff: json['soft_off'] as bool? ?? false,
      hubType: json['hub_type'] as String?,
      deviceIds: (json['device_ids'] as List<dynamic>?)
              ?.map((e) => e as String)
              .toList() ??
          [],
      devices: (json['devices'] as List<dynamic>?)
              ?.map((e) => TypedDevice.fromJson(e as Map<String, dynamic>))
              .toList() ??
          [],
      lightsOn: json['lights_on'] as bool?,
      brightness: (json['brightness'] as num?)?.toInt(),
      kelvin: (json['kelvin'] as num?)?.toInt(),
    );
  }
}

/// Rhythm state from server poll diffs and SSE events.
///
/// Includes display values (lights_on, brightness, kelvin) so Flutter
/// doesn't need to compute or fetch them independently.
class ServerRhythmState {
  final String roomId;
  final bool rhythmEnabled;
  final double timeOffset;
  final double brightnessOffset;
  final bool softOff;

  /// Whether lights are currently on (server-tracked).
  final bool? lightsOn;

  /// Effective brightness percentage (1-100) after offsets.
  final int? brightness;

  /// Effective color temperature in Kelvin.
  final int? kelvin;

  const ServerRhythmState({
    required this.roomId,
    required this.rhythmEnabled,
    required this.timeOffset,
    required this.brightnessOffset,
    required this.softOff,
    this.lightsOn,
    this.brightness,
    this.kelvin,
  });

  factory ServerRhythmState.fromJson(Map<String, dynamic> json) {
    return ServerRhythmState(
      roomId: json['room_id'] as String? ?? json['id'] as String? ?? '',
      rhythmEnabled: json['rhythm_enabled'] as bool? ?? false,
      timeOffset: (json['time_offset'] as num?)?.toDouble() ?? 0.0,
      brightnessOffset:
          (json['brightness_offset'] as num?)?.toDouble() ?? 0.0,
      softOff: json['soft_off'] as bool? ?? false,
      lightsOn: json['lights_on'] as bool?,
      brightness: (json['brightness'] as num?)?.toInt(),
      kelvin: (json['kelvin'] as num?)?.toInt(),
    );
  }
}

/// Motion timer state from server poll diffs.
class ServerMotionTimer {
  final String roomId;
  final bool motionActive;
  final bool motionOwned;
  final int? remainingSecs;
  final int timeoutSecs;

  const ServerMotionTimer({
    required this.roomId,
    required this.motionActive,
    required this.motionOwned,
    this.remainingSecs,
    required this.timeoutSecs,
  });

  /// A clearing event — room no longer has motion tracking.
  const ServerMotionTimer.cleared(this.roomId)
      : motionActive = false,
        motionOwned = false,
        remainingSecs = null,
        timeoutSecs = 0;

  bool get isCleared => timeoutSecs == 0 && !motionActive;
}

/// Global device settings from the server.
class ServerSettings {
  final int bulbFadeMs;
  final int rhythmIntervalSecs;
  final int defaultMotionTimeoutSecs;
  final bool powerSave;
  final int softOffBrightness;

  const ServerSettings({
    required this.bulbFadeMs,
    required this.rhythmIntervalSecs,
    required this.defaultMotionTimeoutSecs,
    required this.powerSave,
    required this.softOffBrightness,
  });

  // Fallback defaults match Rust constants in rhythm-core::primitives
  // and rhythm-runtime::config (cross-language boundary requires literals).
  factory ServerSettings.fromJson(Map<String, dynamic> json) {
    return ServerSettings(
      bulbFadeMs: (json['bulb_fade_ms'] as num?)?.toInt() ?? 500,
      rhythmIntervalSecs:
          (json['rhythm_interval_secs'] as num?)?.toInt() ?? 60,
      defaultMotionTimeoutSecs:
          (json['default_motion_timeout_secs'] as num?)?.toInt() ?? 600,
      powerSave: json['power_save'] as bool? ?? false,
      softOffBrightness:
          (json['soft_off_brightness'] as num?)?.toInt() ?? 1,
    );
  }
}

/// Full state from server on connect (GET /api/state).
class ServerHello {
  final String version;

  /// Platform type: "desktop" or "embedded".
  final String platformType;

  /// Deployment context: "ha_addon", "server", "embedded", etc.
  final String platformContext;

  /// The port the server is listening on (for direct connections bypassing
  /// reverse proxies like HA ingress).
  final int? listenPort;

  final List<ServerRoom> rooms;

  /// Primary hub (first connected hub, backward compat).
  final Map<String, dynamic> hub;

  /// All configured hubs (multi-hub support). Falls back to [hub] if absent.
  final List<Map<String, dynamic>> hubs;

  final Map<String, dynamic> config;
  final Map<String, dynamic> location;
  final ServerSettings? settings;

  const ServerHello({
    required this.version,
    required this.platformType,
    required this.platformContext,
    this.listenPort,
    required this.rooms,
    required this.hub,
    required this.hubs,
    required this.config,
    required this.location,
    this.settings,
  });

  factory ServerHello.fromJson(Map<String, dynamic> json) {
    final settingsJson = json['settings'] as Map<String, dynamic>?;
    final hub = json['hub'] as Map<String, dynamic>? ?? {};
    final hubsList = (json['hubs'] as List<dynamic>?)
        ?.map((h) => h as Map<String, dynamic>)
        .toList();
    // Fallback: if hubs absent, derive from hub (backward compat with older servers).
    final hubs = hubsList ??
        (hub.isNotEmpty && hub['type'] != 'none' ? [hub] : <Map<String, dynamic>>[]);
    return ServerHello(
      version: json['version'] as String? ?? '0.0.0',
      platformType: json['platform'] as String? ?? 'desktop',
      platformContext: json['context'] as String? ?? 'server',
      listenPort: (json['listen_port'] as num?)?.toInt(),
      rooms: (json['rooms'] as List<dynamic>?)
              ?.map((r) => ServerRoom.fromJson(r as Map<String, dynamic>))
              .where((r) => r.id.isNotEmpty)
              .toList() ??
          [],
      hub: hub,
      hubs: hubs,
      config: json['config'] as Map<String, dynamic>? ?? {},
      location: json['location'] as Map<String, dynamic>? ?? {},
      settings:
          settingsJson != null ? ServerSettings.fromJson(settingsJson) : null,
    );
  }
}

// ---------------------------------------------------------------------------
// HTTP Client
// ---------------------------------------------------------------------------

/// HTTP client for server communication.
///
/// Provides a stream-based interface for the sync provider's event handling.
///
/// State flow:
/// 1. `connect()` → `GET /api/state` → emits [helloEvents] → starts 15s poll
/// 2. Poll loop: `GET /api/rooms/state` → diff against cache → emits
///    [rhythmStateEvents] for changed rooms
/// 3. On 3 consecutive poll failures → `reconnecting` state → full reconnect
class ServerHttpClient extends ChangeNotifier {
  // Stream controllers (broadcast so multiple listeners work).
  final _helloController = StreamController<ServerHello>.broadcast();
  final _rhythmStateController =
      StreamController<ServerRhythmState>.broadcast();
  final _hubEventController = StreamController<String>.broadcast();
  final _motionTimerController =
      StreamController<ServerMotionTimer>.broadcast();
  final _newRoomsController = StreamController<void>.broadcast();
  final _triageChangedController =
      StreamController<Map<String, dynamic>>.broadcast();

  // Connection state.
  ServerConnectionState _connectionState = ServerConnectionState.disconnected;
  String? _host;
  int _port = 80;
  Dio? _dio;

  // Polling.
  Timer? _pollTimer;
  int _consecutivePollFailures = 0;
  static const int _maxPollFailures = 3;
  static const Duration _pollInterval = Duration(seconds: 15);

  // Reconnect.
  Timer? _reconnectTimer;
  int _reconnectAttempts = 0;

  // SSE state.
  StreamSubscription? _sseSubscription;
  CancelToken? _sseCancelToken;
  bool _sseConnected = false;
  bool _sseSupported = true;
  Timer? _sseReconnectTimer;
  int _sseReconnectAttempts = 0;
  bool _sseConnecting = false;

  // Direct SSE state (for HA addon web — bypasses ingress for SSE).
  String? _serverPlatformContext;
  int? _listenPort;
  bool _sseDirectAttempted = false;

  // SSE keepalive watchdog.
  // Server sends `: keep-alive` every 15s. If no activity for 45s, SSE is stale.
  DateTime _lastSseActivity = DateTime.fromMillisecondsSinceEpoch(0);
  Timer? _sseWatchdogTimer;
  static const Duration _sseWatchdogInterval = Duration(seconds: 15);
  static const Duration _sseStaleThreshold = Duration(seconds: 45);

  // Cached room states for diff detection.
  final Map<String, _CachedRoomState> _cachedRoomStates = {};
  bool? _cachedHubConnected;

  /// Room IDs from poll that already triggered a re-hello but weren't in
  /// the subsequent hello response. Prevents infinite poll→re-hello loops
  /// when the server's poll and hello endpoints disagree on room sets.
  final Set<String> _reHelloSuppressedRoomIds = {};

  // --------------------------------------------------------------------------
  // Public getters
  // --------------------------------------------------------------------------

  /// Stream of hello messages (emitted on each successful connect).
  Stream<ServerHello> get helloEvents => _helloController.stream;

  /// Stream of rhythm state updates (emitted when poll detects changes).
  Stream<ServerRhythmState> get rhythmStateEvents =>
      _rhythmStateController.stream;

  /// Stream of hub events (connected/disconnected changes).
  Stream<String> get hubEvents => _hubEventController.stream;

  /// Stream of motion timer updates (emitted when poll detects motion changes).
  Stream<ServerMotionTimer> get motionTimerEvents =>
      _motionTimerController.stream;

  /// Fires when the poll detects room IDs the client doesn't know about.
  ///
  /// The sync provider should trigger a full re-hello to pick up the new
  /// rooms (which include name, grouped_light_id, etc. not in the poll).
  Stream<void> get newRoomsDetected => _newRoomsController.stream;

  /// Emits triage count data when triage queue changes (SSE `triage_changed`).
  Stream<Map<String, dynamic>> get triageChangedEvents =>
      _triageChangedController.stream;

  /// Current connection state.
  ServerConnectionState get connectionState => _connectionState;

  /// Whether connected to the server.
  bool get connected => _connectionState == ServerConnectionState.connected;

  /// Host (IP or hostname) of the connected device.
  String? get host => _host;

  // --------------------------------------------------------------------------
  // Connect / Disconnect
  // --------------------------------------------------------------------------

  /// Connect to a server device.
  ///
  /// Fetches full state via `GET /api/state`, emits a hello event, and starts
  /// the 15-second poll loop. If already connected to the same host, this is a
  /// no-op. If connecting to a different host, tears down the old connection.
  Future<void> connect(String host, {int port = 80}) async {
    if (_connectionState == ServerConnectionState.connected &&
        _host == host && _port == port) {
      return;
    }

    // Switching to a different device — tear down first.
    if (_host != null && (_host != host || _port != port)) {
      _stopPolling();
      _disconnectSse();
      _sseReconnectTimer?.cancel();
      _sseReconnectTimer = null;
      _sseReconnectAttempts = 0;
      _reconnectTimer?.cancel();
      _reconnectTimer = null;
      _reconnectAttempts = 0;
      _cachedRoomStates.clear();
      _cachedHubConnected = null;
      _reHelloSuppressedRoomIds.clear();
      _sseSupported = true;
      _sseDirectAttempted = false;
    }

    _host = host;
    _port = port;
    final baseUrl = _buildBaseUrl(host, port);
    _dio = Dio(BaseOptions(
      baseUrl: baseUrl,
      connectTimeout: const Duration(seconds: 5),
      receiveTimeout: const Duration(seconds: 10),
    ));

    await _connectInternal();
  }

  /// Force a full reconnect (re-fetches `/api/state` and emits a fresh hello).
  ///
  /// Use after OTA reboot so the sync provider picks up the new firmware version.
  Future<void> reconnect() async {
    if (_host == null) return;
    _stopPolling();
    _disconnectSse();
    _sseReconnectTimer?.cancel();
    _sseReconnectTimer = null;
    _sseReconnectAttempts = 0;
    _reconnectAttempts = 0;
    _consecutivePollFailures = 0;
    _cachedRoomStates.clear();
    _cachedHubConnected = null;
    _sseDirectAttempted = false;
    await _connectInternal();
  }

  /// Disconnect from the server.
  void disconnect() {
    _stopPolling();
    _disconnectSse();
    _sseReconnectTimer?.cancel();
    _sseReconnectTimer = null;
    _sseReconnectAttempts = 0;
    _reconnectTimer?.cancel();
    _reconnectTimer = null;
    _reconnectAttempts = 0;

    _host = null;
    _dio?.close();
    _dio = null;

    _cachedRoomStates.clear();
    _cachedHubConnected = null;
    _reHelloSuppressedRoomIds.clear();
    _sseSupported = true;
    _sseDirectAttempted = false;

    _connectionState = ServerConnectionState.disconnected;
    notifyListeners();
  }

  // --------------------------------------------------------------------------
  // Config push methods (fire-and-forget HTTP PUTs/DELETEs)
  // --------------------------------------------------------------------------

  /// Push room preferences (user-state only, no topology).
  ///
  /// Uses `PUT /api/rooms/preferences` — the server owns room topology
  /// via hub discovery; the app only sends user preferences.
  Future<void> roomPreferencesSet({
    required String roomId,
    bool? rhythmEnabled,
    bool? disabled,
    bool? softOff,
  }) async {
    await _safePut('api/rooms/preferences', data: {
      'room_id': roomId,
      if (rhythmEnabled != null) 'rhythm_enabled': rhythmEnabled,
      if (disabled != null) 'disabled': disabled,
      if (softOff != null) 'soft_off': softOff,
    });
  }

  /// Dispatch a room action via the server runtime.
  ///
  /// Returns the resulting [ServerRhythmState] if the server responds with
  /// room state, enabling immediate convergence instead of waiting for
  /// SSE/poll. Returns null on failure or if server omits state.
  Future<ServerRhythmState?> roomAction({
    required String roomId,
    required String action,
  }) async {
    if (_dio == null || !connected) {
      debugPrint('ServerHttp: Cannot PUT /api/rooms/action, not connected');
      return null;
    }
    try {
      final response = await _dio!.put('api/rooms/action', data: {
        'room_id': roomId,
        'action': action,
      });
      final data = response.data as Map<String, dynamic>?;
      // New shape: {"rooms":[...]} — extract first room
      final rooms = data?['rooms'] as List<dynamic>?;
      Map<String, dynamic>? roomJson;
      if (rooms != null && rooms.isNotEmpty) {
        roomJson = rooms[0] as Map<String, dynamic>?;
      } else if (data != null && data.containsKey('rhythm_enabled')) {
        // Backward compat: old shape (raw room object)
        roomJson = data;
      }
      if (roomJson != null && roomJson.containsKey('rhythm_enabled')) {
        final state = ServerRhythmState.fromJson(roomJson);
        // Update cache so subsequent SSE/poll becomes a no-op.
        final existing = _cachedRoomStates[roomId];
        _cachedRoomStates[roomId] = _CachedRoomState(
          rhythmEnabled: state.rhythmEnabled,
          timeOffset: state.timeOffset,
          brightnessOffset: state.brightnessOffset,
          softOff: state.softOff,
          lightsOn: state.lightsOn ?? existing?.lightsOn,
          brightness: state.brightness ?? existing?.brightness,
          kelvin: state.kelvin ?? existing?.kelvin,
          motionActive: existing?.motionActive,
          motionOwned: existing?.motionOwned,
          motionRemaining: existing?.motionRemaining,
          motionTimeout: existing?.motionTimeout,
          hasMotionSensor: existing?.hasMotionSensor ?? false,
        );
        return state;
      }
    } catch (e) {
      debugPrint('ServerHttp: PUT /api/rooms/action failed: $e');
    }
    return null;
  }

  /// Dispatch actions for multiple rooms in a single request.
  ///
  /// Sends an array body to `PUT /api/rooms/action`. The server processes
  /// each action sequentially and returns all room states. Uses a longer
  /// timeout since N rooms are processed in one request.
  Future<List<ServerRhythmState>> roomActionBatch(
    List<({String roomId, String action})> actions,
  ) async {
    if (_dio == null || !connected || actions.isEmpty) return [];
    try {
      final response = await _dio!.put(
        'api/rooms/action',
        data: [for (final a in actions) {'room_id': a.roomId, 'action': a.action}],
        options: Options(receiveTimeout: const Duration(seconds: 30)),
      );
      final data = response.data as Map<String, dynamic>?;
      final rooms = data?['rooms'] as List<dynamic>?;
      if (rooms == null) return [];
      final results = <ServerRhythmState>[];
      for (final r in rooms) {
        if (r is Map<String, dynamic> && r.containsKey('rhythm_enabled')) {
          final state = ServerRhythmState.fromJson(r);
          final existing = _cachedRoomStates[state.roomId];
          _cachedRoomStates[state.roomId] = _CachedRoomState(
            rhythmEnabled: state.rhythmEnabled,
            timeOffset: state.timeOffset,
            brightnessOffset: state.brightnessOffset,
            softOff: state.softOff,
            lightsOn: state.lightsOn ?? existing?.lightsOn,
            brightness: state.brightness ?? existing?.brightness,
            kelvin: state.kelvin ?? existing?.kelvin,
            motionActive: existing?.motionActive,
            motionOwned: existing?.motionOwned,
            motionRemaining: existing?.motionRemaining,
            motionTimeout: existing?.motionTimeout,
            hasMotionSensor: existing?.hasMotionSensor ?? false,
          );
          results.add(state);
        }
      }
      return results;
    } catch (e) {
      debugPrint('ServerHttp: PUT /api/rooms/action (batch) failed: $e');
    }
    return [];
  }

  /// Set room brightness via the server runtime.
  Future<void> roomBrightness({
    required String roomId,
    required int brightness,
  }) async {
    await _safePut('api/rooms/brightness', data: {
      'room_id': roomId,
      'brightness': brightness,
    });
  }

  /// Set brightness for multiple rooms in a single request.
  ///
  /// Sends an array body to `PUT /api/rooms/brightness`.
  Future<List<ServerRhythmState>> roomBrightnessBatch(
    List<({String roomId, int brightness})> items,
  ) async {
    if (_dio == null || !connected || items.isEmpty) return [];
    try {
      final response = await _dio!.put(
        'api/rooms/brightness',
        data: [for (final i in items) {'room_id': i.roomId, 'brightness': i.brightness}],
        options: Options(receiveTimeout: const Duration(seconds: 30)),
      );
      return _parseRoomsResponse(response.data);
    } catch (e) {
      debugPrint('ServerHttp: PUT /api/rooms/brightness (batch) failed: $e');
    }
    return [];
  }

  /// Set the time offset for a single room.
  Future<void> roomOffset({
    required String roomId,
    required double timeOffset,
  }) async {
    await _safePut('api/rooms/offset', data: {
      'room_id': roomId,
      'time_offset': timeOffset,
    });
  }

  /// Set time offset for multiple rooms in a single request.
  ///
  /// Sends an array body to `PUT /api/rooms/offset`.
  Future<List<ServerRhythmState>> roomOffsetBatch(
    List<({String roomId, double timeOffset})> items,
  ) async {
    if (_dio == null || !connected || items.isEmpty) return [];
    try {
      final response = await _dio!.put(
        'api/rooms/offset',
        data: [for (final i in items) {'room_id': i.roomId, 'time_offset': i.timeOffset}],
        options: Options(receiveTimeout: const Duration(seconds: 30)),
      );
      return _parseRoomsResponse(response.data);
    } catch (e) {
      debugPrint('ServerHttp: PUT /api/rooms/offset (batch) failed: $e');
    }
    return [];
  }

  /// Absorb a time offset into the curve config by adjusting ramp widths.
  ///
  /// Posts to `POST /api/config/absorb-offset` with `{"offset_minutes": value}`.
  /// The server adjusts the curve's width parameters so the current lighting
  /// values become the new zero-offset baseline, then resets all room offsets.
  /// Returns the updated [CurveConfigDto] if the server responded with one.
  Future<CurveConfigDto?> absorbTimeOffset(double offsetMinutes) async {
    if (_dio == null || !connected) return null;
    try {
      final response = await _dio!.post(
        'api/config/absorb-offset',
        data: {'offset_minutes': offsetMinutes},
      );
      final data = response.data;
      if (data is Map<String, dynamic>) {
        return _parseCurveConfig(data);
      }
    } catch (e) {
      debugPrint('ServerHttp: POST /api/config/absorb-offset failed: $e');
    }
    return null;
  }

  CurveConfigDto? _parseCurveConfig(Map<String, dynamic> json) {
    final minBri = json['min_brightness'] as int?;
    final maxBri = json['max_brightness'] as int?;
    final minCct = json['min_color_temp'] as int?;
    final maxCct = json['max_color_temp'] as int?;
    final wlBri = (json['width_left_bri'] as num?)?.toDouble();
    final wrBri = (json['width_right_bri'] as num?)?.toDouble();
    final wlCct = (json['width_left_cct'] as num?)?.toDouble();
    final wrCct = (json['width_right_cct'] as num?)?.toDouble();
    final shapeP = (json['shape_p'] as num?)?.toDouble();
    final maxDim = json['max_dim_steps'] as int?;
    if (minBri == null || maxBri == null || minCct == null || maxCct == null ||
        wlBri == null || wrBri == null || wlCct == null || wrCct == null ||
        shapeP == null || maxDim == null) {
      return null;
    }
    return CurveConfigDto(
      minBrightness: minBri,
      maxBrightness: maxBri,
      minColorTemp: minCct,
      maxColorTemp: maxCct,
      widthLeftBri: wlBri,
      widthRightBri: wrBri,
      widthLeftCct: wlCct,
      widthRightCct: wrCct,
      shapeP: shapeP,
      maxDimSteps: maxDim,
    );
  }

  /// Reset the curve config to factory defaults.
  ///
  /// Returns the default [CurveConfigDto] from the server.
  Future<CurveConfigDto?> resetConfig() async {
    if (_dio == null || !connected) return null;
    try {
      final response = await _dio!.post('api/config/reset');
      final data = response.data;
      if (data is Map<String, dynamic>) {
        return _parseCurveConfig(data);
      }
    } catch (e) {
      debugPrint('ServerHttp: POST /api/config/reset failed: $e');
    }
    return null;
  }

  /// Push room preferences for multiple rooms in a single request.
  ///
  /// Uses `PUT /api/rooms/preferences` with an array body to avoid
  /// per-room socket overhead on constrained servers (ESP32).
  Future<void> roomPreferencesBatchSet(List<Map<String, dynamic>> items) async {
    if (items.isEmpty) return;
    await _safePut('api/rooms/preferences', data: items);
  }

  /// Reset all on-rooms back to their current adaptive curve position.
  ///
  /// Returns the room states for all affected rooms, or empty on failure.
  Future<List<ServerRhythmState>> fixMyLights() async {
    if (_dio == null || !connected) {
      debugPrint('ServerHttp: Cannot POST /api/rooms/fix, not connected');
      return [];
    }
    try {
      final response = await _dio!.post(
        'api/rooms/fix',
        options: Options(receiveTimeout: const Duration(seconds: 30)),
      );
      final data = response.data as Map<String, dynamic>?;
      final roomStates = data?['rooms'] as List<dynamic>?
          ?? data?['room_states'] as List<dynamic>?;
      if (roomStates == null) return [];
      final results = <ServerRhythmState>[];
      for (final r in roomStates) {
        if (r is Map<String, dynamic> && r.containsKey('rhythm_enabled')) {
          final state = ServerRhythmState.fromJson(r);
          final existing = _cachedRoomStates[state.roomId];
          _cachedRoomStates[state.roomId] = _CachedRoomState(
            rhythmEnabled: state.rhythmEnabled,
            timeOffset: state.timeOffset,
            brightnessOffset: state.brightnessOffset,
            softOff: state.softOff,
            lightsOn: state.lightsOn ?? existing?.lightsOn,
            brightness: state.brightness ?? existing?.brightness,
            kelvin: state.kelvin ?? existing?.kelvin,
            motionActive: existing?.motionActive,
            motionOwned: existing?.motionOwned,
            motionRemaining: existing?.motionRemaining,
            motionTimeout: existing?.motionTimeout,
            hasMotionSensor: existing?.hasMotionSensor ?? false,
          );
          results.add(state);
        }
      }
      return results;
    } catch (e) {
      debugPrint('ServerHttp: POST /api/rooms/fix failed: $e');
      return [];
    }
  }

  /// Trigger server-side room discovery from the connected hub.
  ///
  /// Returns the sync result payload, or null on failure.
  Future<Map<String, dynamic>?> triggerSync() async {
    if (_dio == null || !connected) {
      debugPrint('ServerHttp: Cannot POST /api/sync, not connected');
      return null;
    }
    try {
      final response = await _dio!.post('api/sync');
      return response.data as Map<String, dynamic>?;
    } catch (e) {
      debugPrint('ServerHttp: POST /api/sync failed: $e');
      return null;
    }
  }

  /// Set per-room motion timeout on the server.
  Future<void> motionTimeoutSet({
    required String roomId,
    required int timeoutSecs,
  }) async {
    await _safePut('api/motion-timeout', data: {
      'room_id': roomId,
      'timeout_secs': timeoutSecs,
    });
  }

  /// Push global curve configuration to the server.
  Future<void> configSet(CurveConfigDto config) async {
    await _safePut('api/config', data: _curveConfigToJson(config));
  }

  /// Push location to the server.
  Future<void> locationSet({
    required double lat,
    required double lon,
    double? utcOffset,
    String? timezoneName,
  }) async {
    await _safePut('api/location', data: {
      'lat': lat,
      'lon': lon,
      if (utcOffset != null) 'utc_offset': utcOffset,
      if (timezoneName != null) 'timezone_name': timezoneName,
    });
  }

  /// Push hub credentials to the server.
  Future<void> hubCredentials({
    required String hubType,
    required String address,
    required Map<String, dynamic> credentials,
  }) async {
    await _safePut('api/hub/credentials', data: {
      'hub_type': hubType,
      'address': address,
      'credentials': credentials,
    });
  }

  /// Disconnect ALL hubs on the server — clears all credentials, runtimes, and rooms.
  Future<void> hubDisconnect() async {
    if (_dio == null || !connected) return;
    try {
      await _dio!.delete('api/hub/credentials');
    } catch (e) {
      debugPrint('ServerHttp: DELETE /api/hub/credentials failed: $e');
    }
  }

  /// Disconnect a single hub by type + address.
  Future<void> hubDisconnectOne({
    required String hubType,
    required String address,
  }) async {
    if (_dio == null || !connected) return;
    try {
      await _dio!.delete('api/hub/credentials', queryParameters: {
        'hub_type': hubType,
        'address': address,
      });
    } catch (e) {
      debugPrint('ServerHttp: DELETE /api/hub/credentials ($hubType) failed: $e');
    }
  }

  // =========================================================================
  // Canonical device API
  // =========================================================================

  /// Fetch a single canonical device by ID.
  Future<Map<String, dynamic>?> getCanonicalDevice(String id) async {
    if (_dio == null || !connected) return null;
    try {
      final response = await _dio!.get('api/devices/canonical/$id');
      return response.data as Map<String, dynamic>?;
    } catch (e) {
      debugPrint('ServerHttp: GET canonical device $id failed: $e');
      return null;
    }
  }

  /// Fetch all canonical devices.
  Future<List<Map<String, dynamic>>?> getCanonicalDevices() async {
    if (_dio == null || !connected) return null;
    try {
      final response = await _dio!.get('api/devices/canonical');
      return (response.data as List<dynamic>?)
          ?.cast<Map<String, dynamic>>();
    } catch (e) {
      debugPrint('ServerHttp: GET canonical devices failed: $e');
      return null;
    }
  }

  // =========================================================================
  // Triage API
  // =========================================================================

  /// Fetch pending triage entries.
  Future<List<Map<String, dynamic>>?> getTriageEntries() async {
    if (_dio == null || !connected) return null;
    try {
      final response = await _dio!.get('api/triage');
      return (response.data as List<dynamic>?)
          ?.cast<Map<String, dynamic>>();
    } catch (e) {
      debugPrint('ServerHttp: GET triage entries failed: $e');
      return null;
    }
  }

  /// Fetch triage pending counts ({devices, rooms, total}).
  Future<Map<String, dynamic>?> getTriageCount() async {
    if (_dio == null || !connected) return null;
    try {
      final response = await _dio!.get('api/triage/count');
      return response.data as Map<String, dynamic>?;
    } catch (e) {
      debugPrint('ServerHttp: GET triage count failed: $e');
      return null;
    }
  }

  /// Resolve a triage entry by merging with an existing canonical device.
  Future<bool> resolveTriageMerge(String entryId, String canonicalId) async {
    if (_dio == null || !connected) return false;
    try {
      await _dio!.put('api/triage/$entryId/merge', data: {
        'canonical_id': canonicalId,
      });
      return true;
    } catch (e) {
      debugPrint('ServerHttp: PUT triage merge failed: $e');
      return false;
    }
  }

  /// Resolve a triage entry by creating a new canonical device.
  Future<String?> resolveTriageNew(String entryId) async {
    if (_dio == null || !connected) return null;
    try {
      final response = await _dio!.put('api/triage/$entryId/new');
      return (response.data as Map<String, dynamic>?)
          ?['canonical_id'] as String?;
    } catch (e) {
      debugPrint('ServerHttp: PUT triage new failed: $e');
      return null;
    }
  }

  /// Dismiss a triage entry.
  Future<bool> resolveTriageDismiss(String entryId) async {
    if (_dio == null || !connected) return false;
    try {
      await _dio!.put('api/triage/$entryId/dismiss');
      return true;
    } catch (e) {
      debugPrint('ServerHttp: PUT triage dismiss failed: $e');
      return false;
    }
  }

  /// Approve a room binding (merge two rooms from different hubs).
  Future<bool> resolveTriageBind(String entryId, {String? targetRoomId}) async {
    if (_dio == null || !connected) return false;
    try {
      await _dio!.put('api/triage/$entryId/bind', data: {
        if (targetRoomId != null) 'target_room_id': targetRoomId,
      });
      return true;
    } catch (e) {
      debugPrint('ServerHttp: PUT triage bind failed: $e');
      return false;
    }
  }

  // =========================================================================
  // Topology API
  // =========================================================================

  /// Rename a topology room.
  Future<bool> topologyRenameRoom(String roomId, String name) async {
    if (_dio == null || !connected) return false;
    try {
      await _dio!.put('api/topology/rooms/$roomId', data: {'name': name});
      return true;
    } catch (e) {
      debugPrint('ServerHttp: PUT topology rename failed: $e');
      return false;
    }
  }

  /// Merge two topology rooms.
  Future<bool> topologyMergeRooms(String targetId, String sourceId) async {
    if (_dio == null || !connected) return false;
    try {
      await _dio!.put('api/topology/rooms/$targetId/merge', data: {
        'source_id': sourceId,
      });
      return true;
    } catch (e) {
      debugPrint('ServerHttp: PUT topology merge failed: $e');
      return false;
    }
  }

  /// Move a device between topology rooms.
  Future<bool> topologyMoveDevice({
    required String deviceId,
    required String fromRoomId,
    required String toRoomId,
  }) async {
    if (_dio == null || !connected) return false;
    try {
      await _dio!.put('api/topology/rooms/$toRoomId/devices/move', data: {
        'device_id': deviceId,
        'from_room': fromRoomId,
      });
      return true;
    } catch (e) {
      debugPrint('ServerHttp: PUT topology move device failed: $e');
      return false;
    }
  }

  /// Fetch global device settings from the server.
  Future<ServerSettings?> getSettings() async {
    if (_dio == null || !connected) return null;
    try {
      final response = await _dio!.get('api/settings');
      return ServerSettings.fromJson(response.data as Map<String, dynamic>);
    } catch (e) {
      debugPrint('ServerHttp: GET /api/settings failed: $e');
      return null;
    }
  }

  /// Push a partial settings update to the server.
  ///
  /// Only non-null fields are sent.
  Future<void> settingsSet({
    int? bulbFadeMs,
    int? rhythmIntervalSecs,
    int? defaultMotionTimeoutSecs,
    bool? powerSave,
    int? softOffBrightness,
    double? timeOffsetMinutes,
  }) async {
    final data = <String, dynamic>{
      if (bulbFadeMs != null) 'bulb_fade_ms': bulbFadeMs,
      if (rhythmIntervalSecs != null)
        'rhythm_interval_secs': rhythmIntervalSecs,
      if (defaultMotionTimeoutSecs != null)
        'default_motion_timeout_secs': defaultMotionTimeoutSecs,
      if (powerSave != null) 'power_save': powerSave,
      if (softOffBrightness != null) 'soft_off_brightness': softOffBrightness,
      if (timeOffsetMinutes != null) 'time_offset_minutes': timeOffsetMinutes,
    };
    if (data.isEmpty) return;
    await _safePut('api/settings', data: data);
  }

  /// Ping the server (health check).
  Future<bool> ping() async {
    if (_dio == null) return false;
    try {
      final response = await _dio!.get('health');
      return response.statusCode == 200;
    } catch (_) {
      return false;
    }
  }

  /// Trigger an immediate poll cycle (reuses existing diff/emit logic).
  ///
  /// Used by pull-to-refresh so the UI gets fresh state without
  /// waiting for the next 15s tick.
  Future<void> pollNow() async {
    if (connected) {
      _cachedRoomStates.clear(); // Force full diff on manual refresh
      await _poll();
    }
  }

  /// Ping if connected, or attempt immediate reconnect if not.
  ///
  /// Called on app foreground to aggressively restore the connection
  /// rather than waiting for the next poll/backoff cycle. Also checks
  /// SSE liveness — if SSE hasn't received anything recently, forces
  /// SSE reconnect without tearing down the whole connection.
  Future<void> pingOrReconnect() async {
    if (_host == null || _dio == null) return;

    if (connected) {
      final ok = await ping();
      if (!ok) {
        debugPrint('ServerHttp: Foreground ping failed, reconnecting...');
        _stopPolling();
        _disconnectSse();
        _sseReconnectTimer?.cancel();
        _sseReconnectTimer = null;
        _sseReconnectAttempts = 0;
        _connectionState = ServerConnectionState.reconnecting;
        notifyListeners();
        _reconnectTimer?.cancel();
        _reconnectAttempts = 0;
        _connectInternal();
      } else if (_sseConnected) {
        // Server is reachable but check if SSE stream is stale
        final elapsed = DateTime.now().difference(_lastSseActivity);
        if (elapsed > _sseStaleThreshold) {
          debugPrint('ServerHttp: Foreground resume — SSE stale (${elapsed.inSeconds}s), reconnecting SSE');
          _sseReconnectAttempts = 0; // Fresh start after foreground resume
          _handleSseDisconnect();
        }
      } else if (!_sseConnected && _sseSupported && !_sseConnecting) {
        // Connected but SSE not active — try to establish it
        debugPrint('ServerHttp: Foreground resume — SSE not connected, attempting');
        _sseReconnectAttempts = 0;
        _connectSse();
      }
    } else {
      // Not connected — cancel any pending backoff and try now.
      debugPrint('ServerHttp: Foreground resume — attempting immediate reconnect');
      _reconnectTimer?.cancel();
      _reconnectAttempts = 0;
      _connectInternal();
    }
  }

  // --------------------------------------------------------------------------
  // Internal: connect
  // --------------------------------------------------------------------------

  Future<void> _connectInternal() async {
    if (_dio == null || _host == null) return;

    // Cancel any pending SSE reconnect to avoid overlapping connects.
    _sseReconnectTimer?.cancel();
    _sseReconnectTimer = null;
    _sseReconnectAttempts = 0;

    _connectionState = _reconnectAttempts > 0
        ? ServerConnectionState.reconnecting
        : ServerConnectionState.connecting;
    notifyListeners();

    try {
      debugPrint('ServerHttp: Connecting to ${_dio!.options.baseUrl}/api/state');

      final response = await _dio!.get('api/state');
      final data = response.data as Map<String, dynamic>;

      // Parse hello.
      final hello = ServerHello.fromJson(data);
      _serverPlatformContext = hello.platformContext;
      _listenPort = hello.listenPort;
      _sseDirectAttempted = false;

      // Cache initial room states for diff detection.
      _cachedRoomStates.clear();
      for (final room in hello.rooms) {
        _cachedRoomStates[room.id] = _CachedRoomState(
          rhythmEnabled: room.rhythmEnabled,
          timeOffset: room.timeOffset,
          brightnessOffset: room.brightnessOffset,
          softOff: room.softOff,
          lightsOn: room.lightsOn,
          brightness: room.brightness,
          kelvin: room.kelvin,
          hasMotionSensor: room.hasMotionSensor,
        );
      }

      // Clear suppressed IDs that now appear in hello (room was added to registry).
      _reHelloSuppressedRoomIds.removeAll(_cachedRoomStates.keys);

      // Cache hub connected state.
      _cachedHubConnected = hello.hub['connected'] as bool?;

      // Mark connected.
      _connectionState = ServerConnectionState.connected;
      _reconnectAttempts = 0;
      _consecutivePollFailures = 0;
      notifyListeners();

      // Emit hello.
      _helloController.add(hello);

      // Start poll loop (will be stopped if SSE connects successfully).
      _startPolling();

      // Try SSE for real-time updates (stops polling on success).
      // ESP32 doesn't support SSE — skip to avoid wasting a socket.
      if (hello.platformType == 'embedded') {
        _sseSupported = false;
      } else {
        _connectSse();
      }
    } catch (e) {
      debugPrint('ServerHttp: Failed to connect: $e');
      _scheduleReconnect();
    }
  }

  // --------------------------------------------------------------------------
  // Internal: polling
  // --------------------------------------------------------------------------

  void _startPolling() {
    _stopPolling();
    _poll();
    _pollTimer = Timer.periodic(_pollInterval, (_) => _poll());
  }

  void _stopPolling() {
    _pollTimer?.cancel();
    _pollTimer = null;
    _consecutivePollFailures = 0;
  }

  Future<void> _poll() async {
    if (_dio == null || !connected) return;

    try {
      final response = await _dio!.get('api/rooms/state');
      final data = response.data as Map<String, dynamic>;


      _consecutivePollFailures = 0;

      // Check hub_connected changes.
      final hubConnected = data['hub_connected'] as bool?;
      if (hubConnected != null && hubConnected != _cachedHubConnected) {
        _cachedHubConnected = hubConnected;
        _hubEventController.add(hubConnected ? 'connected' : 'disconnected');
      }

      // Snapshot cached room IDs before diffing (for new-room detection).
      final previousRoomIds = _cachedRoomStates.keys.toSet();

      // Diff room states.
      final rooms = data['rooms'] as List<dynamic>? ?? [];
      final seenRoomIds = <String>{};
      for (final raw in rooms) {
        final roomJson = raw as Map<String, dynamic>;
        final roomId = roomJson['id'] as String? ?? '';
        if (roomId.isEmpty) continue;
        seenRoomIds.add(roomId);

        final rhythmEnabled = roomJson['rhythm_enabled'] as bool? ?? false;
        final timeOffset =
            (roomJson['time_offset'] as num?)?.toDouble() ?? 0.0;
        final brightnessOffset =
            (roomJson['brightness_offset'] as num?)?.toDouble() ?? 0.0;
        final softOff = roomJson['soft_off'] as bool? ?? false;
        final lightsOn = roomJson['lights_on'] as bool?;
        final brightness = (roomJson['brightness'] as num?)?.toInt();
        final kelvin = (roomJson['kelvin'] as num?)?.toInt();

        // Motion fields (omitted when room has no motion tracking).
        // Accept both old names (motion_remaining/motion_timeout/motion_warning)
        // and new SSE-aligned names (remaining_secs/timeout_secs/warning_active).
        final motionActive = roomJson['motion_active'] as bool?;
        final motionOwned = roomJson['motion_owned'] as bool?;
        final motionRemaining = (roomJson['remaining_secs'] as num?)?.toInt()
            ?? (roomJson['motion_remaining'] as num?)?.toInt();
        final motionTimeout = (roomJson['timeout_secs'] as num?)?.toInt()
            ?? (roomJson['motion_timeout'] as num?)?.toInt();
        final cached = _cachedRoomStates[roomId];
        // Server always populates motion_active for rooms with sensors.
        final hasMotionSensor = motionActive != null;
        final rhythmChanged = cached == null ||
            cached.rhythmEnabled != rhythmEnabled ||
            cached.timeOffset != timeOffset ||
            cached.brightnessOffset != brightnessOffset ||
            cached.softOff != softOff ||
            cached.lightsOn != lightsOn ||
            cached.brightness != brightness ||
            cached.kelvin != kelvin;

        final motionChanged = cached == null ||
            cached.motionActive != motionActive ||
            cached.motionOwned != motionOwned ||
            cached.motionRemaining != motionRemaining ||
            cached.motionTimeout != motionTimeout;

        if (rhythmChanged || motionChanged) {
          _cachedRoomStates[roomId] = _CachedRoomState(
            rhythmEnabled: rhythmEnabled,
            timeOffset: timeOffset,
            brightnessOffset: brightnessOffset,
            softOff: softOff,
            lightsOn: lightsOn,
            brightness: brightness,
            kelvin: kelvin,
            motionActive: motionActive,
            motionOwned: motionOwned,
            motionRemaining: motionRemaining,
            motionTimeout: motionTimeout,
            hasMotionSensor: hasMotionSensor,
          );

          if (rhythmChanged) {
            _rhythmStateController.add(ServerRhythmState(
              roomId: roomId,
              rhythmEnabled: rhythmEnabled,
              timeOffset: timeOffset,
              brightnessOffset: brightnessOffset,
              softOff: softOff,
              lightsOn: lightsOn,
              brightness: brightness,
              kelvin: kelvin,
            ));
          }

          if (motionChanged) {
            if (motionActive != null && motionTimeout != null) {
              _motionTimerController.add(ServerMotionTimer(
                roomId: roomId,
                motionActive: motionActive,
                motionOwned: motionOwned ?? false,
                remainingSecs: motionRemaining,
                timeoutSecs: motionTimeout,
              ));
            } else if (cached?.motionActive != null) {
              // Had motion before, now gone — emit clearing event
              _motionTimerController.add(ServerMotionTimer.cleared(roomId));
            }
          }

        }
      }

      // Detect new rooms the client doesn't know about (e.g. server
      // discovered rooms after a credential push). Trigger a re-hello so
      // the sync provider gets full room data (name, grouped_light_id, etc.).
      final newRoomIds = seenRoomIds.difference(previousRoomIds);
      final unsuppressedIds = newRoomIds.difference(_reHelloSuppressedRoomIds);
      if (unsuppressedIds.isNotEmpty && previousRoomIds.isNotEmpty) {
        debugPrint('ServerHttp: Detected ${unsuppressedIds.length} new room(s) in poll, signalling re-hello');
        _reHelloSuppressedRoomIds.addAll(unsuppressedIds);
        _newRoomsController.add(null);
      }

      // Emit clearing events for rooms that previously had motion but are
      // no longer in the response (room removed or motion fully expired).
      for (final entry in _cachedRoomStates.entries.toList()) {
        if (!seenRoomIds.contains(entry.key) &&
            entry.value.motionActive != null) {
          _motionTimerController.add(ServerMotionTimer.cleared(entry.key));
          _cachedRoomStates[entry.key] = _CachedRoomState(
            rhythmEnabled: entry.value.rhythmEnabled,
            timeOffset: entry.value.timeOffset,
            brightnessOffset: entry.value.brightnessOffset,
            softOff: entry.value.softOff,
            hasMotionSensor: entry.value.hasMotionSensor,
          );
        }
      }
    } catch (e) {
      _consecutivePollFailures++;
      debugPrint(
          'ServerHttp: Poll failed ($_consecutivePollFailures/$_maxPollFailures): $e');

      if (_consecutivePollFailures >= _maxPollFailures) {
        debugPrint('ServerHttp: Too many poll failures, reconnecting...');
        _stopPolling();
        _connectionState = ServerConnectionState.reconnecting;
        notifyListeners();
        _scheduleReconnect();
      }
    }
  }

  // --------------------------------------------------------------------------
  // Internal: SSE
  // --------------------------------------------------------------------------

  /// Attempt to connect to the SSE event stream.
  ///
  /// If the server supports SSE (`GET /api/events`), stops polling and
  /// uses the event stream for real-time updates. Falls back to polling
  /// on failure or if the server returns 404.
  ///
  /// Guarded against overlapping calls — returns immediately if already
  /// connecting.
  void _connectSse() async {
    if (_dio == null || !connected || !_sseSupported || _sseConnecting) return;

    _sseConnecting = true;
    _disconnectSse();

    final cancelToken = CancelToken();
    _sseCancelToken = cancelToken;

    // On web inside HA addon: try direct connection to the addon's listen port,
    // bypassing HA ingress which can drop SSE connections.
    String sseBaseUrl = _dio!.options.baseUrl;
    final useDirectSse = kIsWeb &&
        _serverPlatformContext == 'ha_addon' &&
        _listenPort != null &&
        !_sseDirectAttempted;
    if (useDirectSse) {
      final host = Uri.base.host;
      sseBaseUrl = 'http://$host:$_listenPort/';
      _sseDirectAttempted = true;
      debugPrint('ServerHttp: SSE trying direct connection to $sseBaseUrl');
    }

    try {
      // Use a dedicated Dio instance for SSE with a longer connect timeout.
      // Dio's Options class doesn't expose connectTimeout, and HA ingress
      // proxying can add significant latency to the initial SSE handshake.
      final sseDio = Dio(BaseOptions(
        baseUrl: sseBaseUrl,
        connectTimeout: const Duration(seconds: 30),
        receiveTimeout: Duration.zero,
      ));
      final response = await sseDio.get(
        'api/events',
        options: Options(
          responseType: ResponseType.stream,
          headers: {'Accept': 'text/event-stream'},
        ),
        cancelToken: cancelToken,
      );

      final stream = (response.data as ResponseBody?)?.stream;
      if (stream == null) {
        debugPrint('ServerHttp: SSE response has no stream');
        _sseConnecting = false;
        return;
      }

      _sseConnected = true;
      _sseConnecting = false;
      _sseReconnectAttempts = 0; // Reset backoff on successful connect
      _lastSseActivity = DateTime.now();
      _stopPolling(); // SSE replaces polling
      _startSseWatchdog();
      debugPrint('ServerHttp: SSE connected');

      String? eventType;
      final dataBuffer = StringBuffer();

      _sseSubscription = stream
          .cast<List<int>>()
          .transform(utf8.decoder)
          .transform(const LineSplitter())
          .listen(
        (line) {
          // Track activity for keepalive watchdog — every line counts,
          // including `:` comments (server keepalives) and empty lines.
          _lastSseActivity = DateTime.now();

          if (line.startsWith('event:')) {
            eventType = line.substring(6).trim();
          } else if (line.startsWith('data:')) {
            if (dataBuffer.isNotEmpty) dataBuffer.write('\n');
            dataBuffer.write(line.substring(5).trim());
          } else if (line.isEmpty && eventType != null) {
            _handleSseEvent(eventType!, dataBuffer.toString());
            eventType = null;
            dataBuffer.clear();
          } else if (line.isEmpty) {
            // Reset state on empty line even without event type.
            dataBuffer.clear();
          }
          // Lines starting with ':' are comments (keep-alive) — tracked
          // above for watchdog, no further processing needed.
        },
        onError: (error) {
          if (!cancelToken.isCancelled) {
            debugPrint('ServerHttp: SSE error: $error');
            _handleSseDisconnect();
          }
        },
        onDone: () {
          if (!cancelToken.isCancelled) {
            debugPrint('ServerHttp: SSE stream closed');
            _handleSseDisconnect();
          }
        },
      );
    } on DioException catch (e) {
      _sseConnecting = false;
      if (e.type == DioExceptionType.cancel) return;
      if (e.response?.statusCode == 404) {
        _sseSupported = false;
        debugPrint('ServerHttp: SSE not supported (404), using polling');
      } else if (useDirectSse) {
        // Direct SSE failed (e.g. mixed content on HTTPS) — retry through ingress.
        debugPrint('ServerHttp: Direct SSE failed, falling back to ingress: $e');
        _connectSse();
      } else {
        debugPrint('ServerHttp: SSE connection failed: $e');
        _handleSseDisconnect();
      }
    } catch (e) {
      _sseConnecting = false;
      if (useDirectSse) {
        debugPrint('ServerHttp: Direct SSE failed, falling back to ingress: $e');
        _connectSse();
      } else {
        debugPrint('ServerHttp: SSE unexpected error: $e');
        _handleSseDisconnect();
      }
    }
  }

  /// Handle a parsed SSE event.
  void _handleSseEvent(String eventType, String data) {
    try {
      switch (eventType) {
        case 'room_state':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final rooms = (json['data']
                      as Map<String, dynamic>?)?['rooms']
                  as List<dynamic>? ??
              [];
          for (final raw in rooms) {
            final room = raw as Map<String, dynamic>;
            final roomId = room['id'] as String? ?? '';
            if (roomId.isEmpty) continue;

            final rhythmEnabled =
                room['rhythm_enabled'] as bool? ?? false;
            final timeOffset =
                (room['time_offset'] as num?)?.toDouble() ?? 0.0;
            final brightnessOffset =
                (room['brightness_offset'] as num?)?.toDouble() ?? 0.0;
            final softOff = room['soft_off'] as bool? ?? false;
            final lightsOn = room['lights_on'] as bool?;
            final brightness = (room['brightness'] as num?)?.toInt();
            final kelvin = (room['kelvin'] as num?)?.toInt();

            // Preserve existing motion cache fields.
            final existing = _cachedRoomStates[roomId];
            _cachedRoomStates[roomId] = _CachedRoomState(
              rhythmEnabled: rhythmEnabled,
              timeOffset: timeOffset,
              brightnessOffset: brightnessOffset,
              softOff: softOff,
              lightsOn: lightsOn,
              brightness: brightness,
              kelvin: kelvin,
              motionActive: existing?.motionActive,
              motionOwned: existing?.motionOwned,
              motionRemaining: existing?.motionRemaining,
              motionTimeout: existing?.motionTimeout,
              hasMotionSensor: existing?.hasMotionSensor ?? false,
            );

            _rhythmStateController.add(ServerRhythmState(
              roomId: roomId,
              rhythmEnabled: rhythmEnabled,
              timeOffset: timeOffset,
              brightnessOffset: brightnessOffset,
              softOff: softOff,
              lightsOn: lightsOn,
              brightness: brightness,
              kelvin: kelvin,
            ));
          }

        case 'motion_timer':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final timers = (json['data']
                      as Map<String, dynamic>?)?['timers']
                  as List<dynamic>? ??
              [];

          final seenRoomIds = <String>{};
          for (final raw in timers) {
            final timer = raw as Map<String, dynamic>;
            final roomId = timer['room_id'] as String? ?? '';
            if (roomId.isEmpty) continue;
            seenRoomIds.add(roomId);

            final motionActive =
                timer['motion_active'] as bool? ?? false;
            final motionOwned =
                timer['motion_owned'] as bool? ?? false;
            final remainingSecs =
                (timer['remaining_secs'] as num?)?.toInt();
            final timeoutSecs =
                (timer['timeout_secs'] as num?)?.toInt() ?? 0;

            // Update motion fields in cache.
            final cached = _cachedRoomStates[roomId];
            if (cached != null) {
              _cachedRoomStates[roomId] = _CachedRoomState(
                rhythmEnabled: cached.rhythmEnabled,
                timeOffset: cached.timeOffset,
                brightnessOffset: cached.brightnessOffset,
                softOff: cached.softOff,
                motionActive: motionActive,
                motionOwned: motionOwned,
                motionRemaining: remainingSecs,
                motionTimeout: timeoutSecs,
                hasMotionSensor: true,
              );
            }

            _motionTimerController.add(ServerMotionTimer(
              roomId: roomId,
              motionActive: motionActive,
              motionOwned: motionOwned,
              remainingSecs: remainingSecs,
              timeoutSecs: timeoutSecs,
            ));
          }

          // Clear rooms that had motion but aren't in this event.
          for (final entry in _cachedRoomStates.entries.toList()) {
            if (entry.value.motionActive != null &&
                !seenRoomIds.contains(entry.key)) {
              _motionTimerController
                  .add(ServerMotionTimer.cleared(entry.key));
              _cachedRoomStates[entry.key] = _CachedRoomState(
                rhythmEnabled: entry.value.rhythmEnabled,
                timeOffset: entry.value.timeOffset,
                brightnessOffset: entry.value.brightnessOffset,
                softOff: entry.value.softOff,
                hasMotionSensor: entry.value.hasMotionSensor,
              );
            }
          }

        case 'hub_status':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final hubConnected = (json['data']
                  as Map<String, dynamic>?)?['connected'] as bool? ??
              false;
          if (hubConnected != _cachedHubConnected) {
            _cachedHubConnected = hubConnected;
            _hubEventController
                .add(hubConnected ? 'connected' : 'disconnected');
          }

        case 'settings_changed':
        case 'config_changed':
        case 'rooms_changed':
          // Legitimate server-side change — clear suppression before re-hello.
          _reHelloSuppressedRoomIds.clear();
          _newRoomsController.add(null);

        case 'triage_changed':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final payload = json['data'] as Map<String, dynamic>? ?? json;
          _triageChangedController.add(payload);

        case 'lagged':
          debugPrint('ServerHttp: SSE lagged, triggering reconnect');
          reconnect();
      }
    } catch (e) {
      debugPrint('ServerHttp: SSE event parse error ($eventType): $e');
    }
  }

  /// Clean up SSE connection.
  void _disconnectSse() {
    _sseCancelToken?.cancel();
    _sseCancelToken = null;
    _sseSubscription?.cancel();
    _sseSubscription = null;
    _sseConnected = false;
    _sseConnecting = false;
    _stopSseWatchdog();
  }

  /// Handle SSE disconnection — fall back to polling and schedule SSE reconnect
  /// with exponential backoff.
  void _handleSseDisconnect() {
    _disconnectSse();
    if (connected) {
      _startPolling();
    }

    // Exponential backoff for SSE reconnect: 2s, 5s, 10s, 20s, max 30s.
    final delaySecs = switch (_sseReconnectAttempts) {
      0 => 2,
      1 => 5,
      2 => 10,
      3 => 20,
      _ => 30,
    };
    _sseReconnectAttempts++;

    debugPrint('ServerHttp: SSE reconnect in ${delaySecs}s (attempt $_sseReconnectAttempts)');

    _sseReconnectTimer?.cancel();
    _sseReconnectTimer = Timer(Duration(seconds: delaySecs), () {
      if (connected && _sseSupported) {
        _connectSse();
      }
    });
  }

  /// Start the SSE watchdog timer that checks for keepalive staleness.
  void _startSseWatchdog() {
    _stopSseWatchdog();
    _sseWatchdogTimer = Timer.periodic(_sseWatchdogInterval, (_) {
      if (!_sseConnected) {
        _stopSseWatchdog();
        return;
      }
      final elapsed = DateTime.now().difference(_lastSseActivity);
      if (elapsed > _sseStaleThreshold) {
        debugPrint('ServerHttp: SSE stale (no activity for ${elapsed.inSeconds}s), reconnecting');
        _handleSseDisconnect();
      }
    });
  }

  /// Stop the SSE watchdog timer.
  void _stopSseWatchdog() {
    _sseWatchdogTimer?.cancel();
    _sseWatchdogTimer = null;
  }

  // --------------------------------------------------------------------------
  // Internal: reconnect
  // --------------------------------------------------------------------------

  void _scheduleReconnect() {
    _reconnectTimer?.cancel();

    // Exponential backoff: 1s, 2s, 4s, 8s, 16s, max 30s.
    final delaySecs = (1 << _reconnectAttempts).clamp(1, 30);
    _reconnectAttempts++;

    debugPrint(
        'ServerHttp: Reconnecting in ${delaySecs}s (attempt $_reconnectAttempts)');

    _reconnectTimer = Timer(Duration(seconds: delaySecs), () {
      if (_host != null) {
        _connectInternal();
      }
    });
  }

  // --------------------------------------------------------------------------
  // Internal: safe HTTP helpers
  // --------------------------------------------------------------------------

  /// Parse a `{"rooms":[...]}` response into a list of [ServerRhythmState],
  /// updating the cache for each parsed room.
  List<ServerRhythmState> _parseRoomsResponse(dynamic responseData) {
    final data = responseData as Map<String, dynamic>?;
    final rooms = data?['rooms'] as List<dynamic>?;
    if (rooms == null) return [];
    final results = <ServerRhythmState>[];
    for (final r in rooms) {
      if (r is Map<String, dynamic> && r.containsKey('rhythm_enabled')) {
        final state = ServerRhythmState.fromJson(r);
        final existing = _cachedRoomStates[state.roomId];
        _cachedRoomStates[state.roomId] = _CachedRoomState(
          rhythmEnabled: state.rhythmEnabled,
          timeOffset: state.timeOffset,
          brightnessOffset: state.brightnessOffset,
          softOff: state.softOff,
          lightsOn: state.lightsOn ?? existing?.lightsOn,
          brightness: state.brightness ?? existing?.brightness,
          kelvin: state.kelvin ?? existing?.kelvin,
          motionActive: existing?.motionActive,
          motionOwned: existing?.motionOwned,
          motionRemaining: existing?.motionRemaining,
          motionTimeout: existing?.motionTimeout,
          hasMotionSensor: existing?.hasMotionSensor ?? false,
        );
        results.add(state);
      }
    }
    return results;
  }

  /// Fire-and-forget PUT. Logs errors but does not throw.
  Future<void> _safePut(
    String path, {
    required Object data,
    Duration? timeout,
  }) async {
    if (_dio == null || !connected) {
      debugPrint('ServerHttp: Cannot PUT $path, not connected');
      return;
    }
    try {
      final options =
          timeout != null ? Options(receiveTimeout: timeout) : null;
      await _dio!.put(path, data: data, options: options);
    } catch (e) {
      debugPrint('ServerHttp: PUT $path failed: $e');
    }
  }

  // --------------------------------------------------------------------------
  // Internal: helpers
  // --------------------------------------------------------------------------

  /// Build the Dio base URL from host and port.
  ///
  /// On web, uses [Uri.base] so that requests go through the same origin
  /// (including any reverse-proxy path prefix such as HA addon ingress).
  /// Paths in this client are relative (no leading '/') so Dio appends
  /// them to the base URL path rather than replacing it.
  static String _buildBaseUrl(String host, int port) {
    if (kIsWeb) {
      // Uri.base includes the full page URL (e.g. ingress path prefix).
      // Keep trailing slash so relative paths (e.g. 'api/state') concatenate
      // correctly — Dio uses simple string concatenation (baseUrl + path).
      final base = Uri.base.toString();
      return base.endsWith('/') ? base : '$base/';
    }
    return 'http://$host:$port/';
  }

  Map<String, dynamic> _curveConfigToJson(CurveConfigDto config) {
    return {
      'min_brightness': config.minBrightness,
      'max_brightness': config.maxBrightness,
      'min_color_temp': config.minColorTemp,
      'max_color_temp': config.maxColorTemp,
      'width_left_bri': config.widthLeftBri,
      'width_right_bri': config.widthRightBri,
      'width_left_cct': config.widthLeftCct,
      'width_right_cct': config.widthRightCct,
      'shape_p': config.shapeP,
      'max_dim_steps': config.maxDimSteps,
    };
  }

  @override
  void dispose() {
    disconnect();
    _helloController.close();
    _rhythmStateController.close();
    _hubEventController.close();
    _motionTimerController.close();
    _newRoomsController.close();
    _triageChangedController.close();
    super.dispose();
  }
}

// ---------------------------------------------------------------------------
// DeviceDiagClient — lightweight standalone client for diagnostics
// ---------------------------------------------------------------------------

/// Lightweight HTTP client for device diagnostic endpoints.
///
/// Unlike [ServerHttpClient], this can be instantiated directly with a host
/// for one-off operations like health checks and diagnostics. Used by
/// discovery screens and settings panels.
class DeviceDiagClient {
  final Dio _dio;
  final String host;

  DeviceDiagClient({required this.host, int? port})
      : _dio = Dio(BaseOptions(
          baseUrl: ServerHttpClient._buildBaseUrl(
            host,
            port ?? 80,
          ),
          connectTimeout: const Duration(seconds: 5),
          receiveTimeout: const Duration(seconds: 5),
        ));

  /// Check if the device is reachable.
  Future<bool> healthCheck() async {
    try {
      final response = await _dio.get('health');
      return response.data['status'] == 'healthy';
    } catch (_) {
      return false;
    }
  }

  /// Get diagnostic vitals.
  Future<Map<String, dynamic>?> getDiagVitals() async {
    try {
      final response = await _dio.get('api/diag/vitals');
      return Map<String, dynamic>.from(response.data);
    } catch (_) {
      return null;
    }
  }

  /// Get diagnostic log entries.
  ///
  /// Returns a list of log entries, each with `ts`, `level`, `cat`, and `msg`.
  /// [limit] controls max entries (default 50). [category] filters by log
  /// category (conn, hub, cmd, evt, sys).
  Future<List<Map<String, dynamic>>?> getDiagLogs({
    int limit = 50,
    String? category,
  }) async {
    try {
      final params = <String, dynamic>{'limit': limit};
      if (category != null) params['cat'] = category;
      final response = await _dio.get(
        'api/diag/logs',
        queryParameters: params,
      );
      final data = response.data as Map<String, dynamic>;
      final logs = data['logs'] as List<dynamic>? ?? [];
      return logs.cast<Map<String, dynamic>>();
    } catch (_) {
      return null;
    }
  }

  /// Clear persisted crash info.
  Future<bool> clearCrashInfo() async {
    try {
      await _dio.delete('api/diag/crash');
      return true;
    } catch (_) {
      return false;
    }
  }

  /// Reset WiFi credentials, putting the device back in setup mode.
  Future<bool> resetWifi() async {
    try {
      await _dio.delete('api/wifi');
      return true;
    } catch (_) {
      return false;
    }
  }

  /// Reboot the device. Will restart after ~2s.
  Future<bool> reboot() async {
    try {
      await _dio.post('api/system/reboot');
      return true;
    } catch (_) {
      return false;
    }
  }
}

// ---------------------------------------------------------------------------
// Internal cached state for diff detection
// ---------------------------------------------------------------------------

class _CachedRoomState {
  final bool rhythmEnabled;
  final double timeOffset;
  final double brightnessOffset;
  final bool softOff;
  final bool? lightsOn;
  final int? brightness;
  final int? kelvin;
  final bool? motionActive;
  final bool? motionOwned;
  final int? motionRemaining;
  final int? motionTimeout;
  final bool hasMotionSensor;

  const _CachedRoomState({
    required this.rhythmEnabled,
    required this.timeOffset,
    required this.brightnessOffset,
    required this.softOff,
    this.lightsOn,
    this.brightness,
    this.kelvin,
    this.motionActive,
    this.motionOwned,
    this.motionRemaining,
    this.motionTimeout,
    this.hasMotionSensor = false,
  });
}
