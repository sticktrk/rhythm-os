/// Real Hue bridge service implementation.
///
/// Connects to actual Philips Hue bridges via REST API and SSE.
/// Wraps existing HueProvider and HueRoomSync functionality.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';

import '../settings_service.dart';
import 'hue_bridge_service.dart';

/// Real implementation of [HueBridgeService].
///
/// Connects to actual Philips Hue bridges and handles:
/// - Bridge discovery and pairing via mDNS
/// - Room operations via Hue REST API
/// - SSE events for button presses
class RealHueBridgeService implements HueBridgeService {
  static final RealHueBridgeService instance = RealHueBridgeService._();

  RealHueBridgeService._();

  HueConfig? _config;

  // SSE resources
  HueSseSource? _sseSource;
  HueDeviceRegistry? _deviceRegistry;
  StreamSubscription<InputEvent>? _sseSubscription;
  StreamSubscription<EventSourceState>? _sseStateSubscription;
  final StreamController<EventSourceState> _sseStateController =
      StreamController<EventSourceState>.broadcast();

  // Callback for button events
  void Function(String roomId, RhythmActionDto action)? _onButtonEvent;

  // Light monitoring SSE resources (separate from button SSE)
  HueSseSource? _lightMonitorSseSource;
  StreamSubscription<EventSourceState>? _lightMonitorStateSubscription;
  void Function(String roomId, bool isOn)? _onLightStateChanged;

  // Cached rooms for auto-mapping
  List<RoomDto> _rooms = [];

  // V2 API cache: room UUID -> grouped_light resource ID
  final Map<String, String> _roomToGroupedLight = {};

  /// Connection timeout for REST API calls.
  /// 30s allows for slow home WiFi without false disconnections.
  static const _connectionTimeout = Duration(seconds: 30);

  // Shared HTTP client for REST API calls (avoids fd leak from creating
  // a new HttpClient per request - each creates its own connection pool).
  HttpClient? _restClient;

  /// Get or create the shared REST API HttpClient.
  HttpClient get _httpClient {
    if (_restClient == null) {
      _restClient = HttpClient()
        ..connectionTimeout = _connectionTimeout
        ..idleTimeout = const Duration(seconds: 15);
      _restClient!.badCertificateCallback = (cert, host, port) => true;
    }
    return _restClient!;
  }

  /// Close the shared REST API HttpClient (e.g. on reset or dispose).
  void _closeRestClient() {
    _restClient?.close(force: true);
    _restClient = null;
  }

  @override
  HueConfig? get config => _config;

  @override
  void configure(HueConfig config) {
    _config = config;

    // Restore persisted grouped_light mappings
    if (_roomToGroupedLight.isEmpty) {
      final saved = SettingsService.instance.getHueGroupedLightMap();
      if (saved != null && saved.isNotEmpty) {
        _roomToGroupedLight.addAll(saved);
        debugPrint('RealHueBridgeService: Restored ${saved.length} grouped_light mappings');
      }
    }

    // Restore persisted device registry (gives immediate device data
    // when ESP32 connects before a fresh room fetch)
    if (_deviceRegistry == null) {
      final cachedRegistry = SettingsService.instance.getHueDeviceRegistry();
      if (cachedRegistry != null) {
        _deviceRegistry = HueDeviceRegistry(
          bridgeIp: config.bridgeIp,
          applicationKey: config.username,
        );
        _deviceRegistry!.loadFromJson(cachedRegistry);
        debugPrint('RealHueBridgeService: Restored cached device registry with ${_deviceRegistry!.devices.length} devices');
      }
    }
  }

  @override
  void reset() {
    _closeRestClient();
    _config = null;
    _rooms = [];
    _deviceRegistry = null;
    _roomToGroupedLight.clear();
    SettingsService.instance.clearHueGroupedLightMap();
  }

  // ============================================================================
  // Bridge Discovery and Pairing
  // ============================================================================

  @override
  Future<List<String>> discoverBridges({Duration? timeout}) {
    return HueProvider.discoverBridges(
      timeout: timeout ?? const Duration(seconds: 8),
    );
  }

  @override
  Future<String?> pair(String bridgeIp) {
    return HueProvider.pair(bridgeIp);
  }

  // ============================================================================
  // Room Operations
  // ============================================================================

  @override
  Future<List<RoomDto>> fetchRooms() async {
    if (_config == null) {
      throw StateError('Service not configured. Call configure() first.');
    }

    final roomData = await _fetchV2Resource('room');
    _roomToGroupedLight.clear();

    final rooms = <RoomDto>[];

    for (final room in roomData) {
      final id = room['id'] as String?;
      final metadata = room['metadata'] as Map<String, dynamic>?;
      final name = metadata?['name'] as String?;

      if (id == null || name == null) continue;

      // Find grouped_light service for this room
      final services = room['services'] as List<dynamic>? ?? [];
      String? groupedLightId;

      for (final service in services) {
        if (service is! Map<String, dynamic>) continue;
        final rtype = service['rtype'] as String?;
        final rid = service['rid'] as String?;
        if (rid == null) continue;

        if (rtype == 'grouped_light') {
          groupedLightId = rid;
          break;
        }
      }

      // Get device children (these are the lights in the room)
      final children = room['children'] as List<dynamic>? ?? [];
      final deviceIds = <String>[];

      for (final child in children) {
        if (child is! Map<String, dynamic>) continue;
        final rtype = child['rtype'] as String?;
        final rid = child['rid'] as String?;
        if (rtype == 'device' && rid != null) {
          deviceIds.add(rid);
        }
      }

      // Cache the grouped_light mapping for control operations
      if (groupedLightId != null) {
        _roomToGroupedLight[id] = groupedLightId;
      }

      rooms.add(RoomDto.raw(
        id: id, // Use raw UUID as room ID
        name: name,
        source: RoomSourceDto.hue,
        deviceIds: deviceIds,
        rhythmEnabled: false,
        disabled: false,
        lightsOn: false, // Will be updated below
        timeOffsetMinutes: 0.0,
        brightnessOffset: 0.0,
        curveConfig: null,
      ));
    }

    // Fetch actual light states and update rooms
    final lightStates = await fetchAllRoomStates();
    final updatedRooms = rooms.map((room) {
      final isOn = lightStates[room.id] ?? false;
      if (isOn != room.lightsOn) {
        return RoomDto.raw(
          id: room.id,
          name: room.name,
          source: room.source,
          deviceIds: room.deviceIds,
          rhythmEnabled: room.rhythmEnabled,
          disabled: room.disabled,
          lightsOn: isOn,
          timeOffsetMinutes: room.timeOffsetMinutes,
          brightnessOffset: room.brightnessOffset,
          curveConfig: room.curveConfig,
        );
      }
      return room;
    }).toList();

    // Persist grouped_light mappings so they survive app restarts
    if (_roomToGroupedLight.isNotEmpty) {
      SettingsService.instance
          .saveHueGroupedLightMap(Map<String, String>.from(_roomToGroupedLight));
    }

    debugPrint('RealHueBridgeService: Fetched ${updatedRooms.length} rooms via v2 API');

    // Discover devices alongside rooms so they flow to ESP32
    await _discoverDevices(updatedRooms);

    return updatedRooms;
  }

  @override
  Future<bool> isRoomOn(String roomId) async {
    if (_config == null) return false;

    final groupedLightId = getGroupedLightId(roomId);
    if (groupedLightId == null) return false;

    final uri = Uri.parse(
      'https://${_config!.bridgeIp}/clip/v2/resource/grouped_light/$groupedLightId',
    );

    final request = await _httpClient.getUrl(uri);
    request.headers.set('hue-application-key', _config!.username);

    final response = await request.close();

    if (response.statusCode == 200) {
      final body = await response.transform(utf8.decoder).join();
      final json = jsonDecode(body) as Map<String, dynamic>;
      final data = json['data'] as List<dynamic>?;
      if (data != null && data.isNotEmpty) {
        final groupedLight = data[0] as Map<String, dynamic>;
        final on = groupedLight['on'] as Map<String, dynamic>?;
        return on?['on'] == true;
      }
    }
    return false;
  }

  @override
  Future<bool> toggleRoom(String roomId, {int? brightness, int? mireds}) async {
    if (_config == null) return false;

    final groupedLightId = getGroupedLightId(roomId);
    if (groupedLightId == null) return false;

    final anyOn = await isRoomOn(roomId);
    final newState = !anyOn;

    final uri = Uri.parse(
      'https://${_config!.bridgeIp}/clip/v2/resource/grouped_light/$groupedLightId',
    );

    final body = <String, dynamic>{
      'on': {'on': newState},
    };

    if (newState) {
      if (brightness != null) {
        body['dimming'] = {'brightness': brightness.toDouble().clamp(1.0, 100.0)};
      }
      if (mireds != null) {
        body['color_temperature'] = {'mirek': mireds.clamp(153, 500)};
      }
    }

    final request = await _httpClient.putUrl(uri);
    request.headers.set('hue-application-key', _config!.username);
    request.headers.set('Content-Type', 'application/json');
    request.add(utf8.encode(jsonEncode(body)));
    await request.close();

    return newState;
  }

  @override
  Future<void> setRoomState(
    String roomId, {
    required bool on,
    int? brightness,
    int? mireds,
  }) async {
    if (_config == null) return;

    final groupedLightId = getGroupedLightId(roomId);
    if (groupedLightId == null) return;

    final uri = Uri.parse(
      'https://${_config!.bridgeIp}/clip/v2/resource/grouped_light/$groupedLightId',
    );

    final body = <String, dynamic>{
      'on': {'on': on},
    };

    if (on) {
      if (brightness != null) {
        body['dimming'] = {'brightness': brightness.toDouble().clamp(1.0, 100.0)};
      }
      if (mireds != null) {
        body['color_temperature'] = {'mirek': mireds.clamp(153, 500)};
      }
    }

    final request = await _httpClient.putUrl(uri);
    request.headers.set('hue-application-key', _config!.username);
    request.headers.set('Content-Type', 'application/json');
    request.add(utf8.encode(jsonEncode(body)));
    await request.close();
  }

  @override
  Future<Map<String, bool>> fetchAllRoomStates() async {
    if (_config == null) return {};

    // Build reverse mapping: grouped_light ID -> room UUID
    final groupedLightToRoom = <String, String>{};
    for (final entry in _roomToGroupedLight.entries) {
      groupedLightToRoom[entry.value] = entry.key;
    }

    final groupedLightData = await _fetchV2Resource('grouped_light');
    final states = <String, bool>{};

    for (final groupedLight in groupedLightData) {
      final groupedLightId = groupedLight['id'] as String?;
      if (groupedLightId == null) continue;

      // Find the room UUID for this grouped_light
      final roomId = groupedLightToRoom[groupedLightId];
      if (roomId == null) continue;

      final on = groupedLight['on'] as Map<String, dynamic>?;
      states[roomId] = on?['on'] == true;
    }

    return states;
  }

  // ============================================================================
  // SSE for Button Events
  // ============================================================================

  @override
  bool get supportsSse => true;

  @override
  EventSourceState get sseState => _sseSource?.state ?? EventSourceState.disconnected;

  @override
  Stream<EventSourceState> get sseStateChanges => _sseStateController.stream;

  @override
  Future<void> initializeSse({
    required List<RoomDto> rooms,
    required void Function(String roomId, RhythmActionDto action) onButtonEvent,
  }) async {
    if (_config == null) return;

    // Dispose any existing SSE connection first
    await disposeSse();

    // Store callback and rooms after dispose (dispose clears these)
    _rooms = rooms;
    _onButtonEvent = onButtonEvent;

    try {
      debugPrint('RealHueBridgeService: Initializing SSE...');

      // Create device registry
      _deviceRegistry = HueDeviceRegistry(
        bridgeIp: _config!.bridgeIp,
        applicationKey: _config!.username,
      );

      // Load cached registry
      final cachedRegistry = SettingsService.instance.getHueDeviceRegistry();
      if (cachedRegistry != null) {
        _deviceRegistry!.loadFromJson(cachedRegistry);
        debugPrint('RealHueBridgeService: Loaded cached registry with ${_deviceRegistry!.devices.length} devices');
      }

      // Discover devices
      await _deviceRegistry!.discover();
      debugPrint('RealHueBridgeService: Discovered ${_deviceRegistry!.devices.length} switches, ${_deviceRegistry!.buttons.length} buttons');

      // Fetch behavior_instances
      await _deviceRegistry!.fetchBehaviorInstances();
      debugPrint('RealHueBridgeService: ${_deviceRegistry!.configuredDeviceIds.length} devices configured in Hue app');

      // Auto-map Hue rooms to Rhythm rooms
      await _autoMapRooms();

      // Save updated registry
      await SettingsService.instance.saveHueDeviceRegistry(_deviceRegistry!.toJson());

      // Create SSE source
      _sseSource = HueSseSource(
        config: HueSseConfig(
          bridgeIp: _config!.bridgeIp,
          applicationKey: _config!.username,
        ),
        resourceToRoomMapper: (deviceId) => _deviceRegistry!.getRoomForDevice(deviceId),
        buttonControlIdLookup: (buttonId) => _deviceRegistry!.getButton(buttonId)?.controlId,
        isDeviceConfigured: (deviceId) => _deviceRegistry!.isDeviceConfigured(deviceId),
        onBehaviorInstanceEvent: _handleBehaviorInstanceEvent,
        onButtonEvent: _handleButtonEvent,
      );

      // Subscribe to events
      _sseSubscription = _sseSource!.events.listen((event) {
        _onButtonEvent?.call(event.roomId, event.action);
      });

      // Subscribe to state changes and forward to broadcast controller
      _sseStateSubscription = _sseSource!.stateChanges.listen((state) {
        debugPrint('RealHueBridgeService: SSE state changed to $state');
        _sseStateController.add(state);
      });

      // Connect
      await _sseSource!.connect();
      debugPrint('RealHueBridgeService: SSE connected');
    } catch (e) {
      debugPrint('RealHueBridgeService: Failed to initialize SSE: $e');
    }
  }

  @override
  Future<void> disposeSse() async {
    await _sseSubscription?.cancel();
    _sseSubscription = null;

    await _sseStateSubscription?.cancel();
    _sseStateSubscription = null;

    await _sseSource?.dispose();
    _sseSource = null;

    _closeRestClient();

    // Emit disconnected state
    _sseStateController.add(EventSourceState.disconnected);

    _deviceRegistry = null;
    _onButtonEvent = null;
  }

  @override
  Future<void> resyncDevices() async {
    if (_deviceRegistry == null || _config == null) {
      debugPrint('RealHueBridgeService: Cannot resync - not configured');
      return;
    }

    debugPrint('RealHueBridgeService: Resyncing devices...');

    try {
      await _deviceRegistry!.discover();
      debugPrint('RealHueBridgeService: Discovered ${_deviceRegistry!.devices.length} switches');

      await _deviceRegistry!.fetchBehaviorInstances();
      debugPrint('RealHueBridgeService: ${_deviceRegistry!.configuredDeviceIds.length} devices configured');

      await _autoMapRooms();
      await SettingsService.instance.saveHueDeviceRegistry(_deviceRegistry!.toJson());

      debugPrint('RealHueBridgeService: Resync complete');
    } catch (e) {
      debugPrint('RealHueBridgeService: Resync failed: $e');
      rethrow;
    }
  }

  // ============================================================================
  // Light State Monitoring
  // ============================================================================

  @override
  Future<void> startLightMonitoring({
    required void Function(String roomId, bool isOn) onLightStateChanged,
  }) async {
    if (_config == null) return;

    // Need grouped_light mappings to reverse-lookup room IDs
    if (_roomToGroupedLight.isEmpty) {
      debugPrint('RealHueBridgeService: Cannot start light monitor — no grouped_light mappings');
      return;
    }

    // Dispose any existing light monitor SSE first
    await stopLightMonitoring();

    _onLightStateChanged = onLightStateChanged;

    try {
      _lightMonitorSseSource = HueSseSource(
        config: HueSseConfig(
          bridgeIp: _config!.bridgeIp,
          applicationKey: _config!.username,
        ),
        resourceToRoomMapper: (_) => null,
        buttonControlIdLookup: (_) => null,
        onGroupedLightEvent: _handleGroupedLightEvent,
      );

      _lightMonitorStateSubscription = _lightMonitorSseSource!.stateChanges.listen((state) {
        debugPrint('RealHueBridgeService: Light monitor SSE state: $state');
      });

      await _lightMonitorSseSource!.connect();
      debugPrint('RealHueBridgeService: Light monitor SSE connected');
    } catch (e) {
      debugPrint('RealHueBridgeService: Failed to start light monitor: $e');
    }
  }

  @override
  Future<void> stopLightMonitoring() async {
    await _lightMonitorStateSubscription?.cancel();
    _lightMonitorStateSubscription = null;

    await _lightMonitorSseSource?.dispose();
    _lightMonitorSseSource = null;

    _onLightStateChanged = null;
  }

  void _handleGroupedLightEvent(String groupedLightId, bool isOn) {
    // Reverse-lookup: grouped_light ID -> room ID
    String? roomId;
    for (final entry in _roomToGroupedLight.entries) {
      if (entry.value == groupedLightId) {
        roomId = entry.key;
        break;
      }
    }

    if (roomId == null) return;

    _onLightStateChanged?.call(roomId, isOn);
  }

  // ============================================================================
  // Geolocation
  // ============================================================================

  @override
  Future<(double, double)?> fetchGeolocation() async {
    if (_config == null) return null;

    try {
      final data = await _fetchV2Resource('geolocation');
      if (data.isEmpty) return null;

      final geo = data[0] as Map<String, dynamic>;
      final lat = geo['latitude'] as num?;
      final lng = geo['longitude'] as num?;

      if (lat != null && lng != null) {
        return (lat.toDouble(), lng.toDouble());
      }
      return null;
    } catch (e) {
      debugPrint('RealHueBridgeService: Failed to fetch geolocation: $e');
      return null;
    }
  }

  // ============================================================================
  // Private Helpers
  // ============================================================================

  /// Get the grouped_light resource ID for a room.
  ///
  /// For demo mode rooms (hue_demo_N), returns null.
  /// For v2 API rooms, looks up the cached mapping.
  String? getGroupedLightId(String roomId) {
    if (roomId.startsWith('hue_demo_')) return null;
    return _roomToGroupedLight[roomId];
  }

  /// Get all room-to-grouped-light mappings.
  ///
  /// Used to push grouped_light_ids with room_set.
  Map<String, String> get roomToGroupedLight =>
      Map.unmodifiable(_roomToGroupedLight);

  /// Get the device registry for reading device/button data.
  ///
  /// Used to push device_set messages.
  HueDeviceRegistry? get deviceRegistry => _deviceRegistry;

  /// Fetch a v2 API resource type.
  Future<List<dynamic>> _fetchV2Resource(String resourceType) async {
    if (_config == null) return [];

    final uri = Uri.parse(
      'https://${_config!.bridgeIp}/clip/v2/resource/$resourceType',
    );

    final request = await _httpClient.getUrl(uri);
    request.headers.set('hue-application-key', _config!.username);

    final response = await request.close();

    if (response.statusCode != 200) {
      throw HttpException(
        'Failed to fetch $resourceType: ${response.statusCode}',
        uri: uri,
      );
    }

    final body = await response.transform(utf8.decoder).join();
    final json = jsonDecode(body) as Map<String, dynamic>;

    return json['data'] as List<dynamic>? ?? [];
  }

  /// Discover devices (switches/remotes) and map them to rooms.
  ///
  /// Called from [fetchRooms] so device data flows alongside room data
  /// without depending on the SSE code path.
  /// Non-fatal — rooms still work without device data.
  Future<void> _discoverDevices(List<RoomDto> rooms) async {
    if (_config == null) return;

    try {
      _deviceRegistry ??= HueDeviceRegistry(
        bridgeIp: _config!.bridgeIp,
        applicationKey: _config!.username,
      );

      await _deviceRegistry!.discover();
      debugPrint('RealHueBridgeService: Discovered ${_deviceRegistry!.devices.length} switches, ${_deviceRegistry!.buttons.length} buttons');

      await _deviceRegistry!.fetchBehaviorInstances();
      debugPrint('RealHueBridgeService: ${_deviceRegistry!.configuredDeviceIds.length} devices configured in Hue app');

      _rooms = rooms;
      await _autoMapRooms();

      await SettingsService.instance.saveHueDeviceRegistry(_deviceRegistry!.toJson());
    } catch (e) {
      debugPrint('RealHueBridgeService: Device discovery failed (non-fatal): $e');
    }
  }

  /// Auto-map Hue rooms to Rhythm rooms.
  ///
  /// First tries to match by ID (Hue room UUID == Rhythm room ID).
  /// Falls back to matching by name (case-insensitive).
  ///
  /// Always clears and rebuilds mappings to ensure they stay fresh
  /// when room IDs change (e.g., after Supabase sync).
  Future<void> _autoMapRooms() async {
    if (_deviceRegistry == null) return;

    // Clear all existing mappings to ensure fresh rebuild
    // This prevents stale mappings when room IDs change
    for (final key in _deviceRegistry!.roomMappings.keys.toList()) {
      _deviceRegistry!.removeRoomMapping(key);
    }

    int mapped = 0;
    for (final hueRoom in _deviceRegistry!.rooms) {
      // First try: match by ID (Hue room UUID == Rhythm room ID)
      var rhythmRoom = _rooms.firstWhere(
        (r) => r.id == hueRoom.id,
        orElse: () => RoomDto(id: '', name: ''),
      );

      // Second try: match by name (case-insensitive)
      if (rhythmRoom.id.isEmpty) {
        rhythmRoom = _rooms.firstWhere(
          (r) => r.name.toLowerCase() == hueRoom.name.toLowerCase(),
          orElse: () => RoomDto(id: '', name: ''),
        );
      }

      if (rhythmRoom.id.isNotEmpty) {
        _deviceRegistry!.setRoomMapping(hueRoom.id, rhythmRoom.id);
        debugPrint('RealHueBridgeService: Auto-mapped "${hueRoom.name}" -> ${rhythmRoom.id}');
        mapped++;
      }
    }

    if (mapped > 0) {
      debugPrint('RealHueBridgeService: Rebuilt $mapped room mappings');
    }
  }

  void _handleBehaviorInstanceEvent(String eventType, Map<String, dynamic> data) {
    if (_deviceRegistry == null) return;

    final config = data['configuration'] as Map<String, dynamic>?;
    final device = config?['device'] as Map<String, dynamic>?;
    final deviceId = device?['rid'] as String?;

    String? deviceName;
    if (deviceId != null) {
      deviceName = _deviceRegistry!.getDevice(deviceId)?.name;
    }

    _deviceRegistry!.handleBehaviorInstanceEvent(eventType, data);

    switch (eventType) {
      case 'add':
        debugPrint('RealHueBridgeService: Switch "${deviceName ?? deviceId}" configured in Hue app');
        break;
      case 'delete':
        debugPrint('RealHueBridgeService: Switch unconfigured - now handling with Rhythm');
        break;
    }

    SettingsService.instance.saveHueDeviceRegistry(_deviceRegistry!.toJson());
  }

  void _handleButtonEvent(String deviceId, int buttonIndex, String eventType, bool ignored) {
    final deviceName = _deviceRegistry?.getDevice(deviceId)?.name ?? deviceId;
    final roomId = _deviceRegistry?.getRoomForDevice(deviceId);

    String? roomName;
    if (roomId != null) {
      roomName = _rooms.where((r) => r.id == roomId).map((r) => r.name).firstOrNull;
    }
    final roomDisplay = roomName ?? roomId ?? 'unknown';

    if (ignored) {
      debugPrint('RealHueBridgeService: [IGNORED] $roomDisplay: button $buttonIndex $eventType from "$deviceName"');
    } else {
      debugPrint('RealHueBridgeService: [PROCESS] $roomDisplay: button $buttonIndex $eventType from "$deviceName"');
    }
  }
}
