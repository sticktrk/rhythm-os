import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  group('RhythmTimeInfo', () {
    group('fromJson', () {
      test('parses all fields including nested lighting object', () {
        final info = RhythmTimeInfo.fromJson({
          'current_time': '2025-06-15T14:30:00Z',
          'current_hour': 14.5,
          'timezone': 'America/New_York',
          'lighting': {
            'brightness': 85,
            'kelvin': 4500,
            'solar_position': 0.75,
          },
        });
        expect(info.currentTime, '2025-06-15T14:30:00Z');
        expect(info.currentHour, 14.5);
        expect(info.timezone, 'America/New_York');
        expect(info.brightness, 85);
        expect(info.kelvin, 4500);
        expect(info.solarPosition, 0.75);
      });

      test('timezone is nullable', () {
        final info = RhythmTimeInfo.fromJson({
          'current_time': '2025-01-01T00:00:00Z',
          'current_hour': 0.0,
          'lighting': {
            'brightness': 2,
            'kelvin': 1800,
            'solar_position': -1.0,
          },
        });
        expect(info.timezone, isNull);
      });

      test('handles num to double coercion for current_hour', () {
        final info = RhythmTimeInfo.fromJson({
          'current_time': '2025-06-15T12:00:00Z',
          'current_hour': 12,
          'lighting': {
            'brightness': 100,
            'kelvin': 5500,
            'solar_position': 1.0,
          },
        });
        expect(info.currentHour, 12.0);
      });

      test('handles num to int coercion for brightness and kelvin', () {
        final info = RhythmTimeInfo.fromJson({
          'current_time': '2025-06-15T12:00:00Z',
          'current_hour': 12.0,
          'lighting': {
            'brightness': 75.0,
            'kelvin': 4000.0,
            'solar_position': 0.5,
          },
        });
        expect(info.brightness, 75);
        expect(info.kelvin, 4000);
      });

      test('handles num to double coercion for solar_position', () {
        final info = RhythmTimeInfo.fromJson({
          'current_time': '2025-06-15T06:00:00Z',
          'current_hour': 6.0,
          'lighting': {
            'brightness': 30,
            'kelvin': 2700,
            'solar_position': 0,
          },
        });
        expect(info.solarPosition, 0.0);
      });

      test('parses midnight scenario', () {
        final info = RhythmTimeInfo.fromJson({
          'current_time': '2025-12-21T00:00:00Z',
          'current_hour': 0.0,
          'timezone': 'UTC',
          'lighting': {
            'brightness': 2,
            'kelvin': 1800,
            'solar_position': -1.0,
          },
        });
        expect(info.currentTime, '2025-12-21T00:00:00Z');
        expect(info.currentHour, 0.0);
        expect(info.brightness, 2);
        expect(info.kelvin, 1800);
        expect(info.solarPosition, -1.0);
      });

      test('parses high-precision values', () {
        final info = RhythmTimeInfo.fromJson({
          'current_time': '2025-03-20T15:45:30Z',
          'current_hour': 15.758333,
          'timezone': 'Europe/London',
          'lighting': {
            'brightness': 92,
            'kelvin': 5200,
            'solar_position': 0.623456,
          },
        });
        expect(info.currentHour, closeTo(15.758333, 0.000001));
        expect(info.solarPosition, closeTo(0.623456, 0.000001));
      });

      test('accepts current_local_time from /api/state payloads', () {
        final info = RhythmTimeInfo.fromJson({
          'location': {
            'current_local_time': '2026-04-06T17:51:13-04:00',
          },
          'current_hour': 17.85,
          'lighting': {
            'brightness': 61,
            'kelvin': 3900,
            'solar_position': 0.42,
          },
        });

        expect(info.currentTime, '2026-04-06T17:51:13-04:00');
        expect(info.currentHour, 17.85);
      });
    });
  });
}
