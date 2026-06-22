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
  final bool _useDioBackupTransport;

  RhythmBundleApi({
    required String baseUrl,
    Dio? dio,
    http.Client? httpClient,
    String? authToken,
  })  : _baseUri = Uri.parse(_normalizeBaseUrl(baseUrl)),
        _httpClient = httpClient ?? http.Client(),
        _authToken = authToken,
        _useDioBackupTransport = dio != null,
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
    return _getBundleJson(
      'api/profile-bundle',
      message: 'Failed to fetch configuration bundle',
      errorMessage: 'Server returned an invalid configuration bundle.',
    );
  }

  Future<Map<String, dynamic>> putConfigurationBundle(
    Map<String, dynamic> bundle,
  ) async {
    return _putBundleJson(
      'api/profile-bundle',
      bundle,
      message: 'Failed to save configuration bundle',
      errorMessage: 'Server returned an invalid configuration response.',
    );
  }

  Future<Map<String, dynamic>> getFactoryDefaultConfigurationBundle() {
    return _getBundleJson(
      'api/profile-bundle/factory-default',
      message: 'Failed to fetch factory-default configuration bundle',
      errorMessage:
          'Server returned an invalid factory-default configuration bundle.',
    );
  }

  Future<Map<String, dynamic>> resetConfigurationBundle() {
    return _postBundleJson(
      'api/profile-bundle/reset',
      message: 'Failed to reset configuration bundle',
      errorMessage: 'Server returned an invalid configuration reset response.',
    );
  }

  Future<Map<String, dynamic>> getShareBundle() {
    return _getBundleJson(
      'api/share-bundle',
      message: 'Failed to fetch share bundle',
      errorMessage: 'Server returned an invalid share bundle.',
    );
  }

  Future<Map<String, dynamic>> putShareBundle(
    Map<String, dynamic> bundle,
  ) {
    return _putBundleJson(
      'api/share-bundle',
      bundle,
      message: 'Failed to save share bundle',
      errorMessage: 'Server returned an invalid share bundle response.',
    );
  }

  Future<Map<String, dynamic>> getFactoryDefaultShareBundle() {
    return _getBundleJson(
      'api/share-bundle/factory-default',
      message: 'Failed to fetch factory-default share bundle',
      errorMessage: 'Server returned an invalid factory-default share bundle.',
    );
  }

  Future<Map<String, dynamic>> resetShareBundle() {
    return _postBundleJson(
      'api/share-bundle/reset',
      message: 'Failed to reset share bundle',
      errorMessage: 'Server returned an invalid share reset response.',
    );
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
    if (_useDioBackupTransport) {
      return _fetchBackupJsonWithDio(includeSecrets: includeSecrets);
    }

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
    if (_useDioBackupTransport) {
      return _restoreBackupJsonWithDio(backupJson);
    }

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

  Future<String> _fetchBackupJsonWithDio({
    bool includeSecrets = false,
  }) async {
    try {
      final response = await _dio.get(
        'api/backup',
        queryParameters: includeSecrets
            ? const <String, dynamic>{'include_secrets': 'true'}
            : null,
        options: Options(
          responseType: ResponseType.plain,
          validateStatus: (_) => true,
          headers: const {'Accept': 'application/json'},
        ),
      );
      _throwForUnexpectedStatus(
        response,
        message: 'Failed to fetch backup bundle',
      );
      return _asJsonText(
        response.data,
        errorMessage: 'Server returned an invalid backup bundle.',
      );
    } on DioException catch (error) {
      throw _wrapDioException(
        error,
        message: 'Failed to fetch backup bundle',
      );
    } catch (error) {
      throw _wrapGenericException(
        error,
        message: 'Failed to fetch backup bundle',
      );
    }
  }

  Future<Map<String, dynamic>> _restoreBackupJsonWithDio(
    String backupJson,
  ) async {
    try {
      final response = await _dio.put(
        'api/backup',
        data: jsonDecode(backupJson),
        options: Options(
          validateStatus: (_) => true,
          headers: const {'Content-Type': 'application/json'},
        ),
      );
      _throwForUnexpectedStatus(
        response,
        message: 'Failed to restore backup bundle',
      );
      return _decodeJsonObject(
        response.data,
        errorMessage: 'Server returned an invalid backup response.',
      );
    } on DioException catch (error) {
      throw _wrapDioException(
        error,
        message: 'Failed to restore backup bundle',
      );
    } catch (error) {
      throw _wrapGenericException(
        error,
        message: 'Failed to restore backup bundle',
      );
    }
  }

  Future<Map<String, dynamic>> _getBundleJson(
    String path, {
    required String message,
    required String errorMessage,
  }) async {
    try {
      final response = await _dio.get(
        path,
        options: Options(validateStatus: (_) => true),
      );
      _throwForUnexpectedStatus(response, message: message);
      return _decodeJsonObject(response.data, errorMessage: errorMessage);
    } on DioException catch (error) {
      throw _wrapDioException(error, message: message);
    }
  }

  Future<Map<String, dynamic>> _putBundleJson(
    String path,
    Map<String, dynamic> bundle, {
    required String message,
    required String errorMessage,
  }) async {
    try {
      final response = await _dio.put(
        path,
        data: bundle,
        options: Options(validateStatus: (_) => true),
      );
      _throwForUnexpectedStatus(response, message: message);
      return _decodeJsonObject(response.data, errorMessage: errorMessage);
    } on DioException catch (error) {
      throw _wrapDioException(error, message: message);
    }
  }

  Future<Map<String, dynamic>> _postBundleJson(
    String path, {
    required String message,
    required String errorMessage,
  }) async {
    try {
      final response = await _dio.post(
        path,
        options: Options(validateStatus: (_) => true),
      );
      _throwForUnexpectedStatus(response, message: message);
      return _decodeJsonObject(response.data, errorMessage: errorMessage);
    } on DioException catch (error) {
      throw _wrapDioException(error, message: message);
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

  RhythmApiException _wrapGenericException(
    Object error, {
    required String message,
  }) {
    if (error is RhythmApiException) return error;
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
