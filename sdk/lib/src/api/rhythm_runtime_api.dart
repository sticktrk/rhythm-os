import 'package:dio/dio.dart';
import 'package:logging/logging.dart';

import '../api_auth.dart';
import '../json_parsing.dart';
import '../models/rhythm_environment.dart';
import '../models/rhythm_room.dart';
import '../models/rhythm_runtime.dart';
import '../rhythm_log_interceptor.dart';
import 'rhythm_server_api.dart';

class RhythmRuntimeApi {
  static final _log = Logger('rhythm_sdk.api');

  final Dio _dio;
  final RhythmCacheUpdater? _onStatesReceived;

  RhythmRuntimeApi(this._dio, {RhythmCacheUpdater? onStatesReceived})
      : _onStatesReceived = onStatesReceived;

  RhythmRuntimeApi.fromBaseUrl({
    required String baseUrl,
    Dio? dio,
    String? authToken,
    RhythmCacheUpdater? onStatesReceived,
    Duration connectTimeout = const Duration(seconds: 10),
    Duration receiveTimeout = const Duration(seconds: 10),
  })  : _dio = dio ??
            Dio(
              BaseOptions(
                baseUrl: baseUrl,
                connectTimeout: connectTimeout,
                receiveTimeout: receiveTimeout,
                headers: bearerAuthHeaders(authToken),
              ),
            ),
        _onStatesReceived = onStatesReceived {
    final headers = bearerAuthHeaders(authToken);
    if (headers != null) {
      _dio.options.headers.addAll(headers);
    }
    _dio.interceptors.add(RhythmLogInterceptor(_log));
  }

  Future<RhythmNodesPollResponse> getNodesState() async {
    try {
      final response = await _dio.get('api/nodes/state');
      final data = jsonMap(response.data);
      if (data == null) return const RhythmNodesPollResponse.empty();
      final result = RhythmNodesPollResponse.fromJson(data);
      _cacheStates(result.nodes);
      return result;
    } catch (e) {
      _log.warning('getNodesState failed', e);
    }
    return const RhythmNodesPollResponse.empty();
  }

  Future<RhythmNodeNow?> getNodeNow(String nodeId) async {
    try {
      final response = await _dio.get(
        'api/nodes/${Uri.encodeComponent(nodeId)}/now',
      );
      final data = jsonMap(response.data);
      if (data == null) return null;
      final result = RhythmNodeNow.fromJson(data);
      _cacheStates([result.node]);
      return result;
    } catch (e) {
      _log.warning('getNodeNow failed', e);
    }
    return null;
  }

  Future<RhythmDispatchResult> previewNodeSlider({
    required String nodeId,
    double? timeOffset,
    int? brightness,
    int? colorTemperature,
    bool? preserveBrightness,
    Map<String, dynamic> extra = const <String, dynamic>{},
  }) async {
    try {
      final response = await _dio.post(
        'api/nodes/${Uri.encodeComponent(nodeId)}/slider-preview',
        data: <String, dynamic>{
          ...extra,
          if (timeOffset != null) 'time_offset': timeOffset,
          if (brightness != null) 'brightness': brightness,
          if (colorTemperature != null) 'color_temperature': colorTemperature,
          if (preserveBrightness != null)
            'preserve_brightness': preserveBrightness,
        },
      );
      return _parseAndCacheDispatchResponse(response.data);
    } catch (e) {
      _log.warning('previewNodeSlider failed', e);
    }
    return const RhythmDispatchResult();
  }

  Future<RhythmScopeLayerWritesResult> putNodeLayer(
    String nodeId,
    Map<String, dynamic> body,
  ) async {
    try {
      final response = await _dio.put(
        'api/nodes/${Uri.encodeComponent(nodeId)}/layer',
        data: body,
      );
      return _parseScopeLayerResult(response.data);
    } catch (e) {
      _log.warning('putNodeLayer failed', e);
    }
    return const RhythmScopeLayerWritesResult.empty();
  }

  Future<RhythmScopeLayerWritesResult> promoteNodeLayer(String nodeId) async {
    try {
      final response = await _dio.post(
        'api/nodes/${Uri.encodeComponent(nodeId)}/promote',
      );
      return _parseScopeLayerResult(response.data);
    } catch (e) {
      _log.warning('promoteNodeLayer failed', e);
    }
    return const RhythmScopeLayerWritesResult.empty();
  }

  Future<RhythmScopeLayerWritesResult> clearNodeLayer(
    String nodeId, {
    String? axis,
    bool cascade = false,
  }) async {
    try {
      final response = await _dio.post(
        'api/nodes/${Uri.encodeComponent(nodeId)}/clear',
        data: <String, dynamic>{
          if (axis != null) 'axis': axis,
          if (cascade) 'cascade': true,
        },
      );
      return _parseScopeLayerResult(response.data);
    } catch (e) {
      _log.warning('clearNodeLayer failed', e);
    }
    return const RhythmScopeLayerWritesResult.empty();
  }

  Future<RhythmScopeLayerWritesResult> setNodesFreeze(
    List<Map<String, dynamic>> items,
  ) {
    return _putScopeLayerItems('api/nodes/freeze', items, 'setNodesFreeze');
  }

  Future<RhythmScopeLayerWritesResult> freezeNode({
    required String nodeId,
    double? frozenAtHour,
    int? expiresAtEpochMs,
    int? expiresInSecs,
  }) {
    return setNodesFreeze([
      <String, dynamic>{
        'node_id': nodeId,
        if (frozenAtHour != null) 'frozen_at_hour': frozenAtHour,
        if (expiresAtEpochMs != null) 'expires_at_epoch_ms': expiresAtEpochMs,
        if (expiresInSecs != null) 'expires_in_secs': expiresInSecs,
      },
    ]);
  }

  Future<RhythmScopeLayerWritesResult> clearNodeFreeze(String nodeId) {
    return setNodesFreeze([
      <String, dynamic>{'node_id': nodeId, 'clear': true},
    ]);
  }

  Future<RhythmScopeLayerWritesResult> setNodesBoost(
    List<Map<String, dynamic>> items,
  ) {
    return _putScopeLayerItems('api/nodes/boost', items, 'setNodesBoost');
  }

  Future<RhythmScopeLayerWritesResult> boostNode({
    required String nodeId,
    required double brightness,
    int? expiresAtEpochMs,
    int? expiresInSecs,
  }) {
    return setNodesBoost([
      <String, dynamic>{
        'node_id': nodeId,
        'brightness': brightness,
        if (expiresAtEpochMs != null) 'expires_at_epoch_ms': expiresAtEpochMs,
        if (expiresInSecs != null) 'expires_in_secs': expiresInSecs,
      },
    ]);
  }

  Future<RhythmScopeLayerWritesResult> clearNodeBoost(String nodeId) {
    return setNodesBoost([
      <String, dynamic>{'node_id': nodeId, 'clear': true},
    ]);
  }

  Future<RhythmScopeLayerWritesResult> setNodesAutoOff(
    List<Map<String, dynamic>> items,
  ) {
    return _putScopeLayerItems('api/nodes/auto-off', items, 'setNodesAutoOff');
  }

  Future<RhythmScopeLayerWritesResult> armNodeAutoOff({
    required String nodeId,
    int? expiresAtEpochMs,
    int? expiresInSecs,
    String mode = 'replace',
  }) {
    return setNodesAutoOff([
      <String, dynamic>{
        'node_id': nodeId,
        if (expiresAtEpochMs != null) 'expires_at_epoch_ms': expiresAtEpochMs,
        if (expiresInSecs != null) 'expires_in_secs': expiresInSecs,
        'mode': mode,
      },
    ]);
  }

  Future<RhythmScopeLayerWritesResult> clearNodeAutoOff(String nodeId) {
    return setNodesAutoOff([
      <String, dynamic>{'node_id': nodeId, 'clear': true},
    ]);
  }

  Future<RhythmEnvironmentSnapshot?> getOutdoor() async {
    try {
      final response = await _dio.get('api/outdoor');
      final data = jsonMap(response.data);
      return data == null ? null : RhythmEnvironmentSnapshot.fromJson(data);
    } catch (e) {
      _log.warning('getOutdoor failed', e);
    }
    return null;
  }

  Future<RhythmEnvironmentSnapshot?> setOutdoorOverride({
    required RhythmSkyCondition condition,
    int? expiresInSecs,
    int? expiresAtEpochMs,
  }) async {
    try {
      final response = await _dio.put(
        'api/outdoor/override',
        data: <String, dynamic>{
          'condition': condition.wireValue,
          if (expiresInSecs != null) 'expires_in_secs': expiresInSecs,
          if (expiresAtEpochMs != null) 'expires_at_epoch_ms': expiresAtEpochMs,
        },
      );
      final data = jsonMap(response.data);
      return data == null ? null : RhythmEnvironmentSnapshot.fromJson(data);
    } catch (e) {
      _log.warning('setOutdoorOverride failed', e);
    }
    return null;
  }

  Future<RhythmEnvironmentSnapshot?> clearOutdoorOverride() async {
    try {
      final response = await _dio.put(
        'api/outdoor/override',
        data: const <String, dynamic>{'clear': true},
      );
      final data = jsonMap(response.data);
      return data == null ? null : RhythmEnvironmentSnapshot.fromJson(data);
    } catch (e) {
      _log.warning('clearOutdoorOverride failed', e);
    }
    return null;
  }

  Future<RhythmLearnBaselinesResponse?> learnEnvironmentBaselines(
    List<double> luxSamples,
  ) async {
    try {
      final response = await _dio.post(
        'api/environment/learn-baselines',
        data: <String, dynamic>{'lux_samples': luxSamples},
      );
      final data = jsonMap(response.data);
      return data == null ? null : RhythmLearnBaselinesResponse.fromJson(data);
    } catch (e) {
      _log.warning('learnEnvironmentBaselines failed', e);
    }
    return null;
  }

  Future<RhythmPowerSchedules> getPowerSchedules() async {
    try {
      final response = await _dio.get('api/nodes/power-schedule');
      final data = jsonMap(response.data);
      return data == null
          ? const RhythmPowerSchedules.empty()
          : RhythmPowerSchedules.fromJson(data);
    } catch (e) {
      _log.warning('getPowerSchedules failed', e);
    }
    return const RhythmPowerSchedules.empty();
  }

  Future<RhythmPowerSchedules> setPowerSchedules(
    List<Map<String, dynamic>> schedules,
  ) async {
    try {
      final response = await _dio.put(
        'api/nodes/power-schedule',
        data: <String, dynamic>{'schedules': schedules},
      );
      final data = jsonMap(response.data);
      return data == null
          ? const RhythmPowerSchedules.empty()
          : RhythmPowerSchedules.fromJson(data);
    } catch (e) {
      _log.warning('setPowerSchedules failed', e);
    }
    return const RhythmPowerSchedules.empty();
  }

  Future<RhythmInputActionsCatalog> getInputActions() async {
    try {
      final response = await _dio.get('api/input-actions');
      final data = jsonMap(response.data);
      return data == null
          ? const RhythmInputActionsCatalog.empty()
          : RhythmInputActionsCatalog.fromJson(data);
    } catch (e) {
      _log.warning('getInputActions failed', e);
    }
    return const RhythmInputActionsCatalog.empty();
  }

  Future<Map<String, dynamic>?> syncIntegrationDevices() {
    return _postMap(
      'api/integrations/sync-devices',
      logName: 'syncIntegrationDevices',
    );
  }

  Future<Map<String, dynamic>?> syncIntegrationControls() {
    return _postMap(
      'api/integrations/sync-controls',
      logName: 'syncIntegrationControls',
    );
  }

  Future<Map<String, dynamic>?> reportDevices(
    Map<String, dynamic> report,
  ) {
    return _postMap(
      'api/devices/report',
      data: report,
      logName: 'reportDevices',
    );
  }

  Future<RhythmControlPause?> setControlPause(
    String controlId, {
    int? expiresInSecs,
  }) async {
    try {
      final response = await _dio.put(
        'api/controls/${Uri.encodeComponent(controlId)}/pause',
        data: <String, dynamic>{
          if (expiresInSecs != null) 'expires_in_secs': expiresInSecs,
        },
      );
      final data = jsonMap(response.data);
      return data == null ? null : RhythmControlPause.fromJson(data);
    } catch (e) {
      _log.warning('setControlPause failed', e);
    }
    return null;
  }

  Future<RhythmControlPause?> clearControlPause(String controlId) async {
    try {
      final response = await _dio.put(
        'api/controls/${Uri.encodeComponent(controlId)}/pause',
        data: const <String, dynamic>{'clear': true},
      );
      final data = jsonMap(response.data);
      return data == null ? null : RhythmControlPause.fromJson(data);
    } catch (e) {
      _log.warning('clearControlPause failed', e);
    }
    return null;
  }

  Future<Map<String, dynamic>?> getControlHardwareSettings(
    String controlId,
  ) async {
    try {
      final response = await _dio.get(
        'api/controls/${Uri.encodeComponent(controlId)}/hardware-settings',
      );
      return jsonMap(response.data);
    } catch (e) {
      _log.warning('getControlHardwareSettings failed', e);
    }
    return null;
  }

  Future<Map<String, dynamic>?> setControlHardwareSettings(
    String controlId,
    Map<String, dynamic> settings,
  ) async {
    try {
      final response = await _dio.put(
        'api/controls/${Uri.encodeComponent(controlId)}/hardware-settings',
        data: settings,
      );
      return jsonMap(response.data);
    } catch (e) {
      _log.warning('setControlHardwareSettings failed', e);
    }
    return null;
  }

  Future<RhythmHistory> getHistory() async {
    try {
      final response = await _dio.get('api/history');
      final data = jsonMap(response.data);
      return data == null
          ? const RhythmHistory.empty()
          : RhythmHistory.fromJson(data);
    } catch (e) {
      _log.warning('getHistory failed', e);
    }
    return const RhythmHistory.empty();
  }

  Future<RhythmFilterPresetDocument> getFilterPresets() async {
    try {
      final response = await _dio.get('api/filter-presets');
      final data = jsonMap(response.data);
      return data == null
          ? const RhythmFilterPresetDocument.empty()
          : RhythmFilterPresetDocument.fromJson(data);
    } catch (e) {
      _log.warning('getFilterPresets failed', e);
    }
    return const RhythmFilterPresetDocument.empty();
  }

  Future<RhythmFilterPresetDocument> setFilterPresets(
    Map<String, dynamic> document,
  ) async {
    try {
      final response = await _dio.put('api/filter-presets', data: document);
      final data = jsonMap(response.data);
      return data == null
          ? const RhythmFilterPresetDocument.empty()
          : RhythmFilterPresetDocument.fromJson(data);
    } catch (e) {
      _log.warning('setFilterPresets failed', e);
    }
    return const RhythmFilterPresetDocument.empty();
  }

  Future<RhythmScopeLayerWritesResult> _putScopeLayerItems(
    String path,
    List<Map<String, dynamic>> items,
    String logName,
  ) async {
    if (items.isEmpty) return const RhythmScopeLayerWritesResult.empty();
    try {
      final response = await _dio.put(path, data: items);
      return _parseScopeLayerResult(response.data);
    } catch (e) {
      _log.warning('$logName failed', e);
    }
    return const RhythmScopeLayerWritesResult.empty();
  }

  Future<Map<String, dynamic>?> _postMap(
    String path, {
    Object? data,
    required String logName,
  }) async {
    try {
      final response = data == null
          ? await _dio.post(path)
          : await _dio.post(path, data: data);
      return jsonMap(response.data);
    } catch (e) {
      _log.warning('$logName failed', e);
    }
    return null;
  }

  RhythmDispatchResult _parseAndCacheDispatchResponse(Object? responseData) {
    if (responseData is List<dynamic>) {
      return RhythmDispatchResult(
          states: _parseAndCacheStatesList(responseData));
    }
    final data = jsonMap(responseData);
    final states =
        data?['nodes'] as List<dynamic>? ?? data?['rooms'] as List<dynamic>?;
    return RhythmDispatchResult(
      states: states == null ? const [] : _parseAndCacheStatesList(states),
      metadata: data == null
          ? const RhythmDispatchMetadata()
          : RhythmDispatchMetadata.fromJson(data),
    );
  }

  RhythmScopeLayerWritesResult _parseScopeLayerResult(Object? responseData) {
    final data = jsonMap(responseData);
    return data == null
        ? const RhythmScopeLayerWritesResult.empty()
        : RhythmScopeLayerWritesResult.fromJson(data);
  }

  List<RhythmRoomState> _parseAndCacheStatesList(List<dynamic> states) {
    final results = <RhythmRoomState>[];
    for (final stateJson in states.map(jsonMap).nonNulls) {
      final state = RhythmRoomState.fromJson(stateJson);
      if (state.nodeId.isNotEmpty) results.add(state);
    }
    _cacheStates(results);
    return results;
  }

  void _cacheStates(List<RhythmRoomState> states) {
    if (states.isNotEmpty) {
      _onStatesReceived?.call(states);
    }
  }
}
