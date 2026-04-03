import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_core/rhythm_core.dart';

/// Helper to create a test RawConfig without requiring FFI initialization.
/// Uses hardcoded values that match the expected Rust defaults.
RawConfig _createTestConfig({
  int minColorTemp = 2000,
  int maxColorTemp = 6500,
  int minBrightness = 10,
  int maxBrightness = 100,
  double widthLeftBri = 1.0,
  double widthRightBri = 1.0,
  double widthLeftCct = 1.0,
  double widthRightCct = 1.0,
  double shapeP = 4.0,
  int maxDimSteps = 10,
  int fadeMs = 500,
  int motionTimeoutSecs = 600,
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
    // Note: Tests for RawConfig.defaults() require FFI initialization.
    // For pure unit tests, we use _createTestConfig() instead.

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

    // Note: fromJson tests require FFI initialization because
    // RawConfig.fromJson internally calls CurveConfigDto.default_()
    // to get fallback values. These tests are covered in ffi/ tests.
    group('fromJson (requires FFI)', () {
      test('parses complete JSON correctly', () {
        // This test requires FFI - skip for unit tests
      }, skip: 'Requires FFI initialization');

      test('handles integer values as doubles', () {
        // This test requires FFI - skip for unit tests
      }, skip: 'Requires FFI initialization');

      test('handles double values as integers', () {
        // This test requires FFI - skip for unit tests
      }, skip: 'Requires FFI initialization');
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

      // round-trip test requires fromJson which needs FFI
      test('round-trip serialization preserves values', () {
        // This test requires FFI - skip for unit tests
      }, skip: 'Requires FFI initialization (fromJson)');
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
