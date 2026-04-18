import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  group('RhythmHello', () {
    test('parses a populated state payload with current settings', () {
      final hello = RhythmHello.fromJson({
        'version': '1.2.3',
        'platform': 'linux',
        'context': 'embedded',
        'listen_port': 8080,
        'rooms': [
          {
            'id': 'room-1',
            'name': 'Living Room',
            'grouped_light_id': 'gl-1',
          },
        ],
        'hub': {'type': 'hue', 'ip': '192.168.1.100'},
        'hubs': [
          {'type': 'hue', 'ip': '192.168.1.100'},
          {'type': 'wiz', 'ip': '192.168.1.101'},
        ],
        'active_profile': {
          'id': 'rhythm',
          'name': 'Day',
          'curve': {'type': 'super-gaussian', 'shape_p': 5.0},
          'min_color_temp': 2000,
        },
        'mode': {
          'active': 'sleep',
          'configs': [
            {
              'mode': 'day',
              'active_profile_id': 'rhythm',
              'idle_profile_id': 'day_idle',
            },
            {
              'mode': 'sleep',
              'active_profile_id': 'sleep',
              'idle_profile_id': 'sleep_idle',
            },
          ],
        },
        'profiles': [
          {
            'id': 'day_idle',
            'name': 'Day Idle',
            'curve': {'type': 'inherit-active'},
          },
          {
            'id': 'rhythm',
            'name': 'Day',
            'curve': {'type': 'super-gaussian'},
          },
          {
            'id': 'sleep',
            'name': 'Sleep',
            'curve': {'type': 'super-gaussian'},
          },
          {
            'id': 'sleep_idle',
            'name': 'Sleep Idle',
            'curve': {'type': 'constant', 'brightness': 1, 'color_temp': 0},
          },
        ],
        'location': {'lat': 40.7128, 'lon': -74.006},
        'settings': {
          'power_save': true,
        },
      });

      expect(hello.version, '1.2.3');
      expect(hello.platformType, 'linux');
      expect(hello.platformContext, 'embedded');
      expect(hello.listenPort, 8080);
      expect(hello.rooms, hasLength(1));
      expect(hello.rooms.first.id, 'room-1');
      expect(hello.hub, {'type': 'hue', 'ip': '192.168.1.100'});
      expect(hello.hubs, hasLength(2));
      expect(hello.activeProfile['id'], 'rhythm');
      expect((hello.activeProfile['curve'] as Map<String, dynamic>)['type'],
          'super-gaussian');
      expect(hello.location, {'lat': 40.7128, 'lon': -74.006});
      expect(hello.settings, isNotNull);
      expect(hello.settings!.powerSave, isTrue);
      expect(hello.mode, isNotNull);
      expect(hello.mode!.active, RhythmMode.sleep);
      expect(hello.mode!.activeConfig?.idleProfileId, 'sleep_idle');
      expect(hello.profiles, hasLength(4));
      expect(hello.profiles.last.id, 'sleep_idle');
    });

    test('uses documented defaults when fields are missing', () {
      final hello = RhythmHello.fromJson({});

      expect(hello.version, '0.0.0');
      expect(hello.platformType, 'desktop');
      expect(hello.platformContext, 'server');
      expect(hello.listenPort, isNull);
      expect(hello.rooms, isEmpty);
      expect(hello.activeProfile, isEmpty);
      expect(hello.location, isEmpty);
      expect(hello.settings, isNull);
    });

    test('derives hubs from hub when hubs is missing', () {
      final hello = RhythmHello.fromJson({
        'hub': {'type': 'hue', 'ip': '192.168.1.100'},
      });

      expect(hello.hubs, hasLength(1));
      expect(hello.hubs.first['type'], 'hue');
    });

    test('derives primary hub from hubs when hub is missing', () {
      final hello = RhythmHello.fromJson({
        'hubs': [
          {
            'type': 'homeassistant',
            'address': 'http://ha.local',
            'connected': true
          },
        ],
      });

      expect(hello.hub['type'], 'homeassistant');
      expect(hello.hub['connected'], isTrue);
    });

    test('normalizes current_time into location.current_local_time', () {
      final hello = RhythmHello.fromJson({
        'current_time': '2026-04-06T17:51:13-04:00',
        'location': {
          'latitude': 35.0,
          'longitude': -97.0,
          'utc_offset_hours': -4.0,
        },
      });

      expect(
        hello.location['current_local_time'],
        '2026-04-06T17:51:13-04:00',
      );
    });

    test('ignores empty room ids', () {
      final hello = RhythmHello.fromJson({
        'rooms': [
          {'id': 'room-1', 'name': 'Valid Room'},
          {'id': '', 'name': 'Empty ID Room'},
          {'name': 'Missing ID Room'},
          {'id': 'room-2', 'name': 'Another Valid Room'},
        ],
      });

      expect(hello.rooms, hasLength(2));
      expect(hello.rooms.first.id, 'room-1');
      expect(hello.rooms.last.id, 'room-2');
    });

    test('leaves settings null when absent', () {
      expect(RhythmHello.fromJson({}).settings, isNull);
      expect(RhythmHello.fromJson({'settings': null}).settings, isNull);
    });

    test('parses split mode and profile state when present', () {
      final hello = RhythmHello.fromJson({
        'mode': {
          'active': 'day',
          'configs': [
            {
              'mode': 'day',
              'active_profile_id': 'rhythm',
              'idle_profile_id': 'day_idle',
            },
          ],
        },
        'profiles': [
          {
            'id': 'rhythm',
            'name': 'Day',
            'curve': {'type': 'super-gaussian'},
          },
          {
            'id': 'day_idle',
            'name': 'Day Idle',
            'curve': {'type': 'inherit-active'},
          },
        ],
        'settings': {
          'power_save': false,
        },
      });

      expect(hello.settings, isNotNull);
      expect(hello.settings!.powerSave, isFalse);
      expect(hello.mode, isNotNull);
      expect(hello.mode!.active, RhythmMode.day);
      expect(hello.mode!.activeConfig?.activeProfileId, 'rhythm');
      expect(hello.profiles.last.name, 'Day Idle');
    });

    test('preserves null idle_profile_id from hello mode payload', () {
      final hello = RhythmHello.fromJson({
        'mode': {
          'active': 'day',
          'configs': [
            {
              'mode': 'day',
              'active_profile_id': 'rhythm',
              'idle_profile_id': null,
            },
          ],
        },
      });

      expect(hello.mode, isNotNull);
      expect(hello.mode!.activeConfig?.idleProfileId, isNull);
    });

    test('parses listen_port from int or double', () {
      expect(RhythmHello.fromJson({'listen_port': 9090}).listenPort, 9090);
      expect(RhythmHello.fromJson({'listen_port': 8080.0}).listenPort, 8080);
    });

    test('flattens active_profile config/effective payloads', () {
      final hello = RhythmHello.fromJson({
        'active_profile': {
          'config': {
            'id': 'rhythm',
            'name': 'Day',
            'curve': {
              'type': 'super-gaussian',
              'shape_p': 6,
            },
            'fade_ms': {'mode': 'auto'},
            'motion_timeout_secs': {'mode': 'auto'},
            'rhythm_interval_secs': {'mode': 'auto'},
          },
          'effective': {
            'fade_ms': 500,
            'motion_timeout_secs': 1200,
            'rhythm_interval_secs': 60,
          },
        },
      });

      expect(hello.activeProfile['id'], 'rhythm');
      expect(hello.activeProfile['name'], 'Day');
      expect(hello.activeProfile['fade_ms'], 500);
      expect(hello.activeProfile['motion_timeout_secs'], 1200);
      expect(hello.activeProfile['rhythm_interval_secs'], 60);
      expect(hello.effectiveFadeMs, 500);
      expect(hello.effectiveMotionTimeoutSecs, 1200);
    });
  });
}
