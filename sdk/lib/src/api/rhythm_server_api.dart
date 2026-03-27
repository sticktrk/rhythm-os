import 'package:dio/dio.dart';

import '../models/rhythm_curve_config.dart';
import '../models/rhythm_room.dart';
import '../models/rhythm_settings.dart';

/// Callback to update the connection manager's internal cache after action
/// responses. This keeps SSE/poll diffs from re-emitting state that was
/// already applied via an action response.
typedef RhythmCacheUpdater = void Function(List<RhythmRoomState> states);

/// Stateless API client for room/device/state management endpoints.
///
/// All methods are fire-and-forget safe (log errors, don't throw for
/// expected failures). Methods that return data throw [DioException]
/// on network errors.
class RhythmServerApi {
  final Dio _dio;
  final RhythmCacheUpdater? _onStatesReceived;

  RhythmServerApi(this._dio, {RhythmCacheUpdater? onStatesReceived})
      : _onStatesReceived = onStatesReceived;

  // =========================================================================
  // Room actions
  // =========================================================================

  /// Dispatch a room action via the server runtime.
  Future<RhythmRoomState?> roomAction({
    required String roomId,
    required String action,
  }) async {
    try {
      final response = await _dio.put('api/rooms/action', data: {
        'room_id': roomId,
        'action': action,
      });
      final data = response.data as Map<String, dynamic>?;
      final rooms = data?['rooms'] as List<dynamic>?;
      Map<String, dynamic>? roomJson;
      if (rooms != null && rooms.isNotEmpty) {
        roomJson = rooms[0] as Map<String, dynamic>?;
      } else if (data != null && data.containsKey('rhythm_enabled')) {
        roomJson = data;
      }
      if (roomJson != null && roomJson.containsKey('rhythm_enabled')) {
        final state = RhythmRoomState.fromJson(roomJson);
        _onStatesReceived?.call([state]);
        return state;
      }
    } catch (_) {}
    return null;
  }

  /// Dispatch actions for multiple rooms in a single request.
  Future<List<RhythmRoomState>> roomActionBatch(
    List<({String roomId, String action})> actions,
  ) async {
    if (actions.isEmpty) return [];
    try {
      final response = await _dio.put(
        'api/rooms/action',
        data: [
          for (final a in actions)
            {'room_id': a.roomId, 'action': a.action}
        ],
        options: Options(receiveTimeout: const Duration(seconds: 30)),
      );
      return _parseAndCacheRoomsResponse(response.data);
    } catch (_) {}
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
  Future<List<RhythmRoomState>> roomBrightnessBatch(
    List<({String roomId, int brightness})> items,
  ) async {
    if (items.isEmpty) return [];
    try {
      final response = await _dio.put(
        'api/rooms/brightness',
        data: [
          for (final i in items)
            {'room_id': i.roomId, 'brightness': i.brightness}
        ],
        options: Options(receiveTimeout: const Duration(seconds: 30)),
      );
      return _parseAndCacheRoomsResponse(response.data);
    } catch (_) {}
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
  Future<List<RhythmRoomState>> roomOffsetBatch(
    List<({String roomId, double timeOffset})> items,
  ) async {
    if (items.isEmpty) return [];
    try {
      final response = await _dio.put(
        'api/rooms/offset',
        data: [
          for (final i in items)
            {'room_id': i.roomId, 'time_offset': i.timeOffset}
        ],
        options: Options(receiveTimeout: const Duration(seconds: 30)),
      );
      return _parseAndCacheRoomsResponse(response.data);
    } catch (_) {}
    return [];
  }

  /// Push room preferences (rhythm_enabled, disabled, soft_off).
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

  /// Push room preferences for multiple rooms.
  Future<void> roomPreferencesBatchSet(
      List<Map<String, dynamic>> items) async {
    if (items.isEmpty) return;
    await _safePut('api/rooms/preferences', data: items);
  }

  /// Reset all on-rooms back to their current adaptive curve position.
  Future<List<RhythmRoomState>> fixMyLights() async {
    try {
      final response = await _dio.post(
        'api/rooms/fix',
        options: Options(receiveTimeout: const Duration(seconds: 30)),
      );
      final data = response.data as Map<String, dynamic>?;
      final roomStates = data?['rooms'] as List<dynamic>? ??
          data?['room_states'] as List<dynamic>?;
      if (roomStates == null) return [];
      return _parseAndCacheRoomsList(roomStates);
    } catch (_) {}
    return [];
  }

  // =========================================================================
  // Config
  // =========================================================================

  /// Absorb a time offset into the curve config by adjusting ramp widths.
  Future<RhythmCurveConfig?> absorbTimeOffset(double offsetMinutes) async {
    try {
      final response = await _dio.post(
        'api/config/absorb-offset',
        data: {'offset_minutes': offsetMinutes},
      );
      final data = response.data;
      if (data is Map<String, dynamic>) {
        return _parseCurveConfig(data);
      }
    } catch (_) {}
    return null;
  }

  /// Reset the curve config to factory defaults.
  Future<RhythmCurveConfig?> resetConfig() async {
    try {
      final response = await _dio.post('api/config/reset');
      final data = response.data;
      if (data is Map<String, dynamic>) {
        return _parseCurveConfig(data);
      }
    } catch (_) {}
    return null;
  }

  /// Push global curve configuration to the server.
  Future<void> configSet(RhythmCurveConfig config) async {
    await _safePut('api/config', data: config.toJson());
  }

  // =========================================================================
  // Location
  // =========================================================================

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

  // =========================================================================
  // Hub credentials
  // =========================================================================

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

  /// Disconnect ALL hubs on the server.
  Future<void> hubDisconnect() async {
    try {
      await _dio.delete('api/hub/credentials');
    } catch (_) {}
  }

  /// Disconnect a single hub by type + address.
  Future<void> hubDisconnectOne({
    required String hubType,
    required String address,
  }) async {
    try {
      await _dio.delete('api/hub/credentials', queryParameters: {
        'hub_type': hubType,
        'address': address,
      });
    } catch (_) {}
  }

  // =========================================================================
  // Motion
  // =========================================================================

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

  // =========================================================================
  // Settings
  // =========================================================================

  /// Fetch global device settings from the server.
  Future<RhythmSettings?> getSettings() async {
    try {
      final response = await _dio.get('api/settings');
      return RhythmSettings.fromJson(response.data as Map<String, dynamic>);
    } catch (_) {}
    return null;
  }

  /// Push a partial settings update to the server.
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

  // =========================================================================
  // Devices
  // =========================================================================

  /// Fetch a single canonical device by ID.
  Future<Map<String, dynamic>?> getCanonicalDevice(String id) async {
    try {
      final response = await _dio.get('api/devices/canonical/$id');
      return response.data as Map<String, dynamic>?;
    } catch (_) {}
    return null;
  }

  /// Fetch all canonical devices.
  Future<List<Map<String, dynamic>>?> getCanonicalDevices() async {
    try {
      final response = await _dio.get('api/devices/canonical');
      return (response.data as List<dynamic>?)?.cast<Map<String, dynamic>>();
    } catch (_) {}
    return null;
  }

  // =========================================================================
  // Triage
  // =========================================================================

  /// Fetch pending triage entries.
  Future<List<Map<String, dynamic>>?> getTriageEntries() async {
    try {
      final response = await _dio.get('api/triage');
      return (response.data as List<dynamic>?)?.cast<Map<String, dynamic>>();
    } catch (_) {}
    return null;
  }

  /// Fetch triage pending counts.
  Future<Map<String, dynamic>?> getTriageCount() async {
    try {
      final response = await _dio.get('api/triage/count');
      return response.data as Map<String, dynamic>?;
    } catch (_) {}
    return null;
  }

  /// Resolve a triage entry by merging with an existing canonical device.
  Future<bool> resolveTriageMerge(String entryId, String canonicalId) async {
    try {
      await _dio.put('api/triage/$entryId/merge', data: {
        'canonical_id': canonicalId,
      });
      return true;
    } catch (_) {}
    return false;
  }

  /// Resolve a triage entry by creating a new canonical device.
  Future<String?> resolveTriageNew(String entryId) async {
    try {
      final response = await _dio.put('api/triage/$entryId/new');
      return (response.data as Map<String, dynamic>?)
          ?['canonical_id'] as String?;
    } catch (_) {}
    return null;
  }

  /// Dismiss a triage entry.
  Future<bool> resolveTriageDismiss(String entryId) async {
    try {
      await _dio.put('api/triage/$entryId/dismiss');
      return true;
    } catch (_) {}
    return false;
  }

  /// Approve a room binding (merge two rooms from different hubs).
  Future<bool> resolveTriageBind(String entryId,
      {String? targetRoomId}) async {
    try {
      await _dio.put('api/triage/$entryId/bind', data: {
        if (targetRoomId != null) 'target_room_id': targetRoomId,
      });
      return true;
    } catch (_) {}
    return false;
  }

  // =========================================================================
  // Topology
  // =========================================================================

  /// Rename a topology room.
  Future<bool> topologyRenameRoom(String roomId, String name) async {
    try {
      await _dio.put('api/topology/rooms/$roomId', data: {'name': name});
      return true;
    } catch (_) {}
    return false;
  }

  /// Merge two topology rooms.
  Future<bool> topologyMergeRooms(String targetId, String sourceId) async {
    try {
      await _dio.put('api/topology/rooms/$targetId/merge', data: {
        'source_id': sourceId,
      });
      return true;
    } catch (_) {}
    return false;
  }

  /// Move a device between topology rooms.
  Future<bool> topologyMoveDevice({
    required String deviceId,
    required String fromRoomId,
    required String toRoomId,
  }) async {
    try {
      await _dio.put('api/topology/rooms/$toRoomId/devices/move', data: {
        'device_id': deviceId,
        'from_room': fromRoomId,
      });
      return true;
    } catch (_) {}
    return false;
  }

  // =========================================================================
  // Device pairing
  // =========================================================================

  /// Commission a new device (Matter, Zigbee, etc.).
  ///
  /// Returns the [PairingSession] JSON from the server, or `null` on error.
  Future<Map<String, dynamic>?> pairDevice({
    required String hubType,
    Map<String, dynamic> params = const {},
  }) async {
    try {
      final response = await _dio.post('api/devices/pair', data: {
        'hub_type': hubType,
        'params': params,
      });
      return response.data as Map<String, dynamic>?;
    } catch (_) {}
    return null;
  }

  /// Assign a canonical device to a room (or unassign with `null`).
  Future<bool> assignDeviceRoom(String deviceId, String? roomId) async {
    try {
      await _dio.put('api/devices/canonical/$deviceId/room', data: {
        if (roomId != null) 'room_id': roomId,
      });
      return true;
    } catch (_) {}
    return false;
  }

  /// Create a new topology room. Returns the created room JSON.
  Future<Map<String, dynamic>?> createTopologyRoom(String name) async {
    try {
      final response =
          await _dio.post('api/topology/rooms', data: {'name': name});
      return response.data as Map<String, dynamic>?;
    } catch (_) {}
    return null;
  }

  // =========================================================================
  // Sync
  // =========================================================================

  /// Trigger server-side room discovery from the connected hub.
  Future<Map<String, dynamic>?> triggerSync() async {
    try {
      final response = await _dio.post('api/sync');
      return response.data as Map<String, dynamic>?;
    } catch (_) {}
    return null;
  }

  // =========================================================================
  // Health
  // =========================================================================

  /// Ping the server (health check).
  Future<bool> ping() async {
    try {
      final response = await _dio.get('health');
      return response.statusCode == 200;
    } catch (_) {}
    return false;
  }

  // =========================================================================
  // Internal helpers
  // =========================================================================

  Future<void> _safePut(String path, {required Object data}) async {
    try {
      await _dio.put(path, data: data);
    } catch (_) {}
  }

  RhythmCurveConfig? _parseCurveConfig(Map<String, dynamic> json) {
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
    if (minBri == null ||
        maxBri == null ||
        minCct == null ||
        maxCct == null ||
        wlBri == null ||
        wrBri == null ||
        wlCct == null ||
        wrCct == null ||
        shapeP == null ||
        maxDim == null) {
      return null;
    }
    return RhythmCurveConfig(
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

  List<RhythmRoomState> _parseAndCacheRoomsResponse(dynamic responseData) {
    final data = responseData as Map<String, dynamic>?;
    final rooms = data?['rooms'] as List<dynamic>?;
    if (rooms == null) return [];
    return _parseAndCacheRoomsList(rooms);
  }

  List<RhythmRoomState> _parseAndCacheRoomsList(List<dynamic> rooms) {
    final results = <RhythmRoomState>[];
    for (final r in rooms) {
      if (r is Map<String, dynamic> && r.containsKey('rhythm_enabled')) {
        final state = RhythmRoomState.fromJson(r);
        results.add(state);
      }
    }
    if (results.isNotEmpty) {
      _onStatesReceived?.call(results);
    }
    return results;
  }
}
