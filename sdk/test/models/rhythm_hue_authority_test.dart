import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  test('parses unreviewed Hue rooms fail closed', () {
    final authority = RhythmHueAuthority.fromJson({
      'schema_version': 1,
      'bridges': [
        {
          'address': 'bridge.local',
          'revision': '0123456789abcdef',
          'takeover_scope': 'bridge',
          'bridge_takeover_requested': false,
          'rooms': [
            {
              'room_id': 'office',
              'name': 'Office',
              'owner': 'unreviewed',
              'rhythm_automation_enabled': false,
              'future_effective_state': 'blocked',
            },
          ],
        },
      ],
    });

    expect(authority.schemaVersion, 1);
    expect(authority.bridges, hasLength(1));
    expect(authority.bridges.single.rooms.single.owner,
        RhythmHueRoomAuthorityOwner.unreviewed);
    expect(
        authority.bridges.single.rooms.single.rhythmAutomationEnabled, isFalse);
  });

  test('unknown owner values remain fail closed', () {
    final room = RhythmHueRoomAuthority.fromJson({
      'room_id': 'office',
      'name': 'Office',
      'owner': 'future_owner',
      'rhythm_automation_enabled': false,
    });

    expect(room.owner, RhythmHueRoomAuthorityOwner.unreviewed);
    expect(room.rhythmAutomationEnabled, isFalse);
  });
}
