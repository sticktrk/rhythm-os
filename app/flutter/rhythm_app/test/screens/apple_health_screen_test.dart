import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/settings/apple_health_screen.dart';
import 'package:rhythm_app/services/apple_health_service.dart';
import 'package:rhythm_app/services/apple_health_sync_controller.dart';

import '../helpers/fake_apple_health_writer.dart';

void main() {
  testWidgets(
      'explicit source and consent, denial and retry work on a compact screen',
      (tester) async {
    tester.view.physicalSize = const Size(360, 780);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    const environment = AppleHealthScope(
        accountId: 'owner', homeId: 'home', serverId: 'box', nodeId: 'global');
    Object? preference;
    final writer = FakeAppleHealthWriter()
      ..state = AppleHealthAuthorization.denied;
    final controller = AppleHealthSyncController(
        writer: writer,
        currentEnvironment: () => environment,
        readPreference: () => preference,
        writePreference: (value) async {
          preference = value;
        },
        loadHistory: (_, __) async =>
            const AppleHealthHistorySnapshot(events: []));
    addTearDown(controller.dispose);
    await tester.pumpWidget(MaterialApp(
        home: Scaffold(
            body: AppleHealthControls(controller: controller, sources: const {
      'global': 'Home buttons',
      'bedroom': 'My bedroom'
    }))));
    expect(tester.widget<SwitchListTile>(find.byType(SwitchListTile)).onChanged,
        isNull);
    expect(find.textContaining('do not measure sleep'), findsOneWidget);
    await tester.tap(find.byType(DropdownButtonFormField<String>));
    await tester.pumpAndSettle();
    await tester.tap(find.text('My bedroom').last);
    await tester.pumpAndSettle();
    await tester.tap(find.byType(Switch));
    await tester.pumpAndSettle();
    expect(controller.enabled, isFalse);
    expect(find.textContaining('Allow Rhythm to write Sleep data'),
        findsOneWidget);
    writer.state = AppleHealthAuthorization.authorized;
    await tester.tap(find.byType(Switch));
    await tester.pumpAndSettle();
    expect(controller.enabled, isTrue);
    expect(controller.scope?.nodeId, 'bedroom');
    expect(find.text('Sync now'), findsOneWidget);
    expect(find.textContaining('No completed'), findsOneWidget);
    await tester.tap(find.byType(Switch));
    await tester.pumpAndSettle();
    expect(controller.enabled, isFalse);
    expect(preference, isNull);
    expect(tester.takeException(), isNull);
  });
}
