import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_core/rhythm_core.dart';

/// Helper to create a test RawConfig without requiring FFI initialization.
RawConfig _createTestRawConfig({
  int minColorTemp = 1800,
  int maxColorTemp = 5500,
  int minBrightness = 2,
  int maxBrightness = 100,
  double widthLeftBri = 0.95,
  double widthRightBri = 0.85,
  double widthLeftCct = 0.95,
  double widthRightCct = 1.15,
  double shapeP = 6.0,
  int maxDimSteps = 6,
  int fadeMs = 500,
  int motionTimeoutSecs = 1200,
}) {
  return RawConfig(
    minColorTemp: minColorTemp,
    maxColorTemp: maxColorTemp,
    minBrightness: minBrightness,
    maxBrightness: maxBrightness,
    widthLeftBri: widthLeftBri,
    widthRightBri: widthRightBri,
    widthLeftCct: widthLeftCct,
    widthRightCct: widthRightCct,
    shapeP: shapeP,
    maxDimSteps: maxDimSteps,
    fadeMs: fadeMs,
    motionTimeoutSecs: motionTimeoutSecs,
  );
}

/// Helper to create a test ConfigState without requiring FFI initialization.
ConfigState _createTestConfigState({
  RawConfig? config,
  SolarContext? solar,
  double? latitude,
  double? longitude,
  String? timezone,
}) {
  return ConfigState(
    config: config ?? _createTestRawConfig(),
    solar: solar ?? SolarContext.defaults(),
    latitude: latitude,
    longitude: longitude,
    timezone: timezone,
  );
}

void main() {
  group('SolarContext', () {
    group('defaults', () {
      test('creates context with sensible default values', () {
        final solar = SolarContext.defaults();

        expect(solar.sunrise, equals(6.0));
        expect(solar.sunset, equals(20.0));
        expect(solar.solarNoon, equals(12.0));
        expect(solar.solarMidnight, equals(0.0));
        expect(solar.dayLength, equals(14.0));
      });
    });

    group('fromJson', () {
      test('parses all fields correctly', () {
        final json = {
          'sunrise': 5.5,
          'sunset': 20.5,
          'solar_noon': 13.0,
          'solar_midnight': 1.0,
          'day_length': 15.0,
        };

        final solar = SolarContext.fromJson(json);

        expect(solar.sunrise, equals(5.5));
        expect(solar.sunset, equals(20.5));
        expect(solar.solarNoon, equals(13.0));
        expect(solar.solarMidnight, equals(1.0));
        expect(solar.dayLength, equals(15.0));
      });

      test('handles integer values as doubles', () {
        final json = {
          'sunrise': 6,
          'sunset': 20,
          'solar_noon': 13,
          'solar_midnight': 1,
          'day_length': 14,
        };

        final solar = SolarContext.fromJson(json);

        expect(solar.sunrise, equals(6.0));
        expect(solar.sunset, equals(20.0));
      });
    });

    group('toJson', () {
      test('serializes all fields correctly', () {
        final solar = SolarContext(
          sunrise: 5.5,
          sunset: 20.5,
          solarNoon: 13.0,
          solarMidnight: 1.0,
          dayLength: 15.0,
        );

        final json = solar.toJson();

        expect(json['sunrise'], equals(5.5));
        expect(json['sunset'], equals(20.5));
        expect(json['solar_noon'], equals(13.0));
        expect(json['solar_midnight'], equals(1.0));
        expect(json['day_length'], equals(15.0));
      });

      test('round-trip serialization preserves values', () {
        final original = SolarContext(
          sunrise: 5.5,
          sunset: 20.5,
          solarNoon: 13.0,
          solarMidnight: 1.0,
          dayLength: 15.0,
        );

        final json = original.toJson();
        final restored = SolarContext.fromJson(json);

        expect(restored.sunrise, equals(original.sunrise));
        expect(restored.sunset, equals(original.sunset));
        expect(restored.solarNoon, equals(original.solarNoon));
        expect(restored.solarMidnight, equals(original.solarMidnight));
        expect(restored.dayLength, equals(original.dayLength));
      });
    });
  });

  group('ConfigState', () {
    group('defaults', () {
      test('creates default config and solar context', () {
        final state = ConfigState.defaults();

        expect(state.config.minColorTemp, defaultCurveConfig.minColorTemp);
        expect(state.config.maxColorTemp, defaultCurveConfig.maxColorTemp);
        expect(state.config.fadeMs, defaultCurveConfig.fadeMs);
        expect(
          state.config.motionTimeoutSecs,
          defaultCurveConfig.motionTimeoutSecs,
        );
        expect(state.solar, SolarContext.defaults());
        expect(state.latitude, isNull);
        expect(state.longitude, isNull);
        expect(state.timezone, isNull);
      });
    });

    group('constructor', () {
      test('creates state with valid config and solar', () {
        final state = _createTestConfigState();

        // Config should have valid values
        expect(state.config.minColorTemp, greaterThan(0));
        expect(state.config.maxBrightness, lessThanOrEqualTo(100));

        // Solar should have default values
        expect(state.solar.solarNoon, equals(12.0));

        // Location should be null by default
        expect(state.latitude, isNull);
        expect(state.longitude, isNull);
        expect(state.timezone, isNull);
      });
    });

    group('fromJson', () {
      test('parses nested config and solar correctly', () {
        final state = ConfigState.fromJson({
          'config': {
            'min_color_temp': 2200,
            'max_color_temp': 6000,
            'shape_p': 5.0,
          },
          'solar': {
            'sunrise': 5.75,
            'sunset': 20.25,
            'solar_noon': 13.0,
            'solar_midnight': 1.0,
            'day_length': 14.5,
          },
          'latitude': 35.0,
          'longitude': -78.5,
          'timezone': 'America/New_York',
        });

        expect(state.config.minColorTemp, 2200);
        expect(state.config.maxColorTemp, 6000);
        expect(state.config.shapeP, 5.0);
        expect(state.solar.sunrise, 5.75);
        expect(state.solar.solarNoon, 13.0);
        expect(state.latitude, 35.0);
        expect(state.longitude, -78.5);
        expect(state.timezone, 'America/New_York');
      });

      test('handles missing optional location fields', () {
        final state = ConfigState.fromJson({
          'min_brightness': 10,
          'shape_p': 4.5,
        });

        expect(state.config.minBrightness, 10);
        expect(state.config.shapeP, 4.5);
        expect(state.solar, SolarContext.defaults());
        expect(state.latitude, isNull);
        expect(state.longitude, isNull);
        expect(state.timezone, isNull);
      });
    });

    group('withConfig', () {
      test('creates new state with updated config', () {
        final original = _createTestConfigState();
        final newConfig = original.config.copyWith(minColorTemp: 2500);
        final updated = original.withConfig(newConfig);

        expect(updated.config.minColorTemp, equals(2500));
        expect(updated.solar.solarNoon, equals(original.solar.solarNoon));
        expect(updated.latitude, equals(original.latitude));
      });

      test('preserves solar and location data', () {
        final original = ConfigState(
          config: _createTestRawConfig(),
          solar: SolarContext(
            sunrise: 5.5,
            sunset: 20.5,
            solarNoon: 13.0,
            solarMidnight: 1.0,
            dayLength: 15.0,
          ),
          latitude: 35.0,
          longitude: -78.5,
          timezone: 'America/New_York',
        );

        final newConfig = original.config.copyWith(maxBrightness: 90);
        final updated = original.withConfig(newConfig);

        expect(updated.solar.sunrise, equals(5.5));
        expect(updated.latitude, equals(35.0));
        expect(updated.longitude, equals(-78.5));
        expect(updated.timezone, equals('America/New_York'));
      });

      test('does not modify original state', () {
        final original = _createTestConfigState();
        final originalMinTemp = original.config.minColorTemp;
        original.withConfig(original.config.copyWith(minColorTemp: 2500));

        expect(original.config.minColorTemp, equals(originalMinTemp));
      });
    });

    group('rawConfigToDto', () {
      test('converts all fields correctly', () {
        final raw = RawConfig(
          minColorTemp: 2200,
          maxColorTemp: 6000,
          minBrightness: 5,
          maxBrightness: 95,
          widthLeftBri: 0.8,
          widthRightBri: 1.2,
          widthLeftCct: 0.9,
          widthRightCct: 1.1,
          shapeP: 6.0,
          maxDimSteps: 15,
          fadeMs: 500,
          motionTimeoutSecs: 600,
        );

        final dto = ConfigState.rawConfigToDto(raw);

        expect(dto.minColorTemp, equals(2200));
        expect(dto.maxColorTemp, equals(6000));
        expect(dto.minBrightness, equals(5));
        expect(dto.maxBrightness, equals(95));
        expect(dto.widthLeftBri, equals(0.8));
        expect(dto.widthRightBri, equals(1.2));
        expect(dto.widthLeftCct, equals(0.9));
        expect(dto.widthRightCct, equals(1.1));
        expect(dto.shapeP, equals(6.0));
        expect(dto.maxDimSteps, equals(15));
      });

      test('conversion is consistent', () {
        final raw = _createTestRawConfig();
        final dto1 = ConfigState.rawConfigToDto(raw);
        final dto2 = ConfigState.rawConfigToDto(raw);

        expect(dto1.minColorTemp, equals(dto2.minColorTemp));
        expect(dto1.maxBrightness, equals(dto2.maxBrightness));
        expect(dto1.shapeP, equals(dto2.shapeP));
      });
    });
  });
}
