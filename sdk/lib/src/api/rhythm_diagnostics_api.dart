import 'dart:convert';
import 'dart:typed_data';

import 'package:dio/dio.dart';
import 'package:logging/logging.dart';

import '../api_auth.dart';
import '../errors/rhythm_exception.dart';
import '../models/rhythm_debug_bundle.dart';
import '../rhythm_log_interceptor.dart';

class RhythmWifiChangeResponse {
  const RhythmWifiChangeResponse({
    this.httpStatus,
    this.status,
    this.message,
    this.ssid,
    this.error,
  });

  factory RhythmWifiChangeResponse.fromHttp({
    required int? statusCode,
    required Object? data,
  }) {
    if (data is Map) {
      final json = Map<String, dynamic>.from(data);
      return RhythmWifiChangeResponse(
        httpStatus: statusCode,
        status: json['status'] as String?,
        message: (json['message'] ?? json['error']) as String?,
        ssid: json['ssid'] as String?,
        error: statusCode == 200
            ? null
            : (json['error'] ?? json['message']) as String?,
      );
    }

    final text = data?.toString().trim();
    return RhythmWifiChangeResponse(
      httpStatus: statusCode,
      error: text == null || text.isEmpty
          ? (statusCode == null
              ? 'Wi-Fi change request failed.'
              : 'Wi-Fi change request failed with HTTP $statusCode.')
          : text,
    );
  }

  final int? httpStatus;
  final String? status;
  final String? message;
  final String? ssid;
  final String? error;

  bool get accepted => httpStatus == 200 && error == null;
}

/// Lightweight HTTP client for device diagnostic endpoints.
///
/// Can be instantiated directly with a host for one-off operations
/// like health checks and diagnostics.
class RhythmDiagnosticsApi {
  static final _log = Logger('rhythm_sdk.api');
  static const Duration defaultConnectTimeout = Duration(seconds: 5);
  static const Duration defaultReceiveTimeout = Duration(seconds: 5);
  static const Duration defaultDebugBundleReceiveTimeout = Duration(minutes: 2);

  final Dio _dio;
  final Duration _debugBundleReceiveTimeout;

  RhythmDiagnosticsApi({
    required String host,
    int port = 80,
    bool useSsl = false,
    Duration connectTimeout = defaultConnectTimeout,
    Duration receiveTimeout = defaultReceiveTimeout,
    Duration debugBundleReceiveTimeout = defaultDebugBundleReceiveTimeout,
    String? authToken,
  }) : this.fromBaseUrl(
          baseUrl: _endpointBaseUrl(host: host, port: port, useSsl: useSsl),
          connectTimeout: connectTimeout,
          receiveTimeout: receiveTimeout,
          debugBundleReceiveTimeout: debugBundleReceiveTimeout,
          authToken: authToken,
        );

  RhythmDiagnosticsApi.fromBaseUrl({
    required String baseUrl,
    Duration connectTimeout = defaultConnectTimeout,
    Duration receiveTimeout = defaultReceiveTimeout,
    Duration debugBundleReceiveTimeout = defaultDebugBundleReceiveTimeout,
    String? authToken,
  })  : _debugBundleReceiveTimeout = debugBundleReceiveTimeout,
        _dio = Dio(BaseOptions(
          baseUrl: _normalizeBaseUrl(baseUrl),
          connectTimeout: connectTimeout,
          receiveTimeout: receiveTimeout,
          headers: bearerAuthHeaders(authToken),
        )) {
    _dio.interceptors.add(RhythmLogInterceptor(_log));
  }

  /// Check if the device is reachable.
  Future<bool> healthCheck() async {
    try {
      final response = await _dio.get('health');
      return response.data['status'] == 'healthy';
    } catch (e) {
      _log.warning('healthCheck failed', e);
      return false;
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
      final response = await _dio.get('api/diag/logs', queryParameters: params);
      final data = response.data as Map<String, dynamic>;
      final logs = data['logs'] as List<dynamic>? ?? [];
      return logs.cast<Map<String, dynamic>>();
    } catch (e) {
      _log.warning('getDiagLogs failed', e);
      return null;
    }
  }

  /// Build and download a gzip-compressed debug bundle attachment.
  Future<RhythmDebugBundle> downloadDebugBundle() async {
    try {
      final response = await _dio.post(
        'api/diag/debug-bundle',
        options: Options(
          responseType: ResponseType.bytes,
          receiveTimeout: _debugBundleReceiveTimeout,
          validateStatus: (_) => true,
        ),
      );
      _throwForUnexpectedStatus(
        response,
        message: 'Failed to generate debug bundle',
      );

      return RhythmDebugBundle(
        fileName: _attachmentFileName(
              response.headers.value('content-disposition'),
            ) ??
            'rhythm-debug-bundle.tar.gz',
        bytes: _asBytes(
          response.data,
          errorMessage: 'Server returned an invalid debug bundle.',
        ),
        contentType:
            response.headers.value('content-type') ?? 'application/gzip',
      );
    } on DioException catch (e) {
      throw _wrapDioException(
        e,
        message: 'Failed to generate debug bundle',
      );
    }
  }

  /// Reset WiFi credentials, putting the device back in setup mode.
  Future<bool> resetWifi() async {
    try {
      await _dio.delete('api/wifi');
      return true;
    } catch (e) {
      _log.warning('resetWifi failed', e);
      return false;
    }
  }

  /// Schedule a Wi-Fi credential change on an already reachable appliance.
  Future<RhythmWifiChangeResponse> changeWifi({
    required String ssid,
    required String password,
  }) async {
    if (ssid.trim().isEmpty) {
      return const RhythmWifiChangeResponse(error: 'SSID is required.');
    }

    try {
      final response = await _dio.put(
        'api/wifi',
        data: {
          'ssid': ssid,
          'password': password,
        },
        options: Options(validateStatus: (_) => true),
      );
      return RhythmWifiChangeResponse.fromHttp(
        statusCode: response.statusCode,
        data: response.data,
      );
    } on DioException catch (error) {
      return RhythmWifiChangeResponse(
        httpStatus: error.response?.statusCode,
        error: _responseBodyText(error.response?.data) ??
            _networkErrorText(error) ??
            error.message,
      );
    } catch (error) {
      return RhythmWifiChangeResponse(error: error.toString());
    }
  }

  /// Full factory reset via the shared cross-platform endpoint.
  /// Wipes paired hubs and configuration back to defaults on every platform.
  Future<bool> factoryReset({
    String? platformType,
    String? platformContext,
  }) async {
    try {
      await _dio.post('api/factory-reset');
      if (_isRpizPlatform(
        platformType: platformType,
        platformContext: platformContext,
      )) {
        try {
          await _dio.delete('api/wifi');
        } catch (e) {
          // rpiz clears Wi-Fi via a follow-up call and may drop off the
          // network immediately after the reset succeeds.
          _log.warning('factoryReset follow-up wifi reset failed', e);
        }
      }
      return true;
    } catch (e) {
      _log.warning('factoryReset failed', e);
      return false;
    }
  }

  /// Reboot the device.
  Future<bool> reboot() async {
    try {
      await _dio.post('api/restart');
      return true;
    } catch (e) {
      _log.warning('reboot failed', e);
      return false;
    }
  }

  Uint8List _asBytes(
    Object? payload, {
    required String errorMessage,
  }) {
    if (payload is Uint8List) return payload;
    if (payload is List<int>) return Uint8List.fromList(payload);
    if (payload is String) return Uint8List.fromList(utf8.encode(payload));
    throw StateError(errorMessage);
  }

  void _throwForUnexpectedStatus(
    Response response, {
    required String message,
  }) {
    final status = response.statusCode;
    if (status == 200) return;

    throw RhythmApiException(
      message,
      statusCode: status,
      serverMessage: _responseBodyText(response.data),
    );
  }

  RhythmApiException _wrapDioException(
    DioException error, {
    required String message,
  }) {
    return RhythmApiException(
      message,
      statusCode: error.response?.statusCode,
      serverMessage:
          _responseBodyText(error.response?.data) ?? _networkErrorText(error),
    );
  }

  String? _responseBodyText(Object? data) {
    if (data == null) return null;
    if (data is String) {
      final trimmed = data.trim();
      return trimmed.isEmpty ? null : trimmed;
    }
    if (data is Uint8List) {
      final text = utf8.decode(data, allowMalformed: true).trim();
      return text.isEmpty ? null : text;
    }
    if (data is List<int>) {
      final text = utf8.decode(data, allowMalformed: true).trim();
      return text.isEmpty ? null : text;
    }
    if (data is Map || data is List) {
      final text = jsonEncode(data);
      return text.isEmpty ? null : text;
    }
    final text = data.toString().trim();
    return text.isEmpty ? null : text;
  }

  String? _networkErrorText(DioException error) {
    return switch (error.type) {
      DioExceptionType.connectionTimeout ||
      DioExceptionType.receiveTimeout ||
      DioExceptionType.sendTimeout =>
        'The server took too long to respond.',
      DioExceptionType.connectionError => 'Could not reach the server.',
      DioExceptionType.badCertificate => 'The server certificate was invalid.',
      DioExceptionType.cancel => 'The request was cancelled.',
      _ => null,
    };
  }

  String? _attachmentFileName(String? contentDisposition) {
    if (contentDisposition == null || contentDisposition.trim().isEmpty) {
      return null;
    }

    final encodedMatch =
        RegExp(r'''filename\*\s*=\s*(?:UTF-8'')?("?)([^";]+)\1''')
            .firstMatch(contentDisposition);
    if (encodedMatch != null) {
      return Uri.decodeComponent(encodedMatch.group(2)!);
    }

    final plainMatch = RegExp(r'''filename\s*=\s*("?)([^";]+)\1''')
        .firstMatch(contentDisposition);
    return plainMatch?.group(2);
  }

  bool _isRpizPlatform({
    String? platformType,
    String? platformContext,
  }) {
    final normalizedType = platformType?.trim().toLowerCase();
    final normalizedContext = platformContext?.trim().toLowerCase();
    return normalizedType == 'rpiz' || normalizedContext == 'rpiz';
  }
}

String _endpointBaseUrl({
  required String host,
  required int port,
  required bool useSsl,
}) {
  final scheme = useSsl ? 'https' : 'http';
  return '$scheme://$host:$port/';
}

String _normalizeBaseUrl(String baseUrl) {
  final trimmed = baseUrl.trim();
  if (trimmed.isEmpty) {
    throw ArgumentError.value(baseUrl, 'baseUrl', 'must not be empty');
  }
  return trimmed.endsWith('/') ? trimmed : '$trimmed/';
}
