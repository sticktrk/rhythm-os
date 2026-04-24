import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  group('RhythmSolarContext', () {
    group('defaults', () {
      test('returns correct default values', () {
        final solar = RhythmSolarContext.defaults();
        expect(solar.sunrise, 6.0);
        expect(solar.sunset, 20.0);
        expect(solar.solarNoon, 12.0);
        expect(solar.solarMidnight, 0.0);
        expect(solar.dayLength, 14.0);
      });
    });

    group('fromJson', () {
      test('parses all required fields', () {
        final solar = RhythmSolarContext.fromJson({
          'sunrise': 5.5,
          'sunset': 19.8,
          'solar_noon': 12.5,
          'solar_midnight': 0.5,
          'day_length': 14.3,
        });
        expect(solar.sunrise, 5.5);
        expect(solar.sunset, 19.8);
        expect(solar.solarNoon, 12.5);
        expect(solar.solarMidnight, 0.5);
        expect(solar.dayLength, 14.3);
      });

      test('handles num to double coercion (int values)', () {
        final solar = RhythmSolarContext.fromJson({
          'sunrise': 6,
          'sunset': 20,
          'solar_noon': 12,
          'solar_midnight': 0,
          'day_length': 14,
        });
        expect(solar.sunrise, 6.0);
        expect(solar.sunset, 20.0);
        expect(solar.solarNoon, 12.0);
        expect(solar.solarMidnight, 0.0);
        expect(solar.dayLength, 14.0);
      });
    });

    group('toJson', () {
      test('produces snake_case map', () {
        final solar = RhythmSolarContext.defaults();
        final json = solar.toJson();
        expect(json, {
          'sunrise': 6.0,
          'sunset': 20.0,
          'solar_noon': 12.0,
          'solar_midnight': 0.0,
          'day_length': 14.0,
        });
      });

      test('round-trips through fromJson', () {
        final original = RhythmSolarContext.fromJson({
          'sunrise': 7.2,
          'sunset': 18.5,
          'solar_noon': 12.85,
          'solar_midnight': 0.85,
          'day_length': 11.3,
        });
        final roundTripped = RhythmSolarContext.fromJson(original.toJson());
        expect(roundTripped.sunrise, original.sunrise);
        expect(roundTripped.sunset, original.sunset);
        expect(roundTripped.solarNoon, original.solarNoon);
        expect(roundTripped.solarMidnight, original.solarMidnight);
        expect(roundTripped.dayLength, original.dayLength);
      });
    });
  });

  group('RhythmRawConfig', () {
    group('defaults', () {
      test('uses RhythmCurveConfig default constants', () {
        final config = RhythmRawConfig.defaults();
        expect(config.minColorTemp, RhythmCurveConfig.defaultMinColorTemp);
        expect(config.maxColorTemp, RhythmCurveConfig.defaultMaxColorTemp);
        expect(config.minBrightness, RhythmCurveConfig.defaultMinBrightness);
        expect(config.maxBrightness, RhythmCurveConfig.defaultMaxBrightness);
        expect(config.widthLeftBri, RhythmCurveConfig.defaultWidthLeftBri);
        expect(config.widthRightBri, RhythmCurveConfig.defaultWidthRightBri);
        expect(config.widthLeftCct, RhythmCurveConfig.defaultWidthLeftCct);
        expect(config.widthRightCct, RhythmCurveConfig.defaultWidthRightCct);
        expect(config.shapeP, RhythmCurveConfig.defaultShapeP);
        expect(config.maxDimSteps, RhythmCurveConfig.defaultMaxDimSteps);
      });
    });

    group('fromJson', () {
      test('parses all fields', () {
        final config = RhythmRawConfig.fromJson({
          'min_color_temp': 2000,
          'max_color_temp': 6000,
          'min_brightness': 5,
          'max_brightness': 90,
          'width_left_bri': 0.8,
          'width_right_bri': 0.7,
          'width_left_cct': 0.6,
          'width_right_cct': 1.2,
          'shape_p': 4.0,
          'max_dim_steps': 8,
        });
        expect(config.minColorTemp, 2000);
        expect(config.maxColorTemp, 6000);
        expect(config.minBrightness, 5);
        expect(config.maxBrightness, 90);
        expect(config.widthLeftBri, 0.8);
        expect(config.widthRightBri, 0.7);
        expect(config.widthLeftCct, 0.6);
        expect(config.widthRightCct, 1.2);
        expect(config.shapeP, 4.0);
        expect(config.maxDimSteps, 8);
      });

      test('uses RhythmCurveConfig defaults for missing fields', () {
        final config = RhythmRawConfig.fromJson({});
        expect(config.minColorTemp, RhythmCurveConfig.defaultMinColorTemp);
        expect(config.maxColorTemp, RhythmCurveConfig.defaultMaxColorTemp);
        expect(config.minBrightness, RhythmCurveConfig.defaultMinBrightness);
        expect(config.maxBrightness, RhythmCurveConfig.defaultMaxBrightness);
        expect(config.widthLeftBri, RhythmCurveConfig.defaultWidthLeftBri);
        expect(config.widthRightBri, RhythmCurveConfig.defaultWidthRightBri);
        expect(config.widthLeftCct, RhythmCurveConfig.defaultWidthLeftCct);
        expect(config.widthRightCct, RhythmCurveConfig.defaultWidthRightCct);
        expect(config.shapeP, RhythmCurveConfig.defaultShapeP);
        expect(config.maxDimSteps, RhythmCurveConfig.defaultMaxDimSteps);
      });

      test('handles partial JSON with defaults for missing', () {
        final config = RhythmRawConfig.fromJson({
          'min_color_temp': 3000,
          'max_brightness': 75,
        });
        expect(config.minColorTemp, 3000);
        expect(config.maxBrightness, 75);
        expect(config.maxColorTemp, RhythmCurveConfig.defaultMaxColorTemp);
        expect(config.shapeP, RhythmCurveConfig.defaultShapeP);
      });

      test('parses fixed timer-setting maps', () {
        final config = RhythmRawConfig.fromJson({
          'fade_ms': {'mode': 'fixed', 'value': 450},
          'motion_timeout_secs': {'mode': 'fixed', 'value': 900},
        });

        expect(config.fadeMs, 450);
        expect(config.motionTimeoutSecs, 900);
      });
    });

    group('toJson', () {
      test('produces snake_case map with all fields', () {
        final config = RhythmRawConfig.defaults();
        final json = config.toJson();
        expect(json, {
          'min_color_temp': RhythmCurveConfig.defaultMinColorTemp,
          'max_color_temp': RhythmCurveConfig.defaultMaxColorTemp,
          'min_brightness': RhythmCurveConfig.defaultMinBrightness,
          'max_brightness': RhythmCurveConfig.defaultMaxBrightness,
          'width_left_bri': RhythmCurveConfig.defaultWidthLeftBri,
          'width_right_bri': RhythmCurveConfig.defaultWidthRightBri,
          'width_left_cct': RhythmCurveConfig.defaultWidthLeftCct,
          'width_right_cct': RhythmCurveConfig.defaultWidthRightCct,
          'shape_p': RhythmCurveConfig.defaultShapeP,
          'max_dim_steps': RhythmCurveConfig.defaultMaxDimSteps,
          'fade_ms': RhythmCurveConfig.defaultFadeMs,
          'motion_timeout_secs': RhythmCurveConfig.defaultMotionTimeoutSecs,
        });
      });

      test('round-trips through fromJson', () {
        final original = RhythmRawConfig.fromJson({
          'min_color_temp': 2500,
          'max_color_temp': 5800,
          'min_brightness': 3,
          'max_brightness': 95,
          'width_left_bri': 0.9,
          'width_right_bri': 0.8,
          'width_left_cct': 0.7,
          'width_right_cct': 1.1,
          'shape_p': 5.5,
          'max_dim_steps': 10,
        });
        final roundTripped = RhythmRawConfig.fromJson(original.toJson());
        expect(roundTripped.minColorTemp, original.minColorTemp);
        expect(roundTripped.maxColorTemp, original.maxColorTemp);
        expect(roundTripped.shapeP, original.shapeP);
      });
    });

    group('copyWith', () {
      test('returns a copy with one changed field', () {
        final original = RhythmRawConfig.defaults();
        final copied = original.copyWith(minColorTemp: 3000);
        expect(copied.minColorTemp, 3000);
        expect(copied.maxColorTemp, original.maxColorTemp);
        expect(copied.shapeP, original.shapeP);
      });

      test('returns a copy with multiple changed fields', () {
        final original = RhythmRawConfig.defaults();
        final copied = original.copyWith(
          minColorTemp: 2200,
          maxBrightness: 80,
          shapeP: 4.0,
          maxDimSteps: 3,
        );
        expect(copied.minColorTemp, 2200);
        expect(copied.maxBrightness, 80);
        expect(copied.shapeP, 4.0);
        expect(copied.maxDimSteps, 3);
        expect(copied.maxColorTemp, original.maxColorTemp);
      });

      test('returns equivalent copy when no arguments are provided', () {
        final original = RhythmRawConfig.defaults();
        final copied = original.copyWith();
        expect(copied.minColorTemp, original.minColorTemp);
        expect(copied.maxColorTemp, original.maxColorTemp);
        expect(copied.shapeP, original.shapeP);
        expect(copied.maxDimSteps, original.maxDimSteps);
      });
    });
  });

  group('RhythmConfigState', () {
    group('defaults', () {
      test('returns default config and solar', () {
        final state = RhythmConfigState.defaults();
        expect(
            state.config.minColorTemp, RhythmCurveConfig.defaultMinColorTemp);
        expect(state.solar.sunrise, 6.0);
        expect(state.solar.sunset, 20.0);
        expect(state.latitude, isNull);
        expect(state.longitude, isNull);
        expect(state.timezone, isNull);
      });
    });

    group('fromJson', () {
      test('parses nested config and solar objects', () {
        final state = RhythmConfigState.fromJson({
          'config': {
            'min_color_temp': 2500,
            'max_color_temp': 6000,
            'min_brightness': 5,
            'max_brightness': 90,
            'width_left_bri': 0.8,
            'width_right_bri': 0.7,
            'width_left_cct': 0.6,
            'width_right_cct': 1.2,
            'shape_p': 4.0,
            'max_dim_steps': 8,
          },
          'solar': {
            'sunrise': 7.0,
            'sunset': 19.0,
            'solar_noon': 13.0,
            'solar_midnight': 1.0,
            'day_length': 12.0,
          },
          'latitude': 40.7128,
          'longitude': -74.0060,
          'timezone': 'America/New_York',
        });
        expect(state.config.minColorTemp, 2500);
        expect(state.config.maxColorTemp, 6000);
        expect(state.solar.sunrise, 7.0);
        expect(state.solar.sunset, 19.0);
        expect(state.latitude, 40.7128);
        expect(state.longitude, -74.0060);
        expect(state.timezone, 'America/New_York');
      });

      test('supports flat format (top-level config keys)', () {
        final state = RhythmConfigState.fromJson({
          'min_color_temp': 3000,
          'max_color_temp': 5000,
          'min_brightness': 10,
          'max_brightness': 80,
          'width_left_bri': 0.9,
          'width_right_bri': 0.8,
          'width_left_cct': 0.7,
          'width_right_cct': 1.1,
          'shape_p': 5.0,
          'max_dim_steps': 4,
        });
        expect(state.config.minColorTemp, 3000);
        expect(state.config.maxColorTemp, 5000);
        expect(state.config.shapeP, 5.0);
      });

      test('uses solar defaults when solar is missing', () {
        final state = RhythmConfigState.fromJson({
          'config': {
            'min_color_temp': 2500,
          },
        });
        expect(state.solar.sunrise, 6.0);
        expect(state.solar.sunset, 20.0);
        expect(state.solar.solarNoon, 12.0);
        expect(state.solar.solarMidnight, 0.0);
        expect(state.solar.dayLength, 14.0);
      });

      test('latitude, longitude, and timezone are optional', () {
        final state = RhythmConfigState.fromJson({
          'config': {'min_color_temp': 2000},
          'solar': {
            'sunrise': 6,
            'sunset': 20,
            'solar_noon': 12,
            'solar_midnight': 0,
            'day_length': 14,
          },
        });
        expect(state.latitude, isNull);
        expect(state.longitude, isNull);
        expect(state.timezone, isNull);
      });

      test('parses latitude and longitude as doubles from int', () {
        final state = RhythmConfigState.fromJson({
          'config': <String, dynamic>{},
          'latitude': 51,
          'longitude': -1,
        });
        expect(state.latitude, 51.0);
        expect(state.longitude, -1.0);
      });
    });

    group('withConfig', () {
      test('preserves solar, latitude, longitude, and timezone', () {
        final original = RhythmConfigState(
          config: RhythmRawConfig.defaults(),
          solar: RhythmSolarContext.defaults(),
          latitude: 40.0,
          longitude: -74.0,
          timezone: 'America/New_York',
        );
        final newConfig = RhythmRawConfig.defaults().copyWith(
          minColorTemp: 3000,
        );
        final updated = original.withConfig(newConfig);
        expect(updated.config.minColorTemp, 3000);
        expect(updated.solar.sunrise, original.solar.sunrise);
        expect(updated.latitude, 40.0);
        expect(updated.longitude, -74.0);
        expect(updated.timezone, 'America/New_York');
      });

      test('replaces the config entirely', () {
        final original = RhythmConfigState.defaults();
        final newConfig = RhythmRawConfig.fromJson({
          'min_color_temp': 2000,
          'max_color_temp': 4000,
          'shape_p': 3.0,
        });
        final updated = original.withConfig(newConfig);
        expect(updated.config.minColorTemp, 2000);
        expect(updated.config.maxColorTemp, 4000);
        expect(updated.config.shapeP, 3.0);
      });
    });

    group('rawConfigToCurveConfig', () {
      test('converts RhythmRawConfig to RhythmCurveConfig with matching fields',
          () {
        final raw = RhythmRawConfig.fromJson({
          'min_color_temp': 2500,
          'max_color_temp': 6000,
          'min_brightness': 5,
          'max_brightness': 90,
          'width_left_bri': 0.8,
          'width_right_bri': 0.7,
          'width_left_cct': 0.6,
          'width_right_cct': 1.2,
          'shape_p': 4.0,
          'max_dim_steps': 8,
        });
        final curve = RhythmConfigState.rawConfigToCurveConfig(raw);
        expect(curve.minColorTemp, raw.minColorTemp);
        expect(curve.maxColorTemp, raw.maxColorTemp);
        expect(curve.minBrightness, raw.minBrightness);
        expect(curve.maxBrightness, raw.maxBrightness);
        expect(curve.widthLeftBri, raw.widthLeftBri);
        expect(curve.widthRightBri, raw.widthRightBri);
        expect(curve.widthLeftCct, raw.widthLeftCct);
        expect(curve.widthRightCct, raw.widthRightCct);
        expect(curve.shapeP, raw.shapeP);
        expect(curve.maxDimSteps, raw.maxDimSteps);
      });

      test('converts default RhythmRawConfig to default RhythmCurveConfig', () {
        final raw = RhythmRawConfig.defaults();
        final curve = RhythmConfigState.rawConfigToCurveConfig(raw);
        expect(curve, const RhythmCurveConfig());
      });
    });
  });
}
