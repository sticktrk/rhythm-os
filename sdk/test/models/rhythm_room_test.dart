import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  group('RoomModeState', () {
    test('emits mood while accepting legacy idle and standby values', () {
      expect(RoomModeState.idle.wireValue, 'mood');
      expect(RoomModeState.fromString('mood'), RoomModeState.idle);
      expect(RoomModeState.fromString('idle'), RoomModeState.idle);
      expect(RoomModeState.fromString('standby'), RoomModeState.idle);
    });
  });

  group('RhythmDeviceType', () {
    group('fromString', () {
      test('parses "light" to light', () {
        expect(RhythmDeviceType.fromString('light'), RhythmDeviceType.light);
      });

      test('parses "motion" to motion', () {
        expect(RhythmDeviceType.fromString('motion'), RhythmDeviceType.motion);
      });

      test('parses "button" to button', () {
        expect(RhythmDeviceType.fromString('button'), RhythmDeviceType.button);
      });

      test('falls back to button for unknown strings', () {
        expect(RhythmDeviceType.fromString('switch'), RhythmDeviceType.button);
        expect(RhythmDeviceType.fromString('unknown'), RhythmDeviceType.button);
        expect(RhythmDeviceType.fromString(''), RhythmDeviceType.button);
      });
    });
  });

  group('RhythmDevice', () {
    group('fromJson', () {
      test('parses all fields', () {
        final device = RhythmDevice.fromJson({
          'id': 'device-abc-123',
          'type': 'light',
          'name': 'Living Room Light',
          'manufacturer': 'Philips',
          'model': 'Hue White',
        });
        expect(device.id, 'device-abc-123');
        expect(device.type, RhythmDeviceType.light);
        expect(device.name, 'Living Room Light');
        expect(device.manufacturer, 'Philips');
        expect(device.model, 'Hue White');
      });

      test('defaults id to empty string when missing', () {
        final device = RhythmDevice.fromJson({'type': 'light'});
        expect(device.id, '');
      });

      test('defaults type via fromString when missing', () {
        final device = RhythmDevice.fromJson({'id': 'abc'});
        expect(device.type, RhythmDeviceType.button);
      });

      test('name, manufacturer, and model are optional', () {
        final device = RhythmDevice.fromJson({
          'id': 'device-1',
          'type': 'motion',
        });
        expect(device.name, isNull);
        expect(device.manufacturer, isNull);
        expect(device.model, isNull);
      });
    });

    group('displayName', () {
      test('returns name when name is present', () {
        final device = RhythmDevice.fromJson({
          'id': 'device-abc-123',
          'type': 'light',
          'name': 'Kitchen Spot',
        });
        expect(device.displayName, 'Kitchen Spot');
      });

      test('returns first 8 chars of id when name is absent', () {
        final device = RhythmDevice.fromJson({
          'id': 'abcdefghijklmnop',
          'type': 'light',
        });
        expect(device.displayName, 'abcdefgh');
      });

      test('returns full id when id is shorter than 8 chars', () {
        final device = RhythmDevice.fromJson({
          'id': 'abc',
          'type': 'light',
        });
        expect(device.displayName, 'abc');
      });

      test('returns empty string when id is empty and name is absent', () {
        final device = RhythmDevice.fromJson({});
        expect(device.displayName, '');
      });
    });

    group('productInfo', () {
      test('returns "Manufacturer · Model" when both present', () {
        final device = RhythmDevice.fromJson({
          'id': 'dev1',
          'type': 'light',
          'manufacturer': 'Philips',
          'model': 'Hue A19',
        });
        expect(device.productInfo, 'Philips \u00b7 Hue A19');
      });

      test('returns manufacturer only when model is absent', () {
        final device = RhythmDevice.fromJson({
          'id': 'dev1',
          'type': 'light',
          'manufacturer': 'Philips',
        });
        expect(device.productInfo, 'Philips');
      });

      test('returns model only when manufacturer is absent', () {
        final device = RhythmDevice.fromJson({
          'id': 'dev1',
          'type': 'light',
          'model': 'Hue A19',
        });
        expect(device.productInfo, 'Hue A19');
      });

      test('returns null when both manufacturer and model are absent', () {
        final device = RhythmDevice.fromJson({
          'id': 'dev1',
          'type': 'light',
        });
        expect(device.productInfo, isNull);
      });
    });
  });

  group('RhythmRoom', () {
    group('fromJson', () {
      test('parses all fields with defaults', () {
        final room = RhythmRoom.fromJson({});
        expect(room.id, '');
        expect(room.name, '');
        expect(room.groupedLightId, '');
        expect(room.rhythmEnabled, false);
        expect(room.disabled, false);
        expect(room.timeOffset, 0.0);
        expect(room.brightnessOffset, 0.0);
        expect(room.softOff, false);
        expect(room.hubType, isNull);
        expect(room.deviceIds, isEmpty);
        expect(room.devices, isEmpty);
        expect(room.lightsOn, isNull);
        expect(room.brightness, isNull);
        expect(room.kelvin, isNull);
      });

      test('parses all fields from populated JSON', () {
        final room = RhythmRoom.fromJson({
          'id': 'room-1',
          'name': 'Living Room',
          'grouped_light_id': 'gl-1',
          'rhythm_enabled': true,
          'disabled': false,
          'time_offset': 1.5,
          'brightness_offset': -10.0,
          'soft_off': true,
          'hub_type': 'hue',
          'device_ids': ['d1', 'd2', 'd3'],
          'devices': [
            {'id': 'd1', 'type': 'light', 'name': 'Lamp 1'},
            {'id': 'd2', 'type': 'light', 'name': 'Lamp 2'},
            {'id': 'd3', 'type': 'motion'},
          ],
          'lights_on': true,
          'brightness': 80,
          'kelvin': 4000,
          'mood_enabled': true,
          'mood_active': true,
          'profile_settings': {
            'mood_enabled': true,
            'mood_profile_id': 'room-1_mood',
            'motion_timeout_secs': 123,
          },
        });
        expect(room.id, 'room-1');
        expect(room.name, 'Living Room');
        expect(room.groupedLightId, 'gl-1');
        expect(room.rhythmEnabled, true);
        expect(room.disabled, false);
        expect(room.timeOffset, 1.5);
        expect(room.brightnessOffset, -10.0);
        expect(room.softOff, true);
        expect(room.hubType, 'hue');
        expect(room.deviceIds, ['d1', 'd2', 'd3']);
        expect(room.devices.length, 3);
        expect(room.lightsOn, true);
        expect(room.brightness, 80);
        expect(room.kelvin, 4000);
        expect(room.moodEnabled, isTrue);
        expect(room.moodActive, isTrue);
        expect(room.profileSettings?.moodEnabled, isTrue);
        expect(room.profileSettings?.moodProfileId, 'room-1_mood');
        expect(room.profileSettings?.motionTimeoutSecs, 123);
      });

      test('parses typed profile settings fields', () {
        final room = RhythmRoom.fromJson({
          'profile_settings': {
            'profile_id': 'sleep',
            'mood_enabled': false,
            'mood_profile_id': 'sleep_mood',
            'fade_ms': {'mode': 'fixed', 'value': 1200},
            'motion_timeout_secs': {'mode': 'fixed', 'value': 300},
          },
        });

        expect(room.profileSettings?.profileId, 'sleep');
        expect(room.profileSettings?.moodEnabled, isFalse);
        expect(room.profileSettings?.moodProfileId, 'sleep_mood');
        expect(room.profileSettings?.fadeMs, 1200);
        expect(room.profileSettings?.motionTimeoutSecs, 300);
      });

      test('prefers profile_settings over legacy room_profile', () {
        final room = RhythmRoom.fromJson({
          'profile_settings': {'motion_timeout_secs': 111},
          'room_profile': {'motion_timeout_secs': 222},
        });

        expect(room.profileSettings?.motionTimeoutSecs, 111);
        expect(room.roomProfile?.motionTimeoutSecs, 111);
      });

      test('parses deviceIds as List<String>', () {
        final room = RhythmRoom.fromJson({
          'device_ids': ['a', 'b', 'c'],
        });
        expect(room.deviceIds, isA<List<String>>());
        expect(room.deviceIds, ['a', 'b', 'c']);
      });

      test('parses devices as List<RhythmDevice>', () {
        final room = RhythmRoom.fromJson({
          'devices': [
            {'id': 'x', 'type': 'light'},
          ],
        });
        expect(room.devices.length, 1);
        expect(room.devices.first.id, 'x');
        expect(room.devices.first.type, RhythmDeviceType.light);
      });

      test('accepts wrapped numeric fields', () {
        final room = RhythmRoom.fromJson({
          'id': 'room-1',
          'grouped_light_id': 'gl-1',
          'time_offset': {'value': 2.25},
          'brightness_offset': {'value': -4.5},
          'brightness': {'brightness_pct': 81},
          'kelvin': {'color_temp': 4200},
        });

        expect(room.timeOffset, 2.25);
        expect(room.brightnessOffset, -4.5);
        expect(room.brightness, 81);
        expect(room.kelvin, 4200);
      });
    });

    group('device type getters', () {
      late RhythmRoom room;

      setUp(() {
        room = RhythmRoom.fromJson({
          'id': 'room-1',
          'name': 'Test Room',
          'grouped_light_id': 'gl-1',
          'devices': [
            {'id': 'l1', 'type': 'light'},
            {'id': 'l2', 'type': 'light'},
            {'id': 'b1', 'type': 'button'},
            {'id': 'm1', 'type': 'motion'},
          ],
        });
      });

      test('lights returns only light devices', () {
        expect(room.lights.length, 2);
        expect(
            room.lights.every((d) => d.type == RhythmDeviceType.light), isTrue);
      });

      test('buttons returns only button devices', () {
        expect(room.buttons.length, 1);
        expect(room.buttons.first.id, 'b1');
      });

      test('motionSensors returns only motion devices', () {
        expect(room.motionSensors.length, 1);
        expect(room.motionSensors.first.id, 'm1');
      });

      test('hasMotionSensor is true when a motion device exists', () {
        expect(room.hasMotionSensor, isTrue);
      });

      test('hasMotionSensor is false when no motion device exists', () {
        final noMotion = RhythmRoom.fromJson({
          'id': 'r',
          'name': 'r',
          'grouped_light_id': 'g',
          'devices': [
            {'id': 'l1', 'type': 'light'},
          ],
        });
        expect(noMotion.hasMotionSensor, isFalse);
      });
    });

    group('lightCount', () {
      test('returns typed lights count when lights are present', () {
        final room = RhythmRoom.fromJson({
          'id': 'r',
          'name': 'r',
          'grouped_light_id': 'g',
          'device_ids': ['a', 'b', 'c'],
          'devices': [
            {'id': 'a', 'type': 'light'},
            {'id': 'b', 'type': 'light'},
            {'id': 'c', 'type': 'button'},
          ],
        });
        expect(room.lightCount, 2);
      });

      test('infers from deviceIds when no typed lights exist', () {
        final room = RhythmRoom.fromJson({
          'id': 'r',
          'name': 'r',
          'grouped_light_id': 'g',
          'device_ids': ['a', 'b', 'c', 'd'],
          'devices': [
            {'id': 'c', 'type': 'button'},
          ],
        });
        // inferred = deviceIds.length(4) - devices.length(1) = 3
        expect(room.lightCount, 3);
      });

      test('returns 0 when no devices and no deviceIds', () {
        final room = RhythmRoom.fromJson({
          'id': 'r',
          'name': 'r',
          'grouped_light_id': 'g',
        });
        expect(room.lightCount, 0);
      });

      test('returns 0 when inferred count would be negative', () {
        final room = RhythmRoom.fromJson({
          'id': 'r',
          'name': 'r',
          'grouped_light_id': 'g',
          'device_ids': ['a'],
          'devices': [
            {'id': 'a', 'type': 'button'},
            {'id': 'b', 'type': 'button'},
          ],
        });
        // inferred = 1 - 2 = -1 → 0, and typed lights = 0
        expect(room.lightCount, 0);
      });
    });

    group('deviceSummary', () {
      test('returns summary with multiple device types', () {
        final room = RhythmRoom.fromJson({
          'id': 'r',
          'name': 'r',
          'grouped_light_id': 'g',
          'devices': [
            {'id': 'l1', 'type': 'light'},
            {'id': 'l2', 'type': 'light'},
            {'id': 'm1', 'type': 'motion'},
          ],
        });
        expect(room.deviceSummary, '2 lights, 1 sensor');
      });

      test('returns "No devices" when empty', () {
        final room = RhythmRoom.fromJson({
          'id': 'r',
          'name': 'r',
          'grouped_light_id': 'g',
        });
        expect(room.deviceSummary, 'No devices');
      });

      test('uses singular form for single items', () {
        final room = RhythmRoom.fromJson({
          'id': 'r',
          'name': 'r',
          'grouped_light_id': 'g',
          'devices': [
            {'id': 'l1', 'type': 'light'},
          ],
        });
        expect(room.deviceSummary, '1 light');
      });

      test('includes buttons in summary', () {
        final room = RhythmRoom.fromJson({
          'id': 'r',
          'name': 'r',
          'grouped_light_id': 'g',
          'devices': [
            {'id': 'l1', 'type': 'light'},
            {'id': 'b1', 'type': 'button'},
            {'id': 'b2', 'type': 'button'},
          ],
        });
        expect(room.deviceSummary, '1 light, 2 buttons');
      });

      test('uses plural for multiple sensors', () {
        final room = RhythmRoom.fromJson({
          'id': 'r',
          'name': 'r',
          'grouped_light_id': 'g',
          'devices': [
            {'id': 'm1', 'type': 'motion'},
            {'id': 'm2', 'type': 'motion'},
          ],
        });
        expect(room.deviceSummary, '2 sensors');
      });

      test('includes all three types', () {
        final room = RhythmRoom.fromJson({
          'id': 'r',
          'name': 'r',
          'grouped_light_id': 'g',
          'devices': [
            {'id': 'l1', 'type': 'light'},
            {'id': 'l2', 'type': 'light'},
            {'id': 'l3', 'type': 'light'},
            {'id': 'b1', 'type': 'button'},
            {'id': 'm1', 'type': 'motion'},
            {'id': 'm2', 'type': 'motion'},
          ],
        });
        expect(room.deviceSummary, '3 lights, 1 button, 2 sensors');
      });
    });
  });

  group('RhythmTopologyNode', () {
    test('parses controls from topology payload', () {
      final node = RhythmTopologyNode.fromJson({
        'id': 'sensor-1',
        'name': 'Hall Motion',
        'kind': 'motion_sensor',
        'controls': [
          {
            'kind': 'motion',
            'target_id': 'room-1',
            'inherited': true,
          },
        ],
      });

      expect(node.controls, hasLength(1));
      expect(node.controls.first.kind, 'motion');
      expect(node.controls.first.targetId, 'room-1');
      expect(node.controls.first.inherited, isTrue);
      expect(node.motionTargetId, 'room-1');
    });
  });

  group('RhythmRoomState', () {
    group('fromJson', () {
      test('parses node_id field', () {
        final state = RhythmRoomState.fromJson({
          'node_id': 'node-42',
          'rhythm_enabled': true,
          'time_offset': 1.0,
          'brightness_offset': -5.0,
          'soft_off': false,
          'lights_on': true,
          'brightness': 75,
          'kelvin': 3500,
          'mood_enabled': true,
          'mood_active': true,
          'profile_settings': {
            'mood_enabled': true,
            'mood_profile_id': 'node-42_mood',
            'motion_timeout_secs': 77,
          },
        });
        expect(state.nodeId, 'node-42');
        expect(state.roomId, 'node-42');
        expect(state.rhythmEnabled, true);
        expect(state.timeOffset, 1.0);
        expect(state.brightnessOffset, -5.0);
        expect(state.softOff, false);
        expect(state.lightsOn, true);
        expect(state.brightness, 75);
        expect(state.kelvin, 3500);
        expect(state.moodEnabled, isTrue);
        expect(state.moodActive, isTrue);
        expect(state.profileSettings?.moodEnabled, isTrue);
        expect(state.profileSettings?.moodProfileId, 'node-42_mood');
        expect(state.profileSettings?.motionTimeoutSecs, 77);
      });

      test('parses fixed timer settings in profile_settings', () {
        final state = RhythmRoomState.fromJson({
          'node_id': 'room-1',
          'rhythm_enabled': true,
          'profile_settings': {
            'profile_id': 'sleep',
            'mood_enabled': true,
            'mood_profile_id': 'room-1_mood',
            'fade_ms': {'mode': 'fixed', 'value': 900},
            'motion_timeout_secs': {'mode': 'fixed', 'value': 180},
          },
        });

        expect(state.profileSettings?.profileId, 'sleep');
        expect(state.profileSettings?.moodEnabled, isTrue);
        expect(state.profileSettings?.moodProfileId, 'room-1_mood');
        expect(state.profileSettings?.fadeMs, 900);
        expect(state.profileSettings?.motionTimeoutSecs, 180);
      });

      test('falls back to id field when room_id is missing', () {
        final state = RhythmRoomState.fromJson({
          'id': 'room-fallback',
        });
        expect(state.roomId, 'room-fallback');
      });

      test('defaults to empty string when both room_id and id are missing', () {
        final state = RhythmRoomState.fromJson({});
        expect(state.roomId, '');
      });

      test('provides defaults for all missing fields', () {
        final state = RhythmRoomState.fromJson({});
        expect(state.roomId, '');
        expect(state.rhythmEnabled, false);
        expect(state.timeOffset, 0.0);
        expect(state.brightnessOffset, 0.0);
        expect(state.softOff, false);
        expect(state.lightsOn, isNull);
        expect(state.brightness, isNull);
        expect(state.kelvin, isNull);
      });

      test('prefers room_id over id when both are present', () {
        final state = RhythmRoomState.fromJson({
          'room_id': 'preferred',
          'id': 'fallback',
        });
        expect(state.roomId, 'preferred');
      });

      test('accepts wrapped numeric state fields', () {
        final state = RhythmRoomState.fromJson({
          'room_id': 'room-42',
          'time_offset': {'value': 1.0},
          'brightness_offset': {'value': -5.0},
          'brightness': {'current': 75},
          'kelvin': {'value': 3500},
        });

        expect(state.timeOffset, 1.0);
        expect(state.brightnessOffset, -5.0);
        expect(state.brightness, 75);
        expect(state.kelvin, 3500);
      });
    });
  });

  group('RhythmMotionTimer', () {
    test('constructor sets all fields', () {
      const timer = RhythmMotionTimer.node(
        nodeId: 'node-1',
        motionActive: true,
        motionOwned: true,
        remainingSecs: 120,
        timeoutSecs: 300,
      );
      expect(timer.nodeId, 'node-1');
      expect(timer.roomId, 'node-1');
      expect(timer.motionActive, true);
      expect(timer.motionOwned, true);
      expect(timer.remainingSecs, 120);
      expect(timer.timeoutSecs, 300);
    });

    group('cleared', () {
      test('sets motionActive to false and timeoutSecs to 0', () {
        const timer = RhythmMotionTimer.cleared('room-2');
        expect(timer.roomId, 'room-2');
        expect(timer.motionActive, false);
        expect(timer.motionOwned, false);
        expect(timer.remainingSecs, isNull);
        expect(timer.timeoutSecs, 0);
      });
    });

    group('isCleared', () {
      test('returns true when timeoutSecs is 0 and motionActive is false', () {
        const timer = RhythmMotionTimer.cleared('room-1');
        expect(timer.isCleared, isTrue);
      });

      test('returns false when motionActive is true', () {
        const timer = RhythmMotionTimer(
          roomId: 'room-1',
          motionActive: true,
          motionOwned: false,
          timeoutSecs: 0,
        );
        expect(timer.isCleared, isFalse);
      });

      test('returns false when timeoutSecs is non-zero', () {
        const timer = RhythmMotionTimer(
          roomId: 'room-1',
          motionActive: false,
          motionOwned: false,
          timeoutSecs: 60,
        );
        expect(timer.isCleared, isFalse);
      });

      test('returns false when both motionActive and timeoutSecs are non-zero',
          () {
        const timer = RhythmMotionTimer(
          roomId: 'room-1',
          motionActive: true,
          motionOwned: true,
          remainingSecs: 50,
          timeoutSecs: 120,
        );
        expect(timer.isCleared, isFalse);
      });
    });
  });
}
