import 'dart:convert';

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
    this.mobileEnrollmentAvailable = false,
  });

  factory RhythmAuthStatus.fromJson(Map<String, dynamic> json) {
    return RhythmAuthStatus(
      requiresAuth: json['requires_auth'] == true,
      ownerConfigured: json['owner_configured'] == true,
      tokenCount: (json['token_count'] as num?)?.toInt() ?? 0,
      claimAvailable: json['claim_available'] == true,
      authenticatedRole: json['authenticated_role']?.toString(),
      reportsAuthenticatedRole: json.containsKey('authenticated_role'),
      mobileEnrollmentAvailable: json['mobile_enrollment_available'] == true,
    );
  }

  final bool requiresAuth;
  final bool ownerConfigured;
  final int tokenCount;
  final bool claimAvailable;
  final String? authenticatedRole;
  final bool reportsAuthenticatedRole;
  final bool mobileEnrollmentAvailable;

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

/// One-use approval copied from Connect mobile app in the HA admin interface.
/// No API endpoint is inferred from this payload: enrollment uses the LAN
/// endpoint the user selected, and the server must prove the same instance ID.
class RhythmMobileEnrollment {
  const RhythmMobileEnrollment({
    required this.code,
    required this.serverInstanceId,
    required this.expiresAtEpochMs,
  });

  final String code;
  final String serverInstanceId;
  final int expiresAtEpochMs;

  factory RhythmMobileEnrollment.parse(String value) {
    Object? decoded;
    try {
      decoded = jsonDecode(value);
    } catch (_) {
      throw const FormatException('Paste the complete mobile connection code.');
    }
    if (decoded is! Map ||
        decoded['format'] != 'rhythm-mobile-enrollment' ||
        decoded['version'] != 1 ||
        decoded['code'] is! String ||
        (decoded['code'] as String).trim().isEmpty ||
        decoded['server_instance_id'] is! String ||
        (decoded['server_instance_id'] as String).trim().isEmpty ||
        decoded['expires_at_epoch_ms'] is! int ||
        (decoded['expires_at_epoch_ms'] as int) <= 0) {
      throw const FormatException('Paste the complete mobile connection code.');
    }
    return RhythmMobileEnrollment(
      code: (decoded['code'] as String).trim(),
      serverInstanceId: (decoded['server_instance_id'] as String).trim(),
      expiresAtEpochMs: decoded['expires_at_epoch_ms'] as int,
    );
  }

  bool isExpired({DateTime? now}) =>
      (now ?? DateTime.now()).millisecondsSinceEpoch >= expiresAtEpochMs;
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

/// Metadata returned when an owner lists existing agent credentials.
class RhythmAgentCredential {
  const RhythmAgentCredential(this.id, this.kind,
      {this.unattendedChanges = false, this.destination});
  final String? destination;
  final bool unattendedChanges;
  final String id, kind;
}

class RhythmAssistantConnection {
  const RhythmAssistantConnection({
    required this.id,
    required this.destination,
    required this.kind,
    required this.remote,
    required this.expiresAt,
  });

  final String id, destination, kind;
  final bool remote;
  final DateTime expiresAt;

  factory RhythmAssistantConnection.fromJson(Map<String, dynamic> data) =>
      RhythmAssistantConnection(
        id: data['id'] as String,
        destination: data['destination'] as String,
        kind: data['kind'] as String,
        remote: data['remote'] as bool,
        expiresAt: DateTime.fromMillisecondsSinceEpoch(
            data['expires_at_epoch_ms'] as int,
            isUtc: true),
      );
}

enum RhythmConnectionDecision { approved, declined, codeMismatch, expired }

class RhythmConnectionDecisionResult {
  const RhythmConnectionDecisionResult(this.status, {this.attemptsLeft});
  final RhythmConnectionDecision status;
  final int? attemptsLeft;
}

class RhythmAssistantApproval {
  const RhythmAssistantApproval(
      {required this.planId,
      required this.operation,
      required this.credentialId,
      required this.review,
      this.expiresAt});
  final String planId, operation, credentialId;
  final Map<String, dynamic> review;
  final DateTime? expiresAt;

  factory RhythmAssistantApproval.fromJson(Map<String, dynamic> data) =>
      RhythmAssistantApproval(
          planId: data['plan_id'] as String,
          operation: data['plan_operation'] as String,
          credentialId: data['agent_credential_id'] as String,
          review: Map<String, dynamic>.from(data['review'] as Map),
          expiresAt: data['expires_at_epoch_ms'] is int
              ? DateTime.fromMillisecondsSinceEpoch(data['expires_at_epoch_ms'],
                  isUtc: true)
              : null);
}

class RhythmAssistantClientRegistration {
  const RhythmAssistantClientRegistration(
      this.id, this.secret, this.activationCode, this.expiresIn);
  final String id, secret, activationCode;
  final Duration expiresIn;
}

/// Local-owner choices on a hub that supports the Rhythm Pro preview.
class RhythmProSettings {
  final bool bulbProfileUpdates;
  final bool remoteMcp;
  const RhythmProSettings(
      {required this.bulbProfileUpdates, required this.remoteMcp});
  factory RhythmProSettings.fromJson(Map<String, dynamic> json) =>
      RhythmProSettings(
          bulbProfileUpdates: json['bulb_profile_updates'] as bool,
          remoteMcp: json['remote_mcp'] as bool);
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

  Future<RhythmProSettings> getProSettings() async {
    final response = await _dio.get<Map<String, dynamic>>('api/pro/settings');
    return RhythmProSettings.fromJson(response.data!);
  }

  Future<RhythmProSettings> updateProSettings(
      {bool? bulbProfileUpdates, bool? remoteMcp}) async {
    final response =
        await _dio.put<Map<String, dynamic>>('api/pro/settings', data: {
      if (bulbProfileUpdates != null)
        'bulb_profile_updates': bulbProfileUpdates,
      if (remoteMcp != null) 'remote_mcp': remoteMcp,
    });
    return RhythmProSettings.fromJson(response.data!);
  }

  Future<List<RhythmAgentCredential>> listAgentCredentials() async {
    final response = await _dio.get('api/auth/agent-tokens');
    final entries = response.data?['tokens'] as List;
    return entries
        .map((entry) => RhythmAgentCredential(
            entry['id'] as String, entry['kind'] as String,
            destination: entry['destination'] as String?,
            unattendedChanges: entry['unattended_changes'] == true))
        .toList();
  }

  Future<({String id, String token})> createAgentCredential(
      {bool unattendedChanges = false}) async {
    final response = await _dio.post('api/auth/agent-tokens',
        data: {'unattended_changes': unattendedChanges});
    final data = response.data;
    return (id: data['token_id'] as String, token: data['token'] as String);
  }

  Future<RhythmAssistantClientRegistration> registerAssistantClient(
      {bool unattendedChanges = false}) async {
    final response = await _dio.post('api/auth/assistant-clients',
        data: {'unattended_changes': unattendedChanges});
    final data = response.data;
    return RhythmAssistantClientRegistration(
        data['client_id'] as String,
        data['client_secret'] as String,
        data['activation_code'] as String,
        Duration(seconds: data['expires_in'] as int));
  }

  Future<List<RhythmAssistantConnection>> listAssistantConnections() async {
    final response = await _dio.get('api/assistant/connections');
    return (response.data['connections'] as List)
        .map((value) =>
            RhythmAssistantConnection.fromJson(value as Map<String, dynamic>))
        .toList();
  }

  Future<RhythmConnectionDecisionResult> decideAssistantConnection(String id,
      {required bool approve,
      String matchCode = '',
      bool unattendedChanges = false}) async {
    if (!RegExp(r'^[A-Za-z0-9_-]{1,128}$').hasMatch(id)) {
      throw ArgumentError('Invalid assistant connection ID');
    }
    try {
      final response = await _dio
          .post<Map<String, dynamic>>('api/assistant/connections/$id', data: {
        'decision': approve ? 'approve' : 'decline',
        if (approve) 'match_code': matchCode,
        'unattended_changes': approve && unattendedChanges,
      });
      return switch (response.data?['status']) {
        'approved' => const RhythmConnectionDecisionResult(
            RhythmConnectionDecision.approved),
        'declined' => const RhythmConnectionDecisionResult(
            RhythmConnectionDecision.declined),
        _ => throw const FormatException('Unknown connection decision'),
      };
    } on DioException catch (error) {
      final body = error.response?.data;
      if (error.response?.statusCode != 409 || body is! Map) rethrow;
      return switch (body['status']) {
        'code_mismatch' => RhythmConnectionDecisionResult(
            RhythmConnectionDecision.codeMismatch,
            attemptsLeft: body['attempts_left'] as int),
        'expired' => const RhythmConnectionDecisionResult(
            RhythmConnectionDecision.expired),
        _ => throw const FormatException('Unknown connection conflict'),
      };
    }
  }

  Future<List<RhythmAssistantApproval>> listAssistantApprovals() async {
    final response = await _dio.get('api/assistant/approvals');
    return (response.data['approvals'] as List)
        .map((value) =>
            RhythmAssistantApproval.fromJson(value as Map<String, dynamic>))
        .toList();
  }

  Future<Map<String, dynamic>> decideAssistantApproval(String planId,
      {required bool approve}) async {
    if (!RegExp(r'^[A-Za-z0-9_-]+$').hasMatch(planId)) {
      throw ArgumentError('Invalid assistant plan ID');
    }
    final response = await _dio.post<Map<String, dynamic>>(
        'api/assistant/approvals/$planId',
        data: {'decision': approve ? 'approve' : 'decline'});
    return response.data ??
        (throw StateError('The hub returned no approval receipt.'));
  }

  Future<void> revokeAgentCredential(String id) async {
    if (!RegExp(r'^[A-Za-z0-9_-]+$').hasMatch(id)) {
      throw ArgumentError('Invalid agent credential ID');
    }
    await _dio.delete('api/auth/agent-tokens/$id');
  }

  Future<RhythmOwnerClaim> exchangeMobileEnrollment(
    RhythmMobileEnrollment enrollment, {
    String label = 'Rhythm app',
  }) async {
    if (enrollment.isExpired()) {
      throw const FormatException(
        'This connection code expired. Create a new code in Home Assistant.',
      );
    }
    try {
      final response = await _dio.post<Map<String, dynamic>>(
        'api/addon/enrollment/exchange',
        data: {
          'code': enrollment.code,
          'server_instance_id': enrollment.serverInstanceId,
          'label': label.trim(),
        },
      );
      final data = response.data;
      if (data == null ||
          data['server_instance_id'] != enrollment.serverInstanceId) {
        throw StateError(
          'The connection code belongs to another installation.',
        );
      }
      return RhythmOwnerClaim.fromJson(data);
    } on DioException catch (error) {
      // Never surface an echoed response, request body, code, or credential.
      throw RhythmApiException(
        'Connection code was rejected. Create a new code in Home Assistant.',
        statusCode: error.response?.statusCode,
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
