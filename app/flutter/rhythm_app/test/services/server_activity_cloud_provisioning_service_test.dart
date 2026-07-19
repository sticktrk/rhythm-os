import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/server_activity_cloud_provisioning_service.dart';
import 'package:rhythm_core/rhythm_core.dart';

void main() {
  test('activity bootstrap never downgrades a known durable identity', () {
    final hub = Hub.server(
      id: 'hub-1',
      homeId: 'home-1',
      name: 'Kitchen Box',
      host: '192.168.5.123',
      serverInstanceId: 'srv-known',
    );

    final body = ServerActivityCloudProvisioningService.buildBootstrapBody(
      serverHub: hub,
      home: null,
      serverInstanceId: 'endpoint:http://192.168.5.123:54448',
    );

    expect(body['server_instance_id'], 'srv-known');
  });
}
