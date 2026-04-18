import 'dart:convert';

import 'package:dio/dio.dart';
import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';

/// Thin client for the server's portable configuration and full backup routes.
///
/// This stays app-local for now so the cloud-sync plumbing can ship without
/// waiting on a broader SDK rollout.
class ServerBundleApi {
  final Dio _dio;

  ServerBundleApi({
    required HubEndpoint endpoint,
    Dio? dio,
  }) : _dio = dio ??
            Dio(
              BaseOptions(
                baseUrl: endpoint.baseUrl,
                connectTimeout: const Duration(seconds: 5),
                receiveTimeout: const Duration(seconds: 15),
              ),
            );

  Future<Map<String, dynamic>> getConfigurationBundle() async {
    final response = await _dio.get('api/configuration');
    final data = response.data;
    if (data is Map<String, dynamic>) {
      return Map<String, dynamic>.from(data);
    }
    throw StateError('Server returned an invalid configuration bundle.');
  }

  Future<Map<String, dynamic>> putConfigurationBundle(
    Map<String, dynamic> bundle,
  ) async {
    final response = await _dio.put('api/configuration', data: bundle);
    final data = response.data;
    if (data is Map<String, dynamic>) {
      return Map<String, dynamic>.from(data);
    }
    throw StateError('Server returned an invalid configuration response.');
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
    final response = await _dio.get<String>(
      'api/backup',
      queryParameters:
          includeSecrets ? const {'include_secrets': 'true'} : null,
      options: Options(responseType: ResponseType.plain),
    );
    final data = response.data;
    if (data is String && data.isNotEmpty) {
      return data;
    }
    throw StateError('Server returned an invalid backup bundle.');
  }

  Future<Map<String, dynamic>> putBackupBundle(
    Map<String, dynamic> bundle,
  ) async {
    return restoreBackupJson(jsonEncode(bundle));
  }

  Future<Map<String, dynamic>> restoreBackupJson(String backupJson) async {
    final response = await _dio.put<String>(
      'api/backup',
      data: backupJson,
      options: Options(
        responseType: ResponseType.plain,
        headers: const {'Content-Type': 'application/json'},
      ),
    );
    final data = response.data;
    if (data is String && data.isNotEmpty) {
      return _decodeJsonObject(
        data,
        errorMessage: 'Server returned an invalid backup response.',
      );
    }
    throw StateError('Server returned an invalid backup response.');
  }

  @visibleForTesting
  Dio get dio => _dio;

  Map<String, dynamic> _decodeJsonObject(
    String jsonText, {
    required String errorMessage,
  }) {
    final decoded = jsonDecode(jsonText);
    if (decoded is Map<String, dynamic>) {
      return Map<String, dynamic>.from(decoded);
    }
    if (decoded is Map) {
      return Map<String, dynamic>.from(decoded);
    }
    throw StateError(errorMessage);
  }
}
