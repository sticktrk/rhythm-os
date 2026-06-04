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
  });
}
