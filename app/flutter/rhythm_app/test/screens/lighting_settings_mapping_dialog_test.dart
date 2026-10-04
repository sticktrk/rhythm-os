import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/settings/sections/lighting_settings_mapping_dialog.dart';

void main() {
  test('settings warnings use readable copy without exposing unknown codes', () {
    final text = lightingSettingsWarnings([
      'native_scenes_excluded', 'unmapped_scene_entries_excluded',
      'unmapped_mode_defaults_excluded', 'private-custom-code',
    ]).join(' ');
    expect(text, contains('Home Assistant'));
    expect(text, contains('unmatched rooms'));
    expect(text, isNot(contains('_')));
    expect(text, isNot(contains('private-custom-code')));
  });
  testWidgets(
    'matching names stay skipped until reviewed; room and light targets stay separate',
    (tester) async {
      Map<String, String>? result;
      await tester.pumpWidget(
        MaterialApp(
          home: Builder(
            builder: (context) => Scaffold(
              body: TextButton(
                child: const Text('Open'),
                onPressed: () async {
                  result = await showDialog<Map<String, String>>(
                    context: context,
                    builder: (_) => const LightingSettingsMappingDialog(
                      source: {
                        'nodes': [
                          {'id': 'old-room', 'name': 'Kitchen', 'kind': 'room'},
                          {
                            'id': 'old-light',
                            'name': 'Pendant',
                            'kind': 'light_device',
                          },
                        ],
                      },
                      target: {
                        'nodes': [
                          {'id': 'ha-room', 'name': 'Kitchen', 'kind': 'room'},
                          {
                            'id': 'ha-light',
                            'name': 'Pendant',
                            'kind': 'light_device',
                          },
                        ],
                      },
                    ),
                  );
                },
              ),
            ),
          ),
        ),
      );
      await tester.tap(find.text('Open'));
      await tester.pumpAndSettle();
      expect(find.text('Skip for now'), findsNWidgets(2));
      await tester.tap(find.byKey(const ValueKey('mapping-old-room')));
      await tester.pumpAndSettle();
    expect(find.text('Pendant').hitTestable(), findsNothing);
    await tester.tap(find.text('Kitchen').last);
      await tester.pumpAndSettle();
      await tester.tap(find.text('Restore settings'));
      await tester.pumpAndSettle();
      expect(result, {'old-room': 'ha-room'});
    },
  );

  testWidgets('long duplicate names keep room and ID labels readable on phones',
      (tester) async {
    final semantics = tester.ensureSemantics();
    tester.view.physicalSize = const Size(390, 844);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    const lightName = 'Ceiling pendant above the reading armchair';
    const roomName = 'Main downstairs living and dining area';
    const sourceLabel = '$lightName · $roomName · 11111111 (light)';
    const firstTargetLabel = '$lightName · $roomName · 00000001';
    const secondTargetLabel = '$lightName · $roomName · 00000002';
    const thirdTargetLabel = '$lightName · Bedroom';
    Map<String, String>? result;
    await tester.pumpWidget(MaterialApp(
      debugShowCheckedModeBanner: false,
      theme: ThemeData(
        useMaterial3: true,
        brightness: Brightness.dark,
        colorScheme: ColorScheme.fromSeed(
          seedColor: const Color(0xFFF9A825),
          brightness: Brightness.dark,
        ),
        scaffoldBackgroundColor: const Color(0xFF1A1A2E),
      ),
      home: Builder(
        builder: (context) => Scaffold(
          body: TextButton(
            child: const Text('Open'),
            onPressed: () async {
              result = await showDialog<Map<String, String>>(
                context: context,
                builder: (_) => const LightingSettingsMappingDialog(
                  source: {
                    'nodes': [
                      {'id': 'old-room', 'name': roomName, 'kind': 'room'},
                      {
                        'id': 'old-light-11111111',
                        'name': lightName,
                        'parent_id': 'old-room',
                        'kind': 'light_device',
                      },
                      {
                        'id': 'old-light-22222222',
                        'name': lightName,
                        'parent_id': 'old-room',
                        'kind': 'light_device',
                      },
                    ],
                  },
                  target: {
                    'nodes': [
                      {'id': 'ha-room', 'name': roomName, 'kind': 'room'},
                      {'id': 'ha-bedroom', 'name': 'Bedroom', 'kind': 'room'},
                      {
                        'id': 'ha-light-00000001',
                        'name': lightName,
                        'parent_id': 'ha-room',
                        'kind': 'light_device',
                      },
                      {
                        'id': 'ha-light-00000002',
                        'name': lightName,
                        'parent_id': 'ha-room',
                        'kind': 'light_device',
                      },
                      {
                        'id': 'ha-light-00000003',
                        'name': lightName,
                        'parent_id': 'ha-bedroom',
                        'kind': 'light_device',
                      },
                    ],
                  },
                ),
              );
            },
          ),
        ),
      ),
    ));
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    final firstMapping =
        find.byKey(const ValueKey('mapping-old-light-11111111'));
    await tester.ensureVisible(find.text(sourceLabel));
    await tester.pumpAndSettle();
    _expectFullMultilineText(tester, sourceLabel);
    await _captureMappingEvidence(tester, 'phone-source-labels');
    await tester.ensureVisible(firstMapping);
    expect(tester.getSemantics(firstMapping).label, contains(sourceLabel));
    await tester.tap(firstMapping);
    await tester.pumpAndSettle();
    for (final label in [
      firstTargetLabel,
      secondTargetLabel,
      thirdTargetLabel
    ]) {
      await tester.ensureVisible(find.text(label).last);
      await tester.pumpAndSettle();
      _expectFullMultilineText(tester, label);
    }
    // Rooms cannot be chosen for a light, even when their names match.
    expect(find.text(roomName).hitTestable(), findsNothing);
    await _captureMappingEvidence(tester, 'phone-target-choices');
    await tester.ensureVisible(find.text(secondTargetLabel).last);
    await tester.tap(find.text(secondTargetLabel).hitTestable());
    await tester.pumpAndSettle();
    _expectFullMultilineText(tester, secondTargetLabel);
    await _captureMappingEvidence(tester, 'phone-selected-target');
    final secondMapping =
        find.byKey(const ValueKey('mapping-old-light-22222222'));
    await tester.ensureVisible(secondMapping);
    await tester.tap(secondMapping);
    await tester.pumpAndSettle();
    expect(find.text(secondTargetLabel).hitTestable(), findsNothing);
    await tester.tap(find.text(thirdTargetLabel).hitTestable());
    await tester.pumpAndSettle();
    await tester.tap(find.text('Restore settings'));
    await tester.pumpAndSettle();
    expect(result, {
      'old-light-11111111': 'ha-light-00000002',
      'old-light-22222222': 'ha-light-00000003',
    });
    expect(tester.takeException(), isNull);
    semantics.dispose();
  });
}

void _expectFullMultilineText(WidgetTester tester, String text) {
  final finder = find.text(text).hitTestable();
  expect(finder, findsOneWidget);
  final paragraph = tester.renderObject<RenderParagraph>(finder);
  expect(paragraph.didExceedMaxLines, isFalse,
      reason: 'The identifying room and ID suffix must remain visible.');
  final boxes = paragraph.getBoxesForSelection(
    TextSelection(baseOffset: 0, extentOffset: text.length),
  );
  expect(boxes.map((box) => box.top).toSet().length, greaterThan(1));
  expect(boxes.last.bottom, lessThanOrEqualTo(paragraph.size.height));
}

Future<void> _captureMappingEvidence(WidgetTester tester, String name) async {
  final directory =
      Platform.environment['RHYTHM_LIGHTING_MAPPING_SCREENSHOT_DIR'];
  if (directory == null || directory.isEmpty) return;
  await expectLater(
    find.byType(Overlay),
    matchesGoldenFile('$directory/$name.png'),
  );
}
