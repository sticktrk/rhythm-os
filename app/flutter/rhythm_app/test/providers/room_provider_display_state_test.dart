import 'dart:async';

import 'package:fake_async/fake_async.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/providers/room_provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' show RoomModeState;

void main() {
  group('RoomProvider display state', () {
    late RoomProvider provider;

    setUp(() {
      provider = RoomProvider();
    });

    tearDown(() {
      provider.dispose();
    });

    test('keeps raw active state but displays hard-off when lights are off',
        () async {
      await provider.addRoom(
        const RoomDto(
          id: 'room-1',
          name: 'Kitchen',
          source: RoomSourceDto.hue,
          deviceIds: ['light-1'],
          rhythmEnabled: true,
          disabled: false,
          lightsOn: true,
          timeOffsetMinutes: 0,
          brightnessOffset: 0,
        ),
      );

      await provider.applyServerNodeState(
        'room-1',
        rhythmEnabled: true,
        timeOffset: 0,
        brightnessOffset: 0,
        state: RoomModeState.active,
        lightsOn: false,
      );

      expect(provider.getRoomState('room-1'), RoomModeState.active);
      expect(provider.getDisplayRoomState('room-1'), RoomModeState.hardOff);
    });

    test('defaults raw state to active when only power state is known',
        () async {
      await provider.addRoom(
        const RoomDto(
          id: 'room-1',
          name: 'Kitchen',
          source: RoomSourceDto.hue,
          deviceIds: ['light-1'],
          rhythmEnabled: true,
          disabled: false,
          lightsOn: false,
          timeOffsetMinutes: 0,
          brightnessOffset: 0,
        ),
      );

      expect(provider.getRoomState('room-1'), RoomModeState.active);
      expect(provider.getDisplayRoomState('room-1'), RoomModeState.hardOff);
    });

    test('preserves idle display when lights are actually on', () async {
      await provider.addRoom(
        const RoomDto(
          id: 'room-1',
          name: 'Kitchen',
          source: RoomSourceDto.hue,
          deviceIds: ['light-1'],
          rhythmEnabled: true,
          disabled: false,
          lightsOn: true,
          timeOffsetMinutes: 0,
          brightnessOffset: 0,
        ),
      );

      await provider.applyServerNodeState(
        'room-1',
        rhythmEnabled: true,
        timeOffset: 0,
        brightnessOffset: 0,
        state: RoomModeState.idle,
        lightsOn: true,
      );

      expect(provider.getDisplayRoomState('room-1'), RoomModeState.idle);
    });

    test('local power toggles do not overwrite semantic idle state', () async {
      await provider.addRoom(
        const RoomDto(
          id: 'room-1',
          name: 'Kitchen',
          source: RoomSourceDto.hue,
          deviceIds: ['light-1'],
          rhythmEnabled: true,
          disabled: false,
          lightsOn: true,
          timeOffsetMinutes: 0,
          brightnessOffset: 0,
        ),
      );

      await provider.applyServerNodeState(
        'room-1',
        rhythmEnabled: true,
        timeOffset: 0,
        brightnessOffset: 0,
        state: RoomModeState.idle,
        lightsOn: true,
      );

      await provider.setRoomLightsOnLocal('room-1', false);

      expect(provider.getRoomState('room-1'), RoomModeState.idle);
      expect(provider.getDisplayRoomState('room-1'), RoomModeState.hardOff);
    });

    test('clears stale transitioning flag after defensive timeout', () {
      fakeAsync((async) {
        unawaited(provider.addRoom(
          const RoomDto(
            id: 'room-1',
            name: 'Kitchen',
            source: RoomSourceDto.hue,
            deviceIds: ['light-1'],
            rhythmEnabled: true,
            disabled: false,
            lightsOn: true,
            timeOffsetMinutes: 0,
            brightnessOffset: 0,
          ),
        ));
        async.flushMicrotasks();

        unawaited(provider.applyServerNodeState(
          'room-1',
          rhythmEnabled: true,
          timeOffset: 0,
          brightnessOffset: 0,
          state: RoomModeState.active,
          transitioning: true,
          lightsOn: true,
        ));
        async.flushMicrotasks();

        expect(provider.isRoomTransitioning('room-1'), isTrue);

        async.elapse(const Duration(seconds: 14));
        expect(provider.isRoomTransitioning('room-1'), isTrue);

        async.elapse(const Duration(seconds: 1));
        expect(provider.isRoomTransitioning('room-1'), isFalse);
      });
    });
  });
}
