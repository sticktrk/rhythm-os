import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/widgets/mode_room_behavior_section.dart';

void main() {
  group('profile room default state mapping', () {
    test('presents standby compatibility states as Low glow', () {
      expect(roomDefaultStateLabelForTesting('standby'), 'Low glow');
      expect(roomDefaultStateLabelForTesting('idle'), 'Low glow');
      expect(roomDefaultStateLabelForTesting('soft_off'), 'Low glow');
      expect(roomDefaultStateLabelForTesting('hard_off'), 'Off');
    });

    test('cycles active defaults through standby before off', () {
      expect(nextRoomDefaultStateForTesting(null), 'active');
      expect(nextRoomDefaultStateForTesting('active'), 'standby');
      expect(nextRoomDefaultStateForTesting('standby'), 'hard_off');
      expect(nextRoomDefaultStateForTesting('hard_off'), isNull);
    });
  });
}
