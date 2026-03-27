import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  group('RhythmHello', () {
    group('fromJson', () {
      test('parses all fields when fully populated', () {
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
          'config': {'min_color_temp': 2000},
          'location': {'lat': 40.7128, 'lon': -74.006},
          'settings': {
            'bulb_fade_ms': 300,
            'rhythm_interval_secs': 30,
            'default_motion_timeout_secs': 900,
            'power_save': true,
            'soft_off_brightness': 2,
          },
        });
        expect(hello.version, '1.2.3');
        expect(hello.platformType, 'linux');
        expect(hello.platformContext, 'embedded');
        expect(hello.listenPort, 8080);
        expect(hello.rooms.length, 1);
        expect(hello.rooms.first.id, 'room-1');
        expect(hello.hub, {'type': 'hue', 'ip': '192.168.1.100'});
        expect(hello.hubs.length, 2);
        expect(hello.config, {'min_color_temp': 2000});
        expect(hello.location, {'lat': 40.7128, 'lon': -74.006});
        expect(hello.settings, isNotNull);
        expect(hello.settings!.bulbFadeMs, 300);
      });
    });

    group('defaults', () {
      test('version defaults to 0.0.0', () {
        final hello = RhythmHello.fromJson({});
        expect(hello.version, '0.0.0');
      });

      test('platformType defaults to desktop (key is "platform")', () {
        final hello = RhythmHello.fromJson({});
        expect(hello.platformType, 'desktop');
      });

      test('platformContext defaults to server (key is "context")', () {
        final hello = RhythmHello.fromJson({});
        expect(hello.platformContext, 'server');
      });

      test('listenPort defaults to null', () {
        final hello = RhythmHello.fromJson({});
        expect(hello.listenPort, isNull);
      });

      test('rooms defaults to empty list', () {
        final hello = RhythmHello.fromJson({});
        expect(hello.rooms, isEmpty);
      });

      test('config defaults to empty map', () {
        final hello = RhythmHello.fromJson({});
        expect(hello.config, isEmpty);
      });

      test('location defaults to empty map', () {
        final hello = RhythmHello.fromJson({});
        expect(hello.location, isEmpty);
      });
    });

    group('hub fallback', () {
      test('derives hubs from hub when hubs key is missing and hub is non-empty',
          () {
        final hello = RhythmHello.fromJson({
          'hub': {'type': 'hue', 'ip': '192.168.1.100'},
        });
        expect(hello.hubs.length, 1);
        expect(hello.hubs.first['type'], 'hue');
      });

      test('returns empty hubs when hub is empty and hubs key is missing', () {
        final hello = RhythmHello.fromJson({
          'hub': <String, dynamic>{},
        });
        expect(hello.hubs, isEmpty);
      });

      test('returns empty hubs when hub type is "none" and hubs key is missing',
          () {
        final hello = RhythmHello.fromJson({
          'hub': {'type': 'none'},
        });
        expect(hello.hubs, isEmpty);
      });

      test('returns empty hubs when both hub and hubs are missing', () {
        final hello = RhythmHello.fromJson({});
        expect(hello.hubs, isEmpty);
      });

      test('uses explicit hubs list when present, ignoring hub fallback', () {
        final hello = RhythmHello.fromJson({
          'hub': {'type': 'hue', 'ip': '192.168.1.100'},
          'hubs': [
            {'type': 'wiz', 'ip': '192.168.1.200'},
          ],
        });
        expect(hello.hubs.length, 1);
        expect(hello.hubs.first['type'], 'wiz');
      });
    });

    group('room filtering', () {
      test('excludes rooms with empty id', () {
        final hello = RhythmHello.fromJson({
          'rooms': [
            {'id': 'room-1', 'name': 'Valid Room'},
            {'id': '', 'name': 'Empty ID Room'},
            {'name': 'No ID Room'},
            {'id': 'room-2', 'name': 'Another Valid Room'},
          ],
        });
        expect(hello.rooms.length, 2);
        expect(hello.rooms[0].id, 'room-1');
        expect(hello.rooms[1].id, 'room-2');
      });

      test('returns empty list when all rooms have empty ids', () {
        final hello = RhythmHello.fromJson({
          'rooms': [
            {'id': '', 'name': 'Empty1'},
            {'name': 'NoId'},
          ],
        });
        expect(hello.rooms, isEmpty);
      });
    });

    group('settings', () {
      test('is null when settings key is missing', () {
        final hello = RhythmHello.fromJson({});
        expect(hello.settings, isNull);
      });

      test('is null when settings key is null', () {
        final hello = RhythmHello.fromJson({'settings': null});
        expect(hello.settings, isNull);
      });

      test('is parsed when settings key is present', () {
        final hello = RhythmHello.fromJson({
          'settings': {
            'bulb_fade_ms': 250,
            'rhythm_interval_secs': 45,
            'default_motion_timeout_secs': 300,
            'power_save': false,
            'soft_off_brightness': 3,
          },
        });
        expect(hello.settings, isNotNull);
        expect(hello.settings!.bulbFadeMs, 250);
        expect(hello.settings!.rhythmIntervalSecs, 45);
        expect(hello.settings!.defaultMotionTimeoutSecs, 300);
        expect(hello.settings!.powerSave, false);
        expect(hello.settings!.softOffBrightness, 3);
      });

      test('settings uses its own defaults when settings map is empty', () {
        final hello = RhythmHello.fromJson({
          'settings': <String, dynamic>{},
        });
        expect(hello.settings, isNotNull);
        expect(hello.settings!.bulbFadeMs, 500);
        expect(hello.settings!.rhythmIntervalSecs, 60);
      });
    });

    group('listen_port', () {
      test('parses listen_port from int', () {
        final hello = RhythmHello.fromJson({'listen_port': 9090});
        expect(hello.listenPort, 9090);
      });

      test('parses listen_port from double', () {
        final hello = RhythmHello.fromJson({'listen_port': 8080.0});
        expect(hello.listenPort, 8080);
      });
    });
  });
}
