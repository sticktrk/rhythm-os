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

    test('keeps raw hard-off intent but displays on when observed power is on',
        () async {
      await provider.addRoom(
        const RoomDto(
          id: 'room-1',
          name: 'Matter Lamp',
          source: RoomSourceDto.matter,
          deviceIds: ['light-1'],
          rhythmEnabled: true,
          disabled: false,
          lightsOn: false,
          timeOffsetMinutes: 0,
          brightnessOffset: 0,
        ),
      );

      await provider.applyServerNodeState(
        'room-1',
        rhythmEnabled: true,
        timeOffset: 0,
        brightnessOffset: 0,
        state: RoomModeState.hardOff,
        lightsOn: true,
      );

      expect(provider.getRoomState('room-1'), RoomModeState.hardOff);
      expect(provider.getDisplayRoomState('room-1'), RoomModeState.active);
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

    test('preserves mood display when lights are actually on', () async {
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
        state: RoomModeState.mood,
        lightsOn: true,
      );

      expect(provider.getDisplayRoomState('room-1'), RoomModeState.mood);
    });

    test('remembers mood color after active Kelvin state clears direct color',
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
        state: RoomModeState.mood,
        lightsOn: true,
        color: (20, 80, 240),
        moodActive: true,
      );

      expect(provider.getRoomColor('room-1'), (20, 80, 240));
      expect(provider.getMoodColor('room-1'), (20, 80, 240));

      await provider.applyServerNodeState(
        'room-1',
        rhythmEnabled: true,
        timeOffset: 0,
        brightnessOffset: 0,
        state: RoomModeState.active,
        lightsOn: true,
        kelvin: 3200,
        moodActive: false,
      );

      expect(provider.getRoomColor('room-1'), isNull);
      expect(provider.getMoodColor('room-1'), (20, 80, 240));
    });

    test('remembers locally picked mood color before server echo', () async {
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

      provider.setRoomColorLocal(
        'room-1',
        245,
        120,
        40,
        rememberAsMood: true,
      );

      expect(provider.getRoomColor('room-1'), (245, 120, 40));
      expect(provider.getMoodColor('room-1'), (245, 120, 40));

      await provider.applyServerNodeState(
        'room-1',
        rhythmEnabled: true,
        timeOffset: 0,
        brightnessOffset: 0,
        state: RoomModeState.active,
        lightsOn: true,
        kelvin: 4000,
      );

      expect(provider.getRoomColor('room-1'), isNull);
      expect(provider.getMoodColor('room-1'), (245, 120, 40));
    });

    test('local power toggles do not overwrite semantic mood state', () async {
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
        state: RoomModeState.mood,
        lightsOn: true,
      );

      await provider.setRoomLightsOnLocal('room-1', false);

      expect(provider.getRoomState('room-1'), RoomModeState.mood);
      expect(provider.getDisplayRoomState('room-1'), RoomModeState.hardOff);
    });

    test('keeps acknowledged on command through stale lock expiry', () {
      fakeAsync((async) {
        unawaited(provider.addRoom(
          const RoomDto(
            id: 'room-1',
            name: 'Kitchen',
            source: RoomSourceDto.matter,
            deviceIds: ['light-1'],
            rhythmEnabled: true,
            disabled: false,
            lightsOn: false,
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
          state: RoomModeState.hardOff,
          lightsOn: false,
        ));
        async.flushMicrotasks();
        expect(provider.getDisplayRoomState('room-1'), RoomModeState.hardOff);

        unawaited(provider.setRoomLightsOnLocal('room-1', true));
        provider.setRoomStateLocal('room-1', RoomModeState.active);
        async.flushMicrotasks();
        provider.acknowledgeOptimisticNodeState(
          'room-1',
          state: RoomModeState.active,
          lightsOn: true,
        );

        unawaited(provider.applyServerNodeState(
          'room-1',
          rhythmEnabled: true,
          timeOffset: 0,
          brightnessOffset: 0,
          state: RoomModeState.hardOff,
          lightsOn: false,
        ));
        async.flushMicrotasks();
        expect(provider.getRoomState('room-1'), RoomModeState.active);
        expect(provider.getDisplayRoomState('room-1'), RoomModeState.active);

        async.elapse(const Duration(seconds: 4));
        expect(provider.getRoomState('room-1'), RoomModeState.active);
        expect(provider.getDisplayRoomState('room-1'), RoomModeState.active);
      });
    });

    test('keeps acknowledged state while dispatch remains pending', () {
      fakeAsync((async) {
        unawaited(provider.addRoom(
          const RoomDto(
            id: 'room-1',
            name: 'Matter',
            source: RoomSourceDto.matter,
            deviceIds: ['matter-104', 'matter-106', 'matter-107'],
            rhythmEnabled: true,
            disabled: false,
            lightsOn: false,
            timeOffsetMinutes: 0,
            brightnessOffset: 0,
          ),
        ));
        async.flushMicrotasks();

        unawaited(provider.setRoomLightsOnLocal('room-1', true));
        provider.setRoomStateLocal('room-1', RoomModeState.active);
        provider.acknowledgeOptimisticNodeState(
          'room-1',
          state: RoomModeState.active,
          lightsOn: true,
        );
        async.flushMicrotasks();

        async.elapse(const Duration(seconds: 4));
        unawaited(provider.applyServerNodeState(
          'room-1',
          rhythmEnabled: true,
          timeOffset: 0,
          brightnessOffset: 0,
          state: RoomModeState.active,
          pendingDispatch: true,
          lightsOn: false,
        ));
        async.flushMicrotasks();

        expect(provider.isNodeDispatchPending('room-1'), isTrue);
        expect(provider.getRoomState('room-1'), RoomModeState.active);
        expect(provider.getDisplayRoomState('room-1'), RoomModeState.active);

        unawaited(provider.applyServerNodeState(
          'room-1',
          rhythmEnabled: true,
          timeOffset: 0,
          brightnessOffset: 0,
          state: RoomModeState.hardOff,
          pendingDispatch: false,
          lightsOn: false,
        ));
        async.flushMicrotasks();

        expect(provider.isNodeDispatchPending('room-1'), isFalse);
        expect(provider.getDisplayRoomState('room-1'), RoomModeState.hardOff);
      });
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
