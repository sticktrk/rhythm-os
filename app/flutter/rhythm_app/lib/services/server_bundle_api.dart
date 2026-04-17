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
    final response = await _dio.get(
      'api/backup',
      queryParameters: includeSecrets ? const {'include_secrets': true} : null,
    );
    final data = response.data;
    if (data is Map<String, dynamic>) {
      return Map<String, dynamic>.from(data);
    }
    throw StateError('Server returned an invalid backup bundle.');
  }

  Future<Map<String, dynamic>> putBackupBundle(
    Map<String, dynamic> bundle,
  ) async {
    final response = await _dio.put('api/backup', data: bundle);
    final data = response.data;
    if (data is Map<String, dynamic>) {
      return Map<String, dynamic>.from(data);
    }
    throw StateError('Server returned an invalid backup response.');
  }

  @visibleForTesting
  Dio get dio => _dio;
}
