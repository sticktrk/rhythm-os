import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/utils/room_visibility.dart';
import 'package:rhythm_core/rhythm_core.dart';

void main() {
  group('showsInAllRooms', () {
    test('includes topology rooms', () {
      const room = RoomDto(
        id: 'room-1',
        name: 'Living Room',
        source: RoomSourceDto.hue,
        deviceIds: ['device-1'],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: true,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      );

      expect(showsInAllRooms(room), isTrue);
    });

    test('includes standalone bulbs with no room', () {
      const bulb = RoomDto(
        id: 'bulb-standalone',
        name: 'Porch Bulb',
        source: RoomSourceDto.hue,
        kind: RoomNodeKind.lightDevice,
        deviceIds: ['bulb-standalone'],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: true,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      );

      expect(showsInAllRooms(bulb), isTrue);
    });

    test('excludes child bulbs assigned to a room', () {
      const bulb = RoomDto(
        id: 'bulb-child',
        name: 'Lamp Bulb',
        source: RoomSourceDto.hue,
        kind: RoomNodeKind.lightDevice,
        parentId: 'room-1',
        deviceIds: ['bulb-child'],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: true,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      );

      expect(showsInAllRooms(bulb), isFalse);
    });

    test('excludes non-light device nodes', () {
      const button = RoomDto(
        id: 'button-1',
        name: 'Wall Button',
        source: RoomSourceDto.hue,
        kind: RoomNodeKind.button,
        deviceIds: ['button-1'],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: false,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      );

      expect(showsInAllRooms(button), isFalse);
    });
  });
}
