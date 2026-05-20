import 'package:dio/dio.dart';
import 'package:logging/logging.dart';

import '../models/rhythm_curve_config.dart';
import '../models/rhythm_curve_data.dart';
import '../models/rhythm_room.dart';
import '../models/rhythm_settings.dart';
import '../models/rhythm_time_info.dart';

/// Callback to update the connection manager's internal cache after action
/// responses. This keeps SSE/poll diffs from re-emitting state that was
/// already applied via an action response.
typedef RhythmCacheUpdater = void Function(List<RhythmRoomState> states);

/// Stateless API client for node/device/state management endpoints.
///
/// All methods are fire-and-forget safe (log errors, don't throw for
/// expected failures). Methods that return data throw [DioException]
/// on network errors.
class RhythmServerApi {
  static final _log = Logger('rhythm_sdk.api');

  final Dio _dio;
  final RhythmCacheUpdater? _onStatesReceived;

  RhythmServerApi(this._dio, {RhythmCacheUpdater? onStatesReceived})
      : _onStatesReceived = onStatesReceived;

  // =========================================================================
  // Node actions
  // =========================================================================

  /// Dispatch a node action via the server runtime.
  Future<RhythmRoomState?> nodeAction({
    required String nodeId,
    required String action,
  }) async {
    try {
      final response = await _dio.put('api/nodes/action', data: {
        'node_id': nodeId,
        'action': action,
      });
      final data = response.data as Map<String, dynamic>?;
      final nodes =
          data?['nodes'] as List<dynamic>? ?? data?['rooms'] as List<dynamic>?;
      Map<String, dynamic>? nodeJson;
      if (nodes != null && nodes.isNotEmpty) {
        nodeJson = nodes[0] as Map<String, dynamic>?;
      } else if (data != null && data.containsKey('rhythm_enabled')) {
        nodeJson = data;
      }
      if (nodeJson != null && nodeJson.containsKey('rhythm_enabled')) {
        final state = RhythmRoomState.fromJson(nodeJson);
        _onStatesReceived?.call([state]);
        return state;
      }
    } catch (e) {
      _log.warning('nodeAction failed', e);
    }
    return null;
  }

  /// Dispatch a room action via the node-first server contract.
  Future<RhythmRoomState?> roomAction({
    required String roomId,
    required String action,
  }) {
    return nodeAction(nodeId: roomId, action: action);
  }

  /// Dispatch actions for multiple nodes in a single request.
  Future<List<RhythmRoomState>> nodeActionBatch(
    List<({String nodeId, String action})> actions,
  ) async {
    if (actions.isEmpty) return [];
    try {
      final response = await _dio.put(
        'api/nodes/action',
        data: [
          for (final a in actions) {'node_id': a.nodeId, 'action': a.action}
        ],
        options: Options(receiveTimeout: const Duration(seconds: 30)),
      );
      return _parseAndCacheStatesResponse(response.data);
    } catch (e) {
      _log.warning('nodeActionBatch failed', e);
    }
    return [];
  }

  /// Dispatch actions for multiple rooms in a single request.
  Future<List<RhythmRoomState>> roomActionBatch(
    List<({String roomId, String action})> actions,
  ) {
    return nodeActionBatch([
      for (final action in actions)
        (nodeId: action.roomId, action: action.action),
    ]);
  }

  /// Set node brightness via the server runtime.
  Future<void> nodeBrightness({
    required String nodeId,
    required int brightness,
  }) async {
    await _safePut('api/nodes/brightness', data: {
      'node_id': nodeId,
      'brightness': brightness,
    });
  }

  /// Set room brightness via the node-first server contract.
  Future<void> roomBrightness({
    required String roomId,
    required int brightness,
  }) {
    return nodeBrightness(nodeId: roomId, brightness: brightness);
  }

  /// Set brightness for multiple nodes in a single request.
  Future<List<RhythmRoomState>> nodeBrightnessBatch(
    List<({String nodeId, int brightness})> items,
  ) async {
    if (items.isEmpty) return [];
    try {
      final response = await _dio.put(
        'api/nodes/brightness',
        data: [
          for (final i in items)
            {'node_id': i.nodeId, 'brightness': i.brightness}
        ],
        options: Options(receiveTimeout: const Duration(seconds: 30)),
      );
      return _parseAndCacheStatesResponse(response.data);
    } catch (e) {
      _log.warning('nodeBrightnessBatch failed', e);
    }
    return [];
  }

  /// Set brightness for multiple rooms in a single request.
  Future<List<RhythmRoomState>> roomBrightnessBatch(
    List<({String roomId, int brightness})> items,
  ) {
    return nodeBrightnessBatch([
      for (final item in items)
        (nodeId: item.roomId, brightness: item.brightness),
    ]);
  }

  /// Set the time offset for a single node.
  Future<void> nodeOffset({
    required String nodeId,
    required double timeOffset,
  }) async {
    await _safePut('api/nodes/offset', data: {
      'node_id': nodeId,
      'time_offset': timeOffset,
    });
  }

  /// Set the time offset for a single room via the node-first contract.
  Future<void> roomOffset({
    required String roomId,
    required double timeOffset,
  }) {
    return nodeOffset(nodeId: roomId, timeOffset: timeOffset);
  }

  /// Set time offset for multiple nodes in a single request.
  Future<List<RhythmRoomState>> nodeOffsetBatch(
    List<({String nodeId, double timeOffset})> items,
  ) async {
    if (items.isEmpty) return [];
    try {
      final response = await _dio.put(
        'api/nodes/offset',
        data: [
          for (final i in items)
            {'node_id': i.nodeId, 'time_offset': i.timeOffset}
        ],
        options: Options(receiveTimeout: const Duration(seconds: 30)),
      );
      return _parseAndCacheStatesResponse(response.data);
    } catch (e) {
      _log.warning('nodeOffsetBatch failed', e);
    }
    return [];
  }

  /// Set time offset for multiple rooms in a single request.
  Future<List<RhythmRoomState>> roomOffsetBatch(
    List<({String roomId, double timeOffset})> items,
  ) {
    return nodeOffsetBatch([
      for (final item in items)
        (nodeId: item.roomId, timeOffset: item.timeOffset),
    ]);
  }

  /// Push node preferences (rhythm_enabled, disabled, soft_off).
  Future<void> nodePreferencesSet({
    required String nodeId,
    bool? rhythmEnabled,
    bool? disabled,
    RoomModeState? state,
    bool? softOff,
    Map<String, dynamic>? profileSettings,
  }) async {
    final effectiveState = state ??
        (softOff == null
            ? null
            : (softOff ? RoomModeState.idle : RoomModeState.active));
    await _safePut('api/nodes/preferences', data: {
      'node_id': nodeId,
      if (rhythmEnabled != null) 'rhythm_enabled': rhythmEnabled,
      if (disabled != null) 'disabled': disabled,
      if (effectiveState != null) 'state': effectiveState.wireValue,
      if (profileSettings != null) 'profile_settings': profileSettings,
    });
  }

  /// Push room preferences (rhythm_enabled, disabled, soft_off).
  Future<void> roomPreferencesSet({
    required String roomId,
    bool? rhythmEnabled,
    bool? disabled,
    RoomModeState? state,
    bool? softOff,
    Map<String, dynamic>? profileSettings,
  }) {
    return nodePreferencesSet(
      nodeId: roomId,
      rhythmEnabled: rhythmEnabled,
      disabled: disabled,
      state: state,
      softOff: softOff,
      profileSettings: profileSettings,
    );
  }

  /// Push node preferences for multiple nodes.
  Future<void> nodePreferencesBatchSet(List<Map<String, dynamic>> items) async {
    if (items.isEmpty) return;
    await _safePut('api/nodes/preferences', data: [
      for (final item in items)
        {
          for (final entry in item.entries)
            if (entry.key != 'room_id' && entry.key != 'room_profile')
              entry.key: entry.value,
          if (!item.containsKey('node_id') && item.containsKey('room_id'))
            'node_id': item['room_id'],
          if (!item.containsKey('profile_settings') &&
              item.containsKey('room_profile'))
            'profile_settings': item['room_profile'],
        }
    ]);
  }

  /// Push room preferences for multiple rooms.
  Future<void> roomPreferencesBatchSet(List<Map<String, dynamic>> items) async {
    await nodePreferencesBatchSet([
      for (final item in items)
        {
          for (final entry in item.entries)
            if (entry.key != 'room_id' && entry.key != 'room_profile')
              entry.key: entry.value,
          'node_id': item['room_id'],
          if (!item.containsKey('profile_settings') &&
              item.containsKey('room_profile'))
            'profile_settings': item['room_profile'],
        },
    ]);
  }

  /// Reset the provided nodes back to their current adaptive curve position.
  Future<List<RhythmRoomState>> fixMyLights({
    required Iterable<String> nodeIds,
  }) async {
    final ids = nodeIds.where((id) => id.isNotEmpty).toSet().toList();
    if (ids.isEmpty) return [];
    return nodeActionBatch([
      for (final nodeId in ids) (nodeId: nodeId, action: 'reset'),
    ]);
  }

// =========================================================================
// Config
// =========================================================================

  /// Fetch a stored profile config by ID.
  Future<RhythmCurveConfig?> getConfig({required String id}) async {
    try {
      final response = await _dio.get(
        'api/config',
        queryParameters: {'id': id},
      );
      final data = response.data;
      if (data is Map<String, dynamic>) {
        return _parseCurveConfig(data);
      }
    } catch (e) {
      _log.warning('getConfig failed', e);
    }
    return null;
  }

  /// Fetch a stored or draft curve preview for a profile.
  Future<RhythmCurveData?> getCurveData({
    required String id,
    RhythmCurveConfig? overrides,
    DateTime? date,
    int samplesPerHour = 4,
    double startHour = 12,
    int? maxSteps,
  }) async {
    try {
      final queryParameters = _curveQueryParameters(
        id: id,
        date: date ?? DateTime.now(),
        samplesPerHour: samplesPerHour,
        startHour: startHour,
        maxSteps: maxSteps ?? overrides?.maxDimSteps,
      );
      final response = overrides == null
          ? await _dio.get('api/curve', queryParameters: queryParameters)
          : await _dio.post(
              'api/curve',
              queryParameters: queryParameters,
              data: overrides.toJson(),
            );
      return RhythmCurveData.fromJson(response.data as Map<String, dynamic>);
    } catch (e) {
      _log.warning('getCurveData failed', e);
    }
    return null;
  }

  /// Fetch the current sampled value for a profile.
  Future<RhythmTimeInfo?> getCurveNow({
    required String id,
    double? hour,
  }) async {
    try {
      final response = await _dio.get(
        'api/curve/now',
        queryParameters: {
          'id': id,
          if (hour != null) 'hour': hour,
        },
      );
      return RhythmTimeInfo.fromJson(response.data as Map<String, dynamic>);
    } catch (e) {
      _log.warning('getCurveNow failed', e);
    }
    return null;
  }

  /// Absorb a time offset into the profile config by adjusting the curve.
  Future<RhythmCurveConfig?> absorbTimeOffset(
    double offsetMinutes, {
    String? id,
  }) async {
    try {
      final response = await _dio.post(
        'api/config/absorb-offset',
        queryParameters: {
          if (id != null) 'id': id,
        },
        data: {'offset_minutes': offsetMinutes},
      );
      final data = response.data;
      if (data is Map<String, dynamic>) {
        return _parseCurveConfig(data);
      }
    } catch (e) {
      _log.warning('absorbTimeOffset failed', e);
    }
    return null;
  }

  /// Reset a stored profile config to built-in defaults.
  Future<RhythmCurveConfig?> resetConfig({String? id}) async {
    try {
      final response = await _dio.post(
        'api/config/reset',
        queryParameters: {
          if (id != null) 'id': id,
        },
      );
      final data = response.data;
      if (data is Map<String, dynamic>) {
        return _parseCurveConfig(data);
      }
    } catch (e) {
      _log.warning('resetConfig failed', e);
    }
    return null;
  }

  /// Push a full profile configuration to the server.
  ///
  /// Returns `true` when the server accepts the update and `false` on failure.
  Future<bool> configSet(
    RhythmCurveConfig config, {
    String? id,
  }) async {
    final profileId = id ?? config.id;
    if (profileId.isEmpty) {
      _log.warning('configSet skipped: missing profile id');
      return false;
    }
    try {
      await _dio.put(
        'api/config',
        queryParameters: {'id': profileId},
        data: config.toJson(),
      );
      return true;
    } catch (e) {
      _log.warning('configSet failed', e);
      return false;
    }
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
    } catch (e) {
      _log.warning('hubDisconnect failed', e);
    }
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
    } catch (e) {
      _log.warning('hubDisconnectOne failed', e);
    }
  }

  /// Re-arm startup bootstrap for a single configured hub.
  Future<bool> hubRetry({
    required String hubType,
    required String address,
  }) async {
    try {
      final response = await _dio.post('api/hub/retry', data: {
        'hub_type': hubType,
        'address': address,
      });
      return response.statusCode == null ||
          (response.statusCode! >= 200 && response.statusCode! < 300);
    } catch (e) {
      _log.warning('hubRetry failed', e);
    }
    return false;
  }

  // =========================================================================
  // Motion
  // =========================================================================

  /// Set per-node motion timeout on the server.
  Future<void> motionTimeoutSet({
    String? nodeId,
    String? roomId,
    required int? timeoutSecs,
  }) async {
    final effectiveNodeId = nodeId ?? roomId;
    if (effectiveNodeId == null || effectiveNodeId.isEmpty) {
      _log.warning('motionTimeoutSet skipped: missing node id');
      return;
    }
    await _safePut('api/nodes/motion-timeout', data: {
      'node_id': effectiveNodeId,
      'timeout_secs': timeoutSecs,
    });
  }

// =========================================================================
// Settings
// =========================================================================

  /// Fetch app-level settings from the server.
  Future<RhythmSettings?> getSettings() async {
    try {
      final response = await _dio.get('api/settings');
      return RhythmSettings.fromJson(response.data as Map<String, dynamic>);
    } catch (e) {
      _log.warning('getSettings failed', e);
    }
    return null;
  }

  /// Activate the sleep profile.
  Future<void> sleep() async {
    await setActiveMode(RhythmMode.sleep);
  }

  /// Activate the rhythm profile.
  Future<void> wake() async {
    await setActiveMode(RhythmMode.day);
  }

  /// Fetch mode state, profile routing, and transition config.
  Future<RhythmModeResource?> getMode() async {
    try {
      final response = await _dio.get('api/mode');
      return RhythmModeResource.fromJson(
        response.data as Map<String, dynamic>,
      );
    } catch (e) {
      _log.warning('getMode failed', e);
    }
    return null;
  }

  /// Fetch the list of available profile configs.
  Future<List<RhythmCurveConfig>> getProfiles() async {
    try {
      final response = await _dio.get('api/profiles');
      return RhythmProfiles.fromJson(
        response.data as Map<String, dynamic>,
      ).profiles;
    } catch (e) {
      _log.warning('getProfiles failed', e);
    }
    return const [];
  }

  /// Set the active global mode on the server.
  Future<void> setActiveMode(RhythmMode mode) async {
    await modeSet(active: mode);
  }

  /// Push a partial settings update to the server.
  ///
  /// Returns `true` when the server accepts the update and `false` on failure.
  Future<bool> settingsSet({
    bool? powerSave,
  }) async {
    final data = <String, dynamic>{
      if (powerSave != null) 'power_save': powerSave,
    };
    if (data.isEmpty) return true;
    try {
      await _dio.put('api/settings', data: data);
      return true;
    } catch (e) {
      _log.warning('settingsSet failed', e);
      return false;
    }
  }

  // =========================================================================
  // Devices
  // =========================================================================

  /// Fetch a single canonical device by ID.
  Future<Map<String, dynamic>?> getCanonicalDevice(String id) async {
    try {
      final response = await _dio.get('api/devices/canonical/$id');
      return response.data as Map<String, dynamic>?;
    } catch (e) {
      _log.warning('getCanonicalDevice failed', e);
    }
    return null;
  }

  /// Fetch all canonical devices.
  Future<List<Map<String, dynamic>>?> getCanonicalDevices() async {
    try {
      final response = await _dio.get('api/devices/canonical');
      return (response.data as List<dynamic>?)?.cast<Map<String, dynamic>>();
    } catch (e) {
      _log.warning('getCanonicalDevices failed', e);
    }
    return null;
  }

  /// Run a raw Matter bulb tester command variant against a Matter light.
  ///
  /// [deviceId] may be either a canonical Rhythm device ID or a Matter native
  /// ID such as `matter-100`.
  Future<Map<String, dynamic>?> runMatterBulbTest({
    required String deviceId,
    required String test,
  }) async {
    try {
      final response = await _dio.post('api/matter/bulb-test/run', data: {
        'device_id': deviceId,
        'test': test,
      });
      return response.data as Map<String, dynamic>?;
    } catch (e) {
      _log.warning('runMatterBulbTest failed', e);
    }
    return null;
  }

  /// Save a Matter bulb tester report on the server and optionally apply the
  /// inferred local quirks immediately.
  Future<Map<String, dynamic>?> saveMatterBulbTestReport(
    Map<String, dynamic> report, {
    bool applyLocal = true,
  }) async {
    try {
      final response = await _dio.post(
        'api/matter/bulb-test/report',
        data: {
          ...report,
          'apply_local': applyLocal,
        },
      );
      return response.data as Map<String, dynamic>?;
    } catch (e) {
      _log.warning('saveMatterBulbTestReport failed', e);
    }
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
    } catch (e) {
      _log.warning('getTriageEntries failed', e);
    }
    return null;
  }

  /// Fetch triage pending counts.
  Future<Map<String, dynamic>?> getTriageCount() async {
    try {
      final response = await _dio.get('api/triage/count');
      return response.data as Map<String, dynamic>?;
    } catch (e) {
      _log.warning('getTriageCount failed', e);
    }
    return null;
  }

  /// Resolve a triage entry by merging with an existing canonical device.
  Future<bool> resolveTriageMerge(String entryId, String canonicalId) async {
    try {
      await _dio.put('api/triage/$entryId/merge', data: {
        'canonical_id': canonicalId,
      });
      return true;
    } catch (e) {
      _log.warning('resolveTriageMerge failed', e);
    }
    return false;
  }

  /// Push a partial mode update to the server.
  ///
  /// Returns `true` when the server accepts the update and `false` on failure.
  Future<bool> modeSet({
    RhythmMode? active,
    List<RhythmModeConfig>? configs,
  }) async {
    final data = <String, dynamic>{
      if (active != null) 'active': active.wireValue,
      if (configs != null)
        'configs': configs.map((config) => config.toJson()).toList(),
    };
    if (data.isEmpty) return true;
    try {
      await _dio.put('api/mode', data: data);
      return true;
    } catch (e) {
      _log.warning('modeSet failed', e);
    }
    return false;
  }

  /// Fetch transition configs from the server.
  Future<List<RhythmModeTransitionConfig>> getTransitions() async {
    try {
      final response = await _dio.get('api/transitions');
      final data = response.data as Map<String, dynamic>;
      return ((data['transitions'] as List<dynamic>?) ?? const <dynamic>[])
          .map((e) => RhythmModeTransitionConfig.fromJson(
                e as Map<String, dynamic>,
              ))
          .toList();
    } catch (e) {
      _log.warning('getTransitions failed', e);
    }
    return const [];
  }

  /// Push transition configs to the server.
  Future<bool> setTransitions(
      List<RhythmModeTransitionConfig> transitions) async {
    try {
      await _dio.put('api/transitions', data: {
        'transitions':
            transitions.map((transition) => transition.toJson()).toList(),
      });
      return true;
    } catch (e) {
      _log.warning('setTransitions failed', e);
    }
    return false;
  }

  /// Run a saved transition manually by ID.
  Future<bool> triggerTransition(String id) async {
    try {
      await _dio.post(
        'api/transitions/${Uri.encodeComponent(id)}/trigger',
        data: const <String, dynamic>{},
      );
      return true;
    } catch (e) {
      _log.warning('triggerTransition failed', e);
    }
    return false;
  }

  /// Resolve a triage entry by creating a new canonical device.
  ///
  /// For `device_merge`, the response contains `canonical_id`.
  /// For `room_binding`, the response contains `status: kept_separate`.
  Future<Map<String, dynamic>?> resolveTriageNewResult(String entryId) async {
    try {
      final response = await _dio.put('api/triage/$entryId/new');
      return response.data as Map<String, dynamic>?;
    } catch (e) {
      _log.warning('resolveTriageNewResult failed', e);
    }
    return null;
  }

  /// Resolve a triage entry by creating a new canonical device.
  Future<String?> resolveTriageNew(String entryId) async {
    final result = await resolveTriageNewResult(entryId);
    return result?['canonical_id'] as String?;
  }

  /// Dismiss a triage entry.
  Future<bool> resolveTriageDismiss(String entryId) async {
    try {
      await _dio.put('api/triage/$entryId/dismiss');
      return true;
    } catch (e) {
      _log.warning('resolveTriageDismiss failed', e);
    }
    return false;
  }

  /// Approve a room binding (merge two rooms from different hubs).
  Future<bool> resolveTriageBind(String entryId, {String? targetRoomId}) async {
    try {
      await _dio.put('api/triage/$entryId/bind', data: {
        if (targetRoomId != null) 'target_room_id': targetRoomId,
      });
      return true;
    } catch (e) {
      _log.warning('resolveTriageBind failed', e);
    }
    return false;
  }

  /// Assign an unassigned-device triage entry to a room.
  Future<bool> resolveTriageRoom(String entryId, String roomId) async {
    try {
      await _dio.put('api/triage/$entryId/room', data: {
        'room_id': roomId,
      });
      return true;
    } catch (e) {
      _log.warning('resolveTriageRoom failed', e);
    }
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
    } catch (e) {
      _log.warning('topologyRenameRoom failed', e);
    }
    return false;
  }

  /// Merge two topology rooms.
  Future<bool> topologyMergeRooms(String targetId, String sourceId) async {
    try {
      await _dio.put('api/topology/rooms/$targetId/merge', data: {
        'source_id': sourceId,
      });
      return true;
    } catch (e) {
      _log.warning('topologyMergeRooms failed', e);
    }
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
    } catch (e) {
      _log.warning('topologyMoveDevice failed', e);
    }
    return false;
  }

  /// Delete a topology room.
  Future<bool> topologyDeleteRoom(String roomId) async {
    try {
      final response = await _dio.delete('api/topology/rooms/$roomId');
      return response.statusCode == 204;
    } catch (e) {
      _log.warning('topologyDeleteRoom failed', e);
    }
    return false;
  }

  /// Fetch the node topology graph.
  Future<List<RhythmTopologyNode>> getTopologyNodes() async {
    try {
      final response = await _dio.get('api/topology/nodes');
      final data = response.data;
      if (data is List<dynamic>) {
        return data
            .whereType<Map<String, dynamic>>()
            .map((node) => RhythmTopologyNode.fromJson(node))
            .toList();
      }
      if (data is Map<String, dynamic>) {
        final nodes = data['nodes'] as List<dynamic>? ?? const [];
        return nodes
            .whereType<Map<String, dynamic>>()
            .map((node) => RhythmTopologyNode.fromJson(node))
            .toList();
      }
    } catch (e) {
      _log.warning('getTopologyNodes failed', e);
    }
    return const [];
  }

  /// Set or clear an explicit topology control target for a node.
  Future<bool> setTopologyNodeControlTarget({
    required String nodeId,
    required String controlKind,
    required String? targetId,
  }) async {
    try {
      await _dio.put(
        'api/topology/nodes/$nodeId/controls/$controlKind',
        data: {'target_id': targetId},
      );
      return true;
    } catch (e) {
      _log.warning('setTopologyNodeControlTarget failed', e);
    }
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
    Duration receiveTimeout = const Duration(seconds: 45),
  }) async {
    try {
      final response = await _dio.post(
        'api/devices/pair',
        data: {
          'hub_type': hubType,
          'params': params,
        },
        options: Options(
          receiveTimeout: receiveTimeout,
          sendTimeout: receiveTimeout,
          validateStatus: (_) => true,
        ),
      );

      final statusCode = response.statusCode;
      final responseData = response.data;
      if (responseData is Map<String, dynamic>) {
        return {
          ...responseData,
          if (statusCode != null && statusCode != 200)
            'http_status': statusCode,
        };
      }

      if (statusCode != null && statusCode != 200) {
        return {
          'http_status': statusCode,
          'error': 'Pairing request failed with HTTP $statusCode.',
        };
      }
    } catch (e) {
      _log.warning('pairDevice failed', e);
      if (e is DioException) {
        final responseData = e.response?.data;
        if (responseData is Map<String, dynamic>) {
          return {
            ...responseData,
            if (e.response?.statusCode != null)
              'http_status': e.response!.statusCode,
          };
        }

        final statusCode = e.response?.statusCode;
        final message = switch (e.type) {
          DioExceptionType.connectionTimeout ||
          DioExceptionType.receiveTimeout ||
          DioExceptionType.sendTimeout =>
            'Pairing timed out. Please keep the device powered on and try again.',
          DioExceptionType.connectionError => 'Could not reach the server.',
          _ => e.message ?? 'Pairing request failed.',
        };

        return {
          if (statusCode != null) 'http_status': statusCode,
          'error': message,
        };
      }
    }
    return null;
  }

  /// Unpair / decommission a device (Matter, Zigbee, etc.).
  ///
  /// Returns the unpair result JSON, or `null` on error.
  Future<Map<String, dynamic>?> unpairDevice({
    required String hubType,
    required String deviceId,
    bool force = false,
  }) async {
    try {
      final response = await _dio.post('api/devices/unpair', data: {
        'hub_type': hubType,
        'params': {
          'device_id': deviceId,
          'force': force,
        },
      });
      return response.data as Map<String, dynamic>?;
    } catch (e) {
      _log.warning('unpairDevice failed', e);
    }
    return null;
  }

  /// Assign a canonical device to a parent node (or unassign with `null`).
  Future<bool> assignDeviceParent(String deviceId, String? parentId) async {
    try {
      await _dio.put(
        'api/devices/canonical/$deviceId/parent',
        data: {'parent_id': parentId},
      );
      return true;
    } catch (e) {
      _log.warning('assignDeviceParent failed', e);
    }
    return false;
  }

  /// Assign a canonical device to a room (or unassign with `null`).
  Future<bool> assignDeviceRoom(String deviceId, String? roomId) {
    return assignDeviceParent(deviceId, roomId);
  }

  /// Create a new topology room. Returns the created room JSON.
  Future<Map<String, dynamic>?> createTopologyRoom(String name) async {
    try {
      final response =
          await _dio.post('api/topology/rooms', data: {'name': name});
      return response.data as Map<String, dynamic>?;
    } catch (e) {
      _log.warning('createTopologyRoom failed', e);
    }
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
    } catch (e) {
      _log.warning('triggerSync failed', e);
    }
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
    } catch (e) {
      _log.warning('ping failed', e);
    }
    return false;
  }

  // =========================================================================
  // Internal helpers
  // =========================================================================

  Future<void> _safePut(
    String path, {
    required Object data,
    Map<String, dynamic>? queryParameters,
  }) async {
    try {
      await _dio.put(path, data: data, queryParameters: queryParameters);
    } catch (e) {
      _log.warning('PUT $path failed', e);
    }
  }

  String _formatDate(DateTime date) {
    final month = date.month.toString().padLeft(2, '0');
    final day = date.day.toString().padLeft(2, '0');
    return '${date.year}-$month-$day';
  }

  Map<String, dynamic> _curveQueryParameters({
    required String id,
    required DateTime date,
    int samplesPerHour = 4,
    double startHour = 12.0,
    int? maxSteps,
  }) {
    return {
      'id': id,
      'date': _formatDate(date),
      'samples_per_hour': samplesPerHour,
      'start_hour': startHour,
      if (maxSteps != null) 'max_steps': maxSteps,
    };
  }

  RhythmCurveConfig _parseCurveConfig(Map<String, dynamic> json) {
    return RhythmCurveConfig.fromJson(json);
  }

  List<RhythmRoomState> _parseAndCacheStatesResponse(dynamic responseData) {
    final data = responseData as Map<String, dynamic>?;
    final states =
        data?['nodes'] as List<dynamic>? ?? data?['rooms'] as List<dynamic>?;
    if (states == null) return [];
    return _parseAndCacheStatesList(states);
  }

  List<RhythmRoomState> _parseAndCacheStatesList(List<dynamic> states) {
    final results = <RhythmRoomState>[];
    for (final r in states) {
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
