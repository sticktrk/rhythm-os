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

class RhythmFactoryResetResponse {
  const RhythmFactoryResetResponse({
    required this.success,
    this.httpStatus,
    this.error,
  });

  final bool success;
  final int? httpStatus;
  final String? error;
}

/// Outcome of asking the device to submit its debug bundle directly.
class RhythmDebugBundleSubmissionResult {
  const RhythmDebugBundleSubmissionResult({
    this.uploadedFileName,
    this.uploadedSizeBytes,
    this.legacyBundle,
  });

  /// Set when the device uploaded the bundle itself.
  final String? uploadedFileName;
  final int? uploadedSizeBytes;

  /// Set when the firmware predates direct upload and returned the bundle
  /// bytes instead — the caller uploads them the old way.
  final RhythmDebugBundle? legacyBundle;

  bool get uploadedByDevice => uploadedFileName != null;
}

/// Durable acknowledgement for server-owned background bundle collection.
class RhythmDebugBundleQueueResult {
  const RhythmDebugBundleQueueResult({
    required this.submissionId,
    required this.alreadyQueued,
  });

  final String submissionId;
  final bool alreadyQueued;
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

  /// Ask the device to build the debug bundle and upload it straight to
  /// [uploadUrl] (a signed storage upload URL), embedding [appLog] +
  /// [appMetadata] as `app/app.log` / `app/metadata.json`.
  ///
  /// This keeps large bundles out of the app's memory and receive-timeout
  /// window — the transfer that used to time out was device -> app -> cloud.
  /// Firmware that predates direct upload ignores the request body and
  /// replies with the bundle bytes; that surfaces as [legacyBundle] so the
  /// caller can fall back to uploading from the app.
  Future<RhythmDebugBundleSubmissionResult> submitDebugBundle({
    required String uploadUrl,
    String? appLog,
    Map<String, dynamic>? appMetadata,
  }) async {
    try {
      final response = await _dio.post(
        'api/diag/debug-bundle',
        data: {
          'upload_url': uploadUrl,
          if (appLog != null) 'app_log': appLog,
          if (appMetadata != null) 'app_metadata': appMetadata,
        },
        options: Options(
          responseType: ResponseType.bytes,
          receiveTimeout: _debugBundleReceiveTimeout,
          validateStatus: (_) => true,
        ),
      );

      final contentType = response.headers.value('content-type') ?? '';
      if (contentType.contains('application/gzip')) {
        _throwForUnexpectedStatus(
          response,
          message: 'Failed to generate debug bundle',
        );
        return RhythmDebugBundleSubmissionResult(
          legacyBundle: RhythmDebugBundle(
            fileName: _attachmentFileName(
                  response.headers.value('content-disposition'),
                ) ??
                'rhythm-debug-bundle.tar.gz',
            bytes: _asBytes(
              response.data,
              errorMessage: 'Server returned an invalid debug bundle.',
            ),
            contentType: contentType,
          ),
        );
      }

      _throwForUnexpectedStatus(
        response,
        message: 'Device debug bundle upload failed',
      );
      final json = jsonDecode(utf8.decode(_asBytes(
        response.data,
        errorMessage: 'Server returned an invalid upload response.',
      ))) as Map<String, dynamic>;
      if (json['uploaded'] != true) {
        throw RhythmApiException(
          'Device debug bundle upload failed',
          statusCode: response.statusCode,
          serverMessage: json['message'] as String?,
        );
      }
      return RhythmDebugBundleSubmissionResult(
        uploadedFileName: json['file_name'] as String?,
        uploadedSizeBytes: (json['size_bytes'] as num?)?.toInt(),
      );
    } on DioException catch (e) {
      throw _wrapDioException(
        e,
        message: 'Device debug bundle upload failed',
      );
    }
  }

  /// Ask a capable server to durably queue bundle collection and return as
  /// soon as ownership no longer depends on the app process.
  Future<RhythmDebugBundleQueueResult> queueDebugBundle({
    required String submissionId,
    required String uploadUrl,
    required String completionUrl,
    required String completionToken,
    String? appLog,
    Map<String, dynamic>? appMetadata,
  }) async {
    try {
      final response = await _dio.post(
        'api/diag/debug-bundle',
        data: {
          'upload_url': uploadUrl,
          if (appLog != null) 'app_log': appLog,
          if (appMetadata != null) 'app_metadata': appMetadata,
          'async_submission': {
            'submission_id': submissionId,
            'completion_url': completionUrl,
            'completion_token': completionToken,
          },
        },
        options: Options(validateStatus: (_) => true),
      );
      if (response.statusCode != 202) {
        throw RhythmApiException(
          'Failed to queue debug bundle',
          statusCode: response.statusCode,
          serverMessage: _responseBodyText(response.data),
        );
      }
      if (response.data is! Map) {
        throw RhythmApiException(
          'Failed to queue debug bundle',
          statusCode: response.statusCode,
          serverMessage:
              'Server did not return a durable queue acknowledgement.',
        );
      }
      final json = Map<String, dynamic>.from(response.data as Map);
      if (json['queued'] != true || json['submission_id'] != submissionId) {
        throw RhythmApiException(
          'Failed to queue debug bundle',
          statusCode: response.statusCode,
          serverMessage: 'Server returned an invalid queue acknowledgement.',
        );
      }
      return RhythmDebugBundleQueueResult(
        submissionId: submissionId,
        alreadyQueued: json['already_queued'] == true,
      );
    } on DioException catch (error) {
      throw _wrapDioException(
        error,
        message: 'Failed to queue debug bundle',
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
  }) async =>
      (await factoryResetDetailed(
        platformType: platformType,
        platformContext: platformContext,
      ))
          .success;

  /// Detailed factory-reset result, including a server safety-barrier error.
  Future<RhythmFactoryResetResponse> factoryResetDetailed({
    String? platformType,
    String? platformContext,
  }) async {
    try {
      final response = await _dio.post(
        'api/factory-reset',
        options: Options(validateStatus: (_) => true),
      );
      final statusCode = response.statusCode;
      if (statusCode == null || statusCode < 200 || statusCode >= 300) {
        return RhythmFactoryResetResponse(
          success: false,
          httpStatus: statusCode,
          error: _responseBodyText(response.data) ??
              (statusCode == null
                  ? 'Factory reset request failed.'
                  : 'Factory reset request failed with HTTP $statusCode.'),
        );
      }
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
      return RhythmFactoryResetResponse(
        success: true,
        httpStatus: statusCode,
      );
    } on DioException catch (error) {
      _log.warning('factoryReset failed', error);
      return RhythmFactoryResetResponse(
        success: false,
        httpStatus: error.response?.statusCode,
        error: _responseBodyText(error.response?.data) ??
            _networkErrorText(error) ??
            error.message,
      );
    } catch (error) {
      _log.warning('factoryReset failed', error);
      return RhythmFactoryResetResponse(
        success: false,
        error: error.toString(),
      );
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
    if (data is Map) {
      final message = data['error'] ?? data['message'];
      if (message is String && message.trim().isNotEmpty) {
        return message.trim();
      }
      final text = jsonEncode(data);
      return text.isEmpty ? null : text;
    }
    if (data is List) {
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
