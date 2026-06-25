import 'package:dio/dio.dart';
import 'package:logging/logging.dart';

import '../api_auth.dart';
import '../errors/rhythm_exception.dart';
import '../rhythm_log_interceptor.dart';

class RhythmAuthStatus {
  const RhythmAuthStatus({
    required this.requiresAuth,
    required this.ownerConfigured,
    required this.tokenCount,
    required this.claimAvailable,
  });

  factory RhythmAuthStatus.fromJson(Map<String, dynamic> json) {
    return RhythmAuthStatus(
      requiresAuth: json['requires_auth'] == true,
      ownerConfigured: json['owner_configured'] == true,
      tokenCount: (json['token_count'] as num?)?.toInt() ?? 0,
      claimAvailable: json['claim_available'] == true,
    );
  }

  final bool requiresAuth;
  final bool ownerConfigured;
  final int tokenCount;
  final bool claimAvailable;
}

class RhythmOwnerClaim {
  const RhythmOwnerClaim({
    required this.tokenId,
    required this.token,
  });

  factory RhythmOwnerClaim.fromJson(Map<String, dynamic> json) {
    final tokenId = json['token_id']?.toString() ?? '';
    final token = json['token']?.toString() ?? '';
    if (tokenId.isEmpty || token.isEmpty) {
      throw StateError('Server returned an invalid owner claim response.');
    }
    return RhythmOwnerClaim(tokenId: tokenId, token: token);
  }

  final String tokenId;
  final String token;
}

class RhythmAuthSettingsUpdate extends RhythmAuthStatus {
  const RhythmAuthSettingsUpdate({
    required super.requiresAuth,
    required super.ownerConfigured,
    required super.tokenCount,
    required super.claimAvailable,
    this.tokenId,
    this.token,
  });

  factory RhythmAuthSettingsUpdate.fromJson(Map<String, dynamic> json) {
    return RhythmAuthSettingsUpdate(
      requiresAuth: json['requires_auth'] == true,
      ownerConfigured: json['owner_configured'] == true,
      tokenCount: (json['token_count'] as num?)?.toInt() ?? 0,
      claimAvailable: json['claim_available'] == true,
      tokenId: json['token_id']?.toString(),
      token: json['token']?.toString(),
    );
  }

  final String? tokenId;
  final String? token;
}

class RhythmAuthApi {
  static final _log = Logger('rhythm_sdk.api');

  RhythmAuthApi({
    required String baseUrl,
    Dio? dio,
    String? authToken,
  }) : _dio = dio ??
            Dio(
              BaseOptions(
                baseUrl: _normalizeBaseUrl(baseUrl),
                connectTimeout: const Duration(seconds: 5),
                receiveTimeout: const Duration(seconds: 10),
                headers: bearerAuthHeaders(authToken),
              ),
            ) {
    final headers = bearerAuthHeaders(authToken);
    if (headers != null) {
      _dio.options.headers.addAll(headers);
    }
    _dio.interceptors.add(RhythmLogInterceptor(_log));
  }

  final Dio _dio;

  Future<RhythmAuthStatus> getStatus() async {
    try {
      final response = await _dio.get<Map<String, dynamic>>('api/auth/status');
      final data = response.data;
      if (data == null) {
        throw StateError('Server returned an empty auth status response.');
      }
      return RhythmAuthStatus.fromJson(Map<String, dynamic>.from(data));
    } on DioException catch (error) {
      throw RhythmApiException(
        'Failed to fetch auth status',
        statusCode: error.response?.statusCode,
        cause: error,
      );
    }
  }

  Future<RhythmOwnerClaim> claimOwnerToken({
    String label = 'Rhythm app',
  }) async {
    try {
      final response = await _dio.post<Map<String, dynamic>>(
        'api/auth/claim',
        data: {'label': label},
      );
      final data = response.data;
      if (data == null) {
        throw StateError('Server returned an empty owner claim response.');
      }
      return RhythmOwnerClaim.fromJson(Map<String, dynamic>.from(data));
    } on DioException catch (error) {
      throw RhythmApiException(
        'Failed to claim owner token',
        statusCode: error.response?.statusCode,
        cause: error,
      );
    }
  }

  Future<RhythmAuthSettingsUpdate> setSettings({
    required bool requireApiAuth,
    String label = 'Rhythm app',
  }) async {
    final trimmedLabel = label.trim();
    try {
      final response = await _dio.put<Map<String, dynamic>>(
        'api/auth/settings',
        data: {
          'require_api_auth': requireApiAuth,
          if (trimmedLabel.isNotEmpty) 'label': trimmedLabel,
        },
      );
      final data = response.data;
      if (data == null) {
        throw StateError('Server returned an empty auth settings response.');
      }
      return RhythmAuthSettingsUpdate.fromJson(
        Map<String, dynamic>.from(data),
      );
    } on DioException catch (error) {
      throw RhythmApiException(
        'Failed to update auth settings',
        statusCode: error.response?.statusCode,
        cause: error,
      );
    }
  }
}

String _normalizeBaseUrl(String baseUrl) {
  final trimmed = baseUrl.trim();
  return trimmed.endsWith('/') ? trimmed : '$trimmed/';
}
