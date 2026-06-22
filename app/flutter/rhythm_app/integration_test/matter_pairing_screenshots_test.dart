/// Captures tutorial screenshots of the Matter device pairing flow.
///
/// Run via:
///   ./tools/app/scripts/capture-tutorial-screenshots.sh
///
/// Output: flutter/rhythm_app/screenshots/*.png
library;

import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_app/screens/hubs/matter_add_method.dart';
import 'package:rhythm_app/screens/hubs/matter_device_add_screen.dart';

// MatterDeviceAddScreen runs a continuous pulse animation, so pumpAndSettle()
// would never return. Use bounded `pump()` calls instead.
void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('Matter pairing tutorial walkthrough', (tester) async {
    // Non-routable endpoint so the real-network failure path returns fast
    // (connection-refused on port 1 instead of waiting on the 45s receive timeout).
    const endpoint = HubEndpoint(host: '127.0.0.1', port: 1, useSsl: false);

    await tester.pumpWidget(
      const MaterialApp(
        debugShowCheckedModeBanner: false,
        home: MatterDeviceAddScreen(
          endpoint: endpoint,
          addMethod: MatterAddMethod.automatic,
        ),
      ),
    );

    // Lay the screen out (a couple of frames is enough).
    await tester.pump(const Duration(milliseconds: 200));

    // Android: required before takeScreenshot works.
    if (Platform.isAndroid) {
      await binding.convertFlutterSurfaceToImage();
      await tester.pump(const Duration(milliseconds: 100));
    }

    // 1. Empty input phase
    await binding.takeScreenshot('01_input_empty');

    // 2. Filled input phase
    await tester.enterText(
      find.byType(TextField),
      'MT:Y.K9042C00KA0648G00',
    );
    await tester.pump(const Duration(milliseconds: 200));
    await binding.takeScreenshot('02_input_filled');

    // 3. Pairing failure phase. "Add Device" appears as both the page header
    // text and the submit button — `.last` targets the submit button.
    await tester.tap(find.text('Add Device').last);

    final failureFinder = find.text('Pairing Failed');
    var sawFailure = false;
    for (var i = 0; i < 100; i++) {
      await tester.pump(const Duration(milliseconds: 100));
      if (failureFinder.evaluate().isNotEmpty) {
        sawFailure = true;
        break;
      }
    }

    // Let the failure-phase layout settle one more frame.
    await tester.pump(const Duration(milliseconds: 200));
    await binding.takeScreenshot(
      sawFailure ? '03_pairing_failed' : '03_pairing_in_progress',
    );
  });
}
