import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/account_data_encryption_service.dart';
import 'package:rhythm_app/services/account_cloud_sync_service.dart';
import 'package:rhythm_core/rhythm_core.dart';

void main() {
  group('AccountCloudSyncService payloads', () {
    test('home payload normalizes ownership to the signed-in user', () {
      final home = Home.create(
        id: 'd5f28205-02de-4a39-a7fc-35777e4964c7',
        name: 'Local Home',
        ownerId: 'anonymous-user',
        timezone: 'America/New_York',
      );

      final payload = AccountCloudSyncService.homeSnapshotPayload(
        home,
        userId: '8f075ac1-e4e7-40f7-817c-4a27424307f4',
      );

      expect(payload['owner_id'], '8f075ac1-e4e7-40f7-817c-4a27424307f4');
      expect(payload['member_ids'], ['8f075ac1-e4e7-40f7-817c-4a27424307f4']);
      expect(payload['pendingSync'], isNull);
      expect(payload['sleep_schedule'], isA<Map<String, dynamic>>());
    });

    test('home payload preserves cloud owner and existing members', () {
      final home = Home.create(
        id: 'd5f28205-02de-4a39-a7fc-35777e4964c7',
        name: 'Shared Home',
        ownerId: 'owner-user',
      ).copyWith(memberIds: ['owner-user', 'joining-user']);

      final payload = AccountCloudSyncService.homeSnapshotPayload(
        home,
        userId: 'joining-user',
      );

      expect(payload['owner_id'], 'owner-user');
      expect(payload['member_ids'], ['owner-user', 'joining-user']);
    });

    test('home payload can preserve existing cloud membership columns', () {
      final home = Home.create(
        id: 'd5f28205-02de-4a39-a7fc-35777e4964c7',
        name: 'Shared Home',
        ownerId: 'owner-user',
      ).copyWith(memberIds: ['owner-user']);

      final payload = AccountCloudSyncService.homeSnapshotPayload(
        home,
        userId: 'owner-user',
        includeMembership: false,
      );

      expect(payload, isNot(contains('owner_id')));
      expect(payload, isNot(contains('member_ids')));
      expect(payload['name'], 'Shared Home');
    });

    test('home sync requires at least one server hub', () {
      final home = Home.create(
        id: 'd5f28205-02de-4a39-a7fc-35777e4964c7',
        name: 'Local Home',
        ownerId: 'anonymous-user',
      );

      expect(
        shouldSyncHomeAndServerHubsForTesting(
          home: home,
          serverHubs: const [],
        ),
        isFalse,
      );
      expect(
        shouldSyncHomeAndServerHubsForTesting(
          home: home,
          serverHubs: [
            Hub.server(
              id: 'ca2b97f3-0d6e-4396-8a63-c9b22ff2ee04',
              homeId: home.id,
              name: 'Kitchen Server',
              host: '192.168.5.123',
            ),
          ],
        ),
        isTrue,
      );
    });

    test('account home membership requires owner or member match', () {
      final home = Home.create(
        id: 'd5f28205-02de-4a39-a7fc-35777e4964c7',
        name: 'Household',
        ownerId: 'owner-user',
      );

      expect(
        accountHomeBelongsToUserForTesting(home: home, userId: 'owner-user'),
        isTrue,
      );
      expect(
        accountHomeBelongsToUserForTesting(home: home, userId: 'other-user'),
        isFalse,
      );
    });

    test('signed-in sync can claim anonymous homes but rejects other accounts',
        () {
      final anonymousHome = Home.create(
        id: 'd5f28205-02de-4a39-a7fc-35777e4964c7',
        name: 'Local Home',
        ownerId: 'anonymous-user',
      );
      final staleAccountHome = Home.create(
        id: 'e4d37ce1-2ac9-4ac4-a226-e9d1234ca471',
        name: 'Old Household',
        ownerId: 'previous-user',
      );

      expect(
        accountHomeCanSyncForUserForTesting(
          home: anonymousHome,
          userId: 'signed-in-user',
        ),
        isTrue,
      );
      expect(
        accountHomeCanSyncForUserForTesting(
          home: staleAccountHome,
          userId: 'signed-in-user',
        ),
        isFalse,
      );
    });

    test('only the owning account can write shared hub token envelopes', () {
      final ownedHome = Home.create(
        id: 'd5f28205-02de-4a39-a7fc-35777e4964c7',
        name: 'Owned Home',
        ownerId: 'owner-user',
      );
      final joinedHome = ownedHome.copyWith(
        memberIds: ['owner-user', 'joining-user'],
      );

      expect(
        accountHomeCanWriteSharedHubTokensForTesting(
          home: ownedHome,
          userId: 'owner-user',
        ),
        isTrue,
      );
      expect(
        accountHomeCanWriteSharedHubTokensForTesting(
          home: joinedHome,
          userId: 'joining-user',
        ),
        isFalse,
      );
    });

    test('server hub payload encrypts local owner token envelope', () async {
      final hub = Hub.server(
        id: 'ca2b97f3-0d6e-4396-8a63-c9b22ff2ee04',
        homeId: 'd5f28205-02de-4a39-a7fc-35777e4964c7',
        name: 'Kitchen Server',
        host: '192.168.5.123',
        token: 'owner-token',
        serverInstanceId: 'srv-kitchen',
        remoteEndpoint: const HubEndpoint(
          host: 'hub.devices.rhythm.lighting',
          port: 443,
          useSsl: true,
        ),
      );

      final purpose = AccountCloudSyncService.hubTokenPurpose(
        homeId: hub.homeId,
        hubId: hub.id,
      );
      final envelope =
          await AccountDataEncryptionService.instance.encryptStringForTesting(
        hub.token!,
        purpose: purpose,
        keyMaterial: 'signed-in-user-key-material',
      );

      final payload = AccountCloudSyncService.serverHubSnapshotPayload(
        hub,
        encryptedToken: envelope,
      );

      expect(payload['type'], 'server');
      expect(payload['server_instance_id'], 'srv-kitchen');
      expect(payload['token'], isNull);
      expect(payload['encrypted_token'], isA<Map<String, dynamic>>());
      expect(payload['encrypted_token'].toString(), isNot(contains(hub.token)));
      expect(payload['endpoint'], {
        'host': '192.168.5.123',
        'port': 54448,
        'useSsl': false,
      });
      expect(payload['remote_endpoint'], {
        'host': 'hub.devices.rhythm.lighting',
        'port': 443,
        'useSsl': true,
      });

      final restored =
          await AccountDataEncryptionService.instance.decryptStringForTesting(
        Map<String, dynamic>.from(payload['encrypted_token'] as Map),
        purpose: purpose,
        keyMaterial: 'signed-in-user-key-material',
      );
      expect(restored, 'owner-token');
    });

    test('server hub payload can preserve existing cloud token columns', () {
      final hub = Hub.server(
        id: 'ca2b97f3-0d6e-4396-8a63-c9b22ff2ee04',
        homeId: 'd5f28205-02de-4a39-a7fc-35777e4964c7',
        name: 'Kitchen Server',
        host: '192.168.5.123',
        token: 'owner-token',
      );

      final payload = AccountCloudSyncService.serverHubSnapshotPayload(
        hub,
        preserveToken: true,
      );

      expect(payload, isNot(contains('token')));
      expect(payload, isNot(contains('encrypted_token')));
    });

    test('server hub payload can omit server identity for old schemas', () {
      final hub = Hub.server(
        id: 'ca2b97f3-0d6e-4396-8a63-c9b22ff2ee04',
        homeId: 'd5f28205-02de-4a39-a7fc-35777e4964c7',
        name: 'Kitchen Server',
        host: '192.168.5.123',
        token: 'owner-token',
        serverInstanceId: 'srv-kitchen',
      );

      final payload = AccountCloudSyncService.serverHubSnapshotPayload(
        hub,
        includeServerInstanceId: false,
      );

      expect(payload, isNot(contains('server_instance_id')));
    });

    test('server hub payload normalizes server identity for conflict checks',
        () {
      final hub = Hub.server(
        id: 'ca2b97f3-0d6e-4396-8a63-c9b22ff2ee04',
        homeId: 'd5f28205-02de-4a39-a7fc-35777e4964c7',
        name: 'Kitchen Server',
        host: '192.168.5.123',
        token: 'owner-token',
        serverInstanceId: ' SRV-KITCHEN ',
      );

      final payload = AccountCloudSyncService.serverHubSnapshotPayload(hub);

      expect(payload['server_instance_id'], 'srv-kitchen');
    });

    test('server hub payload preserves cloud tunnel when local has none', () {
      final hub = Hub.server(
        id: 'ca2b97f3-0d6e-4396-8a63-c9b22ff2ee04',
        homeId: 'd5f28205-02de-4a39-a7fc-35777e4964c7',
        name: 'Kitchen Server',
        host: '192.168.5.123',
        token: 'owner-token',
      );

      final payload = AccountCloudSyncService.serverHubSnapshotPayload(hub);

      expect(payload, isNot(contains('remote_endpoint')));
    });

    test('server hub payload can explicitly clear cloud tunnel', () {
      final hub = Hub.server(
        id: 'ca2b97f3-0d6e-4396-8a63-c9b22ff2ee04',
        homeId: 'd5f28205-02de-4a39-a7fc-35777e4964c7',
        name: 'Kitchen Server',
        host: '192.168.5.123',
        token: 'owner-token',
      );

      final payload = AccountCloudSyncService.serverHubSnapshotPayload(
        hub,
        clearRemoteEndpoint: true,
      );

      expect(payload, containsPair('remote_endpoint', null));
    });

    test('server hub payload can store encrypted token in legacy token column',
        () async {
      final hub = Hub.server(
        id: 'ca2b97f3-0d6e-4396-8a63-c9b22ff2ee04',
        homeId: 'd5f28205-02de-4a39-a7fc-35777e4964c7',
        name: 'Kitchen Server',
        host: '192.168.5.123',
        token: 'owner-token',
      );
      final purpose = AccountCloudSyncService.hubTokenPurpose(
        homeId: hub.homeId,
        hubId: hub.id,
      );
      final envelope =
          await AccountDataEncryptionService.instance.encryptStringForTesting(
        hub.token!,
        purpose: purpose,
        keyMaterial: 'signed-in-user-key-material',
      );

      final payload = AccountCloudSyncService.serverHubSnapshotPayload(
        hub,
        encryptedToken: envelope,
        useLegacyEncryptedTokenStorage: true,
      );

      expect(payload, isNot(contains('encrypted_token')));
      expect(payload['token'], isA<String>());
      expect(payload['token'], isNot(contains(hub.token)));

      final restored =
          await AccountDataEncryptionService.instance.decryptStringForTesting(
        Map<String, dynamic>.from(
          jsonDecode(payload['token'] as String) as Map,
        ),
        purpose: purpose,
        keyMaterial: 'signed-in-user-key-material',
      );
      expect(restored, 'owner-token');
    });

    test(
        'flags hubs whose local token could not be encrypted so sync logs '
        'the skip', () {
      final hubWithToken = Hub.server(
        id: 'ca2b97f3-0d6e-4396-8a63-c9b22ff2ee04',
        homeId: 'd5f28205-02de-4a39-a7fc-35777e4964c7',
        name: 'Kitchen Server',
        host: '192.168.5.123',
        token: 'owner-token',
      );
      final hubWithoutToken = Hub.server(
        id: '22222222-2222-4222-8222-222222222222',
        homeId: 'd5f28205-02de-4a39-a7fc-35777e4964c7',
        name: 'Garage Server',
        host: '192.168.5.124',
      );

      // Local token present but encryption unavailable -> warn.
      expect(
        hubTokenEncryptionUnavailableForTesting(
          hub: hubWithToken,
          encryptedToken: null,
        ),
        isTrue,
      );
      // Encryption produced an envelope -> nothing to warn about.
      expect(
        hubTokenEncryptionUnavailableForTesting(
          hub: hubWithToken,
          encryptedToken: {'version': 'account_secret_v1'},
        ),
        isFalse,
      );
      // No local token -> nothing was lost.
      expect(
        hubTokenEncryptionUnavailableForTesting(
          hub: hubWithoutToken,
          encryptedToken: null,
        ),
        isFalse,
      );
    });

    test('detects server identity unique-index conflicts narrowly', () {
      expect(
        isServerIdentityUniqueConflictForTesting(
          'PostgrestException: duplicate key value violates unique constraint '
          '"hubs_server_instance_id_unique_idx" code: 23505',
        ),
        isTrue,
      );
      expect(
        isServerIdentityUniqueConflictForTesting(
          'duplicate key value violates unique constraint on server_instance_id',
        ),
        isTrue,
      );
      expect(
        isServerIdentityUniqueConflictForTesting(
          'PostgrestException: duplicate key value violates unique constraint '
          '"other_unique_idx" code: 23505',
        ),
        isFalse,
      );
    });

    test('groups account Homes with their server hubs', () {
      final homeA = Home.create(
        id: 'd5f28205-02de-4a39-a7fc-35777e4964c7',
        name: 'Main Home',
        ownerId: '8f075ac1-e4e7-40f7-817c-4a27424307f4',
      );
      final homeB = Home.create(
        id: 'e4d37ce1-2ac9-4ac4-a226-e9d1234ca471',
        name: 'Cabin',
        ownerId: '8f075ac1-e4e7-40f7-817c-4a27424307f4',
      );
      final serverHub = Hub.server(
        id: 'ca2b97f3-0d6e-4396-8a63-c9b22ff2ee04',
        homeId: homeB.id,
        name: 'Cabin Server',
        host: '192.168.8.10',
        remoteEndpoint: const HubEndpoint(
          host: 'ca2b97f3.rhythm.lighting',
          port: 443,
          useSsl: true,
        ),
      );

      final snapshots = accountHomeServerHubsFromRowsForTesting(
        homes: [homeA, homeB],
        serverHubs: [serverHub],
      );

      expect(snapshots, hasLength(2));
      expect(snapshots.first.home.name, 'Main Home');
      expect(snapshots.first.serverHubs, isEmpty);
      expect(snapshots.last.home.name, 'Cabin');
      expect(snapshots.last.preferredServerHub?.id, serverHub.id);
      expect(snapshots.last.hasRemoteServerHub, isTrue);
    });

    test('collapses same-token tunneled and local-only account Box rows', () {
      final home = Home.create(
        id: 'd5f28205-02de-4a39-a7fc-35777e4964c7',
        name: 'Main Home',
        ownerId: '8f075ac1-e4e7-40f7-817c-4a27424307f4',
      );
      final localOnlyHub = Hub.server(
        id: '11111111-1111-4111-8111-111111111111',
        homeId: home.id,
        name: 'Kitchen Box',
        host: '192.168.5.123',
        token: 'owner-token',
        serverInstanceId: 'srv-kitchen',
        remoteEndpoint: null,
      );
      final remoteHub = Hub.server(
        id: '22222222-2222-4222-8222-222222222222',
        homeId: home.id,
        name: 'Kitchen Box',
        host: '100.64.0.12',
        token: 'owner-token',
        serverInstanceId: 'srv-kitchen',
        remoteEndpoint: const HubEndpoint(
          host: 'kitchen.devices.rhythm.lighting',
          port: 443,
          useSsl: true,
        ),
      );

      final snapshots = accountHomeServerHubsFromRowsForTesting(
        homes: [home],
        serverHubs: [localOnlyHub, remoteHub],
      );

      expect(snapshots.single.serverHubs, hasLength(1));
      final hub = snapshots.single.serverHubs.single;
      expect(hub.remoteEndpoint?.host, 'kitchen.devices.rhythm.lighting');
      expect(hub.token, 'owner-token');
      expect(hub.serverInstanceId, 'srv-kitchen');
    });

    test(
        'collapses account Box rows by server identity across endpoint changes',
        () {
      final home = Home.create(
        id: 'd5f28205-02de-4a39-a7fc-35777e4964c7',
        name: 'Main Home',
        ownerId: '8f075ac1-e4e7-40f7-817c-4a27424307f4',
      );
      final oldEndpointHub = Hub.server(
        id: '11111111-1111-4111-8111-111111111111',
        homeId: home.id,
        name: 'Kitchen Box',
        host: '192.168.5.10',
        token: 'owner-token',
        serverInstanceId: 'srv-kitchen',
      );
      final newEndpointHub = Hub.server(
        id: '22222222-2222-4222-8222-222222222222',
        homeId: home.id,
        name: 'Kitchen Box',
        host: '192.168.5.99',
        token: 'owner-token',
        serverInstanceId: 'srv-kitchen',
      );

      final snapshots = accountHomeServerHubsFromRowsForTesting(
        homes: [home],
        serverHubs: [oldEndpointHub, newEndpointHub],
      );

      expect(snapshots.single.serverHubs, hasLength(1));
      expect(
          snapshots.single.serverHubs.single.serverInstanceId, 'srv-kitchen');
    });

    test(
        'keeps known-different account Box identities separate at same endpoint',
        () {
      final home = Home.create(
        id: 'd5f28205-02de-4a39-a7fc-35777e4964c7',
        name: 'Main Home',
        ownerId: '8f075ac1-e4e7-40f7-817c-4a27424307f4',
      );
      final firstHub = Hub.server(
        id: '11111111-1111-4111-8111-111111111111',
        homeId: home.id,
        name: 'Kitchen Box',
        host: '192.168.5.123',
        token: 'first-owner-token',
        serverInstanceId: 'srv-kitchen',
      );
      final secondHub = Hub.server(
        id: '22222222-2222-4222-8222-222222222222',
        homeId: home.id,
        name: 'Garage Box',
        host: '192.168.5.123',
        token: 'second-owner-token',
        serverInstanceId: 'srv-garage',
      );

      final snapshots = accountHomeServerHubsFromRowsForTesting(
        homes: [home],
        serverHubs: [firstHub, secondHub],
      );

      expect(snapshots.single.serverHubs, hasLength(2));
    });

    test('keeps same-named account Box rows separate without shared identity',
        () {
      final home = Home.create(
        id: 'd5f28205-02de-4a39-a7fc-35777e4964c7',
        name: 'Main Home',
        ownerId: '8f075ac1-e4e7-40f7-817c-4a27424307f4',
      );
      final firstHub = Hub.server(
        id: '11111111-1111-4111-8111-111111111111',
        homeId: home.id,
        name: 'Rhythm OS',
        host: '192.168.5.123',
        token: 'first-owner-token',
        remoteEndpoint: const HubEndpoint(
          host: 'first.devices.rhythm.lighting',
          port: 443,
          useSsl: true,
        ),
      );
      final secondHub = Hub.server(
        id: '22222222-2222-4222-8222-222222222222',
        homeId: home.id,
        name: 'Rhythm OS',
        host: '192.168.5.124',
        token: 'second-owner-token',
        remoteEndpoint: const HubEndpoint(
          host: 'second.devices.rhythm.lighting',
          port: 443,
          useSsl: true,
        ),
      );

      final snapshots = accountHomeServerHubsFromRowsForTesting(
        homes: [home],
        serverHubs: [firstHub, secondHub],
      );

      expect(snapshots.single.serverHubs, hasLength(2));
      expect(
        snapshots.single.serverHubs.map((hub) => hub.id),
        [firstHub.id, secondHub.id],
      );
    });
  });
}
