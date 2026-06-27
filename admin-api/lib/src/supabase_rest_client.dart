import 'dart:convert';

import 'package:http/http.dart' as http;

import 'config.dart';
import 'models.dart';

class AdminApiException implements Exception {
  const AdminApiException(this.statusCode, this.message);

  final int statusCode;
  final String message;

  @override
  String toString() => message;
}

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
