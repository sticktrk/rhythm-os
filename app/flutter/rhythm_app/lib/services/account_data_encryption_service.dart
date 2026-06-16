import 'dart:convert';

import 'package:crypto/crypto.dart' as crypto;
import 'package:cryptography/cryptography.dart';
import 'package:flutter/foundation.dart';

import '../backend/backend.dart' show AuthUser;
import 'auth_service.dart';

/// Client-side encryption for account-scoped cloud payloads.
///
/// Email/password users get a password-derived portable key after sign-in.
/// Other providers use a user-scoped portable fallback until a dedicated
/// recovery-key or wrapped-key flow exists.
class AccountDataEncryptionService {
  AccountDataEncryptionService._();

  static final AccountDataEncryptionService instance =
      AccountDataEncryptionService._();

  static const String envelopeVersion = 'account_secret_v1';
  static const String algorithmName = 'aes-gcm-256';
  static const String keyDerivationName = 'account-user-key-v1';
  static const String _salt = 'rhythm.lighting.account-data-encryption.v1';

  final AesGcm _cipher = AesGcm.with256bits();
  final Hkdf _hkdf = Hkdf(
    hmac: Hmac.sha256(),
    outputLength: 32,
  );
  final Map<String, String> _rememberedUserSecrets = <String, String>{};

  bool get canEncryptForCurrentUser =>
      _currentUserKeyMaterial(forEncryption: true) != null;

  void rememberEmailPasswordKeyMaterial({
    required String userId,
    required String email,
    required String password,
  }) {
    if (userId.isEmpty || password.isEmpty) return;

    _rememberedUserSecrets[userId] = crypto.sha256
        .convert(
          utf8.encode(
            'rhythm-email-password-key-v1:$password',
          ),
        )
        .toString();
  }

  void clearRememberedKeyMaterial() {
    _rememberedUserSecrets.clear();
  }

  @visibleForTesting
  Future<Map<String, dynamic>> encryptStringForTesting(
    String value, {
    required String purpose,
    required String keyMaterial,
  }) async {
    return _encryptString(
      value,
      purpose: purpose,
      keyMaterial: keyMaterial,
    );
  }

  @visibleForTesting
  Future<String?> decryptStringForTesting(
    Map<String, dynamic> envelope, {
    required String purpose,
    required String keyMaterial,
  }) async {
    return _decryptString(
      envelope,
      purpose: purpose,
      keyMaterial: keyMaterial,
    );
  }

  Future<Map<String, dynamic>?> encryptStringForCurrentUser(
    String value, {
    required String purpose,
  }) async {
    final keyMaterial = _currentUserKeyMaterial(forEncryption: true);
    if (keyMaterial == null || value.trim().isEmpty) return null;

    return _encryptString(
      value,
      purpose: purpose,
      keyMaterial: keyMaterial,
    );
  }

  Future<Map<String, dynamic>> _encryptString(
    String value, {
    required String purpose,
    required String keyMaterial,
  }) async {
    final secretKey = await _deriveKey(
      keyMaterial: keyMaterial,
      purpose: purpose,
    );
    final secretBox = await _cipher.encrypt(
      utf8.encode(value),
      secretKey: secretKey,
      aad: utf8.encode(_aad(purpose)),
    );

    return {
      'version': envelopeVersion,
      'algorithm': algorithmName,
      'key_derivation': keyDerivationName,
      'purpose': purpose,
      'nonce': base64UrlEncode(secretBox.nonce),
      'ciphertext': base64UrlEncode(secretBox.cipherText),
      'mac': base64UrlEncode(secretBox.mac.bytes),
    };
  }

  Future<String?> decryptStringForCurrentUser(
    Map<String, dynamic>? envelope, {
    required String purpose,
  }) async {
    if (envelope == null) return null;
    if (envelope['version'] != envelopeVersion ||
        envelope['algorithm'] != algorithmName ||
        envelope['key_derivation'] != keyDerivationName ||
        envelope['purpose'] != purpose) {
      return null;
    }

    final keyMaterial = _currentUserKeyMaterial();
    if (keyMaterial == null) return null;

    return _decryptString(
      envelope,
      purpose: purpose,
      keyMaterial: keyMaterial,
    );
  }

  Future<String?> _decryptString(
    Map<String, dynamic> envelope, {
    required String purpose,
    required String keyMaterial,
  }) async {
    try {
      final secretKey = await _deriveKey(
        keyMaterial: keyMaterial,
        purpose: purpose,
      );
      final bytes = await _cipher.decrypt(
        SecretBox(
          base64Url.decode(envelope['ciphertext'] as String),
          nonce: base64Url.decode(envelope['nonce'] as String),
          mac: Mac(base64Url.decode(envelope['mac'] as String)),
        ),
        secretKey: secretKey,
        aad: utf8.encode(_aad(purpose)),
      );
      return utf8.decode(bytes);
    } catch (error) {
      debugPrint(
        'AccountDataEncryptionService: decrypt failed for purpose=$purpose: '
        '$error',
      );
      return null;
    }
  }

  Future<SecretKey> _deriveKey({
    required String keyMaterial,
    required String purpose,
  }) {
    return _hkdf.deriveKey(
      secretKey: SecretKey(utf8.encode(keyMaterial)),
      nonce: utf8.encode(_salt),
      info: utf8.encode(purpose),
    );
  }

  String? _currentUserKeyMaterial({bool forEncryption = false}) {
    final auth = AuthService();
    final user = auth.currentUser;
    final userId = auth.currentUserId;
    if (!auth.isSignedIn ||
        auth.isAnonymous ||
        user == null ||
        userId == null) {
      return null;
    }

    final rememberedSecret = _rememberedUserSecrets[userId];
    if (rememberedSecret != null) {
      return '$userId|email-password|$rememberedSecret';
    }

    if (forEncryption && _usesEmailPasswordAuth(user)) {
      return null;
    }

    final createdAt = user.createdAt?.toUtc().toIso8601String() ?? '';
    return '$userId|$createdAt';
  }

  bool _usesEmailPasswordAuth(AuthUser user) {
    final providers = user.metadata?['providers'];
    if (providers is! Iterable) return false;
    return providers.any((provider) => provider.toString() == 'email');
  }

  String _aad(String purpose) => '$envelopeVersion:$purpose';
}
