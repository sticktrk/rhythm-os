import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_core/rhythm_core.dart';

void main() {
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
