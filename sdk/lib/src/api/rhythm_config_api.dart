import 'package:dio/dio.dart';
import 'package:logging/logging.dart';

import '../api_auth.dart';
import '../errors/rhythm_exception.dart';
import '../json_parsing.dart';
import '../rhythm_log_interceptor.dart';
import '../models/rhythm_config_state.dart';
import '../models/rhythm_curve_config.dart';
import '../models/rhythm_curve_data.dart';
import '../models/rhythm_hello.dart';
import '../models/rhythm_settings.dart';
import '../models/rhythm_step_sequences.dart';
import '../models/rhythm_time_info.dart';

/// Stateless API client for config and preview endpoints.
class RhythmConfigApi {
  static final _log = Logger('rhythm_sdk.api');

  final Dio _dio;

  RhythmConfigApi({required String baseUrl, Dio? dio, String? authToken})
      : _dio = dio ??
            Dio(
              BaseOptions(
                baseUrl: baseUrl,
                connectTimeout: const Duration(seconds: 10),
                receiveTimeout: const Duration(seconds: 10),
                headers: bearerAuthHeaders(authToken),
              ),
            ) {
    final headers = bearerAuthHeaders(authToken);
    if (headers != null) {
      _dio.options.headers.addAll(headers);
    }
    _dio.interceptors.add(RhythmLogInterceptor(_log));
  }

  /// Get the active profile config from server state.
  Future<RhythmConfigState> getConfigState() async {
    try {
      final response = await _dio.get('api/state');
      final data = response.data as Map<String, dynamic>;
      final configJson = normalizeActiveProfile(data);
      final location = jsonMap(data['location']) ?? const <String, dynamic>{};
      return RhythmConfigState(
        config: RhythmRawConfig.fromJson(configJson),
        solar: RhythmSolarContext.defaults(),
        latitude: _toDouble(location['latitude'] ?? location['lat']),
        longitude: _toDouble(
            location['longitude'] ?? location['lon'] ?? location['lng']),
        timezone: location['timezone'] as String? ??
            location['timezone_name'] as String?,
      );
    } on DioException catch (e) {
      throw RhythmApiException(
        'Failed to get config state',
        statusCode: e.response?.statusCode,
        cause: e,
      );
    }
  }

  /// Save the active profile using the app's raw-config shape.
  Future<void> saveConfig(RhythmRawConfig config) async {
    try {
      final activeId = await _getActiveProfileId();
      final current = await _getProfileConfig(activeId);
      final merged = _mergeRawConfig(current, config);
      await _dio.put(
        'api/config',
        queryParameters: {'id': activeId},
        data: merged.toJson(),
      );
    } on DioException catch (e) {
      throw RhythmApiException(
        'Failed to save config',
        statusCode: e.response?.statusCode,
        cause: e,
      );
    }
  }

  /// Get a curve preview for the active profile.
  Future<RhythmCurveData> getCurveData({
    int? month,
    RhythmCurveConfig? overrides,
  }) async {
    try {
      final activeId = await _getActiveProfileId();
      final current = await _getProfileConfig(activeId);
      final target =
          overrides == null ? null : _mergeCurveOverrides(current, overrides);
      final preview = await _requestCurvePreview(
        id: activeId,
        date: _dateForMonth(month),
        maxSteps: (target ?? current).maxDimSteps,
        overrides: target,
      );
      return RhythmCurveData.fromJson(preview);
    } on DioException catch (e) {
      throw RhythmApiException(
        'Failed to get curve data',
        statusCode: e.response?.statusCode,
        cause: e,
      );
    }
  }

  /// Get solar times and twilight data for a date.
  Future<RhythmSolarInfo> getCurveSolar({DateTime? date}) async {
    try {
      final response = await _dio.get(
        'api/curve/solar',
        queryParameters:
            date == null ? null : <String, dynamic>{'date': _formatDate(date)},
      );
      return RhythmSolarInfo.fromJson(response.data as Map<String, dynamic>);
    } on DioException catch (e) {
      throw RhythmApiException(
        'Failed to get curve solar data',
        statusCode: e.response?.statusCode,
        cause: e,
      );
    }
  }

  /// Get dimming steps for the active profile.
  Future<RhythmStepSequences> getStepSequences({
    required double hour,
    required int maxSteps,
    RhythmCurveConfig? overrides,
  }) async {
    try {
      final activeId = await _getActiveProfileId();
      final current = await _getProfileConfig(activeId);
      final target =
          overrides == null ? null : _mergeCurveOverrides(current, overrides);
      final preview = await _requestCurvePreview(
        id: activeId,
        date: DateTime.now(),
        startHour: hour,
        maxSteps: maxSteps,
        overrides: target,
      );
      return RhythmStepSequences.fromJson(preview);
    } on DioException catch (e) {
      throw RhythmApiException(
        'Failed to get step sequences',
        statusCode: e.response?.statusCode,
        cause: e,
      );
    }
  }

  /// Get the current active-profile sample.
  Future<RhythmTimeInfo> getTime() async {
    try {
      final activeId = await _getActiveProfileId();
      final response = await _dio.get(
        'api/curve/now',
        queryParameters: {'id': activeId},
      );
      return RhythmTimeInfo.fromJson(response.data as Map<String, dynamic>);
    } on DioException catch (e) {
      throw RhythmApiException(
        'Failed to get time',
        statusCode: e.response?.statusCode,
        cause: e,
      );
    }
  }

  /// Health check.
  Future<bool> healthCheck() async {
    try {
      final response = await _dio.get('health');
      return response.data['status'] == 'healthy';
    } catch (e) {
      _log.warning('healthCheck failed', e);
      return false;
    }
  }

  /// Discover available Rhythm servers on the network (web only, same-origin).
  Future<List<Map<String, dynamic>>> discover() async {
    try {
      final response = await _dio.get('api/discover');
      final data = response.data;
      if (data is List) {
        return data.cast<Map<String, dynamic>>();
      }
      return [];
    } catch (e) {
      _log.warning('discover failed', e);
      return [];
    }
  }

  /// Fetch full server state (GET /api/state) as a one-off request.
  Future<RhythmHello> getState({bool authoritative = false}) async {
    try {
      final data = await _getStatePayload(authoritative: authoritative);
      return RhythmHello.fromJson(data);
    } on DioException catch (e) {
      throw RhythmApiException(
        'Failed to get state',
        statusCode: e.response?.statusCode,
        cause: e,
      );
    }
  }

  Future<String> _getActiveProfileId() async {
    try {
      final state = await _getStatePayload();
      final activeProfile = jsonMap(state['active_profile']);
      final activeProfileId = activeProfile?['id'] as String?;
      if (activeProfileId != null && activeProfileId.isNotEmpty) {
        return activeProfileId;
      }

      final modeJson = jsonMap(state['mode']);
      final fromState = _activeProfileIdFromModeJson(modeJson);
      if (fromState != null) return fromState;
    } on DioException {
      rethrow;
    } catch (_) {}

    return 'rhythm';
  }

  String? _activeProfileIdFromModeJson(Map<String, dynamic>? json) {
    if (json == null) return null;
    return RhythmModeResource.fromJson(json).activeConfig?.activeProfileId;
  }

  Future<Map<String, dynamic>> _getStatePayload({
    bool authoritative = false,
  }) async {
    final response = await _dio.get(
      'api/state',
      queryParameters: authoritative ? const {'authoritative': 'true'} : null,
    );
    return Map<String, dynamic>.from(response.data as Map<String, dynamic>);
  }

  Future<RhythmCurveConfig> _getProfileConfig(String id) async {
    final response = await _dio.get('api/config', queryParameters: {'id': id});
    return RhythmCurveConfig.fromJson(response.data as Map<String, dynamic>);
  }

  Future<Map<String, dynamic>> _requestCurvePreview({
    required String id,
    required DateTime date,
    double startHour = 12.0,
    int samplesPerHour = 4,
    int? maxSteps,
    RhythmCurveConfig? overrides,
  }) async {
    final query = <String, dynamic>{
      'id': id,
      'date': _formatDate(date),
      'samples_per_hour': samplesPerHour,
      'start_hour': startHour,
      if (maxSteps != null) 'max_steps': maxSteps,
    };
    final response = overrides == null
        ? await _dio.get('api/curve', queryParameters: query)
        : await _dio.post(
            'api/curve',
            queryParameters: query,
            data: overrides.toJson(),
          );
    return response.data as Map<String, dynamic>;
  }

  RhythmCurveConfig _mergeRawConfig(
    RhythmCurveConfig current,
    RhythmRawConfig raw,
  ) {
    final curve = switch (current.curve) {
      RhythmSuperGaussianCurve curve => curve.copyWith(
          widthLeftBri: raw.widthLeftBri,
          widthRightBri: raw.widthRightBri,
          widthLeftCct: raw.widthLeftCct,
          widthRightCct: raw.widthRightCct,
          shapeP: raw.shapeP,
        ),
      _ => current.curve,
    };

    return current.copyWith(
      minColorTemp: raw.minColorTemp,
      maxColorTemp: raw.maxColorTemp,
      minBrightness: raw.minBrightness,
      maxBrightness: raw.maxBrightness,
      maxDimSteps: raw.maxDimSteps,
      fadeSetting: RhythmTimerSetting.fixed(raw.fadeMs),
      motionTimeoutSetting: RhythmTimerSetting.fixed(raw.motionTimeoutSecs),
      curve: curve,
    );
  }

  RhythmCurveConfig _mergeCurveOverrides(
    RhythmCurveConfig current,
    RhythmCurveConfig overrides,
  ) {
    final nextCurve = switch ((current.curve, overrides.curve)) {
      (
        RhythmSuperGaussianCurve currentCurve,
        RhythmSuperGaussianCurve nextCurve
      ) =>
        currentCurve.copyWith(
          widthLeftBri: nextCurve.widthLeftBri,
          widthRightBri: nextCurve.widthRightBri,
          widthLeftCct: nextCurve.widthLeftCct,
          widthRightCct: nextCurve.widthRightCct,
          shapeP: nextCurve.shapeP,
          directColor: nextCurve.directColor,
        ),
      (_, final curve) => curve,
    };

    return current.copyWith(
      minColorTemp: overrides.minColorTemp,
      maxColorTemp: overrides.maxColorTemp,
      minBrightness: overrides.minBrightness,
      maxBrightness: overrides.maxBrightness,
      maxDimSteps: overrides.maxDimSteps,
      fadeSetting: overrides.fadeSetting ?? current.fadeSetting,
      motionTimeoutSetting:
          overrides.motionTimeoutSetting ?? current.motionTimeoutSetting,
      rhythmIntervalSetting:
          overrides.rhythmIntervalSetting ?? current.rhythmIntervalSetting,
      curve: nextCurve,
    );
  }

  DateTime _dateForMonth(int? month) {
    final now = DateTime.now();
    return DateTime(now.year, month ?? now.month, month == null ? now.day : 15);
  }

  String _formatDate(DateTime date) {
    final month = date.month.toString().padLeft(2, '0');
    final day = date.day.toString().padLeft(2, '0');
    return '${date.year}-$month-$day';
  }

  double? _toDouble(dynamic value) {
    return value is num ? value.toDouble() : null;
  }
}
