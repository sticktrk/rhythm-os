import 'package:dio/dio.dart';
import 'package:logging/logging.dart';

import '../json_parsing.dart';
import '../models/rhythm_curve_config.dart';
import '../models/rhythm_curve_data.dart';
import '../models/rhythm_input_binding.dart';
import '../models/rhythm_room.dart';
import '../models/rhythm_scene.dart';
import '../models/rhythm_settings.dart';
import '../models/rhythm_time_info.dart';

enum RhythmNodeColorScope {
  preview,
  mood,
  auto;

  String get wireValue => switch (this) {
        RhythmNodeColorScope.preview => 'preview',
        RhythmNodeColorScope.mood => 'mood',
        RhythmNodeColorScope.auto => 'auto',
      };
}

class RhythmDispatchMetadata {
  final bool queued;
  final int? dispatchCount;
  final int? dispatchSpacingMs;
  final int? estimatedDispatchMs;

  const RhythmDispatchMetadata({
    this.queued = false,
    this.dispatchCount,
    this.dispatchSpacingMs,
    this.estimatedDispatchMs,
  });

  factory RhythmDispatchMetadata.fromJson(Map<String, dynamic> json) {
    return RhythmDispatchMetadata(
      queued: json['queued'] as bool? ?? false,
      dispatchCount: _jsonInt(json['dispatch_count']),
      dispatchSpacingMs: _jsonInt(json['dispatch_spacing_ms']),
      estimatedDispatchMs: _jsonInt(json['estimated_dispatch_ms']),
    );
  }

  Duration? get estimatedDispatchDuration {
    final explicitMs = estimatedDispatchMs;
    if (explicitMs != null) return Duration(milliseconds: explicitMs);

    final count = dispatchCount;
    final spacingMs = dispatchSpacingMs;
    if (count == null || spacingMs == null || count <= 1) return null;
    return Duration(milliseconds: (count - 1) * spacingMs);
  }

  bool get hasLoadingMetadata =>
      queued ||
      dispatchCount != null ||
      dispatchSpacingMs != null ||
      estimatedDispatchMs != null;
}

class RhythmDispatchResult {
  final List<RhythmRoomState> states;
  final RhythmDispatchMetadata metadata;

  const RhythmDispatchResult({
    this.states = const [],
    this.metadata = const RhythmDispatchMetadata(),
  });

  List<RhythmRoomState> get nodes => states;
  bool get queued => metadata.queued;
  int? get dispatchCount => metadata.dispatchCount;
  int? get dispatchSpacingMs => metadata.dispatchSpacingMs;
  int? get estimatedDispatchMs => metadata.estimatedDispatchMs;
  Duration? get estimatedDispatchDuration => metadata.estimatedDispatchDuration;
}

class RhythmAbsorbTimeOffsetResult {
  final RhythmCurveConfig? config;
  final RhythmDispatchResult dispatch;

  const RhythmAbsorbTimeOffsetResult({
    this.config,
    this.dispatch = const RhythmDispatchResult(),
  });
}

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
      {int? dispatchSpacingMs}) async {
    return (await nodeActionBatchResult(
      actions,
      dispatchSpacingMs: dispatchSpacingMs,
    ))
        .states;
  }

  /// Dispatch actions for multiple nodes in a single request.
  Future<RhythmDispatchResult> nodeActionBatchResult(
      List<({String nodeId, String action})> actions,
      {int? dispatchSpacingMs}) async {
    if (actions.isEmpty) return const RhythmDispatchResult();
    try {
      final response = await _dio.put(
        'api/nodes/action',
        data: _nodesBatchBody([
          for (final a in actions) {'node_id': a.nodeId, 'action': a.action}
        ], dispatchSpacingMs: dispatchSpacingMs),
        options: Options(receiveTimeout: const Duration(seconds: 30)),
      );
      return _parseAndCacheDispatchResponse(response.data);
    } catch (e) {
      _log.warning('nodeActionBatch failed', e);
    }
    return const RhythmDispatchResult();
  }

  /// Dispatch actions for multiple rooms in a single request.
  Future<List<RhythmRoomState>> roomActionBatch(
      List<({String roomId, String action})> actions,
      {int? dispatchSpacingMs}) {
    return nodeActionBatch([
      for (final action in actions)
        (nodeId: action.roomId, action: action.action),
    ], dispatchSpacingMs: dispatchSpacingMs);
  }

  /// Dispatch actions for multiple rooms in a single request.
  Future<RhythmDispatchResult> roomActionBatchResult(
      List<({String roomId, String action})> actions,
      {int? dispatchSpacingMs}) {
    return nodeActionBatchResult([
      for (final action in actions)
        (nodeId: action.roomId, action: action.action),
    ], dispatchSpacingMs: dispatchSpacingMs);
  }

  /// Apply a curve brightness modifier to a node.
  ///
  /// This adjusts the node's active lighting curve rather than issuing a
  /// one-shot brightness command.
  Future<RhythmRoomState?> nodeCurveBrightness({
    required String nodeId,
    required int brightness,
  }) {
    return _putNodeCurveModifier({
      'node_id': nodeId,
      'brightness': brightness,
    });
  }

  /// Apply a curve brightness modifier to a room.
  Future<RhythmRoomState?> roomCurveBrightness({
    required String roomId,
    required int brightness,
  }) {
    return nodeCurveBrightness(nodeId: roomId, brightness: brightness);
  }

  /// Move a node along its active curve to the requested color temperature.
  ///
  /// This is a curve modifier, not a one-shot device color-temperature write.
  Future<RhythmRoomState?> nodeCurveColorTemperature({
    required String nodeId,
    required int kelvin,
    bool preserveBrightness = true,
  }) {
    return _putNodeCurveModifier({
      'node_id': nodeId,
      'color_temperature': kelvin,
      'preserve_brightness': preserveBrightness,
    });
  }

  /// Move a room along its active curve to the requested color temperature.
  Future<RhythmRoomState?> roomCurveColorTemperature({
    required String roomId,
    required int kelvin,
    bool preserveBrightness = true,
  }) {
    return nodeCurveColorTemperature(
      nodeId: roomId,
      kelvin: kelvin,
      preserveBrightness: preserveBrightness,
    );
  }

  /// Set node brightness via the server runtime.
  ///
  /// For scene-backed Mood, the server updates the bound scene brightness and
  /// reapplies it. Use [nodeCurveBrightness] only when intentionally editing a
  /// curve modifier.
  Future<void> nodeBrightness({
    required String nodeId,
    required int brightness,
  }) async {
    await nodeBrightnessResult(nodeId: nodeId, brightness: brightness);
  }

  /// Set node brightness and return the authoritative node state when provided.
  Future<RhythmRoomState?> nodeBrightnessResult({
    required String nodeId,
    required int brightness,
  }) {
    return _putNodeBrightness({
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

  /// Set room brightness and return the authoritative node state when provided.
  Future<RhythmRoomState?> roomBrightnessResult({
    required String roomId,
    required int brightness,
  }) {
    return nodeBrightnessResult(nodeId: roomId, brightness: brightness);
  }

  /// Set node color via the server runtime.
  Future<void> nodeColor({
    required String nodeId,
    required int r,
    required int g,
    required int b,
    int? brightness,
    int? transitionMs,
    String? scope,
    RhythmNodeColorScope? colorScope,
  }) async {
    await nodeColorResult(
      nodeId: nodeId,
      r: r,
      g: g,
      b: b,
      brightness: brightness,
      transitionMs: transitionMs,
      scope: scope,
      colorScope: colorScope,
    );
  }

  /// Set node color and return the authoritative node state when provided.
  Future<RhythmRoomState?> nodeColorResult({
    required String nodeId,
    required int r,
    required int g,
    required int b,
    int? brightness,
    int? transitionMs,
    String? scope,
    RhythmNodeColorScope? colorScope,
  }) async {
    return _putNodeColor({
      'node_id': nodeId,
      'rgb': {'r': r, 'g': g, 'b': b},
      if (brightness != null) 'brightness': brightness,
      if (transitionMs != null) 'transition_ms': transitionMs,
      if (colorScope != null) 'scope': colorScope.wireValue,
      if (colorScope == null && scope != null) 'scope': scope,
    });
  }

  /// Set room color via the node-first server contract.
  Future<void> roomColor({
    required String roomId,
    required int r,
    required int g,
    required int b,
    int? brightness,
    int? transitionMs,
    String? scope,
    RhythmNodeColorScope? colorScope,
  }) {
    return nodeColor(
      nodeId: roomId,
      r: r,
      g: g,
      b: b,
      brightness: brightness,
      transitionMs: transitionMs,
      scope: scope,
      colorScope: colorScope,
    );
  }

  /// Set room color and return the authoritative node state when provided.
  Future<RhythmRoomState?> roomColorResult({
    required String roomId,
    required int r,
    required int g,
    required int b,
    int? brightness,
    int? transitionMs,
    String? scope,
    RhythmNodeColorScope? colorScope,
  }) {
    return nodeColorResult(
      nodeId: roomId,
      r: r,
      g: g,
      b: b,
      brightness: brightness,
      transitionMs: transitionMs,
      scope: scope,
      colorScope: colorScope,
    );
  }

  /// Apply brightness curve modifiers to multiple nodes in a single request.
  Future<List<RhythmRoomState>> nodeCurveBrightnessBatch(
      List<({String nodeId, int brightness})> items,
      {int? dispatchSpacingMs}) async {
    return (await nodeCurveBrightnessBatchResult(
      items,
      dispatchSpacingMs: dispatchSpacingMs,
    ))
        .states;
  }

  /// Apply brightness curve modifiers to multiple nodes in a single request.
  Future<RhythmDispatchResult> nodeCurveBrightnessBatchResult(
      List<({String nodeId, int brightness})> items,
      {int? dispatchSpacingMs}) async {
    if (items.isEmpty) return const RhythmDispatchResult();
    return _putNodeCurveModifierBatch([
      for (final i in items) {'node_id': i.nodeId, 'brightness': i.brightness}
    ], dispatchSpacingMs: dispatchSpacingMs);
  }

  /// Apply color-temperature curve modifiers to multiple nodes in a single request.
  Future<List<RhythmRoomState>> nodeCurveColorTemperatureBatch(
      List<({String nodeId, int kelvin})> items,
      {int? dispatchSpacingMs,
      bool preserveBrightness = true}) async {
    return (await nodeCurveColorTemperatureBatchResult(
      items,
      dispatchSpacingMs: dispatchSpacingMs,
      preserveBrightness: preserveBrightness,
    ))
        .states;
  }

  /// Apply color-temperature curve modifiers to multiple nodes in a single request.
  Future<RhythmDispatchResult> nodeCurveColorTemperatureBatchResult(
      List<({String nodeId, int kelvin})> items,
      {int? dispatchSpacingMs,
      bool preserveBrightness = true}) async {
    if (items.isEmpty) return const RhythmDispatchResult();
    return _putNodeCurveModifierBatch([
      for (final i in items)
        {
          'node_id': i.nodeId,
          'color_temperature': i.kelvin,
          'preserve_brightness': preserveBrightness,
        }
    ], dispatchSpacingMs: dispatchSpacingMs);
  }

  /// Apply brightness curve modifiers to multiple rooms in a single request.
  Future<List<RhythmRoomState>> roomCurveBrightnessBatch(
      List<({String roomId, int brightness})> items,
      {int? dispatchSpacingMs}) {
    return nodeCurveBrightnessBatch([
      for (final item in items)
        (nodeId: item.roomId, brightness: item.brightness),
    ], dispatchSpacingMs: dispatchSpacingMs);
  }

  /// Apply brightness curve modifiers to multiple rooms in a single request.
  Future<RhythmDispatchResult> roomCurveBrightnessBatchResult(
      List<({String roomId, int brightness})> items,
      {int? dispatchSpacingMs}) {
    return nodeCurveBrightnessBatchResult([
      for (final item in items)
        (nodeId: item.roomId, brightness: item.brightness),
    ], dispatchSpacingMs: dispatchSpacingMs);
  }

  /// Apply color-temperature curve modifiers to multiple rooms in a single request.
  Future<List<RhythmRoomState>> roomCurveColorTemperatureBatch(
      List<({String roomId, int kelvin})> items,
      {int? dispatchSpacingMs,
      bool preserveBrightness = true}) {
    return nodeCurveColorTemperatureBatch([
      for (final item in items) (nodeId: item.roomId, kelvin: item.kelvin),
    ],
        dispatchSpacingMs: dispatchSpacingMs,
        preserveBrightness: preserveBrightness);
  }

  /// Apply color-temperature curve modifiers to multiple rooms in a single request.
  Future<RhythmDispatchResult> roomCurveColorTemperatureBatchResult(
      List<({String roomId, int kelvin})> items,
      {int? dispatchSpacingMs,
      bool preserveBrightness = true}) {
    return nodeCurveColorTemperatureBatchResult([
      for (final item in items) (nodeId: item.roomId, kelvin: item.kelvin),
    ],
        dispatchSpacingMs: dispatchSpacingMs,
        preserveBrightness: preserveBrightness);
  }

  /// Set brightness for multiple nodes in a single request.
  Future<List<RhythmRoomState>> nodeBrightnessBatch(
      List<({String nodeId, int brightness})> items,
      {int? dispatchSpacingMs}) async {
    return (await nodeBrightnessBatchResult(
      items,
      dispatchSpacingMs: dispatchSpacingMs,
    ))
        .states;
  }

  /// Set brightness for multiple nodes in a single request.
  Future<RhythmDispatchResult> nodeBrightnessBatchResult(
      List<({String nodeId, int brightness})> items,
      {int? dispatchSpacingMs}) async {
    if (items.isEmpty) return const RhythmDispatchResult();
    return _putNodeBrightnessBatch([
      for (final i in items) {'node_id': i.nodeId, 'brightness': i.brightness}
    ], dispatchSpacingMs: dispatchSpacingMs);
  }

  /// Set brightness for multiple rooms in a single request.
  Future<List<RhythmRoomState>> roomBrightnessBatch(
      List<({String roomId, int brightness})> items,
      {int? dispatchSpacingMs}) {
    return nodeBrightnessBatch([
      for (final item in items)
        (nodeId: item.roomId, brightness: item.brightness),
    ], dispatchSpacingMs: dispatchSpacingMs);
  }

  /// Set brightness for multiple rooms in a single request.
  Future<RhythmDispatchResult> roomBrightnessBatchResult(
      List<({String roomId, int brightness})> items,
      {int? dispatchSpacingMs}) {
    return roomCurveBrightnessBatchResult(
      items,
      dispatchSpacingMs: dispatchSpacingMs,
    );
  }

  /// Set the time offset for a single node.
  Future<void> nodeOffset({
    required String nodeId,
    required double timeOffset,
  }) async {
    await nodeOffsetPreviewResult(
      timeOffset: timeOffset,
      nodes: [nodeId],
    );
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
      {int? dispatchSpacingMs}) async {
    return (await nodeOffsetBatchResult(
      items,
      dispatchSpacingMs: dispatchSpacingMs,
    ))
        .states;
  }

  /// Set time offset for multiple nodes in a single request.
  Future<RhythmDispatchResult> nodeOffsetBatchResult(
      List<({String nodeId, double timeOffset})> items,
      {int? dispatchSpacingMs}) async {
    if (items.isEmpty) return const RhythmDispatchResult();
    final timeOffset = items.first.timeOffset;
    final hasMixedOffsets = items.any((i) => i.timeOffset != timeOffset);
    if (hasMixedOffsets) {
      throw ArgumentError.value(
        items,
        'items',
        'nodeOffsetBatchResult requires the same timeOffset for every node',
      );
    }
    return nodeOffsetPreviewResult(
      timeOffset: timeOffset,
      nodes: [for (final i in items) i.nodeId],
      dispatchSpacingMs: dispatchSpacingMs,
    );
  }

  /// Preview a time offset using the server-owned periodic tick target scope.
  ///
  /// When [nodes] is omitted or empty, the server targets the same nodes it
  /// would target during a periodic tick.
  Future<RhythmDispatchResult> nodeOffsetPreviewResult({
    required double timeOffset,
    List<String>? nodes,
    int? dispatchSpacingMs,
  }) async {
    try {
      final response = await _dio.put(
        'api/nodes/offset',
        data: <String, Object?>{
          'time_offset': timeOffset,
          if (nodes != null) 'nodes': nodes,
          if (dispatchSpacingMs != null)
            'dispatch_spacing_ms': dispatchSpacingMs,
        },
        options: Options(receiveTimeout: const Duration(seconds: 30)),
      );
      return _parseAndCacheDispatchResponse(response.data);
    } catch (e) {
      _log.warning('nodeOffsetPreview failed', e);
    }
    return const RhythmDispatchResult();
  }

  /// Set time offset for multiple rooms in a single request.
  Future<List<RhythmRoomState>> roomOffsetBatch(
      List<({String roomId, double timeOffset})> items,
      {int? dispatchSpacingMs}) {
    return nodeOffsetBatch([
      for (final item in items)
        (nodeId: item.roomId, timeOffset: item.timeOffset),
    ], dispatchSpacingMs: dispatchSpacingMs);
  }

  /// Set time offset for multiple rooms in a single request.
  Future<RhythmDispatchResult> roomOffsetBatchResult(
      List<({String roomId, double timeOffset})> items,
      {int? dispatchSpacingMs}) {
    return nodeOffsetBatchResult([
      for (final item in items)
        (nodeId: item.roomId, timeOffset: item.timeOffset),
    ], dispatchSpacingMs: dispatchSpacingMs);
  }

  /// Push node preferences (rhythm_enabled, disabled, state).
  Future<void> nodePreferencesSet({
    required String nodeId,
    bool? rhythmEnabled,
    bool? disabled,
    bool? standbyEnabled,
    RoomModeState? state,
    bool? softOff,
    Map<String, dynamic>? profileSettings,
  }) async {
    final effectiveState = state ??
        (softOff == null
            ? null
            : (softOff ? RoomModeState.standby : RoomModeState.active));
    final normalizedProfileSettings =
        _normalizeProfileSettings(profileSettings);
    await _safePut('api/nodes/preferences', data: {
      'node_id': nodeId,
      if (rhythmEnabled != null) 'rhythm_enabled': rhythmEnabled,
      if (disabled != null) 'disabled': disabled,
      if (standbyEnabled != null) 'standby_enabled': standbyEnabled,
      if (effectiveState != null) 'state': effectiveState.wireValue,
      if (normalizedProfileSettings != null)
        'profile_settings': normalizedProfileSettings,
    });
  }

  /// Set motion admission and return the authoritative post-apply node state.
  ///
  /// This endpoint is capability-gated because older appliances accept and
  /// ignore unknown profile fields on the generic preferences endpoint.
  Future<RhythmRoomState?> nodeMotionActivationSet({
    required String nodeId,
    required bool enabled,
    required String requestId,
  }) async {
    try {
      final response = await _dio.put(
        'api/nodes/motion-activation',
        data: {'node_id': nodeId, 'enabled': enabled, 'request_id': requestId},
      );
      return _parseAndCacheSingleState(response.data);
    } catch (e) {
      _log.warning(
        'nodeMotionActivationSet failed node=$nodeId enabled=$enabled requestId=$requestId',
        e,
      );
    }
    return null;
  }

  /// Patch per-profile node overrides.
  Future<void> nodeProfileOverridesSet({
    required String nodeId,
    required Map<String, dynamic>? profileOverrides,
  }) async {
    await _safePut('api/nodes/profile-overrides', data: {
      'node_id': nodeId,
      'profile_overrides': _normalizeProfileOverrides(profileOverrides),
    });
  }

  /// Push room preferences (rhythm_enabled, disabled, state).
  Future<void> roomPreferencesSet({
    required String roomId,
    bool? rhythmEnabled,
    bool? disabled,
    bool? standbyEnabled,
    RoomModeState? state,
    bool? softOff,
    Map<String, dynamic>? profileSettings,
  }) {
    return nodePreferencesSet(
      nodeId: roomId,
      rhythmEnabled: rhythmEnabled,
      disabled: disabled,
      standbyEnabled: standbyEnabled,
      state: state,
      softOff: softOff,
      profileSettings: profileSettings,
    );
  }

  /// Push node preferences for multiple nodes.
  Future<void> nodePreferencesBatchSet(
    List<Map<String, dynamic>> items, {
    int? dispatchSpacingMs,
  }) async {
    await nodePreferencesBatchSetResult(
      items,
      dispatchSpacingMs: dispatchSpacingMs,
    );
  }

  /// Push node preferences for multiple nodes.
  Future<RhythmDispatchResult> nodePreferencesBatchSetResult(
    List<Map<String, dynamic>> items, {
    int? dispatchSpacingMs,
  }) async {
    if (items.isEmpty) return const RhythmDispatchResult();
    try {
      final response = await _dio.put(
        'api/nodes/preferences',
        data: _nodesBatchBody(
            [for (final item in items) _normalizeNodePreferencesItem(item)],
            dispatchSpacingMs: dispatchSpacingMs),
        options: Options(receiveTimeout: const Duration(seconds: 30)),
      );
      return _parseAndCacheDispatchResponse(response.data);
    } catch (e) {
      _log.warning('nodePreferencesBatchSet failed', e);
    }
    return const RhythmDispatchResult();
  }

  /// Push room preferences for multiple rooms.
  Future<void> roomPreferencesBatchSet(
    List<Map<String, dynamic>> items, {
    int? dispatchSpacingMs,
  }) async {
    await roomPreferencesBatchSetResult(
      items,
      dispatchSpacingMs: dispatchSpacingMs,
    );
  }

  /// Push room preferences for multiple rooms.
  Future<RhythmDispatchResult> roomPreferencesBatchSetResult(
    List<Map<String, dynamic>> items, {
    int? dispatchSpacingMs,
  }) async {
    return nodePreferencesBatchSetResult([
      for (final item in items)
        {
          ...item,
          'node_id': item['room_id'],
        },
    ], dispatchSpacingMs: dispatchSpacingMs);
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
  // Scenes
  // =========================================================================

  /// Fetch scene definitions from the server.
  ///
  /// When [targetId] is provided, capable appliances also include ephemeral
  /// native scenes that apply to that room.
  Future<List<RhythmSceneDefinition>> getScenes({String? targetId}) async {
    final catalog = await getSceneCatalog(targetId: targetId);
    return catalog?.scenes ?? const [];
  }

  /// Fetch scene definitions together with native-discovery outcome metadata.
  ///
  /// Returns `null` when the request or response fails, allowing callers to
  /// preserve an existing cache. Previous appliances omit the additive
  /// discovery field and therefore parse as a complete stored-scene catalog.
  Future<RhythmSceneCatalogResult?> getSceneCatalog({String? targetId}) async {
    try {
      final response = targetId == null
          ? await _dio.get('api/scenes')
          : await _dio.get(
              'api/scenes',
              queryParameters: {'target_id': targetId},
            );
      return _parseSceneCatalogResponse(response.data);
    } catch (e) {
      _log.warning('getSceneCatalog failed', e);
    }
    return null;
  }

  /// Upsert a scene with the scene ID from the request body.
  Future<RhythmSceneDefinition?> upsertScene(
    RhythmSceneDefinition scene,
  ) async {
    try {
      final response = await _dio.post('api/scenes', data: scene.toJson());
      return _parseSceneResponse(response.data);
    } catch (e) {
      _log.warning('upsertScene failed', e);
    }
    return null;
  }

  /// Upsert a scene with the path ID winning over the body ID.
  Future<RhythmSceneDefinition?> putScene(
    String id,
    RhythmSceneDefinition scene,
  ) async {
    try {
      final response = await _dio.put(
        'api/scenes/${Uri.encodeComponent(id)}',
        data: scene.toJson(),
      );
      return _parseSceneResponse(response.data);
    } catch (e) {
      _log.warning('putScene failed', e);
    }
    return null;
  }

  /// Delete a scene and return the authoritative scene list from the server.
  Future<List<RhythmSceneDefinition>> deleteScene(String id) async {
    try {
      final response =
          await _dio.delete('api/scenes/${Uri.encodeComponent(id)}');
      return _parseScenesResponse(response.data);
    } catch (e) {
      _log.warning('deleteScene failed', e);
    }
    return const [];
  }

  /// Apply a saved scene to a target node or room.
  Future<RhythmSceneActionResult?> applyScene({
    required String sceneId,
    required String targetId,
    int? transitionMs,
  }) {
    return _postSceneAction(
      'api/scenes/${Uri.encodeComponent(sceneId)}/apply',
      {
        'target_id': targetId,
        if (transitionMs != null) 'transition_ms': transitionMs,
      },
      logName: 'applyScene',
    );
  }

  /// Preview a saved scene temporarily.
  Future<RhythmSceneActionResult?> previewScene({
    required String sceneId,
    required String targetId,
    int? transitionMs,
    int? durationMs,
  }) {
    return _postSceneAction(
      'api/scenes/${Uri.encodeComponent(sceneId)}/preview',
      {
        'target_id': targetId,
        if (transitionMs != null) 'transition_ms': transitionMs,
        if (durationMs != null) 'duration_ms': durationMs,
      },
      logName: 'previewScene',
    );
  }

  /// Preview an unsaved draft scene temporarily.
  Future<RhythmSceneActionResult?> previewDraftScene({
    required RhythmSceneDefinition scene,
    required String targetId,
    int? transitionMs,
    int? durationMs,
  }) {
    return _postSceneAction(
      'api/scenes/preview',
      {
        'scene': scene.toJson(),
        'target_id': targetId,
        if (transitionMs != null) 'transition_ms': transitionMs,
        if (durationMs != null) 'duration_ms': durationMs,
      },
      logName: 'previewDraftScene',
    );
  }

  /// Commit a currently active scene preview.
  Future<RhythmSceneActionResult?> commitScenePreview(String previewId) {
    return _postSceneAction(
      'api/scene-previews/${Uri.encodeComponent(previewId)}/commit',
      const <String, dynamic>{},
      logName: 'commitScenePreview',
    );
  }

  /// Cancel a currently active scene preview and restore previous output.
  Future<RhythmSceneActionResult?> cancelScenePreview(String previewId) {
    return _postSceneAction(
      'api/scene-previews/${Uri.encodeComponent(previewId)}/cancel',
      const <String, dynamic>{},
      logName: 'cancelScenePreview',
    );
  }

  /// Bind a node to a scene-backed Mood and enter Mood state.
  Future<void> nodeMoodSceneSet({
    required String nodeId,
    required String sceneId,
  }) {
    return nodePreferencesSet(
      nodeId: nodeId,
      state: RoomModeState.mood,
      profileSettings: {'mood_scene_id': sceneId},
    );
  }

  /// Bind a room to a scene-backed Mood and enter Mood state.
  Future<void> roomMoodSceneSet({
    required String roomId,
    required String sceneId,
  }) {
    return nodeMoodSceneSet(nodeId: roomId, sceneId: sceneId);
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
    return (await absorbTimeOffsetResult(offsetMinutes, id: id)).config;
  }

  /// Absorb a time offset and return dispatch metadata for UI loading.
  Future<RhythmAbsorbTimeOffsetResult> absorbTimeOffsetResult(
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
        final configData = data['config'] is Map<String, dynamic>
            ? data['config'] as Map<String, dynamic>
            : data['profile'] is Map<String, dynamic>
                ? data['profile'] as Map<String, dynamic>
                : data;
        final dispatchData =
            data['dispatch'] ?? data['dispatch_result'] ?? data;
        return RhythmAbsorbTimeOffsetResult(
          config: _parseCurveConfig(configData),
          dispatch: _parseAndCacheDispatchResponse(dispatchData),
        );
      }
    } catch (e) {
      _log.warning('absorbTimeOffsetResult failed', e);
    }
    return const RhythmAbsorbTimeOffsetResult();
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
  /// When [apply] is true and [id] is the server's currently active profile,
  /// the server immediately re-dispatches lights with the new tuning without
  /// changing mode or transition state. Inactive profiles save silently.
  ///
  /// Returns `true` when the server accepts the update and `false` on failure.
  Future<bool> configSet(
    RhythmCurveConfig config, {
    String? id,
    bool apply = false,
  }) async {
    final profileId = id ?? config.id;
    if (profileId.isEmpty) {
      _log.warning('configSet skipped: missing profile id');
      return false;
    }
    try {
      await _dio.put(
        'api/config',
        queryParameters: {
          'id': profileId,
          if (apply) 'apply': 'true',
        },
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
  Future<bool> hubCredentials({
    required String hubType,
    required String address,
    required Map<String, dynamic> credentials,
  }) async {
    try {
      final response = await _dio.put('api/hub/credentials', data: {
        'hub_type': hubType,
        'address': address,
        'credentials': credentials,
      });
      final data = response.data;
      if (data is Map) {
        return data['hub_connected'] == true;
      }
    } catch (e) {
      _log.warning('hubCredentials failed', e);
    }
    return false;
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
    await nodePreferencesSet(
      nodeId: effectiveNodeId,
      profileSettings: {'motion_timeout_secs': timeoutSecs},
    );
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

  /// Fetch the global autonomous light-control switch from the server.
  Future<RhythmLightBreaker?> getLightBreaker() async {
    try {
      final response = await _dio.get('api/light-breaker');
      return RhythmLightBreaker.fromJson(
        response.data as Map<String, dynamic>,
      );
    } catch (e) {
      _log.warning('getLightBreaker failed', e);
    }
    return null;
  }

  /// Set the global autonomous light-control switch on the server.
  ///
  /// Returns `true` when the server accepts the update and `false` on failure.
  Future<bool> setLightBreaker(bool enabled) async {
    try {
      await _dio.put('api/light-breaker', data: {'enabled': enabled});
      return true;
    } catch (e) {
      _log.warning('setLightBreaker failed', e);
      return false;
    }
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

  /// Fetch the active light runtime.
  Future<RhythmLightRuntimeState?> getLightRuntime() async {
    try {
      final response = await _dio.get('api/light-runtime');
      return RhythmLightRuntimeState.fromJson(
        response.data as Map<String, dynamic>,
      );
    } catch (e) {
      _log.warning('getLightRuntime failed', e);
    }
    return null;
  }

  /// Select the active light runtime.
  Future<RhythmLightRuntimeState?> setLightRuntime(
    RhythmLightRuntime runtime, {
    int? transitionMs,
  }) async {
    try {
      final response = await _dio.put(
        'api/light-runtime',
        data: {
          'runtime_id': runtime.id,
          if (transitionMs != null) 'transition_ms': transitionMs,
        },
      );
      return RhythmLightRuntimeState.fromJson(
        response.data as Map<String, dynamic>,
      );
    } catch (e) {
      _log.warning('setLightRuntime failed', e);
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
  /// [powerSave] is retained for older callers, but the server has removed the
  /// setting and this client no longer sends it.
  ///
  /// Returns `true` when the server accepts the update and `false` on failure.
  Future<bool> settingsSet({
    bool? powerSave,
    bool? autoUpdate,
  }) async {
    final data = <String, dynamic>{
      if (autoUpdate != null) 'auto_update': autoUpdate,
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

  /// Rename a canonical device.
  Future<bool> renameCanonicalDevice(String id, String name) async {
    try {
      final response = await _dio.put(
        'api/devices/canonical/$id',
        data: {'name': name},
      );
      final code = response.statusCode ?? 0;
      return code >= 200 && code < 300;
    } catch (e) {
      _log.warning('renameCanonicalDevice failed', e);
    }
    return false;
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

  /// Briefly pulse a Light device for physical identification. The server
  /// only supports this for Light devices; other types return an error.
  Future<bool> flashCanonicalDevice(String id) async {
    try {
      final response = await _dio.post('api/devices/canonical/$id/flash');
      final code = response.statusCode ?? 0;
      return code >= 200 && code < 300;
    } catch (e) {
      _log.warning('flashCanonicalDevice failed', e);
    }
    return false;
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

  /// Fetch persisted physical input bindings.
  Future<List<RhythmInputBinding>> getInputBindings() async {
    try {
      final response = await _dio.get('api/input-bindings');
      return _parseInputBindingsResponse(response.data);
    } catch (e) {
      _log.warning('getInputBindings failed', e);
    }
    return const [];
  }

  /// Create a preset-backed input binding with a server-generated stable ID.
  Future<List<RhythmInputBinding>> createPresetInputBinding({
    required RhythmInputBindingPreset preset,
    required String sourceNodeId,
    RhythmButtonAction? buttonAction,
    bool enabled = true,
  }) async {
    try {
      final response = await _dio.post('api/input-bindings', data: {
        'preset': preset.wireValue,
        'source_node_id': sourceNodeId,
        if (buttonAction != null) 'button_action': buttonAction.wireValue,
        'enabled': enabled,
      });
      return _parseInputBindingsResponse(response.data);
    } catch (e) {
      _log.warning('createPresetInputBinding failed', e);
    }
    return const [];
  }

  /// Create the built-in day/sleep toggle binding for a selected button.
  Future<List<RhythmInputBinding>> createDaySleepToggleInputBinding({
    required String sourceNodeId,
    RhythmButtonAction? buttonAction = RhythmButtonAction.onPress,
    bool enabled = true,
  }) {
    return createPresetInputBinding(
      preset: RhythmInputBindingPreset.daySleepToggle,
      sourceNodeId: sourceNodeId,
      buttonAction: buttonAction,
      enabled: enabled,
    );
  }

  /// Set or replace a preset-backed input binding with an explicit ID.
  Future<List<RhythmInputBinding>> setPresetInputBinding({
    required String id,
    required RhythmInputBindingPreset preset,
    required String sourceNodeId,
    RhythmButtonAction? buttonAction,
    bool enabled = true,
  }) async {
    try {
      final response = await _dio.put(
        'api/input-bindings/${Uri.encodeComponent(id)}',
        data: {
          'preset': preset.wireValue,
          'source_node_id': sourceNodeId,
          if (buttonAction != null) 'button_action': buttonAction.wireValue,
          'enabled': enabled,
        },
      );
      return _parseInputBindingsResponse(response.data);
    } catch (e) {
      _log.warning('setPresetInputBinding failed', e);
    }
    return const [];
  }

  /// Set or replace a built-in day/sleep toggle binding with an explicit ID.
  Future<List<RhythmInputBinding>> setDaySleepToggleInputBinding({
    required String id,
    required String sourceNodeId,
    RhythmButtonAction? buttonAction = RhythmButtonAction.onPress,
    bool enabled = true,
  }) {
    return setPresetInputBinding(
      id: id,
      preset: RhythmInputBindingPreset.daySleepToggle,
      sourceNodeId: sourceNodeId,
      buttonAction: buttonAction,
      enabled: enabled,
    );
  }

  /// Set or replace a generic input binding with explicit trigger/action JSON.
  Future<List<RhythmInputBinding>> setInputBinding(
    RhythmInputBinding binding,
  ) async {
    try {
      final response = await _dio.put(
        'api/input-bindings/${Uri.encodeComponent(binding.id)}',
        data: binding.toJson(),
      );
      return _parseInputBindingsResponse(response.data);
    } catch (e) {
      _log.warning('setInputBinding failed', e);
    }
    return const [];
  }

  /// Delete a persisted input binding by ID.
  Future<List<RhythmInputBinding>> deleteInputBinding(String id) async {
    try {
      final response = await _dio.delete(
        'api/input-bindings/${Uri.encodeComponent(id)}',
      );
      return _parseInputBindingsResponse(response.data);
    } catch (e) {
      _log.warning('deleteInputBinding failed', e);
    }
    return const [];
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

  /// Replace all explicit topology control targets for a node.
  Future<bool> setTopologyNodeControlTargets({
    required String nodeId,
    required String controlKind,
    required List<String> targetIds,
  }) async {
    try {
      await _dio.put(
        'api/topology/nodes/$nodeId/controls/$controlKind',
        data: {'target_ids': targetIds},
      );
      return true;
    } catch (e) {
      _log.warning('setTopologyNodeControlTargets failed', e);
    }
    return false;
  }

  // =========================================================================
  // Device pairing
  // =========================================================================

  /// Commission a new device (Matter, Zigbee, etc.).
  ///
  /// Returns the [PairingSession] JSON from the server, or `null` on error.
  ///
  /// [sessionId] is an optional client-generated correlation ID. When
  /// supplied, the server echoes it on `pairing_progress` SSE events so the
  /// caller can match streamed progress to its own pairing request.
  Future<Map<String, dynamic>?> pairDevice({
    required String hubType,
    Map<String, dynamic> params = const {},
    Duration receiveTimeout = const Duration(seconds: 45),
    String? sessionId,
  }) async {
    try {
      final mergedParams = sessionId == null
          ? params
          : <String, dynamic>{...params, 'session_id': sessionId};
      final response = await _dio.post(
        'api/devices/pair',
        data: {
          'hub_type': hubType,
          if (sessionId != null) 'session_id': sessionId,
          'params': mergedParams,
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
  /// A graceful (non-force) Matter unpair talks to the device over CASE and
  /// can take up to a minute to fail when the device is offline, so the
  /// default timeout is well above the connection-level default.
  ///
  /// Returns the unpair result JSON, or `null` on error.
  Future<Map<String, dynamic>?> unpairDevice({
    required String hubType,
    required String deviceId,
    bool force = false,
    Duration receiveTimeout = const Duration(seconds: 90),
  }) async {
    try {
      final response = await _dio.post(
        'api/devices/unpair',
        data: {
          'hub_type': hubType,
          'params': {
            'device_id': deviceId,
            'force': force,
          },
        },
        options: Options(
          receiveTimeout: receiveTimeout,
          sendTimeout: receiveTimeout,
        ),
      );
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

  Future<RhythmRoomState?> _putNodeBrightness(
    Map<String, dynamic> data,
  ) async {
    try {
      final response = await _dio.put(
        'api/nodes/brightness',
        data: data,
        queryParameters: null,
      );
      return _parseAndCacheSingleState(response.data);
    } catch (e) {
      _log.warning('nodeBrightness failed', e);
    }
    return null;
  }

  Future<RhythmDispatchResult> _putNodeBrightnessBatch(
    List<Map<String, dynamic>> nodes, {
    int? dispatchSpacingMs,
  }) async {
    if (nodes.isEmpty) return const RhythmDispatchResult();
    try {
      final response = await _dio.put(
        'api/nodes/brightness',
        data: _nodesBatchBody(nodes, dispatchSpacingMs: dispatchSpacingMs),
        options: Options(receiveTimeout: const Duration(seconds: 30)),
      );
      return _parseAndCacheDispatchResponse(response.data);
    } catch (e) {
      _log.warning('nodeBrightnessBatch failed', e);
    }
    return const RhythmDispatchResult();
  }

  Future<RhythmRoomState?> _putNodeColor(
    Map<String, dynamic> data,
  ) async {
    try {
      final response = await _dio.put(
        'api/nodes/color',
        data: data,
        queryParameters: null,
      );
      return _parseAndCacheSingleState(response.data);
    } catch (e) {
      _log.warning('nodeColor failed', e);
    }
    return null;
  }

  Future<RhythmRoomState?> _putNodeCurveModifier(
    Map<String, dynamic> data,
  ) async {
    try {
      final response = await _dio.put('api/nodes/curve', data: data);
      return _parseAndCacheSingleState(response.data);
    } catch (e) {
      _log.warning('nodeCurveModifier failed', e);
    }
    return null;
  }

  Future<RhythmDispatchResult> _putNodeCurveModifierBatch(
    List<Map<String, dynamic>> nodes, {
    int? dispatchSpacingMs,
  }) async {
    if (nodes.isEmpty) return const RhythmDispatchResult();
    try {
      final response = await _dio.put(
        'api/nodes/curve',
        data: _nodesBatchBody(nodes, dispatchSpacingMs: dispatchSpacingMs),
        options: Options(receiveTimeout: const Duration(seconds: 30)),
      );
      return _parseAndCacheDispatchResponse(response.data);
    } catch (e) {
      _log.warning('nodeCurveModifierBatch failed', e);
    }
    return const RhythmDispatchResult();
  }

  Future<RhythmSceneActionResult?> _postSceneAction(
    String path,
    Map<String, dynamic> data, {
    required String logName,
  }) async {
    try {
      final response = await _dio.post(path, data: data);
      final responseJson = jsonMap(response.data);
      if (responseJson == null) return null;
      return RhythmSceneActionResult.fromJson(responseJson);
    } catch (e) {
      _log.warning('$logName failed', e);
    }
    return null;
  }

  RhythmSceneDefinition? _parseSceneResponse(dynamic responseData) {
    final sceneJson = jsonMap(responseData);
    if (sceneJson == null) return null;
    return RhythmSceneDefinition.fromJson(sceneJson);
  }

  List<RhythmSceneDefinition> _parseScenesResponse(dynamic responseData) {
    final scenes = responseData is List<dynamic>
        ? responseData
        : jsonMap(responseData)?['scenes'] as List<dynamic>?;
    if (scenes == null) return const [];
    return scenes
        .map(jsonMap)
        .nonNulls
        .map(RhythmSceneDefinition.fromJson)
        .toList();
  }

  RhythmSceneCatalogResult? _parseSceneCatalogResponse(dynamic responseData) {
    if (responseData is List<dynamic>) {
      return RhythmSceneCatalogResult(
        scenes: _parseScenesResponse(responseData),
      );
    }
    final responseJson = jsonMap(responseData);
    if (responseJson == null || responseJson['scenes'] is! List<dynamic>) {
      return null;
    }
    return RhythmSceneCatalogResult.fromJson(responseJson);
  }

  RhythmRoomState? _parseAndCacheSingleState(dynamic responseData) {
    final data = responseData as Map<String, dynamic>?;
    final nodes =
        data?['nodes'] as List<dynamic>? ?? data?['rooms'] as List<dynamic>?;
    Map<String, dynamic>? nodeJson;
    if (nodes != null && nodes.isNotEmpty) {
      nodeJson = nodes[0] as Map<String, dynamic>?;
    } else if (data != null && data.containsKey('rhythm_enabled')) {
      nodeJson = data;
    }
    if (nodeJson == null || !nodeJson.containsKey('rhythm_enabled')) {
      return null;
    }
    final state = RhythmRoomState.fromJson(nodeJson);
    _onStatesReceived?.call([state]);
    return state;
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

  RhythmDispatchResult _parseAndCacheDispatchResponse(dynamic responseData) {
    if (responseData is List<dynamic>) {
      return RhythmDispatchResult(
        states: _parseAndCacheStatesList(responseData),
      );
    }
    final data = responseData as Map<String, dynamic>?;
    final states =
        data?['nodes'] as List<dynamic>? ?? data?['rooms'] as List<dynamic>?;
    return RhythmDispatchResult(
      states: states == null ? const [] : _parseAndCacheStatesList(states),
      metadata: data == null
          ? const RhythmDispatchMetadata()
          : RhythmDispatchMetadata.fromJson(data),
    );
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

  Map<String, dynamic> _normalizeNodePreferencesItem(
    Map<String, dynamic> item,
  ) {
    final normalized = <String, dynamic>{
      for (final entry in item.entries)
        if (entry.key != 'room_id' && entry.key != 'room_profile')
          entry.key: entry.key == 'profile_settings'
              ? _normalizeProfileSettings(
                  entry.value is Map<String, dynamic>
                      ? entry.value as Map<String, dynamic>
                      : null,
                )
              : entry.value,
    };

    if (!normalized.containsKey('node_id') && item.containsKey('room_id')) {
      normalized['node_id'] = item['room_id'];
    }

    if (!normalized.containsKey('profile_settings') &&
        item['room_profile'] is Map<String, dynamic>) {
      normalized['profile_settings'] = _normalizeProfileSettings(
        item['room_profile'] as Map<String, dynamic>,
      );
    }

    return normalized;
  }

  Map<String, dynamic>? _normalizeProfileSettings(
    Map<String, dynamic>? profileSettings,
  ) {
    if (profileSettings == null) return null;
    return <String, dynamic>{
      for (final entry in profileSettings.entries)
        if (entry.key != 'rhythm_interval_secs')
          entry.key: switch (entry.key) {
            'fade_ms' ||
            'motion_timeout_secs' =>
              _normalizeTimerSettingValue(entry.value),
            'profile_overrides' => _normalizeProfileOverrides(entry.value),
            _ => entry.value,
          },
    };
  }

  Map<String, dynamic>? _normalizeProfileOverrides(dynamic value) {
    if (value == null) return null;
    final map = value is Map<String, dynamic>
        ? value
        : value is Map
            ? value.cast<String, dynamic>()
            : null;
    if (map == null) return null;

    return <String, dynamic>{
      for (final entry in map.entries)
        if (entry.key.trim().isNotEmpty)
          entry.key: entry.value == null
              ? null
              : _normalizeProfileOverride(entry.value),
    };
  }

  Map<String, dynamic>? _normalizeProfileOverride(dynamic value) {
    final map = value is Map<String, dynamic>
        ? value
        : value is Map
            ? value.cast<String, dynamic>()
            : null;
    if (map == null) return null;

    return <String, dynamic>{
      for (final entry in map.entries)
        entry.key: switch (entry.key) {
          'fade_ms' ||
          'motion_timeout_secs' =>
            _normalizeTimerSettingValue(entry.value),
          _ => entry.value,
        },
    };
  }

  dynamic _normalizeTimerSettingValue(dynamic value) {
    if (value == null) return null;
    if (value is num) {
      return RhythmTimerSetting.fixed(value.toInt()).toJson();
    }
    if (value is String) {
      final parsed = int.tryParse(value);
      if (parsed != null) {
        return RhythmTimerSetting.fixed(parsed).toJson();
      }
      return value;
    }
    final map = value is Map<String, dynamic>
        ? value
        : value is Map
            ? value.cast<String, dynamic>()
            : null;
    if (map == null) return value;
    if (map['mode'] is String ||
        map['value'] != null ||
        map['breakpoints'] is List<dynamic>) {
      return RhythmTimerSetting.fromJson(map).toJson();
    }
    return value;
  }

  Object _nodesBatchBody(
    List<Map<String, dynamic>> nodes, {
    int? dispatchSpacingMs,
  }) {
    final normalizedSpacing = _normalizedDispatchSpacingMs(dispatchSpacingMs);
    if (normalizedSpacing == null) return nodes;
    return {
      'nodes': nodes,
      'dispatch_spacing_ms': normalizedSpacing,
    };
  }
}

int? _jsonInt(Object? value) => switch (value) {
      int v => v,
      num v => v.toInt(),
      String v => int.tryParse(v),
      _ => null,
    };

List<RhythmInputBinding> _parseInputBindingsResponse(Object? data) {
  if (data is Map<String, dynamic>) {
    return RhythmInputBindings.fromJson(data).bindings;
  }
  if (data is Map) {
    return RhythmInputBindings.fromJson(data.cast<String, dynamic>()).bindings;
  }
  return const [];
}

int? _normalizedDispatchSpacingMs(int? value) {
  if (value == null) return null;
  if (value < 0) return 0;
  if (value > 60000) return 60000;
  return value;
}
