import 'package:dio/dio.dart';
import 'package:logging/logging.dart';

import '../api_auth.dart';
import '../json_parsing.dart';
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

class RhythmMatterCapturesResponse {
  const RhythmMatterCapturesResponse({
    required this.captures,
    this.raw = const <String, dynamic>{},
  });

  const RhythmMatterCapturesResponse.empty()
      : captures = const <RhythmMatterCaptureSummary>[],
        raw = const <String, dynamic>{};

  factory RhythmMatterCapturesResponse.fromJson(Map<String, dynamic> json) {
    final captures = json['captures'];
    return RhythmMatterCapturesResponse(
      captures: captures is List
          ? captures
              .map(jsonMap)
              .nonNulls
              .map(RhythmMatterCaptureSummary.fromJson)
              .toList(growable: false)
          : const <RhythmMatterCaptureSummary>[],
      raw: Map<String, dynamic>.from(json),
    );
  }

  final List<RhythmMatterCaptureSummary> captures;
  final Map<String, dynamic> raw;
}

class RhythmMatterCaptureSummary {
  const RhythmMatterCaptureSummary({
    required this.id,
    this.file,
    this.source,
    this.capturedAtUnixMs,
    this.vendorName,
    this.productName,
    this.vendorId,
    this.productId,
    this.nodeId,
    this.lightEndpoint,
    this.colorModes = const <String>[],
    this.derivedQuirks = const <String>[],
    this.dbMatchName,
    this.raw = const <String, dynamic>{},
  });

  factory RhythmMatterCaptureSummary.fromJson(Map<String, dynamic> json) {
    return RhythmMatterCaptureSummary(
      id: json['id']?.toString() ?? '',
      file: json['file'] as String?,
      source: json['source']?.toString(),
      capturedAtUnixMs: _readInt(json['captured_at_unix_ms']),
      vendorName: json['vendor_name'] as String?,
      productName: json['product_name'] as String?,
      vendorId: _readInt(json['vendor_id']),
      productId: _readInt(json['product_id']),
      nodeId: _readInt(json['node_id']),
      lightEndpoint: _readInt(json['light_endpoint']),
      colorModes: _readStringList(json['color_modes']),
      derivedQuirks: _readStringList(json['derived_quirks']),
      dbMatchName: json['db_match_name'] as String?,
      raw: Map<String, dynamic>.from(json),
    );
  }

  final String id;
  final String? file;
  final String? source;
  final int? capturedAtUnixMs;
  final String? vendorName;
  final String? productName;
  final int? vendorId;
  final int? productId;
  final int? nodeId;
  final int? lightEndpoint;
  final List<String> colorModes;
  final List<String> derivedQuirks;
  final String? dbMatchName;
  final Map<String, dynamic> raw;
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

  Future<RhythmMatterCapturesResponse> getMatterCaptures() async {
    try {
      final response = await _dio.get('api/matter/captures');
      final data = jsonMap(response.data);
      return data == null
          ? const RhythmMatterCapturesResponse.empty()
          : RhythmMatterCapturesResponse.fromJson(data);
    } catch (error) {
      _log.warning('getMatterCaptures failed', error);
    }
    return const RhythmMatterCapturesResponse.empty();
  }

  Future<Map<String, dynamic>?> getMatterCapture(String id) async {
    try {
      final response = await _dio.get(
        'api/matter/captures/${Uri.encodeComponent(id)}',
      );
      return jsonMap(response.data);
    } catch (error) {
      _log.warning('getMatterCapture failed', error);
    }
    return null;
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

int? _readInt(Object? value) {
  if (value is int) return value;
  if (value is num) return value.toInt();
  if (value is String) return int.tryParse(value);
  return null;
}

List<String> _readStringList(Object? value) {
  if (value is! List) return const <String>[];
  return value.map((entry) => entry.toString()).toList(growable: false);
}
