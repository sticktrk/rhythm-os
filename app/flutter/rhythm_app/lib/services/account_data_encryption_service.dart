import 'dart:convert';
import 'dart:math';

import 'package:cryptography/cryptography.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';

import '../backend/backend.dart' show AuthUser;
import 'auth_service.dart';

/// Minimal async key/value seam over the platform keychain so tests can
/// substitute an in-memory fake.
abstract class AccountDataKeyStore {
  Future<String?> read(String key);
  Future<void> write(String key, String value);
}

class _SecureAccountDataKeyStore implements AccountDataKeyStore {
  const _SecureAccountDataKeyStore();

  static const FlutterSecureStorage _storage = FlutterSecureStorage(
    iOptions: IOSOptions(
      synchronizable: true,
      accessibility: KeychainAccessibility.first_unlock,
    ),
    aOptions: AndroidOptions(
      encryptedSharedPreferences: true,
    ),
  );

  @override
  Future<String?> read(String key) => _storage.read(key: key);

  @override
  Future<void> write(String key, String value) =>
      _storage.write(key: key, value: value);
}

/// Client-side encryption for account-scoped cloud payloads.
///
/// Every signed-in, non-anonymous user gets a random per-user data-encryption
/// key that is generated on first use and persisted in the platform keychain
/// (synchronized via iCloud Keychain on Apple platforms). The same path is
/// used for OAuth and email/password accounts.
///
/// Legacy envelopes (written before the device key existed) are still
/// decryptable via the historical `userId|createdAt` fallback material. When
/// decryption is impossible the caller receives null, which the product
/// treats as "no token" — hub owner tokens are recoverable via LAN re-claim.
class AccountDataEncryptionService {
  AccountDataEncryptionService._()
      : _keyStore = const _SecureAccountDataKeyStore(),
        _currentUserOverride = null;

  @visibleForTesting
  AccountDataEncryptionService.withDependencies({
    required AccountDataKeyStore keyStore,
    AuthUser? Function()? currentUser,
  })  : _keyStore = keyStore,
        _currentUserOverride = currentUser;

  static final AccountDataEncryptionService instance =
      AccountDataEncryptionService._();

  static const String envelopeVersion = 'account_secret_v1';
  static const String algorithmName = 'aes-gcm-256';
  static const String keyDerivationName = 'account-user-key-v1';
  static const String deviceKeySource = 'device-key-v1';
  static const String _salt = 'rhythm.lighting.account-data-encryption.v1';
  static const String _keyStorageKeyPrefix = 'account_data_key_v1_';

  final AccountDataKeyStore _keyStore;
  final AuthUser? Function()? _currentUserOverride;
  final AesGcm _cipher = AesGcm.with256bits();
  final Hkdf _hkdf = Hkdf(
    hmac: Hmac.sha256(),
    outputLength: 32,
  );
  final Map<String, String> _cachedDeviceKeys = <String, String>{};

  @visibleForTesting
  static String keyStorageKeyForUser(String userId) =>
      '$_keyStorageKeyPrefix$userId';

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
    final current = _currentNonAnonymousUser();
    if (current == null || value.trim().isEmpty) return null;

    final keyMaterial = await _deviceKeyMaterial(
      userId: current.userId,
      allowCreate: true,
    );
    if (keyMaterial == null) return null;

    final envelope = await _encryptString(
      value,
      purpose: purpose,
      keyMaterial: keyMaterial,
    );
    envelope['key_source'] = deviceKeySource;
    return envelope;
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

    final current = _currentNonAnonymousUser();
    if (current == null) return null;

    final deviceKey = await _deviceKeyMaterial(
      userId: current.userId,
      allowCreate: false,
    );

    if (envelope['key_source'] == deviceKeySource) {
      if (deviceKey == null) return null;
      return _decryptString(
        envelope,
        purpose: purpose,
        keyMaterial: deviceKey,
      );
    }

    // Legacy envelope (no key_source). Try the device key first (cheap),
    // then the historical user-scoped fallback material.
    if (deviceKey != null) {
      final viaDeviceKey = await _decryptString(
        envelope,
        purpose: purpose,
        keyMaterial: deviceKey,
        logFailure: false,
      );
      if (viaDeviceKey != null) return viaDeviceKey;
    }

    final createdAt =
        current.user.createdAt?.toUtc().toIso8601String() ?? '';
    return _decryptString(
      envelope,
      purpose: purpose,
      keyMaterial: '${current.userId}|$createdAt',
    );
  }

  Future<String?> _decryptString(
    Map<String, dynamic> envelope, {
    required String purpose,
    required String keyMaterial,
    bool logFailure = true,
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
      if (logFailure) {
        debugPrint(
          'AccountDataEncryptionService: decrypt failed for purpose=$purpose: '
          '$error',
        );
      }
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

  /// Returns the per-user device key material, reading it from the keychain
  /// (and caching it in memory) or, when [allowCreate] is true, generating a
  /// fresh random key and persisting it.
  Future<String?> _deviceKeyMaterial({
    required String userId,
    required bool allowCreate,
  }) async {
    final cached = _cachedDeviceKeys[userId];
    if (cached != null) return cached;

    final storageKey = keyStorageKeyForUser(userId);
    try {
      final existing = await _keyStore.read(storageKey);
      if (existing != null && existing.trim().isNotEmpty) {
        _cachedDeviceKeys[userId] = existing;
        return existing;
      }
      if (!allowCreate) return null;

      final created = _generateKeyMaterial();
      await _keyStore.write(storageKey, created);
      _cachedDeviceKeys[userId] = created;
      return created;
    } catch (error) {
      debugPrint(
        'AccountDataEncryptionService: keychain access failed for '
        'user=$userId: $error',
      );
      return null;
    }
  }

  String _generateKeyMaterial() {
    final random = Random.secure();
    final bytes = List<int>.generate(32, (_) => random.nextInt(256));
    return base64UrlEncode(bytes);
  }

  ({String userId, AuthUser user})? _currentNonAnonymousUser() {
    final override = _currentUserOverride;
    if (override != null) {
      final user = override();
      if (user == null || user.isAnonymous || user.id.isEmpty) return null;
      return (userId: user.id, user: user);
    }

    final auth = AuthService();
    final user = auth.currentUser;
    final userId = auth.currentUserId;
    if (!auth.isSignedIn ||
        auth.isAnonymous ||
        user == null ||
        userId == null) {
      return null;
    }
    return (userId: userId, user: user);
  }

  String _aad(String purpose) => '$envelopeVersion:$purpose';
}
