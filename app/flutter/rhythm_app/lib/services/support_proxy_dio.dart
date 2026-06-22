import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:dio/dio.dart';

import '../backend/auth/supabase_auth_backend.dart';
import '../backend/backend_provider.dart';
import '../config/supabase_config.dart';
import 'employee_mode_service.dart';

class SupportProxyDioFactory {
  SupportProxyDioFactory({
    required EmployeeModeService employeeMode,
  }) : _employeeMode = employeeMode;

  final EmployeeModeService _employeeMode;

  Dio call({
    required String baseUrl,
    required Duration connectTimeout,
    required Duration receiveTimeout,
    String? authToken,
    Map<String, dynamic>? headers,
  }) {
    final dio = Dio(BaseOptions(
      baseUrl: _normalizeBaseUrl(baseUrl),
      connectTimeout: connectTimeout,
      receiveTimeout: receiveTimeout,
      headers: headers,
    ));
    dio.interceptors.add(SupportProxyInterceptor(employeeMode: _employeeMode));
    return dio;
  }

  Dio configDio({
    Duration connectTimeout = const Duration(seconds: 10),
    Duration receiveTimeout = const Duration(seconds: 10),
  }) {
    return call(
      baseUrl: 'https://support-proxy.invalid/',
      connectTimeout: connectTimeout,
      receiveTimeout: receiveTimeout,
    );
  }
}

class SupportProxyInterceptor extends Interceptor {
  SupportProxyInterceptor({
    required EmployeeModeService employeeMode,
    Dio? edgeDio,
  })  : _employeeMode = employeeMode,
        _edgeDio = edgeDio ??
            Dio(BaseOptions(
              connectTimeout: const Duration(seconds: 10),
              receiveTimeout: Duration.zero,
            ));

  final EmployeeModeService _employeeMode;
  final Dio _edgeDio;

  @override
  void onRequest(
    RequestOptions options,
    RequestInterceptorHandler handler,
  ) {
    unawaited(_proxy(options, handler));
  }

  Future<void> _proxy(
    RequestOptions options,
    RequestInterceptorHandler handler,
  ) async {
    final session = _employeeMode.session;
    if (session == null) {
      handler.reject(_stateError(
        options,
        'Employee mode is not active.',
      ));
      return;
    }

    final accessToken = _accessToken();
    if (accessToken == null) {
      handler.reject(_stateError(
        options,
        'Employee mode requires a staff Supabase session.',
      ));
      return;
    }

    final proxyBody = _buildProxyBody(options, session.grantId);
    if (proxyBody == null) {
      handler.reject(_stateError(
        options,
        'Employee mode only supports JSON request bodies.',
      ));
      return;
    }

    try {
      final response = await _edgeDio.postUri<dynamic>(
        _supportProxyUri(),
        data: proxyBody,
        options: Options(
          responseType: options.responseType,
          sendTimeout: options.sendTimeout,
          receiveTimeout: options.receiveTimeout,
          validateStatus: options.validateStatus,
          headers: {
            'apikey': SupabaseConfig.anonKey,
            'Authorization': 'Bearer $accessToken',
            'Content-Type': Headers.jsonContentType,
          },
        ),
        cancelToken: options.cancelToken,
      );
      await _exitEmployeeModeIfAccessClosed(response);
      handler.resolve(_copyResponse(response, options));
    } on DioException catch (error) {
      final response = error.response;
      if (response != null) {
        await _exitEmployeeModeIfAccessClosed(response);
      }
      handler.reject(_copyDioException(error, options));
    } catch (error) {
      handler.reject(_stateError(options, error.toString()));
    }
  }

  Map<String, dynamic>? _buildProxyBody(
    RequestOptions options,
    String grantId,
  ) {
    final body = options.data;
    if (body is FormData || body is Stream<List<int>> || body is List<int>) {
      return null;
    }
    final headers = _forwardHeaders(options);
    final query = _requestQuery(options);

    return {
      'grant_id': grantId,
      'method': options.method.toUpperCase(),
      'path': _requestPath(options),
      if (headers.isNotEmpty) 'headers': headers,
      if (query.isNotEmpty) 'query': query,
      if (body != null) 'body': body,
    };
  }

  Map<String, dynamic> _requestQuery(RequestOptions options) {
    final query = <String, dynamic>{};
    for (final entry in options.uri.queryParametersAll.entries) {
      query[entry.key] =
          entry.value.length == 1 ? entry.value.single : entry.value;
    }
    if (query.isNotEmpty) return query;
    return Map<String, dynamic>.from(options.queryParameters);
  }

  Map<String, String> _forwardHeaders(RequestOptions options) {
    final headers = <String, String>{};
    for (final entry in options.headers.entries) {
      final name = entry.key.trim().toLowerCase();
      if (!_canForwardHeader(name)) continue;
      final value = entry.value?.toString().trim();
      if (value == null || value.isEmpty) continue;
      headers[name] = value;
    }
    return headers;
  }

  bool _canForwardHeader(String name) {
    return name == 'accept' ||
        name == 'content-type' ||
        name == 'cache-control';
  }

  String _requestPath(RequestOptions options) {
    final path = options.uri.path.isEmpty ? '/' : options.uri.path;
    return path.startsWith('/') ? path : '/$path';
  }

  String? _accessToken() {
    if (!BackendProvider.isInitialized) return null;
    final auth = BackendProvider.instance.auth;
    if (auth is! SupabaseAuthBackend) return null;
    final token = auth.client.auth.currentSession?.accessToken.trim();
    return token == null || token.isEmpty ? null : token;
  }

  DioException _stateError(RequestOptions options, String message) {
    return DioException(
      requestOptions: options,
      type: DioExceptionType.unknown,
      error: StateError(message),
      message: message,
    );
  }

  DioException _copyDioException(
    DioException error,
    RequestOptions originalOptions,
  ) {
    final response = error.response;
    return DioException(
      requestOptions: originalOptions,
      response:
          response == null ? null : _copyResponse(response, originalOptions),
      type: error.type,
      error: error.error,
      message: error.message,
      stackTrace: error.stackTrace,
    );
  }

  Future<void> _exitEmployeeModeIfAccessClosed(
    Response<dynamic> response,
  ) async {
    if (response.statusCode != 403 &&
        response.statusCode != 404 &&
        response.statusCode != 409) {
      return;
    }
    final message = await _responseErrorMessage(response.data);
    if (message == null) return;
    if (message == 'Support access grant expired' ||
        message == 'Support access grant is not active' ||
        message == 'Support access grant not found' ||
        message == 'Staff access required' ||
        message == 'Home not found' ||
        message == 'Support access consent is required' ||
        message == 'Active managed subscription required' ||
        message == 'Support token not configured') {
      _employeeMode.exit();
    }
  }

  Future<String?> _responseErrorMessage(Object? data) async {
    if (data is Map) {
      return _errorMessageFromMap(data);
    }
    if (data is String) {
      return _errorMessageFromText(data);
    }
    if (data is Uint8List) {
      return _errorMessageFromBytes(data);
    }
    if (data is List<int>) {
      return _errorMessageFromBytes(data);
    }
    if (data is ResponseBody) {
      try {
        final bytes = <int>[];
        await for (final chunk in data.stream) {
          bytes.addAll(chunk);
          if (bytes.length > 8192) break;
        }
        return _errorMessageFromBytes(bytes);
      } catch (_) {
        return null;
      }
    }
    return null;
  }

  String? _errorMessageFromBytes(List<int> bytes) {
    if (bytes.isEmpty) return null;
    return _errorMessageFromText(utf8.decode(bytes, allowMalformed: true));
  }

  String? _errorMessageFromText(String text) {
    final trimmed = text.trim();
    if (trimmed.isEmpty) return null;
    try {
      final decoded = jsonDecode(trimmed);
      if (decoded is Map) return _errorMessageFromMap(decoded);
    } catch (_) {
      return null;
    }
    return null;
  }

  String? _errorMessageFromMap(Map<dynamic, dynamic> data) {
    final error = data['error']?.toString().trim();
    return error == null || error.isEmpty ? null : error;
  }

  Response<T> _copyResponse<T>(
    Response<T> response,
    RequestOptions originalOptions,
  ) {
    return Response<T>(
      requestOptions: originalOptions,
      data: response.data,
      statusCode: response.statusCode,
      statusMessage: response.statusMessage,
      headers: response.headers,
      isRedirect: response.isRedirect,
      redirects: response.redirects,
      extra: response.extra,
    );
  }

  Uri _supportProxyUri() {
    final base = SupabaseConfig.url.endsWith('/')
        ? SupabaseConfig.url.substring(0, SupabaseConfig.url.length - 1)
        : SupabaseConfig.url;
    return Uri.parse('$base/functions/v1/support-proxy');
  }

  String _normalizeBaseUrl(String baseUrl) {
    final trimmed = baseUrl.trim();
    return trimmed.endsWith('/') ? trimmed : '$trimmed/';
  }
}
