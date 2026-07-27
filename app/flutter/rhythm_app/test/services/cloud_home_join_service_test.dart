import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/cloud_home_join_service.dart';
import 'package:rhythm_core/rhythm_core.dart';

void main() {
  test('local identity unavailable explains that LAN verification is required',
      () {
    const error = CloudHomeJoinBlockedException(
      code: 'local_identity_unavailable',
    );

    expect(error.userMessage, contains('same local network'));
    expect(error.userMessage, contains('before opening it'));
  });

  group('CloudHomeJoinService', () {
    test('parses joined cloud Home using LAN endpoint and owner token', () {
      final entry = joinedHomeFromFunctionResponseForTesting(
        {
          'home': _homeRow(),
          'server_hub': _hubRow(
            endpoint: {'host': '100.64.0.12', 'port': 54448},
            remoteEndpoint: {
              'host': 'srv-db768b.devices.rhythm.lighting',
              'port': 443,
              'useSsl': true,
            },
          ),
        },
        ownerToken: 'owner-token',
        lanEndpoint: const HubEndpoint(host: '192.168.5.99', port: 54448),
        hubName: 'Rhythm Box',
        serverInstanceId: 'srv-db768b',
      );

      expect(entry, isNotNull);
      expect(entry!.home.id, 'cloud-home');
      expect(entry.home.memberIds, contains('joining-user'));
      expect(entry.serverHubs, hasLength(1));

      final hub = entry.serverHubs.single;
      expect(hub.id, 'cloud-hub');
      expect(hub.homeId, 'cloud-home');
      expect(hub.endpoint.host, '192.168.5.99');
      expect(hub.remoteEndpoint?.host, 'srv-db768b.devices.rhythm.lighting');
      expect(hub.token, 'owner-token');
      expect(hub.serverInstanceId, 'srv-db768b');
      expect(hub.pendingSync, isTrue);
    });

    test('returns null for malformed function response', () {
      expect(
        joinedHomeFromFunctionResponseForTesting(
          {'status': 'ok'},
          ownerToken: 'owner-token',
          lanEndpoint: const HubEndpoint(host: '192.168.5.99', port: 54448),
          hubName: 'Rhythm Box',
          serverInstanceId: 'srv-db768b',
        ),
        isNull,
      );
    });

    test('identity conflicts block Home creation', () {
      final result = blockedCloudHomeJoinResultForTesting({
        'code': 'identity_conflict',
        'error': 'Stored durable identity differs',
      });

      expect(result.disposition, CloudHomeJoinDisposition.blocked);
      expect(result.canCreateHome, isFalse);
      expect(result.code, 'identity_conflict');
      expect(
        CloudHomeJoinBlockedException(code: result.code).userMessage,
        contains('new Home'),
      );
    });

    test('missing device bindings permit a new canonical Home', () {
      final result = cloudHomeJoinFailureResultForTesting({
        'code': 'device_binding_missing',
        'error': 'The Box cloud binding no longer exists',
      });

      expect(result.disposition, CloudHomeJoinDisposition.notAttempted);
      expect(result.canCreateHome, isTrue);
    });

    test('only a join that was not attempted permits Home creation', () {
      const notAttempted = CloudHomeJoinResult.notAttempted();
      const blocked = CloudHomeJoinResult.blocked(code: 'unavailable');

      expect(notAttempted.canCreateHome, isTrue);
      expect(blocked.canCreateHome, isFalse);
    });
  });
}

Map<String, dynamic> _homeRow() {
  return {
    'id': 'cloud-home',
    'name': 'Household',
    'owner_id': 'owner-user',
    'member_ids': ['owner-user', 'joining-user'],
    'sleep_schedule': {'bedtime': 22.0, 'wakeTime': 6.5, 'enabled': true},
    'created_at': '2026-07-01T12:00:00.000Z',
    'updated_at': '2026-07-09T12:00:00.000Z',
  };
}

Map<String, dynamic> _hubRow({
  required Map<String, dynamic> endpoint,
  Map<String, dynamic>? remoteEndpoint,
}) {
  return {
    'id': 'cloud-hub',
    'home_id': 'cloud-home',
    'type': 'server',
    'name': 'Kitchen Box',
    'endpoint': endpoint,
    if (remoteEndpoint != null) 'remote_endpoint': remoteEndpoint,
    'enabled': true,
    'server_instance_id': 'srv-db768b',
    'created_at': '2026-07-01T12:00:00.000Z',
    'updated_at': '2026-07-09T12:00:00.000Z',
  };
}
