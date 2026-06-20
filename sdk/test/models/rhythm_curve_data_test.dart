import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  group('TwilightPhase', () {
    group('fromJson', () {
      test('parses all fields when present', () {
        final phase = TwilightPhase.fromJson({
          'civil': 6.5,
          'nautical': 5.8,
          'astronomical': 5.0,
        });
        expect(phase.civil, 6.5);
        expect(phase.nautical, 5.8);
        expect(phase.astronomical, 5.0);
      });

      test('all fields are nullable and default to null', () {
        final phase = TwilightPhase.fromJson({});
        expect(phase.civil, isNull);
        expect(phase.nautical, isNull);
        expect(phase.astronomical, isNull);
      });

      test('handles partial data', () {
        final phase = TwilightPhase.fromJson({
          'civil': 6.5,
        });
        expect(phase.civil, 6.5);
        expect(phase.nautical, isNull);
        expect(phase.astronomical, isNull);
      });

      test('handles num to double coercion', () {
        final phase = TwilightPhase.fromJson({
          'civil': 7,
          'nautical': 6,
          'astronomical': 5,
        });
        expect(phase.civil, 7.0);
        expect(phase.nautical, 6.0);
        expect(phase.astronomical, 5.0);
      });
    });
  });

  group('RhythmSolarInfo', () {
    group('fromJson', () {
      test('parses all fields when fully populated', () {
        final solar = RhythmSolarInfo.fromJson({
          'sunrise': 6.5,
          'sunset': 19.5,
          'solarNoon': 13.0,
          'solarMidnight': 1.0,
          'dayLength': 13.0,
          'dawn': {
            'civil': 6.0,
            'nautical': 5.5,
            'astronomical': 4.8,
          },
          'dusk': {
            'civil': 20.0,
            'nautical': 20.5,
            'astronomical': 21.2,
          },
        });
        expect(solar.sunrise, 6.5);
        expect(solar.sunset, 19.5);
        expect(solar.solarNoon, 13.0);
        expect(solar.solarMidnight, 1.0);
        expect(solar.dayLength, 13.0);
        expect(solar.dawn, isNotNull);
        expect(solar.dawn!.civil, 6.0);
        expect(solar.dusk, isNotNull);
        expect(solar.dusk!.civil, 20.0);
      });

      test('sunrise, sunset, and dayLength are nullable', () {
        final solar = RhythmSolarInfo.fromJson({
          'solarNoon': 12.0,
          'solarMidnight': 0.0,
        });
        expect(solar.sunrise, isNull);
        expect(solar.sunset, isNull);
        expect(solar.dayLength, isNull);
      });

      test('solarNoon and solarMidnight are required', () {
        final solar = RhythmSolarInfo.fromJson({
          'solarNoon': 12.5,
          'solarMidnight': 0.5,
        });
        expect(solar.solarNoon, 12.5);
        expect(solar.solarMidnight, 0.5);
      });

      test('dawn and dusk are optional', () {
        final solar = RhythmSolarInfo.fromJson({
          'solarNoon': 12.0,
          'solarMidnight': 0.0,
        });
        expect(solar.dawn, isNull);
        expect(solar.dusk, isNull);
      });

      test('handles num to double coercion', () {
        final solar = RhythmSolarInfo.fromJson({
          'sunrise': 6,
          'sunset': 20,
          'solarNoon': 13,
          'solarMidnight': 1,
          'dayLength': 14,
        });
        expect(solar.sunrise, 6.0);
        expect(solar.sunset, 20.0);
        expect(solar.solarNoon, 13.0);
        expect(solar.solarMidnight, 1.0);
        expect(solar.dayLength, 14.0);
      });

      test('parses Rust nested twilight data', () {
        final solar = RhythmSolarInfo.fromJson({
          'solar_noon': 13.0,
          'solar_midnight': 1.0,
          'twilight': {
            'dawn': {'civil': 5.75, 'nautical': 5.25},
            'dusk': {'civil': 20.25, 'nautical': 20.75},
          },
        });

        expect(solar.dawn!.civil, 5.75);
        expect(solar.dawn!.nautical, 5.25);
        expect(solar.dusk!.civil, 20.25);
        expect(solar.dusk!.nautical, 20.75);
      });
    });
  });

  group('RhythmCurveData', () {
    group('fromJson', () {
      test('parses hours, bris, ccts, and nested solar', () {
        final data = RhythmCurveData.fromJson({
          'hours': [0.0, 6.0, 12.0, 18.0, 24.0],
          'bris': [2, 50, 100, 50, 2],
          'ccts': [1800, 3500, 5500, 3500, 1800],
          'solar': {
            'solarNoon': 12.0,
            'solarMidnight': 0.0,
            'sunrise': 6.0,
            'sunset': 18.0,
            'dayLength': 12.0,
          },
        });
        expect(data.hours, [0.0, 6.0, 12.0, 18.0, 24.0]);
        expect(data.brightness, [2, 50, 100, 50, 2]);
        expect(data.kelvin, [1800, 3500, 5500, 3500, 1800]);
        expect(data.solar.solarNoon, 12.0);
        expect(data.solar.sunrise, 6.0);
      });

      test('hours are converted to doubles', () {
        final data = RhythmCurveData.fromJson({
          'hours': [0, 6, 12, 18, 24],
          'bris': [10, 50, 100, 50, 10],
          'ccts': [2000, 4000, 5500, 4000, 2000],
          'solar': {
            'solarNoon': 12.0,
            'solarMidnight': 0.0,
          },
        });
        expect(data.hours, everyElement(isA<double>()));
        expect(data.hours, [0.0, 6.0, 12.0, 18.0, 24.0]);
      });

      test('bris are converted to ints', () {
        final data = RhythmCurveData.fromJson({
          'hours': [0.0],
          'bris': [50.0],
          'ccts': [3000],
          'solar': {
            'solarNoon': 12.0,
            'solarMidnight': 0.0,
          },
        });
        expect(data.brightness, [50]);
        expect(data.brightness, everyElement(isA<int>()));
      });

      test('ccts are converted to ints', () {
        final data = RhythmCurveData.fromJson({
          'hours': [0.0],
          'bris': [50],
          'ccts': [3000.0],
          'solar': {
            'solarNoon': 12.0,
            'solarMidnight': 0.0,
          },
        });
        expect(data.kelvin, [3000]);
        expect(data.kelvin, everyElement(isA<int>()));
      });

      test('handles empty lists', () {
        final data = RhythmCurveData.fromJson({
          'hours': <double>[],
          'bris': <int>[],
          'ccts': <int>[],
          'solar': {
            'solarNoon': 12.0,
            'solarMidnight': 0.0,
          },
        });
        expect(data.hours, isEmpty);
        expect(data.brightness, isEmpty);
        expect(data.kelvin, isEmpty);
      });

      test('parses solar with dawn and dusk', () {
        final data = RhythmCurveData.fromJson({
          'hours': [12.0],
          'bris': [100],
          'ccts': [5500],
          'solar': {
            'solarNoon': 12.0,
            'solarMidnight': 0.0,
            'sunrise': 6.0,
            'sunset': 18.0,
            'dawn': {'civil': 5.5, 'nautical': 5.0},
            'dusk': {'civil': 18.5, 'nautical': 19.0},
          },
        });
        expect(data.solar.dawn, isNotNull);
        expect(data.solar.dawn!.civil, 5.5);
        expect(data.solar.dusk, isNotNull);
        expect(data.solar.dusk!.civil, 18.5);
      });
    });
  });
}
