import 'package:flutter/foundation.dart';

import '../backend/auth/supabase_auth_backend.dart';
import '../backend/backend_provider.dart';

class SupportAccessRevokeResult {
  const SupportAccessRevokeResult({
    required this.grantId,
    required this.status,
    this.revokedAt,
  });

  final String grantId;
  final String status;
  final DateTime? revokedAt;
}

class SupportAccessDirectSession {
  const SupportAccessDirectSession({
    required this.grantId,
    required this.hostname,
    required this.tokenId,
    required this.token,
    required this.expiresAt,
  });

  final String grantId;
  final String hostname;
  final String tokenId;
  final String token;
  final DateTime expiresAt;
}

class SupportStaffMember {
  const SupportStaffMember({
    required this.userId,
    required this.role,
    required this.active,
    this.email,
  });

  final String userId;
  final String? email;
  final String role;
  final bool active;
}

class SupportAccessException implements Exception {
  const SupportAccessException(this.message, {this.statusCode});

  final String message;
  final int? statusCode;

  @override
  String toString() {
    final status = statusCode == null ? '' : ' ($statusCode)';
    return 'SupportAccessException$status: $message';
  }
}

class SupportAccessService {
  SupportAccessService._({dynamic Function()? supabaseClientFactory})
      : _supabaseClientFactory = supabaseClientFactory;

  static final SupportAccessService instance = SupportAccessService._();
  static const _functionName = 'support-access';

  @visibleForTesting
  factory SupportAccessService.testing({
    required dynamic Function() supabaseClientFactory,
  }) {
    return SupportAccessService._(
      supabaseClientFactory: supabaseClientFactory,
    );
  }

  final dynamic Function()? _supabaseClientFactory;

  Future<SupportStaffMember?> currentStaffMember() async {
    final data = await _supabaseClient()
        .from('staff_members')
        .select('user_id,email,role,active')
        .maybeSingle();
    final row = _readMap(data);
    if (row.isEmpty || row['active'] != true) return null;

    final userId = row['user_id']?.toString().trim();
    final role = row['role']?.toString().trim();
    if (userId == null || userId.isEmpty || role == null || role.isEmpty) {
      return null;
    }

    final email = row['email']?.toString().trim();
    return SupportStaffMember(
      userId: userId,
      email: email == null || email.isEmpty ? null : email,
      role: role,
      active: true,
    );
  }

  Future<SupportAccessRevokeResult> revokeGrant(
    String grantId, {
    bool resolveSubmission = false,
  }) async {
    final cleanGrantId = grantId.trim();
    if (cleanGrantId.isEmpty) {
      throw const SupportAccessException('Missing support grant id.');
    }

    final response = await _supabaseClient().functions.invoke(
      _functionName,
      body: {
        'action': 'revoke',
        'grant_id': cleanGrantId,
        if (resolveSubmission) 'resolve_submission': true,
      },
    );
    final statusCode = _responseStatus(response);
    final data = _responseData(response);
    if (statusCode < 200 || statusCode >= 300) {
      final error = _readError(data);
      if (statusCode == 403 &&
          (error == 'Support access grant expired' ||
              error == 'Support access grant is not active')) {
        return SupportAccessRevokeResult(
          grantId: cleanGrantId,
          status:
              error == 'Support access grant expired' ? 'expired' : 'inactive',
        );
      }
      throw SupportAccessException(
        error ?? 'Could not revoke support access grant.',
        statusCode: statusCode,
      );
    }

    final row = _readMap(data);
    return SupportAccessRevokeResult(
      grantId: row['grant_id']?.toString() ?? cleanGrantId,
      status: row['status']?.toString() ?? 'revoked',
      revokedAt: _parseDateTime(row['revoked_at']?.toString()),
    );
  }

  Future<SupportAccessDirectSession> openDirectSession(String grantId) async {
    final cleanGrantId = grantId.trim();
    if (cleanGrantId.isEmpty) {
      throw const SupportAccessException('Missing support grant id.');
    }

    final response = await _supabaseClient().functions.invoke(
      _functionName,
      body: {
        'action': 'direct-session',
        'grant_id': cleanGrantId,
      },
    );
    final statusCode = _responseStatus(response);
    final data = _responseData(response);
    if (statusCode < 200 || statusCode >= 300) {
      throw SupportAccessException(
        _readError(data) ?? 'Could not open direct support session.',
        statusCode: statusCode,
      );
    }

    final row = _readMap(data);
    final direct = _readMap(row['direct_access']);
    final hostname = direct['hostname']?.toString().trim();
    final tokenId = direct['token_id']?.toString().trim();
    final token = direct['token']?.toString().trim();
    final expiresAt = _parseDateTime(direct['expires_at']?.toString());
    if (hostname == null ||
        hostname.isEmpty ||
        tokenId == null ||
        tokenId.isEmpty ||
        token == null ||
        token.isEmpty ||
        expiresAt == null) {
      throw const SupportAccessException(
        'Direct support session response was incomplete.',
      );
    }

    return SupportAccessDirectSession(
      grantId: row['grant_id']?.toString() ?? cleanGrantId,
      hostname: hostname,
      tokenId: tokenId,
      token: token,
      expiresAt: expiresAt,
    );
  }

  dynamic _supabaseClient() {
    final injectedClient = _supabaseClientFactory?.call();
    if (injectedClient != null) return injectedClient;

    if (!BackendProvider.isInitialized) {
      throw const SupportAccessException(
        'Support access requires a Supabase backend.',
      );
    }
    final auth = BackendProvider.instance.auth;
    if (auth is SupabaseAuthBackend) return auth.client;
    throw const SupportAccessException(
      'Support access requires a Supabase backend.',
    );
  }

  static int _responseStatus(dynamic response) {
    final status = response.status;
    if (status is int) return status;
    return 200;
  }

  static Object? _responseData(dynamic response) {
    try {
      return response.data;
    } catch (_) {
      return null;
    }
  }

  static Map<String, dynamic> _readMap(Object? data) {
    if (data is Map) return Map<String, dynamic>.from(data);
    return const <String, dynamic>{};
  }

  static String? _readError(Object? data) {
    final row = _readMap(data);
    final error = row['error']?.toString().trim();
    return error == null || error.isEmpty ? null : error;
  }

  static DateTime? _parseDateTime(String? value) {
    final clean = value?.trim();
    if (clean == null || clean.isEmpty) return null;
    return DateTime.tryParse(clean);
  }
}
