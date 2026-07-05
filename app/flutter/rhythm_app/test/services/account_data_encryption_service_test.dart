import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/backend/backend.dart' show AuthUser;
import 'package:rhythm_app/services/account_data_encryption_service.dart';

class InMemoryAccountDataKeyStore implements AccountDataKeyStore {
  final Map<String, String> values = <String, String>{};
  int reads = 0;
  int writes = 0;

  @override
  Future<String?> read(String key) async {
    reads++;
    return values[key];
  }

  @override
  Future<void> write(String key, String value) async {
    writes++;
    values[key] = value;
  }
}

class ThrowingAccountDataKeyStore implements AccountDataKeyStore {
  @override
  Future<String?> read(String key) async =>
      throw StateError('keychain unavailable');

  @override
  Future<void> write(String key, String value) async =>
      throw StateError('keychain unavailable');
}

void main() {
  const purpose = 'hub-token:home-1:hub-1';

  AuthUser userWithProviders(List<String> providers) {
    return AuthUser(
      id: 'user-1',
      email: 'user@example.com',
      isAnonymous: false,
      createdAt: DateTime.utc(2024, 3, 5, 12),
      metadata: {'providers': providers},
    );
  }

  group('AccountDataEncryptionService device key', () {
    test(
        'encrypt/decrypt round-trips for signed-in users regardless of '
        'auth provider', () async {
      for (final providers in [
        ['email'],
        ['google'],
        ['apple'],
      ]) {
        final store = InMemoryAccountDataKeyStore();
        final user = userWithProviders(providers);
        final service = AccountDataEncryptionService.withDependencies(
          keyStore: store,
          currentUser: () => user,
        );

        final envelope = await service.encryptStringForCurrentUser(
          'owner-token',
          purpose: purpose,
        );

        expect(envelope, isNotNull,
            reason: 'providers=$providers should encrypt');
        expect(envelope!['version'],
            AccountDataEncryptionService.envelopeVersion);
        expect(envelope['key_source'],
            AccountDataEncryptionService.deviceKeySource);
        expect(envelope.toString(), isNot(contains('owner-token')));

        final restored = await service.decryptStringForCurrentUser(
          envelope,
          purpose: purpose,
        );
        expect(restored, 'owner-token',
            reason: 'providers=$providers should decrypt');
      }
    });

    test('stores a random key in the keychain under the per-user key',
        () async {
      final store = InMemoryAccountDataKeyStore();
      final service = AccountDataEncryptionService.withDependencies(
        keyStore: store,
        currentUser: () => userWithProviders(['email']),
      );

      await service.encryptStringForCurrentUser('owner-token',
          purpose: purpose);

      final storageKey =
          AccountDataEncryptionService.keyStorageKeyForUser('user-1');
      expect(store.values.keys, [storageKey]);
      expect(store.values[storageKey], isNotEmpty);
      expect(store.writes, 1);

      // Key is generated once and cached; further encrypts reuse it.
      await service.encryptStringForCurrentUser('another-secret',
          purpose: purpose);
      expect(store.writes, 1);
    });

    test('key persists across service instances via shared storage',
        () async {
      final store = InMemoryAccountDataKeyStore();
      final user = userWithProviders(['google']);

      final first = AccountDataEncryptionService.withDependencies(
        keyStore: store,
        currentUser: () => user,
      );
      final envelope = await first.encryptStringForCurrentUser(
        'owner-token',
        purpose: purpose,
      );
      expect(envelope, isNotNull);

      // Fresh instance (simulates app restart) with the same keychain.
      final second = AccountDataEncryptionService.withDependencies(
        keyStore: store,
        currentUser: () => user,
      );
      final restored = await second.decryptStringForCurrentUser(
        envelope,
        purpose: purpose,
      );
      expect(restored, 'owner-token');
      // The restarted instance read the existing key instead of minting one.
      expect(store.values, hasLength(1));
    });

    test('legacy envelope without key_source decrypts via userId|createdAt '
        'fallback', () async {
      final store = InMemoryAccountDataKeyStore();
      final user = userWithProviders(['google']);
      final service = AccountDataEncryptionService.withDependencies(
        keyStore: store,
        currentUser: () => user,
      );

      final legacyMaterial =
          'user-1|${user.createdAt!.toUtc().toIso8601String()}';
      final legacyEnvelope = await service.encryptStringForTesting(
        'legacy-owner-token',
        purpose: purpose,
        keyMaterial: legacyMaterial,
      );
      expect(legacyEnvelope.containsKey('key_source'), isFalse);

      final restored = await service.decryptStringForCurrentUser(
        legacyEnvelope,
        purpose: purpose,
      );
      expect(restored, 'legacy-owner-token');
    });

    test('legacy envelope still decrypts after a device key exists',
        () async {
      final store = InMemoryAccountDataKeyStore();
      final user = userWithProviders(['email']);
      final service = AccountDataEncryptionService.withDependencies(
        keyStore: store,
        currentUser: () => user,
      );

      // Device key exists because the user already encrypted something new.
      await service.encryptStringForCurrentUser('new-secret',
          purpose: purpose);

      final legacyEnvelope = await service.encryptStringForTesting(
        'legacy-owner-token',
        purpose: purpose,
        keyMaterial: 'user-1|${user.createdAt!.toUtc().toIso8601String()}',
      );

      final restored = await service.decryptStringForCurrentUser(
        legacyEnvelope,
        purpose: purpose,
      );
      expect(restored, 'legacy-owner-token');
    });

    test('device-key envelope without the key in storage returns null',
        () async {
      final sourceStore = InMemoryAccountDataKeyStore();
      final user = userWithProviders(['google']);
      final source = AccountDataEncryptionService.withDependencies(
        keyStore: sourceStore,
        currentUser: () => user,
      );
      final envelope = await source.encryptStringForCurrentUser(
        'owner-token',
        purpose: purpose,
      );

      // Different device without the synced keychain entry.
      final other = AccountDataEncryptionService.withDependencies(
        keyStore: InMemoryAccountDataKeyStore(),
        currentUser: () => user,
      );
      final restored = await other.decryptStringForCurrentUser(
        envelope,
        purpose: purpose,
      );
      expect(restored, isNull);
    });

    test('signed-out user cannot encrypt or decrypt', () async {
      final store = InMemoryAccountDataKeyStore();
      final service = AccountDataEncryptionService.withDependencies(
        keyStore: store,
        currentUser: () => null,
      );

      final envelope = await service.encryptStringForCurrentUser(
        'owner-token',
        purpose: purpose,
      );
      expect(envelope, isNull);
      expect(store.values, isEmpty);
    });

    test('anonymous user cannot encrypt', () async {
      final store = InMemoryAccountDataKeyStore();
      final service = AccountDataEncryptionService.withDependencies(
        keyStore: store,
        currentUser: () => const AuthUser(
          id: 'anon-1',
          isAnonymous: true,
        ),
      );

      final envelope = await service.encryptStringForCurrentUser(
        'owner-token',
        purpose: purpose,
      );
      expect(envelope, isNull);
      expect(store.values, isEmpty);
    });

    test('keychain failure degrades to null instead of throwing', () async {
      final service = AccountDataEncryptionService.withDependencies(
        keyStore: ThrowingAccountDataKeyStore(),
        currentUser: () => userWithProviders(['email']),
      );

      final envelope = await service.encryptStringForCurrentUser(
        'owner-token',
        purpose: purpose,
      );
      expect(envelope, isNull);
    });

    test('rejects envelopes with mismatched purpose or headers', () async {
      final store = InMemoryAccountDataKeyStore();
      final user = userWithProviders(['email']);
      final service = AccountDataEncryptionService.withDependencies(
        keyStore: store,
        currentUser: () => user,
      );

      final envelope = await service.encryptStringForCurrentUser(
        'owner-token',
        purpose: purpose,
      );

      expect(
        await service.decryptStringForCurrentUser(
          envelope,
          purpose: 'hub-token:home-1:other-hub',
        ),
        isNull,
      );
      expect(
        await service.decryptStringForCurrentUser(
          {...envelope!, 'version': 'bogus'},
          purpose: purpose,
        ),
        isNull,
      );
    });
  });
}
