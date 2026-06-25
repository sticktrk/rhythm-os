import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_core/rhythm_core.dart';

void main() {
  test('Hub JSON preserves remote endpoint', () {
    final hub = Hub.server(
      id: 'hub-1',
      homeId: 'home-1',
      name: 'Rhythm',
      host: '192.168.1.10',
      token: 'owner-token',
      serverInstanceId: 'srv-kitchen',
      remoteEndpoint: const HubEndpoint(
        host: 'hub.devices.rhythm.lighting',
        port: 443,
        useSsl: true,
      ),
    );

    final decoded = Hub.fromJson(hub.toJson());

    expect(decoded.remoteEndpoint?.host, 'hub.devices.rhythm.lighting');
    expect(decoded.remoteEndpoint?.port, 443);
    expect(decoded.remoteEndpoint?.useSsl, isTrue);
    expect(decoded.serverInstanceId, 'srv-kitchen');
  });

  test('Hub Supabase conversion preserves remote endpoint', () {
    final row = {
      'id': 'hub-1',
      'home_id': 'home-1',
      'type': 'server',
      'name': 'Rhythm',
      'endpoint': {
        'host': '192.168.1.10',
        'port': 54448,
        'useSsl': false,
      },
      'remote_endpoint': {
        'host': 'hub.devices.rhythm.lighting',
        'port': 443,
        'useSsl': true,
      },
      'enabled': true,
      'token': 'owner-token',
      'server_instance_id': 'srv-kitchen',
      'created_at': DateTime.utc(2026).toIso8601String(),
      'updated_at': DateTime.utc(2026).toIso8601String(),
    };

    final hub = Hub.fromSupabase(row);
    final encoded = hub.toSupabase();

    expect(hub.remoteEndpoint?.host, 'hub.devices.rhythm.lighting');
    expect(hub.serverInstanceId, 'srv-kitchen');
    expect(encoded['remote_endpoint'], {
      'host': 'hub.devices.rhythm.lighting',
      'port': 443,
      'useSsl': true,
    });
    expect(encoded['server_instance_id'], 'srv-kitchen');
  });
}
