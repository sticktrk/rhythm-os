import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/monster_cloud_service.dart';

void main() {
  test('parses broker credentials and adopt params carry every field', () {
    final response = parseMonsterCloudResponse(200, {
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
    final pending =
        parseMonsterCloudResponse(409, {'error': 'device_not_on_lan'});
    expect(pending.lanPending, isTrue);
    expect(pending.hasCredentials, isFalse);
    expect(
      monsterCloudFailureMessage(pending),
      contains('has not appeared on your network'),
    );
    final expired = parseMonsterCloudResponse(400, {'error': 'invalid_ticket'});
    expect(expired.lanPending, isFalse);
    expect(monsterCloudFailureMessage(expired), contains('expired'));
    expect(
      monsterCloudFailureMessage(parseMonsterCloudResponse(502, 'not json')),
      'The Rhythm cloud could not complete Monster setup.',
    );
  });

  test('begin and complete send the broker action shape', () async {
    final bodies = <Map<String, dynamic>>[];
    final service = MonsterCloudService(
      canUseOverride: true,
      invoke: (body) async {
        bodies.add(body);
        return const MonsterCloudResponse(status: 200);
      },
    );
    await service.begin('ACFIXTURE123456');
    await service.complete('ACFIXTURE123456', 'ticket');
    await service.key('ACFIXTURE123456');
    expect(bodies, [
      {'action': 'begin', 'dsn': 'ACFIXTURE123456'},
      {'action': 'complete', 'dsn': 'ACFIXTURE123456', 'ticket': 'ticket'},
      {'action': 'key', 'dsn': 'ACFIXTURE123456'},
    ]);
    expect(service.canUse, isTrue);
  });
}
