import 'dart:convert';

import 'package:cryptography/cryptography.dart';
import 'package:http/http.dart' as http;

import 'config.dart';
import 'models.dart';
import 'supabase_rest_client.dart';

class SupportAccessService {
  SupportAccessService({
    required AdminApiConfig config,
    required SupabaseRestClient supabase,
    http.Client? httpClient,
  })  : _config = config,
        _supabase = supabase,
        _http = httpClient ?? http.Client();

  static const _scope = 'beta_admin';
  static const _envelopeVersion = 'support_access_token_v1';
  static const _algorithm = 'aes-gcm-256';
  static const _keyDerivation = 'sha256-env-v1';
  static const _sessionTokenTtl = Duration(minutes: 15);

  /// Reuse window for minted session tokens. Kept below the requested TTL so
  /// a cached token is never handed out moments before the device expires it.
  static const _sessionTokenReuse = Duration(minutes: 14);

  final AdminApiConfig _config;
  final SupabaseRestClient _supabase;
  final http.Client _http;
  final AesGcm _cipher = AesGcm.with256bits();
  final Sha256 _sha256 = Sha256();
  final Map<String, _CachedSessionToken> _sessionTokens = {};

  bool get canUseSupportAccess =>
      _supabase.canUseServiceRole && _config.hasSupportAccessEncryptionKey;

  Future<String?> createSessionToken({
    required AdminSession session,
    required String hubId,
    required String baseUrl,
  }) async {
    if (!canUseSupportAccess || !session.staff.isAdmin) return null;

    // Session tokens are valid for [_sessionTokenTtl]; reuse them instead of
    // minting one per proxied request (grant lookup + decrypt + device POST).
    final cacheKey = _sessionTokenCacheKey(session, hubId, baseUrl);
    final cached = _sessionTokens[cacheKey];
    if (cached != null && cached.expiresAt.isAfter(DateTime.now().toUtc())) {
      return cached.token;
    }
    _sessionTokens.remove(cacheKey);

    final grant = await _loadActiveGrant(hubId);
    if (grant == null) return null;

    final durableToken = await _decryptGrantToken(grant);
    if (durableToken == null) return null;

    final response = await _http
        .post(
          _uriWithAppendedPath(baseUrl, 'api/auth/support-session-token'),
          headers: {
            'Accept': 'application/json',
            'Content-Type': 'application/json',
            'Authorization': 'Bearer $durableToken',
          },
          body: jsonEncode({
            'label': _sessionLabel(session),
            'ttl_seconds': _sessionTokenTtl.inSeconds,
          }),
        )
        .timeout(const Duration(seconds: 5));

    if (response.statusCode < 200 || response.statusCode >= 300) {
      return null;
    }
    final decoded = jsonDecode(response.body);
    if (decoded is! Map) return null;
    final token = decoded['token'];
    if (token is! String || token.trim().isEmpty) return null;
    final trimmed = token.trim();
    _sessionTokens[cacheKey] = _CachedSessionToken(
      token: trimmed,
      expiresAt: DateTime.now().toUtc().add(_sessionTokenReuse),
    );
    return trimmed;
  }

  /// Drop cached session tokens for a hub endpoint after the device rejects
  /// one (revoked token, device restart, clock skew).
  void invalidateSessionToken({required String hubId, required String baseUrl}) {
    final suffix = '|$hubId|${baseUrl.trim().toLowerCase()}';
    _sessionTokens.removeWhere((key, _) => key.endsWith(suffix));
  }

  String _sessionTokenCacheKey(
    AdminSession session,
    String hubId,
    String baseUrl,
  ) {
    return '${session.user.id}|$hubId|${baseUrl.trim().toLowerCase()}';
  }

  Future<Map<String, dynamic>?> _loadActiveGrant(String hubId) async {
    final rows = await _supabase.select(
      table: 'hub_support_access_grants',
      select:
          'id,hub_id,home_id,token_id,scope,encrypted_token,key_id,expires_at,revoked_at,created_at',
      accessToken: null,
      serviceRole: true,
      filters: {
        'hub_id': 'eq.$hubId',
        'scope': 'eq.$_scope',
        'revoked_at': 'is.null',
      },
      order: 'created_at.desc',
      limit: 5,
    );
    final now = DateTime.now().toUtc();
    for (final row in rows) {
      final expiresAt = parseDateTime(row['expires_at'])?.toUtc();
      if (expiresAt != null && !expiresAt.isAfter(now)) continue;
      if (asStringMap(row['encrypted_token']) != null) return row;
    }
    return null;
  }

  Future<String?> _decryptGrantToken(Map<String, dynamic> row) async {
    final envelope = asStringMap(row['encrypted_token']);
    if (envelope == null) return null;
    if (envelope['version'] != _envelopeVersion ||
        envelope['algorithm'] != _algorithm ||
        envelope['key_derivation'] != _keyDerivation) {
      return null;
    }
    final expectedKeyId = _config.supportAccessKeyId ?? 'default';
    final keyId = cleanString(envelope['key_id']) ?? 'default';
    if (keyId != expectedKeyId) return null;

    final secret = _config.supportAccessEncryptionKey;
    if (secret == null || secret.isEmpty) return null;

    try {
      final digest = await _sha256.hash(utf8.encode(secret));
      final aad = cleanString(envelope['aad']) ??
          _supportAccessAad(
            hubId: row['hub_id'] as String? ?? '',
            tokenId: row['token_id'] as String? ?? '',
            scope: row['scope'] as String? ?? _scope,
          );
      final bytes = await _cipher.decrypt(
        SecretBox(
          _decodeBase64Url(envelope['ciphertext']),
          nonce: _decodeBase64Url(envelope['nonce']),
          mac: Mac(_decodeBase64Url(envelope['mac'])),
        ),
        secretKey: SecretKey(digest.bytes),
        aad: utf8.encode(aad),
      );
      return utf8.decode(bytes);
    } catch (_) {
      return null;
    }
  }

  String _sessionLabel(AdminSession session) {
    final email = session.user.email?.trim();
    final id = session.user.id.length <= 8
        ? session.user.id
        : session.user.id.substring(0, 8);
    return 'admin-api ${email == null || email.isEmpty ? id : email}';
  }

  Uri _uriWithAppendedPath(String baseUrl, String pathToAppend) {
    final uri = Uri.parse(baseUrl.trim());
    final basePath = uri.path.endsWith('/') ? uri.path : '${uri.path}/';
    return uri.replace(
      path: '$basePath$pathToAppend',
      query: null,
      fragment: null,
    );
  }

  String _supportAccessAad({
    required String hubId,
    required String tokenId,
    required String scope,
  }) {
    return 'hub:$hubId:token:$tokenId:scope:$scope';
  }

  List<int> _decodeBase64Url(Object? value) {
    if (value is! String || value.trim().isEmpty) {
      throw const FormatException('Missing base64url value');
    }
    var normalized = value.trim().replaceAll('-', '+').replaceAll('_', '/');
    while (normalized.length % 4 != 0) {
      normalized += '=';
    }
    return base64.decode(normalized);
  }
}

class _CachedSessionToken {
  const _CachedSessionToken({required this.token, required this.expiresAt});

  final String token;
  final DateTime expiresAt;
}
