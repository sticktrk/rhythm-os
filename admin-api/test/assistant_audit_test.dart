import 'dart:convert';
import 'package:rhythm_admin_api/src/assistant_audit.dart';
import 'package:test/test.dart';

void main() {
  test(
      'staff audit binds the recovered scope to a hub and records verification without private fields',
      () async {
    Future<Map<String, dynamic>> audit(String hub, Object args) =>
        assistantAuditReceipt(
            hubId: hub,
            secret: 'fixture-key',
            arguments: args,
            receipt: {
              'status': 'partially_applied',
              'canonical_readback_verified': false,
              'sensor_id': 'private sensor',
              'after_targets': ['private room B', 'private room A'],
              'name': 'private label',
              'lights': [
                {'power_on': true}
              ],
              'token': 'private token'
            });
    final record = await audit('hub-a', {'room_id': 'private room A'});
    expect(record['outcome'], 'partially_applied');
    expect(record['canonical_readback_verified'], false);
    expect(record['scope']['after_targets']['count'], 2);
    expect(jsonEncode(record), isNot(contains('private')));
    expect(jsonEncode(record), isNot(contains('power_on')));
    expect((await audit('hub-b', {}))['scope']['sensor_id'],
        isNot(record['scope']['sensor_id']));
  });
}
