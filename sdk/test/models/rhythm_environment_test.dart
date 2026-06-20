import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  group('RhythmEnvironmentSnapshot', () {
    test('parses source, sky condition, lux calibration, and unavailable list',
        () {
      final snapshot = RhythmEnvironmentSnapshot.fromJson({
        'outdoor_factor': 0.62,
        'source': 'lux_sensor',
        'is_fallback': false,
        'diagnostics': {
          'sun_position': 0.8,
          'sun_elevation_degrees': 31.5,
          'sun_angle_factor': 0.75,
          'sky_condition': 'partly_cloudy',
          'sky_multiplier': 0.82,
          'lux': 450,
          'lux_factor': 0.62,
          'lux_learned_floor': 10,
          'lux_learned_ceiling': 900,
          'unavailable_sources': ['manual_override', 'weather'],
        },
      });

      expect(snapshot.source, RhythmEnvironmentSource.luxSensor);
      expect(
          snapshot.diagnostics.skyCondition, RhythmSkyCondition.partlyCloudy);
      expect(snapshot.diagnostics.lux, 450);
      expect(snapshot.diagnostics.luxLearnedCeiling, 900);
      expect(snapshot.diagnostics.unavailableSources, [
        'manual_override',
        'weather',
      ]);
    });
  });
}
