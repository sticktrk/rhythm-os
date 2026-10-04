import 'package:flutter/material.dart';
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
}
