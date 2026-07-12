import 'dart:convert';

import 'package:http/http.dart' as http;
import 'package:http_parser/http_parser.dart';

import 'config.dart';
import 'models.dart';

class SupabaseRestClient {
  SupabaseRestClient({
    required this.config,
    http.Client? httpClient,
  }) : _http = httpClient ?? http.Client();

  final AdminApiConfig config;
  final http.Client _http;

  bool get canUseServiceRole => config.hasServiceRoleKey;

  Future<AdminUser> fetchUser(String accessToken) async {
    final response = await _http.get(
      _supabaseUri('auth/v1/user'),
      headers: {
        'apikey': config.supabaseAnonKey,
        'Authorization': 'Bearer $accessToken',
        'Accept': 'application/json',
      },
    );
    if (response.statusCode < 200 || response.statusCode >= 300) {
      throw AdminApiException(401, 'Invalid or expired Supabase session.');
    }
    final decoded = jsonDecode(response.body);
    if (decoded is! Map) {
      throw const AdminApiException(401, 'Invalid Supabase user response.');
    }
    final user = AdminUser.fromJson(
      decoded.map((key, value) => MapEntry(key.toString(), value)),
    );
    if (user.id.isEmpty) {
      throw const AdminApiException(401, 'Supabase session has no user id.');
    }
    return user;
  }

  Future<List<Map<String, dynamic>>> select({
    required String table,
    required String select,
    String? accessToken,
    bool serviceRole = false,
    Map<String, String> filters = const {},
    String? order,
    int? limit,
  }) async {
    final key =
        serviceRole ? config.supabaseServiceRoleKey : config.supabaseAnonKey;
    if (key == null || key.isEmpty) {
      throw const AdminApiException(
        500,
        'Supabase service role key is not configured.',
      );
    }
    final bearer = serviceRole ? key : accessToken;
    if (bearer == null || bearer.isEmpty) {
      throw const AdminApiException(401, 'Missing Supabase access token.');
    }

    final query = <String, String>{
      'select': select,
      ...filters,
      if (order != null) 'order': order,
      if (limit != null) 'limit': '$limit',
    };
    final response = await _http.get(
      _supabaseUri('rest/v1/$table', query),
      headers: {
        'apikey': key,
        'Authorization': 'Bearer $bearer',
        'Accept': 'application/json',
      },
    );

    if (response.statusCode < 200 || response.statusCode >= 300) {
      throw AdminApiException(
        response.statusCode,
        _supabaseError(response),
      );
    }

    final decoded = jsonDecode(response.body);
    if (decoded is! List) {
      throw const AdminApiException(
          502, 'Supabase returned a non-list result.');
    }
    return decoded
        .whereType<Map>()
        .map((row) => row.map((key, value) => MapEntry(key.toString(), value)))
        .toList(growable: false);
  }

  Future<Map<String, dynamic>> insert({
    required String table,
    required Map<String, dynamic> values,
    bool serviceRole = false,
    String? accessToken,
  }) async {
    final credentials = _credentials(
      serviceRole: serviceRole,
      accessToken: accessToken,
    );
    final response = await _http.post(
      _supabaseUri('rest/v1/$table'),
      headers: {
        ...credentials.headers,
        'Accept': 'application/json',
        'Content-Type': 'application/json',
        'Prefer': 'return=representation',
      },
      body: jsonEncode(values),
    );
    if (response.statusCode < 200 || response.statusCode >= 300) {
      throw AdminApiException(response.statusCode, _supabaseError(response));
    }
    final decoded = jsonDecode(response.body);
    if (decoded is! List || decoded.isEmpty || decoded.first is! Map) {
      throw const AdminApiException(
        502,
        'Supabase returned an invalid insert result.',
      );
    }
    return (decoded.first as Map)
        .map((key, value) => MapEntry(key.toString(), value));
  }

  Future<void> uploadStorageObject({
    required String bucket,
    required String path,
    required List<int> bytes,
    required String contentType,
  }) async {
    final credentials = _credentials(serviceRole: true);
    final request = http.MultipartRequest(
      'POST',
      _supabaseUri('storage/v1/object/$bucket/$path'),
    )
      ..headers.addAll(credentials.headers)
      ..headers['x-upsert'] = 'false'
      ..fields['cacheControl'] = '3600'
      ..files.add(
        http.MultipartFile.fromBytes(
          '',
          bytes,
          filename: path.split('/').last,
          contentType: MediaType.parse(contentType),
        ),
      );
    final streamed = await _http.send(request);
    final response = await http.Response.fromStream(streamed);
    if (response.statusCode < 200 || response.statusCode >= 300) {
      throw AdminApiException(response.statusCode, _supabaseError(response));
    }
  }

  Future<String> createSignedStorageUploadUrl({
    required String bucket,
    required String path,
  }) async {
    final credentials = _credentials(serviceRole: true);
    final response = await _http.post(
      _supabaseUri('storage/v1/object/upload/sign/$bucket/$path'),
      headers: {
        ...credentials.headers,
        'Accept': 'application/json',
        'Content-Type': 'application/json',
      },
      body: '{}',
    );
    if (response.statusCode < 200 || response.statusCode >= 300) {
      throw AdminApiException(response.statusCode, _supabaseError(response));
    }
    final decoded = jsonDecode(response.body);
    final signedPath = decoded is Map ? decoded['url'] as String? : null;
    if (signedPath == null || signedPath.trim().isEmpty) {
      throw const AdminApiException(
        502,
        'Supabase returned an invalid signed upload URL.',
      );
    }
    final signedUri = Uri.parse(signedPath);
    if (signedUri.hasScheme) return signedUri.toString();
    if (signedPath.startsWith('/storage/v1/')) {
      return config.supabaseUrl.resolve(signedPath).toString();
    }
    final storageBase = _supabaseUri('storage/v1').toString();
    return '$storageBase${signedPath.startsWith('/') ? '' : '/'}$signedPath';
  }

  Future<List<int>> downloadStorageObject({
    required String bucket,
    required String path,
  }) async {
    final credentials = _credentials(serviceRole: true);
    final response = await _http.get(
      _supabaseUri('storage/v1/object/$bucket/$path'),
      headers: credentials.headers,
    );
    if (response.statusCode < 200 || response.statusCode >= 300) {
      throw AdminApiException(response.statusCode, _supabaseError(response));
    }
    return response.bodyBytes;
  }

  Future<void> deleteStorageObject({
    required String bucket,
    required String path,
  }) async {
    final credentials = _credentials(serviceRole: true);
    final request = http.Request(
      'DELETE',
      _supabaseUri('storage/v1/object/$bucket'),
    )
      ..headers.addAll({
        ...credentials.headers,
        'Accept': 'application/json',
        'Content-Type': 'application/json',
      })
      ..body = jsonEncode({
        'prefixes': [path],
      });
    final streamed = await _http.send(request);
    final response = await http.Response.fromStream(streamed);
    if (response.statusCode < 200 || response.statusCode >= 300) {
      throw AdminApiException(response.statusCode, _supabaseError(response));
    }
  }

  Future<Map<String, dynamic>> invokeFunction({
    required String name,
    required String accessToken,
    required Map<String, dynamic> body,
  }) async {
    final response = await _http.post(
      _supabaseUri('functions/v1/$name'),
      headers: {
        'apikey': config.supabaseAnonKey,
        'Authorization': 'Bearer $accessToken',
        'Accept': 'application/json',
        'Content-Type': 'application/json',
      },
      body: jsonEncode(body),
    );
    if (response.statusCode < 200 || response.statusCode >= 300) {
      throw AdminApiException(response.statusCode, _supabaseError(response));
    }
    final decoded = jsonDecode(response.body);
    if (decoded is! Map) {
      throw const AdminApiException(
        502,
        'Supabase function returned an invalid result.',
      );
    }
    return decoded.map((key, value) => MapEntry(key.toString(), value));
  }

  ({String key, String bearer, Map<String, String> headers}) _credentials({
    required bool serviceRole,
    String? accessToken,
  }) {
    final key =
        serviceRole ? config.supabaseServiceRoleKey : config.supabaseAnonKey;
    if (key == null || key.isEmpty) {
      throw const AdminApiException(
        500,
        'Supabase service role key is not configured.',
      );
    }
    final bearer = serviceRole ? key : accessToken;
    if (bearer == null || bearer.isEmpty) {
      throw const AdminApiException(401, 'Missing Supabase access token.');
    }
    return (
      key: key,
      bearer: bearer,
      headers: {
        'apikey': key,
        'Authorization': 'Bearer $bearer',
      },
    );
  }

  Uri _supabaseUri(String path, [Map<String, String>? query]) {
    final basePath = config.supabaseUrl.path;
    final cleanBase = basePath.endsWith('/')
        ? basePath.substring(0, basePath.length - 1)
        : basePath;
    return config.supabaseUrl.replace(
      path: '$cleanBase/$path',
      queryParameters: query,
    );
  }

  String _supabaseError(http.Response response) {
    try {
      final decoded = jsonDecode(response.body);
      if (decoded is Map) {
        final message = decoded['message'] ?? decoded['error'];
        if (message != null) return message.toString();
      }
    } catch (_) {
      // Fall through to raw response body.
    }
    return response.body.trim().isEmpty
        ? 'Supabase request failed with HTTP ${response.statusCode}.'
        : response.body.trim();
  }
}
