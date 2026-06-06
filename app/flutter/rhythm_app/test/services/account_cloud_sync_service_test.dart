import 'package:flutter_test/flutter_test.dart';
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

    test('server hub payload omits local owner token', () {
      final hub = Hub.server(
        id: 'ca2b97f3-0d6e-4396-8a63-c9b22ff2ee04',
        homeId: 'd5f28205-02de-4a39-a7fc-35777e4964c7',
        name: 'Kitchen Server',
        host: '192.168.5.123',
        token: 'owner-token',
        remoteEndpoint: const HubEndpoint(
          host: 'hub.devices.rhythm.lighting',
          port: 443,
          useSsl: true,
        ),
      );

      final payload = AccountCloudSyncService.serverHubSnapshotPayload(hub);

      expect(payload['type'], 'server');
      expect(payload['token'], isNull);
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
  });
}
