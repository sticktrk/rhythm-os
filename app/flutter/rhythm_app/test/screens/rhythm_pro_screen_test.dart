import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/settings/rhythm_pro_screen.dart';
import 'package:rhythm_app/widgets/settings_row.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class _Api extends Fake implements RhythmAuthApi {
  var reads = 0;
  final changes = <(bool?, bool?)>[];
  var fail = false;

  @override
  Future<RhythmProSettings> getProSettings() async {
    reads++;
    return const RhythmProSettings(bulbProfileUpdates: true, remoteMcp: false);
  }

  @override
  Future<RhythmProSettings> updateProSettings(
      {bool? bulbProfileUpdates, bool? remoteMcp}) async {
    changes.add((bulbProfileUpdates, remoteMcp));
    if (fail) throw StateError('unreachable');
    return RhythmProSettings(
        bulbProfileUpdates: bulbProfileUpdates ?? true,
        remoteMcp: remoteMcp ?? false);
  }
}

void main() {
  testWidgets(
      'controls use the selected local hub and recover from unconfirmed writes',
      (tester) async {
    final api = _Api();
    final hub = Hub.server(
        id: 'test', homeId: 'home', name: 'Test hub', host: 'hub.example.test');
    Future<void> show(bool local) async {
      await tester.pumpWidget(MaterialApp(
          home: Scaffold(
              body: RhythmProHubControls(hub: hub, api: api, isLocal: local))));
      await tester.pumpAndSettle();
    }

    await show(false);
    expect(api.reads, 0);
    expect(find.byType(Switch), findsNothing);
    await show(true);
    expect(api.reads, 1);
    expect(find.byType(Switch), findsNWidgets(2));
    // Pro controls are settings rows like the rest of Settings.
    expect(find.byType(SettingsRow), findsNWidgets(2));
    expect(find.byType(ListTile), findsNothing);
    await tester.tap(find.text('Remote MCP'));
    await tester.pumpAndSettle();
    expect(api.changes, [(null, true)]);
    expect(find.text('Manage agent credentials'), findsOneWidget);
    api.fail = true;
    await tester.tap(find.text('Bulb profile updates'));
    await tester.pumpAndSettle();
    expect(api.changes.last, (false, null));
    expect(find.byType(Switch), findsNothing);
    await tester.tap(find.byTooltip('Refresh hub settings'));
    await tester.pumpAndSettle();
    expect(api.reads, 2);
    expect(find.byType(Switch), findsNWidgets(2));
  });
}
