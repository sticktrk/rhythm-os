import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  group('RhythmStepPoint', () {
    group('fromJson', () {
      test('parses all fields', () {
        final point = RhythmStepPoint.fromJson({
          'hour': 12.5,
          'brightness': 100,
          'kelvin': 5500,
          'rgb': [255, 220, 180],
        });
        expect(point.hour, 12.5);
        expect(point.brightness, 100);
        expect(point.kelvin, 5500);
        expect(point.rgb, [255, 220, 180]);
      });

      test('handles num to double coercion for hour', () {
        final point = RhythmStepPoint.fromJson({
          'hour': 14,
          'brightness': 80,
          'kelvin': 4000,
          'rgb': [200, 180, 160],
        });
        expect(point.hour, 14.0);
      });

      test('handles num to int coercion for brightness and kelvin', () {
        final point = RhythmStepPoint.fromJson({
          'hour': 6.0,
          'brightness': 50.0,
          'kelvin': 3000.0,
          'rgb': [100, 90, 80],
        });
        expect(point.brightness, 50);
        expect(point.kelvin, 3000);
      });

      test('handles num to int coercion for rgb values', () {
        final point = RhythmStepPoint.fromJson({
          'hour': 0.0,
          'brightness': 2,
          'kelvin': 1800,
          'rgb': [255.0, 150.0, 50.0],
        });
        expect(point.rgb, [255, 150, 50]);
        expect(point.rgb, everyElement(isA<int>()));
      });
    });
  });

  group('RhythmStepSequences', () {
    group('fromJson', () {
      test('parses nested step_up and step_down steps', () {
        final sequences = RhythmStepSequences.fromJson({
          'step_up': {
            'steps': [
              {
                'hour': 6.0,
                'brightness': 20,
                'kelvin': 2700,
                'rgb': [255, 180, 100],
              },
              {
                'hour': 8.0,
                'brightness': 60,
                'kelvin': 4000,
                'rgb': [255, 220, 200],
              },
              {
                'hour': 12.0,
                'brightness': 100,
                'kelvin': 5500,
                'rgb': [255, 255, 255],
              },
            ],
          },
          'step_down': {
            'steps': [
              {
                'hour': 12.0,
                'brightness': 100,
                'kelvin': 5500,
                'rgb': [255, 255, 255],
              },
              {
                'hour': 18.0,
                'brightness': 40,
                'kelvin': 3000,
                'rgb': [255, 200, 120],
              },
            ],
          },
        });

        expect(sequences.stepUp.length, 3);
        expect(sequences.stepDown.length, 2);

        expect(sequences.stepUp[0].hour, 6.0);
        expect(sequences.stepUp[0].brightness, 20);
        expect(sequences.stepUp[0].kelvin, 2700);
        expect(sequences.stepUp[0].rgb, [255, 180, 100]);

        expect(sequences.stepUp[2].hour, 12.0);
        expect(sequences.stepUp[2].brightness, 100);

        expect(sequences.stepDown[1].hour, 18.0);
        expect(sequences.stepDown[1].brightness, 40);
        expect(sequences.stepDown[1].kelvin, 3000);
        expect(sequences.stepDown[1].rgb, [255, 200, 120]);
      });

      test('handles empty step lists', () {
        final sequences = RhythmStepSequences.fromJson({
          'step_up': {'steps': []},
          'step_down': {'steps': []},
        });
        expect(sequences.stepUp, isEmpty);
        expect(sequences.stepDown, isEmpty);
      });

      test('handles single step in each direction', () {
        final sequences = RhythmStepSequences.fromJson({
          'step_up': {
            'steps': [
              {
                'hour': 12.0,
                'brightness': 100,
                'kelvin': 5500,
                'rgb': [255, 255, 255],
              },
            ],
          },
          'step_down': {
            'steps': [
              {
                'hour': 0.0,
                'brightness': 2,
                'kelvin': 1800,
                'rgb': [255, 140, 50],
              },
            ],
          },
        });
        expect(sequences.stepUp.length, 1);
        expect(sequences.stepDown.length, 1);
        expect(sequences.stepUp.first.brightness, 100);
        expect(sequences.stepDown.first.brightness, 2);
      });

      test('step points contain correct rgb list', () {
        final sequences = RhythmStepSequences.fromJson({
          'step_up': {
            'steps': [
              {
                'hour': 10.0,
                'brightness': 80,
                'kelvin': 4500,
                'rgb': [255, 240, 220],
              },
            ],
          },
          'step_down': {
            'steps': [
              {
                'hour': 22.0,
                'brightness': 10,
                'kelvin': 2000,
                'rgb': [255, 160, 60],
              },
            ],
          },
        });
        expect(sequences.stepUp.first.rgb.length, 3);
        expect(sequences.stepDown.first.rgb, [255, 160, 60]);
      });
    });
  });
}
