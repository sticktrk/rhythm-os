import 'package:dio/dio.dart';
import 'package:logging/logging.dart';

import '../api_auth.dart';
import '../errors/rhythm_exception.dart';
import '../rhythm_log_interceptor.dart';

class RhythmRemoteAccessStatus {
  const RhythmRemoteAccessStatus({
    required this.enabled,
    required this.configured,
    this.hostname,
    this.tunnelId,
    this.tunnelName,
    required this.updatedAtEpochMs,
    required this.cloudflaredAvailable,
    this.cloudflaredVersion,
    required this.serviceAvailable,
    required this.serviceRunning,
    this.supervisorState,
    this.supervisorPid,
    this.childPid,
    required this.restartCount,
    this.lastStartedEpochSecs,
    this.lastExitEpochSecs,
    this.lastExitCode,
    this.nextRestartEpochSecs,
    this.metricsAddr,
    required this.metricsAvailable,
    required this.connectorHealthy,
    this.registeredConnections,
    this.metricsError,
    this.serviceError,
  });

  factory RhythmRemoteAccessStatus.fromJson(Map<String, dynamic> json) {
    return RhythmRemoteAccessStatus(
      enabled: json['enabled'] == true,
      configured: json['configured'] == true,
      hostname: json['hostname']?.toString(),
      tunnelId: json['tunnel_id']?.toString(),
      tunnelName: json['tunnel_name']?.toString(),
      updatedAtEpochMs: (json['updated_at_epoch_ms'] as num?)?.toInt() ?? 0,
      cloudflaredAvailable: json['cloudflared_available'] == true,
      cloudflaredVersion: json['cloudflared_version']?.toString(),
      serviceAvailable: json['service_available'] == true,
      serviceRunning: json['service_running'] == true,
      supervisorState: json['supervisor_state']?.toString(),
      supervisorPid: (json['supervisor_pid'] as num?)?.toInt(),
      childPid: (json['child_pid'] as num?)?.toInt(),
      restartCount: (json['restart_count'] as num?)?.toInt() ?? 0,
      lastStartedEpochSecs: (json['last_started_epoch_secs'] as num?)?.toInt(),
      lastExitEpochSecs: (json['last_exit_epoch_secs'] as num?)?.toInt(),
      lastExitCode: (json['last_exit_code'] as num?)?.toInt(),
      nextRestartEpochSecs: (json['next_restart_epoch_secs'] as num?)?.toInt(),
      metricsAddr: json['metrics_addr']?.toString(),
      metricsAvailable: json['metrics_available'] == true,
      connectorHealthy: json['connector_healthy'] == true,
      registeredConnections: (json['registered_connections'] as num?)?.toInt(),
      metricsError: json['metrics_error']?.toString(),
      serviceError: json['service_error']?.toString(),
    );
  }

  final bool enabled;
  final bool configured;
  final String? hostname;
  final String? tunnelId;
  final String? tunnelName;
  final int updatedAtEpochMs;
  final bool cloudflaredAvailable;
  final String? cloudflaredVersion;
  final bool serviceAvailable;
  final bool serviceRunning;
  final String? supervisorState;
  final int? supervisorPid;
  final int? childPid;
  final int restartCount;
  final int? lastStartedEpochSecs;
  final int? lastExitEpochSecs;
  final int? lastExitCode;
  final int? nextRestartEpochSecs;
  final String? metricsAddr;
  final bool metricsAvailable;
  final bool connectorHealthy;
  final int? registeredConnections;
  final String? metricsError;
  final String? serviceError;
}

class RhythmRemoteAccessApi {
  static final _log = Logger('rhythm_sdk.api');

  RhythmRemoteAccessApi({
    required String baseUrl,
    Dio? dio,
    String? authToken,
  }) : _dio = dio ??
            Dio(
              BaseOptions(
                baseUrl: _normalizeBaseUrl(baseUrl),
                connectTimeout: const Duration(seconds: 5),
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

  final Dio _dio;

  Future<RhythmRemoteAccessStatus> getStatus() async {
    try {
      final response =
          await _dio.get<Map<String, dynamic>>('api/remote-access/status');
      return _decodeStatus(response.data);
    } on DioException catch (error) {
      throw RhythmApiException(
        'Failed to fetch remote access status',
        statusCode: error.response?.statusCode,
        cause: error,
      );
    }
  }

  Future<RhythmRemoteAccessStatus> putConfig({
    required String hostname,
    required String connectorToken,
    bool enabled = true,
    String? tunnelId,
    String? tunnelName,
  }) async {
    try {
      final response = await _dio.put<Map<String, dynamic>>(
        'api/remote-access/config',
        data: {
          'enabled': enabled,
          'hostname': hostname,
          'connector_token': connectorToken,
          if (tunnelId != null) 'tunnel_id': tunnelId,
          if (tunnelName != null) 'tunnel_name': tunnelName,
        },
      );
      return _decodeStatus(response.data);
    } on DioException catch (error) {
      throw RhythmApiException(
        'Failed to configure remote access',
        statusCode: error.response?.statusCode,
        cause: error,
      );
    }
  }

  Future<RhythmRemoteAccessStatus> clearConfig() async {
    try {
      final response =
          await _dio.delete<Map<String, dynamic>>('api/remote-access/config');
      return _decodeStatus(response.data);
    } on DioException catch (error) {
      throw RhythmApiException(
        'Failed to clear remote access config',
        statusCode: error.response?.statusCode,
        cause: error,
      );
    }
  }

  RhythmRemoteAccessStatus _decodeStatus(Map<String, dynamic>? data) {
    if (data == null) {
      throw StateError('Server returned an empty remote access response.');
    }
    return RhythmRemoteAccessStatus.fromJson(Map<String, dynamic>.from(data));
  }
}

String _normalizeBaseUrl(String baseUrl) {
  final trimmed = baseUrl.trim();
  if (trimmed.endsWith('/')) return trimmed;
  return '$trimmed/';
}
