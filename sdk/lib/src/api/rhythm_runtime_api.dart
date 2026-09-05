import 'package:dio/dio.dart';
import 'package:logging/logging.dart';

import '../api_auth.dart';
import '../json_parsing.dart';
import '../models/rhythm_room.dart';
import '../models/rhythm_hello.dart';
import '../models/rhythm_state_scope.dart';
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

  /// Errors propagate so callers can preserve cache instead of treating failure
  /// as a successful empty response. Legacy servers may return their full state.
  Future<RhythmHello> getState({
    Set<RhythmStateInclude> include = const {RhythmStateInclude.base},
    bool authoritative = false,
  }) async {
    if (include.contains(RhythmStateInclude.controls) &&
        include.contains(RhythmStateInclude.nodes)) {
      throw ArgumentError('controls and nodes are mutually exclusive');
    }
    final response = await _dio.get('api/state', queryParameters: {
      'include':
          include.isEmpty ? 'base' : include.map((v) => v.name).join(','),
      if (authoritative) 'authoritative': 'true',
    });
    final data = jsonMap(response.data);
    if (data == null) throw const FormatException('Invalid state response');
    final hello = RhythmHello.fromJson(data);
    if (hello.stateScope case final scope?) {
      if (!scope.included.containsAll(include)) {
        throw const FormatException(
            'State response omitted requested sections');
      }
    } else if (data['nodes'] is! List && data['rooms'] is! List) {
      throw const FormatException('Incomplete legacy state response');
    }
    return hello;
  }

  Future<RhythmNodesPollResponse> getNodesState(
      {bool controlsOnly = false}) async {
    try {
      final response = controlsOnly
          ? await _dio.get('api/nodes/state',
              queryParameters: const {'scope': 'controls'})
          : await _dio.get('api/nodes/state');
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

  Future<RhythmHistory> getHistory({
    int? limit,
    String? area,
    String? source,
    String? action,
  }) async {
    try {
      final query = <String, dynamic>{
        if (limit != null) 'limit': limit,
        if (area != null && area.isNotEmpty) 'area': area,
        if (source != null && source.isNotEmpty) 'source': source,
        if (action != null && action.isNotEmpty) 'action': action,
      };
      final response = query.isEmpty
          ? await _dio.get('api/history')
          : await _dio.get('api/history', queryParameters: query);
      final data = jsonMap(response.data);
      return data == null
          ? const RhythmHistory.empty()
          : RhythmHistory.fromJson(data);
    } catch (e) {
      _log.warning('getHistory failed', e);
    }
    return const RhythmHistory.empty();
  }

  Future<Map<String, dynamic>?> getActivityCloudConfig() async {
    try {
      final response = await _dio.get('api/activity-cloud/config');
      return jsonMap(response.data);
    } catch (e) {
      _log.warning('getActivityCloudConfig failed', e);
    }
    return null;
  }

  Future<Map<String, dynamic>?> putActivityCloudConfig(
    Map<String, dynamic> config,
  ) async {
    try {
      final response = await _dio.put(
        'api/activity-cloud/config',
        data: config,
      );
      return jsonMap(response.data);
    } catch (e) {
      _log.warning('putActivityCloudConfig failed', e);
    }
    return null;
  }

  Future<Map<String, dynamic>?> clearActivityCloudConfig() async {
    try {
      final response = await _dio.delete('api/activity-cloud/config');
      return jsonMap(response.data);
    } catch (e) {
      _log.warning('clearActivityCloudConfig failed', e);
    }
    return null;
  }

  void _cacheStates(List<RhythmRoomState> states) {
    if (states.isNotEmpty) {
      _onStatesReceived?.call(states);
    }
  }
}
