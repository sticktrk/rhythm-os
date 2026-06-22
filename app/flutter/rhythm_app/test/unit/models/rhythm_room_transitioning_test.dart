import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

void main() {
  group('Rhythm room transitioning parsing', () {
    test('RhythmRoom defaults transitioning to false when omitted', () {
      final room = RhythmRoom.fromJson({
        'id': 'living_room',
        'name': 'Living Room',
        'grouped_light_id': 'group-1',
        'state': 'active',
        'rhythm_enabled': true,
        'disabled': false,
        'time_offset': 0,
        'brightness_offset': 0,
      });

      expect(room.transitioning, isFalse);
      expect(room.pendingDispatch, isFalse);
      expect(room.state, RoomModeState.active);
    });

    test('RhythmRoomState parses explicit transitioning flag', () {
      final state = RhythmRoomState.fromJson({
        'id': 'living_room',
        'state': 'active',
        'transitioning': true,
        'pending_dispatch': true,
        'rhythm_enabled': true,
        'time_offset': 0,
        'brightness_offset': 0,
        'lights_on': true,
        'brightness': 62,
        'kelvin': 3200,
      });

      expect(state.roomId, equals('living_room'));
      expect(state.transitioning, isTrue);
      expect(state.pendingDispatch, isTrue);
      expect(state.state, RoomModeState.active);
    });
  });
}
