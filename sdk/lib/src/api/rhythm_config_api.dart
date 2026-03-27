import 'package:dio/dio.dart';

import '../errors/rhythm_exception.dart';
import '../models/rhythm_config_state.dart';
import '../models/rhythm_curve_config.dart';
import '../models/rhythm_curve_data.dart';
import '../models/rhythm_step_sequences.dart';
import '../models/rhythm_time_info.dart';

/// Stateless API client for config/curve endpoints.
///
/// Wraps the 6 endpoints from the RhythmApi interface:
/// - GET /api/config
/// - POST /api/config
/// - GET /api/curve
/// - GET /api/steps
/// - GET /api/time
/// - GET /health
class RhythmConfigApi {
  final Dio _dio;

  RhythmConfigApi({required String baseUrl, Dio? dio})
      : _dio = dio ??
            Dio(BaseOptions(
              baseUrl: baseUrl,
              connectTimeout: const Duration(seconds: 10),
              receiveTimeout: const Duration(seconds: 10),
            ));

  /// Get current configuration state (config + solar + resolved).
  Future<RhythmConfigState> getConfigState() async {
    try {
      final response = await _dio.get('api/config');
      return RhythmConfigState.fromJson(response.data);
    } on DioException catch (e) {
      throw RhythmApiException(
        'Failed to get config state',
        statusCode: e.response?.statusCode,
        cause: e,
      );
    }
  }

  /// Save raw configuration.
  Future<void> saveConfig(RhythmRawConfig config) async {
    try {
      await _dio.post('api/config', data: config.toJson());
    } on DioException catch (e) {
      throw RhythmApiException(
        'Failed to save config',
        statusCode: e.response?.statusCode,
        cause: e,
      );
    }
  }

  /// Get curve data for visualization.
  Future<RhythmCurveData> getCurveData({
    int? month,
    RhythmCurveConfig? overrides,
  }) async {
    try {
      final params = <String, dynamic>{};
      if (month != null) params['month'] = month;
      if (overrides != null) params.addAll(overrides.toQueryParams());

      final response = await _dio.get('api/curve', queryParameters: params);
      return RhythmCurveData.fromJson(response.data);
    } on DioException catch (e) {
      throw RhythmApiException(
        'Failed to get curve data',
        statusCode: e.response?.statusCode,
        cause: e,
      );
    }
  }

  /// Get step sequences for visualization.
  Future<RhythmStepSequences> getStepSequences({
    required double hour,
    required int maxSteps,
    RhythmCurveConfig? overrides,
  }) async {
    try {
      final params = <String, dynamic>{
        'hour': hour,
        'max_steps': maxSteps,
      };
      if (overrides != null) params.addAll(overrides.toQueryParams());

      final response = await _dio.get('api/steps', queryParameters: params);
      return RhythmStepSequences.fromJson(response.data);
    } on DioException catch (e) {
      throw RhythmApiException(
        'Failed to get step sequences',
        statusCode: e.response?.statusCode,
        cause: e,
      );
    }
  }

  /// Get current time and lighting values.
  Future<RhythmTimeInfo> getTime() async {
    try {
      final response = await _dio.get('api/time');
      return RhythmTimeInfo.fromJson(response.data);
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
    } catch (_) {
      return false;
    }
  }
}
