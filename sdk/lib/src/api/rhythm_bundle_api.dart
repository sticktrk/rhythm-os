import 'dart:convert';
import 'dart:typed_data';

import 'package:dio/dio.dart';
import 'package:http/http.dart' as http;
import 'package:logging/logging.dart';

import '../api_auth.dart';
import '../errors/rhythm_exception.dart';
import '../rhythm_log_interceptor.dart';

/// Stateless API client for backup and portable profile bundle routes.
class RhythmBundleApi {
  static final _log = Logger('rhythm_sdk.api');

  final Dio _dio;
  final Uri _baseUri;
  final http.Client _httpClient;
  final String? _authToken;

  RhythmBundleApi({
    required String baseUrl,
    Dio? dio,
    http.Client? httpClient,
    String? authToken,
  })  : _baseUri = Uri.parse(_normalizeBaseUrl(baseUrl)),
        _httpClient = httpClient ?? http.Client(),
        _authToken = authToken,
        _dio = dio ??
            Dio(
              BaseOptions(
                baseUrl: _normalizeBaseUrl(baseUrl),
                connectTimeout: const Duration(seconds: 5),
                receiveTimeout: const Duration(seconds: 15),
                headers: bearerAuthHeaders(authToken),
              ),
            ) {
    final headers = bearerAuthHeaders(authToken);
    if (headers != null) {
      _dio.options.headers.addAll(headers);
    }
    _dio.interceptors.add(RhythmLogInterceptor(_log));
  }

  Future<Map<String, dynamic>> getConfigurationBundle() async {
    try {
      final response = await _dio.get(
        'api/profile-bundle',
        options: Options(validateStatus: (_) => true),
      );
      _throwForUnexpectedStatus(
        response,
        message: 'Failed to fetch configuration bundle',
      );
      return _decodeJsonObject(
        response.data,
        errorMessage: 'Server returned an invalid configuration bundle.',
      );
    } on DioException catch (error) {
      throw _wrapDioException(
        error,
        message: 'Failed to fetch configuration bundle',
      );
    }
  }

  Future<Map<String, dynamic>> putConfigurationBundle(
    Map<String, dynamic> bundle,
  ) async {
    try {
      final response = await _dio.put(
        'api/profile-bundle',
        data: bundle,
        options: Options(validateStatus: (_) => true),
      );
      _throwForUnexpectedStatus(
        response,
        message: 'Failed to save configuration bundle',
      );
      return _decodeJsonObject(
        response.data,
        errorMessage: 'Server returned an invalid configuration response.',
      );
    } on DioException catch (error) {
      throw _wrapDioException(
        error,
        message: 'Failed to save configuration bundle',
      );
    }
  }

  Future<Map<String, dynamic>> getBackupBundle({
    bool includeSecrets = false,
  }) async {
    return _decodeJsonObject(
      await fetchBackupJson(includeSecrets: includeSecrets),
      errorMessage: 'Server returned an invalid backup bundle.',
    );
  }

  Future<String> fetchBackupJson({
    bool includeSecrets = false,
  }) async {
    try {
      final response = await _httpClient.get(
        _backupUri(includeSecrets: includeSecrets),
        headers: bearerAuthHeaders(
          _authToken,
          extra: {'Accept': 'application/json'},
        ),
      );
      _throwForUnexpectedHttpStatus(
        response.statusCode,
        bodyBytes: response.bodyBytes,
        message: 'Failed to fetch backup bundle',
      );
      return _asJsonText(
        response.bodyBytes,
        errorMessage: 'Server returned an invalid backup bundle.',
      );
    } catch (error) {
      throw _wrapHttpException(
        error,
        message: 'Failed to fetch backup bundle',
      );
    }
  }

  Future<Map<String, dynamic>> putBackupBundle(
    Map<String, dynamic> bundle,
  ) async {
    return restoreBackupJson(jsonEncode(bundle));
  }

  Future<Map<String, dynamic>> restoreBackupJson(String backupJson) async {
    try {
      final response = await _httpClient.put(
        _backupUri(),
        headers: bearerAuthHeaders(
          _authToken,
          extra: {'Content-Type': 'application/json'},
        ),
        body: backupJson,
      );
      _throwForUnexpectedHttpStatus(
        response.statusCode,
        bodyBytes: response.bodyBytes,
        message: 'Failed to restore backup bundle',
      );
      return _decodeJsonObject(
        response.bodyBytes,
        errorMessage: 'Server returned an invalid backup response.',
      );
    } catch (error) {
      throw _wrapHttpException(
        error,
        message: 'Failed to restore backup bundle',
      );
    }
  }

  Map<String, dynamic> _decodeJsonObject(
    Object? payload, {
    required String errorMessage,
  }) {
    final decoded = switch (payload) {
      Map<String, dynamic> map => map,
      Map map => Map<String, dynamic>.from(map),
      Uint8List bytes => jsonDecode(utf8.decode(bytes, allowMalformed: false)),
      List<int> bytes => jsonDecode(utf8.decode(bytes, allowMalformed: false)),
      String text => jsonDecode(text),
      _ => null,
    };
    if (decoded is Map<String, dynamic>) {
      return Map<String, dynamic>.from(decoded);
    }
    if (decoded is Map) {
      return Map<String, dynamic>.from(decoded);
    }
    throw StateError(errorMessage);
  }

  String _asJsonText(
    Object? payload, {
    required String errorMessage,
  }) {
    if (payload is String && payload.isNotEmpty) {
      return payload;
    }
    if (payload is Uint8List) {
      final text = utf8.decode(payload, allowMalformed: false).trim();
      if (text.isNotEmpty) return text;
    }
    if (payload is List<int>) {
      final text = utf8.decode(payload, allowMalformed: false).trim();
      if (text.isNotEmpty) return text;
    }
    if (payload is Map || payload is List) {
      return jsonEncode(payload);
    }
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

  void _throwForUnexpectedHttpStatus(
    int statusCode, {
    required List<int> bodyBytes,
    required String message,
  }) {
    if (statusCode == 200) return;

    throw RhythmApiException(
      message,
      statusCode: statusCode,
      serverMessage: _responseBodyText(bodyBytes),
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
      cause: error,
    );
  }

  RhythmApiException _wrapHttpException(
    Object error, {
    required String message,
  }) {
    if (error is RhythmApiException) return error;
    if (error is http.ClientException) {
      final detail = error.message.trim();
      return RhythmApiException(
        message,
        serverMessage: detail.isEmpty ? null : detail,
        cause: error,
      );
    }

    final detail = error.toString().trim();
    return RhythmApiException(
      message,
      serverMessage: detail.isEmpty ? null : detail,
      cause: error,
    );
  }

  Uri _backupUri({bool includeSecrets = false}) {
    final uri = _baseUri.resolve('api/backup');
    if (!includeSecrets) return uri;
    return uri.replace(queryParameters: const {'include_secrets': 'true'});
  }

  static String _normalizeBaseUrl(String baseUrl) {
    return baseUrl.endsWith('/') ? baseUrl : '$baseUrl/';
  }

  String? _responseBodyText(Object? payload) {
    if (payload == null) return null;
    if (payload is String) {
      final trimmed = payload.trim();
      return trimmed.isEmpty ? null : trimmed;
    }
    if (payload is Uint8List) {
      final text = utf8.decode(payload, allowMalformed: false).trim();
      return text.isEmpty ? null : text;
    }
    if (payload is List<int>) {
      final text = utf8.decode(payload, allowMalformed: false).trim();
      return text.isEmpty ? null : text;
    }
    if (payload is Map || payload is List) {
      return jsonEncode(payload);
    }
    final text = payload.toString().trim();
    return text.isEmpty ? null : text;
  }

  String? _networkErrorText(DioException error) {
    return switch (error.type) {
      DioExceptionType.connectionTimeout ||
      DioExceptionType.receiveTimeout ||
      DioExceptionType.sendTimeout =>
        'The request timed out.',
      DioExceptionType.connectionError => 'Could not reach the server.',
      DioExceptionType.badCertificate => 'The server certificate was invalid.',
      DioExceptionType.cancel => 'The request was cancelled.',
      _ => error.message?.trim().isEmpty ?? true ? null : error.message!.trim(),
    };
  }
}
