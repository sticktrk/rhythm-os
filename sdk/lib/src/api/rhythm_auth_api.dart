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
    this.authenticatedRole,
    this.reportsAuthenticatedRole = false,
  });

  factory RhythmAuthStatus.fromJson(Map<String, dynamic> json) {
    return RhythmAuthStatus(
      requiresAuth: json['requires_auth'] == true,
      ownerConfigured: json['owner_configured'] == true,
      tokenCount: (json['token_count'] as num?)?.toInt() ?? 0,
      claimAvailable: json['claim_available'] == true,
      authenticatedRole: json['authenticated_role']?.toString(),
      reportsAuthenticatedRole: json.containsKey('authenticated_role'),
    );
  }

  final bool requiresAuth;
  final bool ownerConfigured;
  final int tokenCount;
  final bool claimAvailable;
  final String? authenticatedRole;
  final bool reportsAuthenticatedRole;

  bool get hasAuthenticatedOwner => authenticatedRole == 'owner';
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

class RhythmCloudJoinProof {
  const RhythmCloudJoinProof({
    required this.proofVersion,
    required this.algorithm,
    required this.serverInstanceId,
    required this.homeId,
    required this.hubId,
    required this.issuedAtEpochMs,
    required this.expiresAtEpochMs,
    required this.nonce,
    required this.signature,
    this.tokenId,
  });

  factory RhythmCloudJoinProof.fromJson(Map<String, dynamic> json) {
    final proofVersion = json['proof_version']?.toString() ?? '';
    final algorithm = json['algorithm']?.toString() ?? '';
    final serverInstanceId = json['server_instance_id']?.toString() ?? '';
    final homeId = json['home_id']?.toString() ?? '';
    final hubId = json['hub_id']?.toString() ?? '';
    final issuedAtEpochMs = (json['issued_at_epoch_ms'] as num?)?.toInt() ?? 0;
    final expiresAtEpochMs =
        (json['expires_at_epoch_ms'] as num?)?.toInt() ?? 0;
    final nonce = json['nonce']?.toString() ?? '';
    final signature = json['signature']?.toString() ?? '';
    if (proofVersion.isEmpty ||
        algorithm.isEmpty ||
        serverInstanceId.isEmpty ||
        homeId.isEmpty ||
        hubId.isEmpty ||
        issuedAtEpochMs <= 0 ||
        expiresAtEpochMs <= issuedAtEpochMs ||
        nonce.isEmpty ||
        signature.isEmpty) {
      throw StateError('Server returned an invalid cloud join proof.');
    }
    return RhythmCloudJoinProof(
      proofVersion: proofVersion,
      algorithm: algorithm,
      serverInstanceId: serverInstanceId,
      homeId: homeId,
      hubId: hubId,
      tokenId: json['token_id']?.toString(),
      issuedAtEpochMs: issuedAtEpochMs,
      expiresAtEpochMs: expiresAtEpochMs,
      nonce: nonce,
      signature: signature,
    );
  }

  final String proofVersion;
  final String algorithm;
  final String serverInstanceId;
  final String homeId;
  final String hubId;
  final String? tokenId;
  final int issuedAtEpochMs;
  final int expiresAtEpochMs;
  final String nonce;
  final String signature;

  Map<String, dynamic> toJson() => {
        'proof_version': proofVersion,
        'algorithm': algorithm,
        'server_instance_id': serverInstanceId,
        'home_id': homeId,
        'hub_id': hubId,
        if (tokenId != null && tokenId!.trim().isNotEmpty) 'token_id': tokenId,
        'issued_at_epoch_ms': issuedAtEpochMs,
        'expires_at_epoch_ms': expiresAtEpochMs,
        'nonce': nonce,
        'signature': signature,
      };
}

class RhythmSupportToken {
  const RhythmSupportToken({
    required this.tokenId,
    required this.token,
    required this.role,
  });

  factory RhythmSupportToken.fromJson(Map<String, dynamic> json) {
    final tokenId = json['token_id']?.toString() ?? '';
    final token = json['token']?.toString() ?? '';
    final role = json['role']?.toString() ?? '';
    if (tokenId.isEmpty || token.isEmpty || role != 'support') {
      throw StateError('Server returned an invalid support token response.');
    }
    return RhythmSupportToken(
      tokenId: tokenId,
      token: token,
      role: role,
    );
  }

  final String tokenId;
  final String token;
  final String role;
}

class RhythmAuthSettingsUpdate extends RhythmAuthStatus {
  const RhythmAuthSettingsUpdate({
    required super.requiresAuth,
    required super.ownerConfigured,
    required super.tokenCount,
    required super.claimAvailable,
    super.authenticatedRole,
    super.reportsAuthenticatedRole,
    this.tokenId,
    this.token,
  });

  factory RhythmAuthSettingsUpdate.fromJson(Map<String, dynamic> json) {
    return RhythmAuthSettingsUpdate(
      requiresAuth: json['requires_auth'] == true,
      ownerConfigured: json['owner_configured'] == true,
      tokenCount: (json['token_count'] as num?)?.toInt() ?? 0,
      claimAvailable: json['claim_available'] == true,
      authenticatedRole: json['authenticated_role']?.toString(),
      reportsAuthenticatedRole: json.containsKey('authenticated_role'),
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

  Future<RhythmCloudJoinProof> createCloudJoinProof() async {
    try {
      final response = await _dio.post<Map<String, dynamic>>(
        'api/cloud/join-proof',
      );
      final data = response.data;
      if (data == null) {
        throw StateError('Server returned an empty cloud join proof response.');
      }
      return RhythmCloudJoinProof.fromJson(Map<String, dynamic>.from(data));
    } on DioException catch (error) {
      throw RhythmApiException(
        'Failed to create cloud join proof',
        statusCode: error.response?.statusCode,
        cause: error,
      );
    }
  }

  Future<RhythmSupportToken> issueSupportToken({
    String label = 'Rhythm support',
  }) async {
    final trimmedLabel = label.trim();
    try {
      final response = await _dio.post<Map<String, dynamic>>(
        'api/auth/support-token',
        data: {
          if (trimmedLabel.isNotEmpty) 'label': trimmedLabel,
        },
      );
      final data = response.data;
      if (data == null) {
        throw StateError('Server returned an empty support token response.');
      }
      return RhythmSupportToken.fromJson(Map<String, dynamic>.from(data));
    } on DioException catch (error) {
      throw RhythmApiException(
        'Failed to issue support token',
        statusCode: error.response?.statusCode,
        serverMessage: _authResponseMessage(error.response?.data),
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

String? _authResponseMessage(Object? data) {
  if (data is Map) {
    final message = data['message']?.toString().trim();
    if (message != null && message.isNotEmpty) return message;
  }
  final text = data?.toString().trim();
  return text == null || text.isEmpty ? null : text;
}

String _normalizeBaseUrl(String baseUrl) {
  final trimmed = baseUrl.trim();
  return trimmed.endsWith('/') ? trimmed : '$trimmed/';
}
