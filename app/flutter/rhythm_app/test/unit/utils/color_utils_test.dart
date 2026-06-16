import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_core/rhythm_core.dart';

void main() {
  group('ColorUtils', () {
    group('cctToColor', () {
      test('returns warm orange for 2000K (candlelight)', () {
        final color = ColorUtils.cctToColor(2000);

        // Very warm light should be heavily red/orange
        expect(color.red255, equals(255));
        expect(color.green255, lessThan(150)); // Relatively low green
        expect(color.blue255, lessThan(50)); // Very low blue
      });

      test('returns warm amber for 2700K (incandescent)', () {
        final color = ColorUtils.cctToColor(2700);

        expect(color.red255, equals(255));
        expect(color.green255, greaterThan(150)); // Higher green than 2000K
        expect(color.blue255, lessThan(100)); // Still low blue
      });

      test('returns neutral white for 4000K', () {
        final color = ColorUtils.cctToColor(4000);

        expect(color.red255, equals(255));
        // Neutral white has higher green/blue than warm temperatures
        expect(color.green255, greaterThan(180));
        expect(color.blue255, greaterThan(150));
      });

      test('returns cool white for 5500K (daylight)', () {
        final color = ColorUtils.cctToColor(5500);

        expect(color.red255, equals(255));
        // Cool daylight has high green and blue (>200)
        expect(color.green255, greaterThan(200));
        expect(color.blue255, greaterThan(200));
      });

      test('returns bluish white for 6500K (cool daylight)', () {
        final color = ColorUtils.cctToColor(6500);

        expect(color.red255, equals(255));
        // High green and blue values for cool white (algorithm may not produce pure white)
        expect(color.green255, greaterThan(240));
        expect(color.blue255, greaterThan(240));
      });

      test('clamps values below 500K', () {
        final color = ColorUtils.cctToColor(100);
        // Should be treated as 500K
        expect(color.red255, equals(255));
        expect(color.green255, isNonNegative);
        expect(color.blue255, isNonNegative);
      });

      test('clamps values above 6500K', () {
        final color = ColorUtils.cctToColor(10000);
        // Should be treated as 6500K - same as 6500K values
        final color6500 = ColorUtils.cctToColor(6500);
        expect(color.red255, equals(color6500.red255));
        expect(color.green255, equals(color6500.green255));
        expect(color.blue255, equals(color6500.blue255));
      });

      test('RGB values are always between 0 and 255', () {
        for (int k = 500; k <= 6500; k += 100) {
          final color = ColorUtils.cctToColor(k);
          expect(color.red255, inInclusiveRange(0, 255));
          expect(color.green255, inInclusiveRange(0, 255));
          expect(color.blue255, inInclusiveRange(0, 255));
        }
      });
    });

    group('lineColorForCCT', () {
      test('returns base color for warm temperatures', () {
        final baseColor = ColorUtils.cctToColor(3000);
        final lineColor = ColorUtils.lineColorForCCT(3000);

        // For temps <= 6000K, should return base color unchanged
        expect(lineColor.r, equals(baseColor.r));
        expect(lineColor.g, equals(baseColor.g));
        expect(lineColor.b, equals(baseColor.b));
      });

      test('returns base color at 6000K boundary', () {
        final baseColor = ColorUtils.cctToColor(6000);
        final lineColor = ColorUtils.lineColorForCCT(6000);

        expect(lineColor.r, equals(baseColor.r));
        expect(lineColor.g, equals(baseColor.g));
        expect(lineColor.b, equals(baseColor.b));
      });

      test('blends toward blue for temperatures above 6000K', () {
        final baseColor = ColorUtils.cctToColor(6500);
        final lineColor = ColorUtils.lineColorForCCT(6500);

        // Should be blended with blue-white
        // The line color should have more blue tint than base
        expect(lineColor.b, greaterThanOrEqualTo(baseColor.b));
      });

      test('blend increases with temperature', () {
        final lineColor6100 = ColorUtils.lineColorForCCT(6100);
        final lineColor6500 = ColorUtils.lineColorForCCT(6500);

        // Higher temperature should have more blue blend
        // This is a relative comparison
        final base6100 = ColorUtils.cctToColor(6100);
        final base6500 = ColorUtils.cctToColor(6500);

        final diff6100 = (lineColor6100.b - base6100.b).abs();
        final diff6500 = (lineColor6500.b - base6500.b).abs();

        // 6500K should have more difference due to higher blend factor
        expect(diff6500, greaterThanOrEqualTo(diff6100));
      });
    });

    group('curveColorForCCT', () {
      test('returns same as lineColorForCCT', () {
        for (int k = 2000; k <= 6500; k += 500) {
          final curveColor = ColorUtils.curveColorForCCT(k);
          final lineColor = ColorUtils.lineColorForCCT(k);

          expect(curveColor.r, equals(lineColor.r));
          expect(curveColor.g, equals(lineColor.g));
          expect(curveColor.b, equals(lineColor.b));
        }
      });
    });

    group('generateCurveColors', () {
      test('returns empty list for empty input', () {
        final colors = ColorUtils.generateCurveColors([]);
        expect(colors, isEmpty);
      });

      test('returns single color for single element list', () {
        final colors = ColorUtils.generateCurveColors([4000]);

        expect(colors.length, equals(1));
        expect(colors[0], equals(ColorUtils.curveColorForCCT(4000)));
      });

      test('returns n-1 colors for n element list', () {
        final kelvinValues = [2000, 3000, 4000, 5000, 6000];
        final colors = ColorUtils.generateCurveColors(kelvinValues);

        // Should return one color per segment (n-1 segments)
        expect(colors.length, equals(4));
      });

      test('uses midpoint kelvin for segment colors', () {
        final kelvinValues = [2000, 4000]; // One segment: 2000-4000
        final colors = ColorUtils.generateCurveColors(kelvinValues);

        // Midpoint is 3000K
        final expectedColor = ColorUtils.curveColorForCCT(3000);
        expect(colors.length, equals(1));
        expect(colors[0].r, equals(expectedColor.r));
        expect(colors[0].g, equals(expectedColor.g));
        expect(colors[0].b, equals(expectedColor.b));
      });

      test('generates correct midpoints for multiple segments', () {
        final kelvinValues = [2000, 4000, 6000];
        final colors = ColorUtils.generateCurveColors(kelvinValues);

        // Two segments: 2000-4000 (mid=3000), 4000-6000 (mid=5000)
        expect(colors.length, equals(2));

        final expected3000 = ColorUtils.curveColorForCCT(3000);
        final expected5000 = ColorUtils.curveColorForCCT(5000);

        expect(colors[0].r, equals(expected3000.r));
        expect(colors[1].r, equals(expected5000.r));
      });
    });
  });
}

/// Extension to get RGB components from Color as 0-255 integers.
extension ColorComponents on Color {
  int get red255 => (r * 255.0).round().clamp(0, 255);
  int get green255 => (g * 255.0).round().clamp(0, 255);
  int get blue255 => (b * 255.0).round().clamp(0, 255);
}
