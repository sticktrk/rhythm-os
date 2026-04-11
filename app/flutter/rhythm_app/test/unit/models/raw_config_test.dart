import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_core/rhythm_core.dart';

/// Helper to create a test RawConfig without requiring FFI initialization.
/// Uses hardcoded values that match the expected Rust defaults.
RawConfig _createTestConfig({
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

void main() {
  group('RawConfig', () {
    group('defaults', () {
      test('matches shared Dart/server defaults', () {
        final config = RawConfig.defaults();

        expect(config.minColorTemp, defaultCurveConfig.minColorTemp);
        expect(config.maxColorTemp, defaultCurveConfig.maxColorTemp);
        expect(config.minBrightness, defaultCurveConfig.minBrightness);
        expect(config.maxBrightness, defaultCurveConfig.maxBrightness);
        expect(config.widthLeftBri, defaultCurveConfig.widthLeftBri);
        expect(config.widthRightBri, defaultCurveConfig.widthRightBri);
        expect(config.widthLeftCct, defaultCurveConfig.widthLeftCct);
        expect(config.widthRightCct, defaultCurveConfig.widthRightCct);
        expect(config.shapeP, defaultCurveConfig.shapeP);
        expect(config.maxDimSteps, defaultCurveConfig.maxDimSteps);
        expect(config.fadeMs, defaultCurveConfig.fadeMs);
        expect(
          config.motionTimeoutSecs,
          defaultCurveConfig.motionTimeoutSecs,
        );
      });
    });

    group('constructor', () {
      test('creates config with valid values', () {
        final config = _createTestConfig();

        // Validate color temp range
        expect(config.minColorTemp, greaterThanOrEqualTo(500));
        expect(config.maxColorTemp, lessThanOrEqualTo(6500));
        expect(config.minColorTemp, lessThan(config.maxColorTemp));

        // Validate brightness range
        expect(config.minBrightness, greaterThanOrEqualTo(0));
        expect(config.maxBrightness, lessThanOrEqualTo(100));
        expect(config.minBrightness, lessThan(config.maxBrightness));

        // Validate width parameters
        expect(config.widthLeftBri, greaterThan(0));
        expect(config.widthRightBri, greaterThan(0));
        expect(config.widthLeftCct, greaterThan(0));
        expect(config.widthRightCct, greaterThan(0));

        // Validate shape parameter
        expect(config.shapeP, greaterThan(0));

        // Validate step count
        expect(config.maxDimSteps, greaterThan(0));
      });

      test('constructor stores all field values', () {
        final config = RawConfig(
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

        expect(config.minColorTemp, equals(2200));
        expect(config.maxColorTemp, equals(6000));
        expect(config.minBrightness, equals(5));
        expect(config.maxBrightness, equals(95));
        expect(config.widthLeftBri, equals(0.8));
        expect(config.widthRightBri, equals(1.2));
        expect(config.widthLeftCct, equals(0.9));
        expect(config.widthRightCct, equals(1.1));
        expect(config.shapeP, equals(6.0));
        expect(config.maxDimSteps, equals(15));
      });
    });

    group('fromJson', () {
      test('parses complete JSON correctly', () {
        final config = RawConfig.fromJson({
          'min_color_temp': 2200,
          'max_color_temp': 6000,
          'min_brightness': 5,
          'max_brightness': 95,
          'width_left_bri': 0.8,
          'width_right_bri': 1.2,
          'width_left_cct': 0.9,
          'width_right_cct': 1.1,
          'shape_p': 5.0,
          'max_dim_steps': 8,
          'fade_ms': 750,
          'motion_timeout_secs': 300,
        });

        expect(config.minColorTemp, 2200);
        expect(config.maxColorTemp, 6000);
        expect(config.minBrightness, 5);
        expect(config.maxBrightness, 95);
        expect(config.widthLeftBri, 0.8);
        expect(config.widthRightBri, 1.2);
        expect(config.widthLeftCct, 0.9);
        expect(config.widthRightCct, 1.1);
        expect(config.shapeP, 5.0);
        expect(config.maxDimSteps, 8);
        expect(config.fadeMs, 750);
        expect(config.motionTimeoutSecs, 300);
      });

      test('handles integer values as doubles', () {
        final config = RawConfig.fromJson({
          'width_left_bri': 1,
          'width_right_bri': 2,
          'width_left_cct': 1,
          'width_right_cct': 2,
          'shape_p': 6,
        });

        expect(config.widthLeftBri, 1.0);
        expect(config.widthRightBri, 2.0);
        expect(config.widthLeftCct, 1.0);
        expect(config.widthRightCct, 2.0);
        expect(config.shapeP, 6.0);
      });

      test('fills missing fields from shared defaults', () {
        final config = RawConfig.fromJson({
          'min_brightness': 10,
          'shape_p': 4.5,
        });

        expect(config.minBrightness, 10);
        expect(config.shapeP, 4.5);
        expect(config.minColorTemp, defaultCurveConfig.minColorTemp);
        expect(config.maxColorTemp, defaultCurveConfig.maxColorTemp);
        expect(config.fadeMs, defaultCurveConfig.fadeMs);
        expect(
          config.motionTimeoutSecs,
          defaultCurveConfig.motionTimeoutSecs,
        );
      });
    });

    group('toJson', () {
      test('serializes all fields correctly', () {
        final config = RawConfig(
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

        final json = config.toJson();

        expect(json['min_color_temp'], equals(2200));
        expect(json['max_color_temp'], equals(6000));
        expect(json['min_brightness'], equals(5));
        expect(json['max_brightness'], equals(95));
        expect(json['width_left_bri'], equals(0.8));
        expect(json['width_right_bri'], equals(1.2));
        expect(json['width_left_cct'], equals(0.9));
        expect(json['width_right_cct'], equals(1.1));
        expect(json['shape_p'], equals(6.0));
        expect(json['max_dim_steps'], equals(15));
      });

      test('round-trip serialization preserves values', () {
        final original = _createTestConfig(
          minColorTemp: 2200,
          maxBrightness: 90,
          widthLeftCct: 1.4,
        );

        final restored = RawConfig.fromJson(original.toJson());

        expect(restored.minColorTemp, original.minColorTemp);
        expect(restored.maxBrightness, original.maxBrightness);
        expect(restored.widthLeftCct, original.widthLeftCct);
        expect(restored.motionTimeoutSecs, original.motionTimeoutSecs);
      });
    });

    group('copyWith', () {
      test('preserves unchanged fields', () {
        final original = _createTestConfig();
        final copy = original.copyWith(minColorTemp: 2500);

        expect(copy.minColorTemp, equals(2500));
        expect(copy.maxColorTemp, equals(original.maxColorTemp));
        expect(copy.minBrightness, equals(original.minBrightness));
        expect(copy.maxBrightness, equals(original.maxBrightness));
        expect(copy.widthLeftBri, equals(original.widthLeftBri));
        expect(copy.widthRightBri, equals(original.widthRightBri));
        expect(copy.widthLeftCct, equals(original.widthLeftCct));
        expect(copy.widthRightCct, equals(original.widthRightCct));
        expect(copy.shapeP, equals(original.shapeP));
        expect(copy.maxDimSteps, equals(original.maxDimSteps));
      });

      test('updates multiple fields', () {
        final original = _createTestConfig();
        final copy = original.copyWith(
          minColorTemp: 2500,
          maxBrightness: 90,
          shapeP: 8.0,
        );

        expect(copy.minColorTemp, equals(2500));
        expect(copy.maxBrightness, equals(90));
        expect(copy.shapeP, equals(8.0));
        expect(copy.maxColorTemp, equals(original.maxColorTemp));
      });

      test('updates all fields when specified', () {
        final original = _createTestConfig();
        final copy = original.copyWith(
          minColorTemp: 2000,
          maxColorTemp: 5500,
          minBrightness: 10,
          maxBrightness: 90,
          widthLeftBri: 0.5,
          widthRightBri: 1.5,
          widthLeftCct: 0.6,
          widthRightCct: 1.4,
          shapeP: 8.0,
          maxDimSteps: 20,
        );

        expect(copy.minColorTemp, equals(2000));
        expect(copy.maxColorTemp, equals(5500));
        expect(copy.minBrightness, equals(10));
        expect(copy.maxBrightness, equals(90));
        expect(copy.widthLeftBri, equals(0.5));
        expect(copy.widthRightBri, equals(1.5));
        expect(copy.widthLeftCct, equals(0.6));
        expect(copy.widthRightCct, equals(1.4));
        expect(copy.shapeP, equals(8.0));
        expect(copy.maxDimSteps, equals(20));
      });

      test('original is not modified', () {
        final original = _createTestConfig();
        final originalMinTemp = original.minColorTemp;
        original.copyWith(minColorTemp: 2500);

        expect(original.minColorTemp, equals(originalMinTemp));
      });
    });
  });
}
