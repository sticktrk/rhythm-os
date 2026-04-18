import 'package:dio/dio.dart';
import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';

class MatterWifiStatus {
  const MatterWifiStatus({
    required this.configPresent,
    required this.connected,
  });

  factory MatterWifiStatus.fromJson(Map<String, dynamic> json) {
    return MatterWifiStatus(
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

class MatterPairingResponse {
  const MatterPairingResponse({
    this.httpStatus,
    this.status,
    this.device,
    this.error,
  });

  factory MatterPairingResponse.fromHttp({
    required int? statusCode,
    required Object? data,
  }) {
    if (data is Map) {
      final json = Map<String, dynamic>.from(data);
      return MatterPairingResponse(
        httpStatus: statusCode,
        status: json['status'] as String?,
        device: json['device'] is Map
            ? Map<String, dynamic>.from(json['device'] as Map)
            : null,
        error: (json['error'] ?? json['message']) as String?,
      );
    }

    final text = data?.toString().trim();
    return MatterPairingResponse(
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

class MatterPairingApi {
  MatterPairingApi({
    required HubEndpoint endpoint,
    Dio? dio,
  }) : _dio = dio ??
            Dio(
              BaseOptions(
                baseUrl: '${endpoint.baseUrl}/',
                connectTimeout: const Duration(seconds: 5),
                receiveTimeout: const Duration(seconds: 45),
              ),
            );

  final Dio _dio;

  Future<MatterWifiStatus?> getWifiStatus() async {
    try {
      final response = await _dio.get(
        'api/wifi',
        options: Options(validateStatus: (_) => true),
      );

      if (response.statusCode != 200) return null;
      final data = response.data;
      if (data is! Map) return null;
      return MatterWifiStatus.fromJson(Map<String, dynamic>.from(data));
    } catch (_) {
      return null;
    }
  }

  Future<MatterPairingResponse> pairDevice({
    required String setupPayload,
    String network = 'wifi',
    String rendezvous = 'auto',
    Duration receiveTimeout = const Duration(seconds: 45),
  }) async {
    try {
      final response = await _dio.post(
        'api/devices/pair',
        data: {
          'hub_type': 'matter',
          'params': {
            'setup_payload': setupPayload,
            'network': network,
            'rendezvous': rendezvous,
          },
        },
        options: Options(
          receiveTimeout: receiveTimeout,
          sendTimeout: receiveTimeout,
          validateStatus: (_) => true,
        ),
      );

      return MatterPairingResponse.fromHttp(
        statusCode: response.statusCode,
        data: response.data,
      );
    } on DioException catch (error) {
      if (error.response != null) {
        return MatterPairingResponse.fromHttp(
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

      return MatterPairingResponse(error: message);
    } catch (error) {
      return MatterPairingResponse(error: error.toString());
    }
  }

  @visibleForTesting
  Dio get dio => _dio;
}
