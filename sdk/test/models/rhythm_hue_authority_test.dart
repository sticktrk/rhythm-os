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
          'topology_sync_enabled': true,
          'topology_sync_status': 'pending',
          'topology_sync_room_count': 1,
          'topology_sync_light_count': 4,
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
    expect(authority.bridges.single.topologySyncEnabled, isTrue);
    expect(authority.bridges.single.topologySyncStatus, 'pending');
    expect(authority.bridges.single.topologySyncRoomCount, 1);
    expect(authority.bridges.single.topologySyncLightCount, 4);
    expect(authority.bridges.single.rooms.single.owner,
        RhythmHueRoomAuthorityOwner.unreviewed);
    expect(
        authority.bridges.single.rooms.single.rhythmAutomationEnabled, isFalse);
  });

  test('older servers default room topology sync off', () {
    final bridge = RhythmHueBridgeAuthority.fromJson({
      'address': 'bridge.local',
      'revision': '0123456789abcdef',
      'rooms': const [],
    });

    expect(bridge.topologySyncEnabled, isFalse);
    expect(bridge.topologySyncStatus, 'disabled');
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
