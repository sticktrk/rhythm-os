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

  test('durable Box identity coalesces provisioning across hub aliases', () {
    final firstAlias = Hub.server(
      id: 'local-hub',
      homeId: 'local-home',
      name: 'Kitchen Box',
      host: '192.168.5.123',
    );
    final cloudAlias = Hub.server(
      id: 'cloud-hub',
      homeId: 'cloud-home',
      name: 'Kitchen Box',
      host: 'box.devices.rhythm.lighting',
      serverInstanceId: 'srv-same-box',
    );

    final localKey = ServerActivityCloudProvisioningService.provisioningKey(
      serverHub: firstAlias,
      serverInstanceId: 'srv-same-box',
    );
    final cloudKey = ServerActivityCloudProvisioningService.provisioningKey(
      serverHub: cloudAlias,
      serverInstanceId: 'SRV-SAME-BOX',
    );

    expect(localKey, cloudKey);
  });

  test('healthy durable credentials survive Home and hub alias changes', () {
    final localAlias = Hub.server(
      id: 'local-hub',
      homeId: 'local-home',
      name: 'Kitchen Box',
      host: '192.168.5.123',
      serverInstanceId: 'srv-same-box',
    );
    final status = <String, dynamic>{
      'configured': true,
      'needs_reprovision': false,
      'upload_status': 'ok',
      'hub_id': 'canonical-cloud-hub',
      'home_id': 'canonical-cloud-home',
      'server_instance_id': 'SRV-SAME-BOX',
    };

    expect(
      ServerActivityCloudProvisioningService.statusMatchesHub(
        status,
        localAlias,
        expectedServerInstanceId: 'srv-same-box',
      ),
      isTrue,
    );
  });

  test('auth failure and a different durable Box still re-provision', () {
    final hub = Hub.server(
      id: 'hub-1',
      homeId: 'home-1',
      name: 'Kitchen Box',
      host: '192.168.5.123',
      serverInstanceId: 'srv-expected',
    );
    final status = <String, dynamic>{
      'configured': true,
      'needs_reprovision': true,
      'upload_status': 'auth_failed',
      'hub_id': 'hub-1',
      'home_id': 'home-1',
      'server_instance_id': 'srv-expected',
    };

    expect(
      ServerActivityCloudProvisioningService.statusMatchesHub(
        status,
        hub,
      ),
      isFalse,
    );

    status
      ..['needs_reprovision'] = false
      ..['upload_status'] = 'ok'
      ..['server_instance_id'] = 'srv-another-box';
    expect(
      ServerActivityCloudProvisioningService.statusMatchesHub(
        status,
        hub,
      ),
      isFalse,
    );
  });
}
