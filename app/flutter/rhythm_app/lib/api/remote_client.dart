import 'package:dio/dio.dart';
import 'package:rhythm_core/rhythm_core.dart';

/// API client for any Rhythm server backend (addon, rhythm-server, ESP32).
class RemoteApiClient implements RhythmApi {
  final Dio _dio;
  final String baseUrl;

  RemoteApiClient({required this.baseUrl})
      : _dio = Dio(BaseOptions(
          baseUrl: baseUrl,
          connectTimeout: const Duration(seconds: 10),
          receiveTimeout: const Duration(seconds: 10),
        ));

  @override
  Future<ConfigState> getConfigState() async {
    final response = await _dio.get('api/config');
    return ConfigState.fromJson(response.data);
  }

  @override
  Future<void> saveConfig(RawConfig config) async {
    await _dio.post('api/config', data: config.toJson());
  }

  @override
  Future<CurveData> getCurveData(
      {int? month, CurveConfigDto? overrides}) async {
    final params = <String, dynamic>{};
    if (month != null) params['month'] = month;
    if (overrides != null) {
      params.addAll(overrides.toQueryParams());
    }

    final response = await _dio.get('api/curve', queryParameters: params);
    return CurveData.fromJson(response.data);
  }

  @override
  Future<StepSequences> getStepSequences({
    required double hour,
    required int maxSteps,
    CurveConfigDto? overrides,
  }) async {
    final params = <String, dynamic>{
      'hour': hour,
      'max_steps': maxSteps,
    };
    if (overrides != null) {
      params.addAll(overrides.toQueryParams());
    }

    final response = await _dio.get('api/steps', queryParameters: params);
    return StepSequences.fromJson(response.data);
  }

  @override
  Future<TimeInfo> getTime() async {
    final response = await _dio.get('api/time');
    return TimeInfo.fromJson(response.data);
  }

  @override
  Future<bool> healthCheck() async {
    try {
      final response = await _dio.get('health');
      return response.data['status'] == 'healthy';
    } catch (_) {
      return false;
    }
  }
}
