import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  group('RhythmSettings', () {
    group('fromJson', () {
      test('parses all fields from a fully populated map', () {
        final settings = RhythmSettings.fromJson({
          'bulb_fade_ms': 250,
          'rhythm_interval_secs': 30,
          'default_motion_timeout_secs': 900,
          'power_save': true,
          'soft_off_brightness': 5,
        });
        expect(settings.bulbFadeMs, 250);
        expect(settings.rhythmIntervalSecs, 30);
        expect(settings.defaultMotionTimeoutSecs, 900);
        expect(settings.powerSave, true);
        expect(settings.softOffBrightness, 5);
      });

      test('uses defaults for empty map', () {
        final settings = RhythmSettings.fromJson({});
        expect(settings.bulbFadeMs, 500);
        expect(settings.rhythmIntervalSecs, 60);
        expect(settings.defaultMotionTimeoutSecs, 600);
        expect(settings.powerSave, false);
        expect(settings.softOffBrightness, 1);
      });

      test('uses defaults for missing fields in partial map', () {
        final settings = RhythmSettings.fromJson({
          'bulb_fade_ms': 1000,
          'power_save': true,
        });
        expect(settings.bulbFadeMs, 1000);
        expect(settings.rhythmIntervalSecs, 60);
        expect(settings.defaultMotionTimeoutSecs, 600);
        expect(settings.powerSave, true);
        expect(settings.softOffBrightness, 1);
      });

      test('handles num to int coercion for integer fields', () {
        final settings = RhythmSettings.fromJson({
          'bulb_fade_ms': 300.0,
          'rhythm_interval_secs': 45.0,
          'default_motion_timeout_secs': 1200.0,
          'soft_off_brightness': 2.0,
        });
        expect(settings.bulbFadeMs, 300);
        expect(settings.rhythmIntervalSecs, 45);
        expect(settings.defaultMotionTimeoutSecs, 1200);
        expect(settings.softOffBrightness, 2);
      });

      test('handles null values by using defaults', () {
        final settings = RhythmSettings.fromJson({
          'bulb_fade_ms': null,
          'rhythm_interval_secs': null,
          'default_motion_timeout_secs': null,
          'power_save': null,
          'soft_off_brightness': null,
        });
        expect(settings.bulbFadeMs, 500);
        expect(settings.rhythmIntervalSecs, 60);
        expect(settings.defaultMotionTimeoutSecs, 600);
        expect(settings.powerSave, false);
        expect(settings.softOffBrightness, 1);
      });
    });
  });
}
