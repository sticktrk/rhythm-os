import 'package:dio/dio.dart';
import 'package:flutter/foundation.dart';

class DeviceRestartResult {
  final bool success;
  final String message;

  const DeviceRestartResult._({
    required this.success,
    required this.message,
  });

  const DeviceRestartResult.success(String message)
      : this._(success: true, message: message);

  const DeviceRestartResult.failure(String message)
      : this._(success: false, message: message);
}

class DeviceRestartService {
  static const Duration defaultConnectTimeout = Duration(seconds: 5);
  static const Duration defaultReceiveTimeout = Duration(seconds: 5);

  final Dio _dio;

  DeviceRestartService({
    required String baseUrl,
    Duration connectTimeout = defaultConnectTimeout,
    Duration receiveTimeout = defaultReceiveTimeout,
  }) : _dio = Dio(
          BaseOptions(
            baseUrl: baseUrl,
            connectTimeout: connectTimeout,
            receiveTimeout: receiveTimeout,
          ),
        );

  @visibleForTesting
  DeviceRestartService.withDio(this._dio);

  void dispose() {
    _dio.close();
  }

  Future<DeviceRestartResult> scheduleRestart() async {
    try {
      final response = await _dio.post<Map<String, dynamic>>('/api/restart');
      final data = response.data;
      if (data == null || data['status'] != 'ok') {
        return const DeviceRestartResult.failure(
          'Server did not confirm the restart.',
        );
      }

      final message = data['message']?.toString().trim();
      return DeviceRestartResult.success(
        message == null || message.isEmpty ? 'Restart scheduled' : message,
      );
    } on DioException catch (error) {
      return DeviceRestartResult.failure(_describeDioError(error));
    } catch (_) {
      return const DeviceRestartResult.failure(
        'Failed to schedule restart.',
      );
    }
  }

  String _describeDioError(DioException error) {
    final statusCode = error.response?.statusCode;
    if (statusCode == 404) {
      return 'This server does not support remote restart yet.';
    }
    if (statusCode != null) {
      return 'Restart request failed ($statusCode).';
    }

    return switch (error.type) {
      DioExceptionType.connectionTimeout ||
      DioExceptionType.receiveTimeout ||
      DioExceptionType.sendTimeout =>
        'The server took too long to respond.',
      DioExceptionType.connectionError => 'Could not reach the server.',
      DioExceptionType.badCertificate => 'The server certificate was invalid.',
      DioExceptionType.cancel => 'The restart request was cancelled.',
      _ => 'Failed to schedule restart.',
    };
  }
}
