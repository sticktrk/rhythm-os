import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  group('RhythmLightSceneOutput', () {
    test('decodes old on-scene JSON with missing power as on', () {
      final output = RhythmLightSceneOutput.fromJson({
        'brightness': 72,
        'color': {'kind': 'kelvin', 'kelvin': 6500},
        'transition_ms': 250,
      });

      expect(output.power, RhythmScenePower.on);
      expect(output.brightness, 72);
      expect(output.color?.kind, RhythmLightColorKind.kelvin);
      expect(output.color?.kelvin, 6500);
      expect(output.transitionMs, 250);
      expect(output.isValid, isTrue);
    });

    test('decodes off output with no color', () {
      final output = RhythmLightSceneOutput.fromJson({
        'power': 'off',
        'transition_ms': 250,
      });

      expect(output.power, RhythmScenePower.off);
      expect(output.color, isNull);
      expect(output.isValid, isTrue);
      expect(output.toJson(), {
        'power': 'off',
        'transition_ms': 250,
      });
    });

    test('marks on output without color invalid for save validation', () {
      final output = RhythmLightSceneOutput.fromJson({
        'power': 'on',
        'brightness': 50,
      });

      expect(output.power, RhythmScenePower.on);
      expect(output.isValid, isFalse);
      expect(output.validationErrors, contains('power=on requires a color'));
    });

    test('marks out-of-range on brightness invalid', () {
      final output = RhythmLightSceneOutput.fromJson({
        'power': 'on',
        'brightness': 0,
        'color': {'kind': 'kelvin', 'kelvin': 2700},
      });

      expect(output.isValid, isFalse);
      expect(
        output.validationErrors,
        contains('brightness must be between 1 and 100'),
      );
    });
  });

  group('RhythmSceneDefinition', () {
    test('parses and serializes scene definitions', () {
      final scene = RhythmSceneDefinition.fromJson({
        'id': 'icy-glow',
        'name': 'Icy Glow',
        'description': null,
        'source': {'kind': 'user'},
        'light': {
          'default_transition_ms': 400,
          'default_output': {
            'power': 'on',
            'brightness': 72,
            'color': {'kind': 'kelvin', 'kelvin': 6500},
          },
          'entries': [
            {
              'target': {'kind': 'node', 'node_id': 'light-node-1'},
              'output': {'power': 'off', 'transition_ms': 250},
            },
          ],
        },
        'extensions': {'client': 'flutter'},
      });

      expect(scene.id, 'icy-glow');
      expect(scene.source.kind, RhythmSceneSourceKind.user);
      expect(scene.light.defaultTransitionMs, 400);
      expect(scene.light.defaultOutput?.power, RhythmScenePower.on);
      expect(scene.light.entries.single.target.nodeId, 'light-node-1');
      expect(scene.light.entries.single.output.power, RhythmScenePower.off);
      expect(scene.isValidForSave, isTrue);

      expect(scene.toJson(), {
        'id': 'icy-glow',
        'name': 'Icy Glow',
        'description': null,
        'source': {'kind': 'user'},
        'light': {
          'default_transition_ms': 400,
          'default_output': {
            'power': 'on',
            'brightness': 72,
            'color': {'kind': 'kelvin', 'kelvin': 6500},
          },
          'entries': [
            {
              'target': {'kind': 'node', 'node_id': 'light-node-1'},
              'output': {'power': 'off', 'transition_ms': 250},
            },
          ],
        },
        'extensions': {'client': 'flutter'},
      });
    });

    test('parses imported source and rgb_xy color', () {
      final scene = RhythmSceneDefinition.fromJson({
        'id': 'hue-scene',
        'name': 'Hue Scene',
        'source': {
          'kind': 'imported',
          'provider': 'hue',
          'external_id': 'abc',
        },
        'extensions': {'hue_palette_scene': true},
        'light': {
          'entries': [
            {
              'target': {'kind': 'node', 'node_id': 'light-1'},
              'output': {
                'power': 'on',
                'brightness': 80,
                'color': {
                  'kind': 'rgb_xy',
                  'rgb': {'r': 255, 'g': 180, 'b': 120},
                  'xy': {'x': 0.45, 'y': 0.41},
                },
              },
            },
          ],
        },
      });

      final color = scene.light.entries.single.output.color;
      expect(scene.source.kind, RhythmSceneSourceKind.imported);
      expect(scene.source.provider, 'hue');
      expect(scene.source.externalId, 'abc');
      expect(scene.isImportedHueScene, isTrue);
      expect(scene.isHuePaletteScene, isTrue);
      expect(color?.kind, RhythmLightColorKind.rgbXy);
      expect(color?.rgb?.r, 255);
      expect(color?.xy?.x, 0.45);
    });

    test('requires the palette marker to recognize an imported Hue scene', () {
      final unmarked = RhythmSceneDefinition.fromJson({
        'id': 'legacy-hue-scene',
        'name': 'Legacy Hue Scene',
        'source': {
          'kind': 'imported',
          'provider': 'Hue',
          'external_id': 'legacy',
        },
      });
      final otherProvider = RhythmSceneDefinition.fromJson({
        'id': 'ha-scene',
        'name': 'Home Assistant Scene',
        'source': {
          'kind': 'imported',
          'provider': 'homeassistant',
          'external_id': 'scene.relax',
        },
        'extensions': {'hue_palette_scene': true},
      });

      expect(unmarked.isImportedHueScene, isTrue);
      expect(unmarked.isHuePaletteScene, isFalse);
      expect(otherProvider.isImportedHueScene, isFalse);
      expect(otherProvider.isHuePaletteScene, isFalse);
    });
  });
}
