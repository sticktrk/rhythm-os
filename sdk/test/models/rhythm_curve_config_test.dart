import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  group('RhythmCurveConfig', () {
    group('RhythmTimerSetting', () {
      test('parses fixed, auto, and scheduled timer settings', () {
        expect(
          RhythmTimerSetting.fromJson({'mode': 'fixed', 'value': 123}),
          const RhythmTimerSetting.fixed(123),
        );
        expect(
          RhythmTimerSetting.fromJson({'mode': 'auto'}),
          const RhythmTimerSetting.auto(),
        );
        expect(
          RhythmTimerSetting.fromJson({
            'mode': 'scheduled',
            'breakpoints': [
              {'hour': 8.0, 'value': 60},
            ],
          }),
          const RhythmTimerSetting.scheduled([
            RhythmTimerBreakpoint(hour: 8.0, value: 60),
          ]),
        );
      });
    });

    group('static constants', () {
      test('defaultMinColorTemp is 2200', () {
        expect(RhythmCurveConfig.defaultMinColorTemp, 2200);
      });

      test('defaultMaxColorTemp is 6500', () {
        expect(RhythmCurveConfig.defaultMaxColorTemp, 6500);
      });

      test('defaultMinBrightness is 1', () {
        expect(RhythmCurveConfig.defaultMinBrightness, 1);
      });

      test('defaultMaxBrightness is 100', () {
        expect(RhythmCurveConfig.defaultMaxBrightness, 100);
      });

      test('defaultWidthLeftBri is 0.95', () {
        expect(RhythmCurveConfig.defaultWidthLeftBri, 0.95);
      });

      test('defaultWidthRightBri is 0.85', () {
        expect(RhythmCurveConfig.defaultWidthRightBri, 0.85);
      });

      test('defaultWidthLeftCct is 0.95', () {
        expect(RhythmCurveConfig.defaultWidthLeftCct, 0.95);
      });

      test('defaultWidthRightCct is 1.15', () {
        expect(RhythmCurveConfig.defaultWidthRightCct, 1.15);
      });

      test('defaultShapeP is 6.0', () {
        expect(RhythmCurveConfig.defaultShapeP, 6.0);
      });

      test('defaultMaxDimSteps is 12', () {
        expect(RhythmCurveConfig.defaultMaxDimSteps, 12);
      });
    });

    group('default constructor', () {
      test('uses default values when no arguments are provided', () {
        const config = RhythmCurveConfig();
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

      test('accepts custom values', () {
        const config = RhythmCurveConfig(
          minColorTemp: 2000,
          maxColorTemp: 6000,
          minBrightness: 5,
          maxBrightness: 80,
          widthLeftBri: 0.5,
          widthRightBri: 0.6,
          widthLeftCct: 0.7,
          widthRightCct: 0.8,
          shapeP: 4.0,
          maxDimSteps: 10,
        );
        expect(config.minColorTemp, 2000);
        expect(config.maxColorTemp, 6000);
        expect(config.minBrightness, 5);
        expect(config.maxBrightness, 80);
        expect(config.widthLeftBri, 0.5);
        expect(config.widthRightBri, 0.6);
        expect(config.widthLeftCct, 0.7);
        expect(config.widthRightCct, 0.8);
        expect(config.shapeP, 4.0);
        expect(config.maxDimSteps, 10);
      });
    });

    group('fromJson', () {
      test('parses all fields from a full JSON map', () {
        final json = {
          'min_color_temp': 2200,
          'max_color_temp': 6500,
          'min_brightness': 10,
          'max_brightness': 90,
          'width_left_bri': 0.8,
          'width_right_bri': 0.7,
          'width_left_cct': 0.6,
          'width_right_cct': 1.0,
          'shape_p': 5.0,
          'max_dim_steps': 8,
          'fade_ms': {'mode': 'fixed', 'value': 450},
          'motion_timeout_secs': {'mode': 'fixed', 'value': 700},
          'rhythm_interval_secs': {'mode': 'fixed', 'value': 75},
        };
        final config = RhythmCurveConfig.fromJson(json);
        expect(config.minColorTemp, 2200);
        expect(config.maxColorTemp, 6500);
        expect(config.minBrightness, 10);
        expect(config.maxBrightness, 90);
        expect(config.widthLeftBri, 0.8);
        expect(config.widthRightBri, 0.7);
        expect(config.widthLeftCct, 0.6);
        expect(config.widthRightCct, 1.0);
        expect(config.shapeP, 5.0);
        expect(config.maxDimSteps, 8);
        expect(config.fadeMs, 450);
        expect(config.motionTimeoutSecs, 700);
        expect(config.rhythmIntervalSecs, 75);
      });

      test('uses defaults for missing fields', () {
        final config = RhythmCurveConfig.fromJson({});
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

      test('uses defaults for partially populated JSON', () {
        final config = RhythmCurveConfig.fromJson({
          'min_color_temp': 3000,
          'shape_p': 8.0,
        });
        expect(config.minColorTemp, 3000);
        expect(config.maxColorTemp, RhythmCurveConfig.defaultMaxColorTemp);
        expect(config.shapeP, 8.0);
        expect(config.maxDimSteps, RhythmCurveConfig.defaultMaxDimSteps);
      });

      test('handles num to int coercion (double values for int fields)', () {
        final config = RhythmCurveConfig.fromJson({
          'min_color_temp': 2000.0,
          'max_color_temp': 5000.0,
          'min_brightness': 3.0,
          'max_brightness': 95.0,
          'max_dim_steps': 4.0,
        });
        expect(config.minColorTemp, 2000);
        expect(config.maxColorTemp, 5000);
        expect(config.minBrightness, 3);
        expect(config.maxBrightness, 95);
        expect(config.maxDimSteps, 4);
      });

      test('handles num to double coercion (int values for double fields)', () {
        final config = RhythmCurveConfig.fromJson({
          'width_left_bri': 1,
          'width_right_bri': 1,
          'width_left_cct': 1,
          'width_right_cct': 1,
          'shape_p': 5,
        });
        expect(config.widthLeftBri, 1.0);
        expect(config.widthRightBri, 1.0);
        expect(config.widthLeftCct, 1.0);
        expect(config.widthRightCct, 1.0);
        expect(config.shapeP, 5.0);
      });

      test('preserves auto and scheduled timer settings', () {
        final config = RhythmCurveConfig.fromJson({
          'fade_ms': {'mode': 'auto'},
          'motion_timeout_secs': {
            'mode': 'scheduled',
            'breakpoints': [
              {'hour': 8.0, 'value': 300},
            ],
          },
          'rhythm_interval_secs': {'mode': 'fixed', 'value': 60},
        });

        expect(config.fadeSetting, const RhythmTimerSetting.auto());
        expect(
          config.motionTimeoutSetting,
          const RhythmTimerSetting.scheduled([
            RhythmTimerBreakpoint(hour: 8.0, value: 300),
          ]),
        );
        expect(config.fadeMs, isNull);
        expect(config.motionTimeoutSecs, isNull);
        expect(config.rhythmIntervalSecs, 60);
      });

      test('parses sigmoid curve fields', () {
        final config = RhythmCurveConfig.fromJson({
          'id': 'expert',
          'name': 'Expert',
          'curve': {
            'type': 'sigmoid',
            'schedule': {
              'wake': {'hour': 7.0, 'offset_minutes': 15},
              'bed': {'hour': 22.5},
              'alternate_days': [],
            },
            'ascend_start': 3.5,
            'descend_start': 12.5,
            'wake_speed': 9,
            'bed_speed': 5,
            'wake_brightness': 45,
            'bed_brightness': 55,
          },
        });

        expect(config.curve, isA<RhythmSigmoidCurve>());
        final curve = config.sigmoidCurve!;
        expect(curve.schedule['wake'], {'hour': 7.0, 'offset_minutes': 15});
        expect(curve.ascendStart, 3.5);
        expect(curve.descendStart, 12.5);
        expect(curve.wakeSpeed, 9);
        expect(curve.bedSpeed, 5);
        expect(curve.wakeBrightness, 45);
        expect(curve.bedBrightness, 55);
      });
    });

    group('toJson', () {
      test('produces a map with snake_case keys', () {
        const config = RhythmCurveConfig();
        final json = config.toJson();
        expect(json, containsPair('id', ''));
        expect(json, containsPair('name', ''));
        expect(json, containsPair('min_color_temp', 2200));
        expect(json, containsPair('max_color_temp', 6500));
        expect(json, containsPair('min_brightness', 1));
        expect(json, containsPair('max_brightness', 100));
        expect(json, containsPair('max_dim_steps', 12));
        expect(json, containsPair('fade_ms', {'mode': 'fixed', 'value': 500}));
        expect(
          json,
          containsPair(
            'motion_timeout_secs',
            {'mode': 'fixed', 'value': 600},
          ),
        );
        expect(
          json,
          containsPair(
            'rhythm_interval_secs',
            {'mode': 'fixed', 'value': 60},
          ),
        );
        expect(
          json['curve'],
          {
            'type': 'super-gaussian',
            'width_left_bri': 0.95,
            'width_right_bri': 0.85,
            'width_left_cct': 0.95,
            'width_right_cct': 1.15,
            'shape_p': 6.0,
          },
        );
        expect(json.length, 11);
      });

      test('round-trips through fromJson', () {
        const original = RhythmCurveConfig(
          minColorTemp: 2500,
          maxColorTemp: 6000,
          minBrightness: 5,
          maxBrightness: 85,
          widthLeftBri: 0.75,
          widthRightBri: 0.65,
          widthLeftCct: 0.88,
          widthRightCct: 1.25,
          shapeP: 7.5,
          maxDimSteps: 4,
        );
        final roundTripped = RhythmCurveConfig.fromJson(original.toJson());
        expect(roundTripped, original);
      });

      test('round-trips a sigmoid curve', () {
        const original = RhythmCurveConfig(
          id: 'expert',
          name: 'Expert',
          curve: RhythmSigmoidCurve(
            schedule: {
              'wake': {'hour': 6.0},
              'bed': {'hour': 22.0},
              'alternate_days': <dynamic>[],
            },
            ascendStart: 3.0,
            descendStart: 12.0,
            wakeSpeed: 8,
            bedSpeed: 6,
            wakeBrightness: 50,
            bedBrightness: 50,
          ),
        );

        final roundTripped = RhythmCurveConfig.fromJson(original.toJson());
        expect(roundTripped, original);
        expect(roundTripped.sigmoidCurve, isNotNull);
      });
    });

    group('toQueryParams', () {
      test('includes profile id, common bounds, and cadence', () {
        const config = RhythmCurveConfig();
        final params = config.toQueryParams();
        expect(params, containsPair('id', ''));
        expect(params, containsPair('min_color_temp', 2200));
        expect(params, containsPair('max_color_temp', 6500));
        expect(params, containsPair('min_brightness', 1));
        expect(params, containsPair('max_brightness', 100));
        expect(params, containsPair('max_dim_steps', 12));
        expect(params, containsPair('rhythm_interval_secs', 60));
        expect(params.length, 7);
      });
    });

    group('copyWith', () {
      test('returns a new instance with a single changed field', () {
        const original = RhythmCurveConfig();
        final copied = original.copyWith(minColorTemp: 3000);
        expect(copied.minColorTemp, 3000);
        expect(copied.maxColorTemp, original.maxColorTemp);
        expect(copied.minBrightness, original.minBrightness);
        expect(copied.maxBrightness, original.maxBrightness);
        expect(copied.widthLeftBri, original.widthLeftBri);
        expect(copied.widthRightBri, original.widthRightBri);
        expect(copied.widthLeftCct, original.widthLeftCct);
        expect(copied.widthRightCct, original.widthRightCct);
        expect(copied.shapeP, original.shapeP);
        expect(copied.maxDimSteps, original.maxDimSteps);
      });

      test('returns a new instance with multiple changed fields', () {
        const original = RhythmCurveConfig();
        final copied = original.copyWith(
          minColorTemp: 2000,
          maxBrightness: 50,
          shapeP: 3.0,
        );
        expect(copied.minColorTemp, 2000);
        expect(copied.maxBrightness, 50);
        expect(copied.shapeP, 3.0);
        expect(copied.maxColorTemp, original.maxColorTemp);
        expect(copied.minBrightness, original.minBrightness);
      });

      test('returns an equal instance when no arguments are provided', () {
        const original = RhythmCurveConfig();
        final copied = original.copyWith();
        expect(copied, original);
      });

      test('can switch a timer field to auto', () {
        const original = RhythmCurveConfig();
        final copied = original.copyWith(motionTimeoutSecs: null);

        expect(copied.motionTimeoutSetting, const RhythmTimerSetting.auto());
        expect(copied.motionTimeoutSecs, isNull);
      });
    });

    group('equality', () {
      test('two default instances are equal', () {
        const a = RhythmCurveConfig();
        const b = RhythmCurveConfig();
        expect(a, b);
        expect(a.hashCode, b.hashCode);
      });

      test('instances with same custom values are equal', () {
        const a = RhythmCurveConfig(minColorTemp: 2500, shapeP: 3.0);
        const b = RhythmCurveConfig(minColorTemp: 2500, shapeP: 3.0);
        expect(a, b);
        expect(a.hashCode, b.hashCode);
      });

      test('instances with different values are not equal', () {
        const a = RhythmCurveConfig(minColorTemp: 2500);
        const b = RhythmCurveConfig(minColorTemp: 3000);
        expect(a, isNot(b));
      });

      test('is not equal to a non-RhythmCurveConfig object', () {
        const config = RhythmCurveConfig();
        expect(config == Object(), isFalse);
      });

      test('identical instance is equal', () {
        const config = RhythmCurveConfig();
        expect(config, same(config));
      });
    });
  });
}
