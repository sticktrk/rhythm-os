import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_core/runner/room_state_store.dart' as room_state;
import 'package:rhythm_core/src/rust/api/dto/curve.dart';
import 'package:rhythm_core/src/rust/api/dto/runner.dart';

const _testConfig = CurveConfigDto(
  minColorTemp: 2200,
  maxColorTemp: 6500,
  minBrightness: 1,
  maxBrightness: 100,
  widthLeftBri: 1.0,
  widthRightBri: 1.0,
  widthLeftCct: 1.0,
  widthRightCct: 1.0,
  shapeP: 4.0,
  maxDimSteps: 10,
  fadeMs: 500,
  motionTimeoutSecs: 600,
);

RoomDto _room({
  String id = 'living_room',
  String name = 'Living Room',
  RoomSourceDto source = RoomSourceDto.hue,
  bool rhythmEnabled = false,
  bool disabled = false,
  bool lightsOn = false,
  double timeOffsetMinutes = 0,
  double brightnessOffset = 0,
}) {
  return RoomDto.raw(
    id: id,
    name: name,
    source: source,
    deviceIds: const [],
    rhythmEnabled: rhythmEnabled,
    disabled: disabled,
    lightsOn: lightsOn,
    timeOffsetMinutes: timeOffsetMinutes,
    brightnessOffset: brightnessOffset,
    curveConfig: null,
  );
}

void main() {
  group('room_state_store', () {
    test('emptyRunnerState starts with no rooms', () {
      final state = room_state.emptyRunnerState();
      expect(state.rooms, isEmpty);
    });

    test('addRoom replaces existing room with same id', () {
      final initial = room_state.addRoom(
        state: room_state.emptyRunnerState(),
        room: _room(name: 'Old Name'),
      );
      final updated = room_state.addRoom(
        state: initial,
        room: _room(name: 'New Name', lightsOn: true),
      );

      expect(updated.rooms, hasLength(1));
      expect(updated.rooms.single.name, 'New Name');
      expect(updated.rooms.single.lightsOn, isTrue);
    });

    test('roomsBySource filters and enabledRooms excludes disabled rooms', () {
      var state = room_state.emptyRunnerState();
      state = room_state.addRoom(
        state: state,
        room: _room(id: 'hue', source: RoomSourceDto.hue),
      );
      state = room_state.addRoom(
        state: state,
        room: _room(
          id: 'ha',
          source: RoomSourceDto.homeAssistant,
          disabled: true,
        ),
      );

      expect(
        room_state.roomsBySource(state: state, source: RoomSourceDto.hue),
        hasLength(1),
      );
      expect(room_state.enabledRooms(state: state), hasLength(1));
      expect(room_state.enabledRooms(state: state).single.id, 'hue');
    });

    test('setRoom helpers update individual fields', () {
      var state = room_state.addRoom(
        state: room_state.emptyRunnerState(),
        room: _room(),
      );

      state = room_state.setRoomDisabled(
        state: state,
        roomId: 'living_room',
        disabled: true,
      );
      state = room_state.setRoomLightsOn(
        state: state,
        roomId: 'living_room',
        lightsOn: true,
      );
      state = room_state.setRoomRhythmEnabled(
        state: state,
        roomId: 'living_room',
        rhythmEnabled: true,
      );
      state = room_state.setRoomTimeOffset(
        state: state,
        roomId: 'living_room',
        timeOffsetMinutes: 45,
      );
      state = room_state.setRoomBrightnessOffset(
        state: state,
        roomId: 'living_room',
        brightnessOffset: -10,
      );
      state = room_state.setRoomDevices(
        state: state,
        roomId: 'living_room',
        deviceIds: const ['a', 'b'],
      );

      final room = room_state.roomById(state: state, roomId: 'living_room');
      expect(room, isNotNull);
      expect(room!.disabled, isTrue);
      expect(room.lightsOn, isTrue);
      expect(room.rhythmEnabled, isTrue);
      expect(room.timeOffsetMinutes, 45);
      expect(room.brightnessOffset, -10);
      expect(room.deviceIds, ['a', 'b']);
    });

    test('onPress turns an already-on room off without curve math', () {
      final state = room_state.addRoom(
        state: room_state.emptyRunnerState(),
        room: _room(lightsOn: true),
      );

      final result = room_state.calculateRoomActionResult(
        state: state,
        config: _testConfig,
        solarNoonHour: 12,
        latitude: 35,
        dayOfYear: 172,
        currentHour: 9,
        roomId: 'living_room',
        action: RhythmActionDto.onPress,
      );

      expect(result.stateChanged, isTrue);
      expect(result.commands, hasLength(1));
      expect(result.commands.single.commandType, LightCommandType.turnOff);
      expect(
        room_state.roomById(state: result.state, roomId: 'living_room')!.lightsOn,
        isFalse,
      );
    });

    test('offPress keeps command semantics even when state is already off', () {
      final state = room_state.addRoom(
        state: room_state.emptyRunnerState(),
        room: _room(lightsOn: false),
      );

      final result = room_state.calculateRoomActionResult(
        state: state,
        config: _testConfig,
        solarNoonHour: 12,
        latitude: 35,
        dayOfYear: 172,
        currentHour: 9,
        roomId: 'living_room',
        action: RhythmActionDto.offPress,
      );

      expect(result.stateChanged, isFalse);
      expect(result.commands, hasLength(1));
      expect(result.commands.single.commandType, LightCommandType.turnOff);
    });

    test('rhythmOn and rhythmOff toggle state without commands', () {
      final state = room_state.addRoom(
        state: room_state.emptyRunnerState(),
        room: _room(rhythmEnabled: false),
      );

      final enabled = room_state.calculateRoomActionResult(
        state: state,
        config: _testConfig,
        solarNoonHour: 12,
        latitude: 35,
        dayOfYear: 172,
        currentHour: 9,
        roomId: 'living_room',
        action: RhythmActionDto.rhythmOn,
      );

      expect(enabled.stateChanged, isTrue);
      expect(enabled.commands, isEmpty);
      expect(
        room_state.roomById(state: enabled.state, roomId: 'living_room')!
            .rhythmEnabled,
        isTrue,
      );

      final disabled = room_state.calculateRoomActionResult(
        state: enabled.state,
        config: _testConfig,
        solarNoonHour: 12,
        latitude: 35,
        dayOfYear: 172,
        currentHour: 9,
        roomId: 'living_room',
        action: RhythmActionDto.rhythmOff,
      );

      expect(disabled.stateChanged, isTrue);
      expect(disabled.commands, isEmpty);
      expect(
        room_state.roomById(state: disabled.state, roomId: 'living_room')!
            .rhythmEnabled,
        isFalse,
      );
    });
  });
}
