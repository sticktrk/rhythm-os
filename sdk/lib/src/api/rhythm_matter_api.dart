import 'package:dio/dio.dart';
import 'package:logging/logging.dart';

import '../api_auth.dart';
import '../rhythm_log_interceptor.dart';

class RhythmMatterWifiStatus {
  const RhythmMatterWifiStatus({
    required this.configPresent,
    required this.connected,
  });

  factory RhythmMatterWifiStatus.fromJson(Map<String, dynamic> json) {
    return RhythmMatterWifiStatus(
      configPresent: _readBool(json['config_present']),
      connected: _readBool(json['connected']),
    );
  }

  final bool? configPresent;
  final bool? connected;

  bool get readyForMatterPairing => configPresent == true && connected == true;

  static bool? _readBool(Object? value) {
    if (value is bool) return value;
    if (value is String) {
      if (value == 'true') return true;
      if (value == 'false') return false;
    }
    return null;
  }
}

class RhythmMatterPairingResponse {
  const RhythmMatterPairingResponse({
    this.httpStatus,
    this.status,
    this.device,
    this.error,
  });

  factory RhythmMatterPairingResponse.fromHttp({
    required int? statusCode,
    required Object? data,
  }) {
    if (data is Map) {
      final json = Map<String, dynamic>.from(data);
      return RhythmMatterPairingResponse(
        httpStatus: statusCode,
        status: json['status'] as String?,
        device: json['device'] is Map
            ? Map<String, dynamic>.from(json['device'] as Map)
            : null,
        error: (json['error'] ?? json['message']) as String?,
      );
    }

    final text = data?.toString().trim();
    return RhythmMatterPairingResponse(
      httpStatus: statusCode,
      error: text == null || text.isEmpty
          ? (statusCode == null
              ? 'Pairing request failed.'
              : 'Pairing request failed with HTTP $statusCode.')
          : text,
    );
  }

  final int? httpStatus;
  final String? status;
  final Map<String, dynamic>? device;
  final String? error;
}

/// Stateless API client for Matter pairing endpoints on a Rhythm server.
class RhythmMatterApi {
  static final _log = Logger('rhythm_sdk.api');

  final Dio _dio;

  RhythmMatterApi({required String baseUrl, Dio? dio, String? authToken})
      : _dio = dio ??
            Dio(
              BaseOptions(
                baseUrl: '$baseUrl/',
                connectTimeout: const Duration(seconds: 5),
                receiveTimeout: const Duration(seconds: 45),
                headers: bearerAuthHeaders(authToken),
              ),
            ) {
    final headers = bearerAuthHeaders(authToken);
    if (headers != null) {
      _dio.options.headers.addAll(headers);
    }
    _dio.interceptors.add(RhythmLogInterceptor(_log));
  }

  Future<RhythmMatterWifiStatus?> getWifiStatus() async {
    try {
      final response = await _dio.get(
        'api/wifi',
        options: Options(validateStatus: (_) => true),
      );

      if (response.statusCode != 200) return null;
      final data = response.data;
      if (data is! Map) return null;
      return RhythmMatterWifiStatus.fromJson(Map<String, dynamic>.from(data));
    } catch (_) {
      return null;
    }
  }

  Future<RhythmMatterPairingResponse> pairDevice({
    required String setupPayload,
    String network = 'wifi',
    String rendezvous = 'auto',
    Duration receiveTimeout = const Duration(seconds: 45),
    String? sessionId,
  }) async {
    try {
      final response = await _dio.post(
        'api/devices/pair',
        data: {
          'hub_type': 'matter',
          if (sessionId != null) 'session_id': sessionId,
          'params': {
            'setup_payload': setupPayload,
            'network': network,
            'rendezvous': rendezvous,
            if (sessionId != null) 'session_id': sessionId,
          },
        },
        options: Options(
          receiveTimeout: receiveTimeout,
          sendTimeout: receiveTimeout,
          validateStatus: (_) => true,
        ),
      );

      return RhythmMatterPairingResponse.fromHttp(
        statusCode: response.statusCode,
        data: response.data,
      );
    } on DioException catch (error) {
      if (error.response != null) {
        return RhythmMatterPairingResponse.fromHttp(
          statusCode: error.response!.statusCode,
          data: error.response!.data,
        );
      }

      final message = switch (error.type) {
        DioExceptionType.connectionTimeout ||
        DioExceptionType.receiveTimeout ||
        DioExceptionType.sendTimeout =>
          'Pairing timed out. Please keep the device powered on and try again.',
        DioExceptionType.connectionError => 'Could not reach the server.',
        _ => error.message ?? 'Pairing request failed.',
      };

      return RhythmMatterPairingResponse(error: message);
    } catch (error) {
      return RhythmMatterPairingResponse(error: error.toString());
    }
  }
}
