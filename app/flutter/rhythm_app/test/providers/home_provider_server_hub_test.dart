import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/services/account_cloud_sync_service.dart';
import 'package:rhythm_core/rhythm_core.dart';

void main() {
  group('home selection', () {
    test('restores the saved Home when it still exists', () {
      final kitchen = Home.create(
        id: 'home-a',
        name: 'Kitchen',
        ownerId: 'user-1',
      );
      final cabin = Home.create(
        id: 'home-b',
        name: 'Cabin',
        ownerId: 'user-1',
      );

      expect(
        selectedHomeFromForTesting(
          [kitchen, cabin],
          selectedHomeId: cabin.id,
        )?.id,
        cabin.id,
      );
    });

    test('falls back to the first Home when the saved Home is gone', () {
      final kitchen = Home.create(
        id: 'home-a',
        name: 'Kitchen',
        ownerId: 'user-1',
      );
      final cabin = Home.create(
        id: 'home-b',
        name: 'Cabin',
        ownerId: 'user-1',
      );

      expect(
        selectedHomeFromForTesting(
          [kitchen, cabin],
          selectedHomeId: 'deleted-home',
        )?.id,
        kitchen.id,
      );
    });
  });

  group('server hub switching', () {
    test('prefers the enabled server hub by recency', () {
      final older = _serverHub(
        id: 'rpiz-a',
        host: '192.168.5.10',
        token: 'token-a',
        enabled: true,
        updatedAt: DateTime.utc(2026, 6, 1),
      );
      final newer = _serverHub(
        id: 'rpiz-b',
        host: '192.168.5.11',
        token: 'token-b',
        enabled: true,
        updatedAt: DateTime.utc(2026, 6, 2),
      );

      expect(preferredServerHubForTesting([older, newer])?.id, 'rpiz-b');
    });

    test('switching servers preserves inactive hub tokens', () {
      final now = DateTime.utc(2026, 6, 4, 18);
      final rpizA = _serverHub(
        id: 'rpiz-a',
        host: '192.168.5.10',
        token: 'token-a',
        enabled: true,
      );
      final rpizB = _serverHub(
        id: 'rpiz-b',
        host: '192.168.5.11',
        token: 'token-b',
        enabled: false,
      );

      final next = activateServerHubSnapshotForTesting(
        hubs: [rpizA, rpizB, _hueHub()],
        selectedHub: rpizB,
        now: now,
      );

      final switchedA = next.firstWhere((hub) => hub.id == 'rpiz-a');
      final switchedB = next.firstWhere((hub) => hub.id == 'rpiz-b');

      expect(next, hasLength(3));
      expect(switchedA.enabled, isFalse);
      expect(switchedA.token, 'token-a');
      expect(switchedB.enabled, isTrue);
      expect(switchedB.token, 'token-b');
      expect(switchedB.lastConnected, now);
      expect(preferredServerHubForTesting(next)?.id, 'rpiz-b');
    });

    test('cloud server hub import preserves local owner token', () {
      final local = _serverHub(
        id: 'rpiz-a',
        host: '192.168.5.10',
        token: 'local-owner-token',
        enabled: true,
        updatedAt: DateTime.utc(2026, 6, 1),
      );
      final cloud = Hub(
        id: 'rpiz-a',
        homeId: 'home-1',
        type: HubType.server,
        name: 'Remote Server',
        endpoint: const HubEndpoint(host: '192.168.5.10', port: 54448),
        remoteEndpoint: const HubEndpoint(
          host: 'rpiz-a.rhythm.lighting',
          port: 443,
          useSsl: true,
        ),
        enabled: true,
        requiresCredentials: false,
        createdAt: DateTime.utc(2026, 6, 2),
        updatedAt: DateTime.utc(2026, 6, 2),
      );

      final merged = mergeCloudServerHubForLocalStorageForTesting(
        cloudHub: cloud,
        existingHubs: [local],
      );

      expect(merged.name, 'Remote Server');
      expect(merged.token, 'local-owner-token');
      expect(merged.remoteEndpoint?.host, 'rpiz-a.rhythm.lighting');
    });

    test('cloud server hub import clears stale local remote endpoint', () {
      final local = Hub(
        id: 'rpiz-a',
        homeId: 'home-1',
        type: HubType.server,
        name: 'Local Server',
        endpoint: const HubEndpoint(host: '192.168.5.10', port: 54448),
        enabled: true,
        requiresCredentials: false,
        token: 'local-owner-token',
        remoteEndpoint: const HubEndpoint(
          host: 'rpiz-a.rhythm.lighting',
          port: 443,
          useSsl: true,
        ),
        createdAt: DateTime.utc(2026, 6, 1),
        updatedAt: DateTime.utc(2026, 6, 1),
      );
      final cloud = Hub(
        id: 'rpiz-a',
        homeId: 'home-1',
        type: HubType.server,
        name: 'Remote Server',
        endpoint: const HubEndpoint(host: '192.168.5.10', port: 54448),
        enabled: true,
        requiresCredentials: false,
        createdAt: DateTime.utc(2026, 6, 2),
        updatedAt: DateTime.utc(2026, 6, 2),
      );

      final merged = mergeCloudServerHubForLocalStorageForTesting(
        cloudHub: cloud,
        existingHubs: [local],
      );

      expect(merged.token, 'local-owner-token');
      expect(merged.remoteEndpoint, isNull);
    });

    test('cloud server hub import preserves newer local remote endpoint', () {
      final local = Hub(
        id: 'rpiz-a',
        homeId: 'home-1',
        type: HubType.server,
        name: 'Local Server',
        endpoint: const HubEndpoint(host: '192.168.5.10', port: 54448),
        remoteEndpoint: const HubEndpoint(
          host: 'rpiz-a.rhythm.lighting',
          port: 443,
          useSsl: true,
        ),
        enabled: true,
        requiresCredentials: false,
        token: 'local-owner-token',
        createdAt: DateTime.utc(2026, 6, 3),
        updatedAt: DateTime.utc(2026, 6, 3),
      );
      final cloud = Hub(
        id: 'rpiz-a',
        homeId: 'home-1',
        type: HubType.server,
        name: 'Remote Server',
        endpoint: const HubEndpoint(host: '192.168.5.10', port: 54448),
        enabled: true,
        requiresCredentials: false,
        createdAt: DateTime.utc(2026, 6, 2),
        updatedAt: DateTime.utc(2026, 6, 2),
      );

      final merged = mergeCloudServerHubForLocalStorageForTesting(
        cloudHub: cloud,
        existingHubs: [local],
      );

      expect(merged.remoteEndpoint?.host, 'rpiz-a.rhythm.lighting');
    });

    test('cloud server hub import merges by server identity when IP changed',
        () {
      final local = _serverHub(
        id: 'local-rpiz-a',
        host: '192.168.5.10',
        token: 'local-owner-token',
        enabled: true,
        serverInstanceId: 'srv-rpiz-a',
      );
      final cloud = Hub(
        id: 'cloud-rpiz-a',
        homeId: 'home-1',
        type: HubType.server,
        name: 'Remote Server',
        endpoint: const HubEndpoint(host: '192.168.5.99', port: 54448),
        enabled: true,
        requiresCredentials: false,
        token: 'cloud-owner-token',
        serverInstanceId: 'srv-rpiz-a',
        createdAt: DateTime.utc(2026, 6, 2),
        updatedAt: DateTime.utc(2026, 6, 2),
      );

      final merged = mergeCloudServerHubForLocalStorageForTesting(
        cloudHub: cloud,
        existingHubs: [local],
      );

      expect(merged.id, cloud.id);
      expect(merged.endpoint.host, '192.168.5.99');
      expect(merged.token, 'local-owner-token');
      expect(merged.serverInstanceId, 'srv-rpiz-a');
    });

    test('cloud import never mixes a tunnel with a conflicting local Box', () {
      final local = _serverHub(
        id: '8d9238cf-567f-4507-b69b-cd6fd7a0346e',
        host: '192.168.0.13',
        token: 'other-house-token',
        enabled: true,
        serverInstanceId: 'srv-db768b1130ef45e69feb13e504e65458',
      );
      final cloud = Hub.server(
        id: local.id,
        homeId: 'home-1',
        name: 'Anna Lake Box',
        host: '192.168.5.10',
        serverInstanceId: 'srv-0edeba3871eb2955198f015a01c17994',
        remoteEndpoint: const HubEndpoint(
          host: '8d9238cf-567f-4507-b69b-cd6fd7a0346e.rhythm.lighting',
          port: 443,
          useSsl: true,
        ),
      );

      final merged = mergeCloudServerHubForLocalStorageForTesting(
        cloudHub: cloud,
        existingHubs: [local],
      );

      expect(merged.endpoint, cloud.endpoint);
      expect(merged.remoteEndpoint, cloud.remoteEndpoint);
      expect(merged.serverInstanceId, cloud.serverInstanceId);
      expect(merged.token, cloud.token);
      expect(merged.token, isNot(local.token));
    });

    test('server pairing reuses existing Home by server identity', () {
      final home = Home.create(
        id: 'home-1',
        name: 'Kitchen',
        ownerId: 'user-1',
      );
      final hub = _serverHub(
        id: 'server-1',
        host: '100.64.0.12',
        token: 'owner-token',
        enabled: true,
        serverInstanceId: 'srv-db768b',
      );

      final entry = serverHomeEntryForPairingForTesting(
        homes: [
          AccountHomeServerHubs(home: home, serverHubs: [hub]),
        ],
        host: '192.168.5.99',
        port: 54448,
        token: null,
        serverInstanceId: 'srv-db768b',
        hubName: 'Rhythm OS (rhythm-server-31810e88)',
        now: DateTime.utc(2026, 7, 9, 12),
      );

      expect(entry, isNotNull);
      expect(entry!.home.id, home.id);
      expect(entry.serverHubs, hasLength(1));
      final updatedHub = entry.serverHubs.single;
      expect(updatedHub.id, hub.id);
      expect(updatedHub.endpoint.host, '192.168.5.99');
      expect(updatedHub.serverInstanceId, 'srv-db768b');
      expect(updatedHub.name, hub.name);
      expect(updatedHub.pendingSync, isTrue);
    });

    test('server pairing backfills identity on legacy same-token hub', () {
      final home = Home.create(
        id: 'home-1',
        name: 'Kitchen',
        ownerId: 'user-1',
      );
      final hub = _serverHub(
        id: 'server-1',
        host: '100.64.0.12',
        token: 'owner-token',
        enabled: true,
      );

      final entry = serverHomeEntryForPairingForTesting(
        homes: [
          AccountHomeServerHubs(home: home, serverHubs: [hub]),
        ],
        host: '192.168.5.99',
        port: 54448,
        token: 'owner-token',
        serverInstanceId: 'srv-db768b',
        hubName: 'Rhythm Box',
        now: DateTime.utc(2026, 7, 9, 12),
      );

      expect(entry, isNotNull);
      final updatedHub = entry!.serverHubs.single;
      expect(updatedHub.id, hub.id);
      expect(updatedHub.endpoint.host, '192.168.5.99');
      expect(updatedHub.token, 'owner-token');
      expect(updatedHub.serverInstanceId, 'srv-db768b');
    });

    test('server pairing does not merge known-different server identities', () {
      final home = Home.create(
        id: 'home-1',
        name: 'Kitchen',
        ownerId: 'user-1',
      );
      final hub = _serverHub(
        id: 'server-1',
        host: '192.168.5.99',
        token: 'owner-token',
        enabled: true,
        serverInstanceId: 'srv-other',
      );

      final entry = serverHomeEntryForPairingForTesting(
        homes: [
          AccountHomeServerHubs(home: home, serverHubs: [hub]),
        ],
        host: '192.168.5.99',
        port: 54448,
        token: 'owner-token',
        serverInstanceId: 'srv-db768b',
        hubName: 'Rhythm Box',
        now: DateTime.utc(2026, 7, 9, 12),
      );

      expect(entry, isNull);
    });

    test('cloud server hub import uses decrypted cloud owner token', () {
      final cloud = Hub(
        id: 'rpiz-a',
        homeId: 'home-1',
        type: HubType.server,
        name: 'Remote Server',
        endpoint: const HubEndpoint(host: '192.168.5.10', port: 54448),
        enabled: true,
        requiresCredentials: false,
        token: 'cloud-owner-token',
        createdAt: DateTime.utc(2026, 6, 2),
        updatedAt: DateTime.utc(2026, 6, 2),
      );

      final merged = mergeCloudServerHubForLocalStorageForTesting(
        cloudHub: cloud,
        existingHubs: const [],
      );

      expect(merged.token, 'cloud-owner-token');
    });

    test(
        'cloud server hub import is scoped when the id belongs to another Home',
        () {
      final otherHomeHub = Hub.server(
        id: 'ca2b97f3-0d6e-4396-8a63-c9b22ff2ee04',
        homeId: 'home-a',
        name: 'Kitchen Server',
        host: '192.168.5.10',
      );
      final cloudHub = Hub.server(
        id: otherHomeHub.id,
        homeId: 'home-b',
        name: 'Cabin Server',
        host: '192.168.8.10',
      );

      final scoped = accountHomeServerHubForLocalStorageForTesting(
        cloudHub: cloudHub,
        homeId: 'home-b',
        existingHubs: [otherHomeHub],
      );

      expect(scoped.homeId, 'home-b');
      expect(scoped.id, isNot(cloudHub.id));
      expect(
        scoped.id,
        matches(
          RegExp(
            r'^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$',
          ),
        ),
      );

      final scopedAgain = accountHomeServerHubForLocalStorageForTesting(
        cloudHub: cloudHub,
        homeId: 'home-b',
        existingHubs: [otherHomeHub, scoped],
      );
      expect(scopedAgain.id, scoped.id);
    });
  });
}

Hub _serverHub({
  required String id,
  required String host,
  required String token,
  required bool enabled,
  DateTime? updatedAt,
  String? serverInstanceId,
}) {
  final timestamp = updatedAt ?? DateTime.utc(2026, 6, 1);
  return Hub(
    id: id,
    homeId: 'home-1',
    type: HubType.server,
    name: id,
    endpoint: HubEndpoint(host: host, port: 54448),
    enabled: enabled,
    requiresCredentials: false,
    token: token,
    serverInstanceId: serverInstanceId,
    createdAt: timestamp,
    updatedAt: timestamp,
  );
}

Hub _hueHub() {
  final timestamp = DateTime.utc(2026, 6, 1);
  return Hub(
    id: 'hue',
    homeId: 'home-1',
    type: HubType.hue,
    name: 'Hue',
    endpoint: const HubEndpoint(host: '192.168.5.221', port: 443, useSsl: true),
    enabled: true,
    requiresCredentials: true,
    token: 'hue-token',
    createdAt: timestamp,
    updatedAt: timestamp,
  );
}
