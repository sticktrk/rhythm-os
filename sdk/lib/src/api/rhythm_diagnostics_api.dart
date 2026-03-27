import 'package:dio/dio.dart';

/// Lightweight HTTP client for device diagnostic endpoints.
///
/// Can be instantiated directly with a host for one-off operations
/// like health checks and diagnostics.
class RhythmDiagnosticsApi {
  final Dio _dio;

  RhythmDiagnosticsApi({required String host, int port = 80})
      : _dio = Dio(BaseOptions(
          baseUrl: 'http://$host:$port/',
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
  Future<List<Map<String, dynamic>>?> getDiagLogs({
    int limit = 50,
    String? category,
  }) async {
    try {
      final params = <String, dynamic>{'limit': limit};
      if (category != null) params['cat'] = category;
      final response =
          await _dio.get('api/diag/logs', queryParameters: params);
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

  /// Reboot the device.
  Future<bool> reboot() async {
    try {
      await _dio.post('api/system/reboot');
      return true;
    } catch (_) {
      return false;
    }
  }
}
