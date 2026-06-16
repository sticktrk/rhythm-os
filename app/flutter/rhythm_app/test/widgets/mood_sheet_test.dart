import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/widgets/mood_sheet.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

RhythmSceneDefinition _scene(String id) => RhythmSceneDefinition(
      id: id,
      name: 'Evening Glow',
      light: const RhythmLightScene(
        defaultOutput: RhythmLightSceneOutput.on(
          brightness: 60,
          color: RhythmLightColor.rgb(
            RhythmSceneRgbColor(r: 240, g: 80, b: 24),
          ),
        ),
      ),
    );

RhythmSceneDefinition _decodedPaletteScene(String id) =>
    RhythmSceneDefinition.fromJson({
      'id': id,
      'name': 'Color Carnival',
      'light': {
        'palette': [
          {
            'power': 'on',
            'brightness': 72,
            'color': {
              'kind': 'rgb',
              'rgb': {'r': 10, 'g': 20, 'b': 30},
            },
          },
          {
            'power': 'on',
            'brightness': 72,
            'color': {
              'kind': 'rgb',
              'rgb': {'r': 200, 'g': 40, 'b': 120},
            },
          },
        ],
      },
    });

void main() {
  test('scene swatch samples palette outputs', () {
    expect(
      rhythmSceneSwatch(_decodedPaletteScene('color-carnival')),
      const [Color(0xFF0A141E), Color(0xFFC82878)],
    );
  });

  testWidgets('choosing a color clears the active scene selection',
      (tester) async {
    final scene = _scene('evening-glow');
    var colorChangeCount = 0;

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: MoodSheet(
            initialTab: MoodTab.scenes,
            initialSceneId: scene.id,
            initialScenes: [scene],
            scenesLoader: () async => [scene],
            onColorChanged: (_) => colorChangeCount += 1,
            onSceneSelected: (_) {},
          ),
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 700));

    expect(find.byIcon(Icons.check_rounded), findsOneWidget);

    await tester.tap(find.byKey(const Key('mood_tab_color')));
    await tester.pump(const Duration(milliseconds: 320));
    await tester.tap(find.byKey(const Key('mood_color_spectrum')));
    await tester.pump(const Duration(milliseconds: 80));

    expect(colorChangeCount, 1);

    await tester.tap(find.byKey(const Key('mood_tab_scenes')));
    await tester.pump(const Duration(milliseconds: 320));

    expect(find.byIcon(Icons.check_rounded), findsNothing);
  });
}
