import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/device_cloud_broker_service.dart';

void main() {
  test('parses broker credentials and adopt params carry every field', () {
    final response = parseDeviceCloudBrokerResponse(200, {
      'dsn': ' ACFIXTURE123456 ',
      'ip': '192.168.4.20',
      'local_key': 'synthetic-fixture-key',
      'local_key_id': 7.0,
    });
    expect(response.ok, isTrue);
    expect(response.hasCredentials, isTrue);
    expect(response.adoptParams(name: ' Desk strip '), {
      'stage': 'adopt',
      'dsn': 'ACFIXTURE123456',
      'ip': '192.168.4.20',
      'local_key': 'synthetic-fixture-key',
      'local_key_id': 7,
      'name': 'Desk strip',
    });
  });

  test('recognizes the retryable LAN-pending answer and expiry', () {
    final pending = parseDeviceCloudBrokerResponse(409, {
      'error': 'device_not_on_lan',
    });
    expect(pending.lanPending, isTrue);
    expect(pending.hasCredentials, isFalse);
    expect(
      deviceCloudBrokerFailureMessage(pending),
      contains('has not appeared on your network'),
    );
    final expired = parseDeviceCloudBrokerResponse(400, {
      'error': 'invalid_ticket',
    });
    expect(expired.lanPending, isFalse);
    expect(deviceCloudBrokerFailureMessage(expired), contains('expired'));
    expect(
      deviceCloudBrokerFailureMessage(
        parseDeviceCloudBrokerResponse(502, 'not json'),
      ),
      'The Rhythm cloud could not complete setup for this device.',
    );
    for (final response in [
      pending,
      expired,
      parseDeviceCloudBrokerResponse(502, {'error': 'monster_login'}),
    ]) {
      expect(
        deviceCloudBrokerFailureMessage(response),
        isNot(contains('Monster')),
        reason: 'guidance never names a vendor',
      );
    }
  });

  test(
    'calls the broker named by the family with the shared action shape',
    () async {
      final calls = <String>[];
      final bodies = <Map<String, dynamic>>[];
      final service = DeviceCloudBrokerService(
        canUseOverride: true,
        invoke: (functionName, body) async {
          calls.add(functionName);
          bodies.add(body);
          return const DeviceCloudBrokerResponse(status: 200);
        },
      );
      await service.begin('vendor-device', 'ACFIXTURE123456');
      await service.complete('vendor-device', 'ACFIXTURE123456', 'ticket');
      await service.key('other-vendor', 'ACFIXTURE123456');
      expect(calls, ['vendor-device', 'vendor-device', 'other-vendor']);
      expect(bodies, [
        {'action': 'begin', 'dsn': 'ACFIXTURE123456'},
        {'action': 'complete', 'dsn': 'ACFIXTURE123456', 'ticket': 'ticket'},
        {'action': 'key', 'dsn': 'ACFIXTURE123456'},
      ]);
      expect(service.canUse, isTrue);

      final rejected = await service.begin('Bad Name!', 'ACFIXTURE123456');
      expect(rejected.error, 'invalid_broker');
      expect(calls.length, 3, reason: 'unsafe function names are never sent');
    },
  );
}
