import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/config/platform_capabilities.dart';
import 'package:rhythm_app/main.dart';

void main() {
  testWidgets('startup loading surface renders in a landscape viewport',
      (tester) async {
    tester.view.physicalSize = const Size(844, 390);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    final startup = Completer<RhythmStartupResult>();
    await tester.pumpWidget(RhythmBootstrap(
      capabilities: PlatformCapabilities.fromPlatform(),
      initializer: (_) => startup.future,
    ));

    expect(tester.view.physicalSize.width,
        greaterThan(tester.view.physicalSize.height));
    expect(find.byKey(const Key('rhythm_startup_loading')), findsOneWidget);
    expect(find.text('Connecting to your lights'), findsOneWidget);

    startup.complete(const RhythmStartupResult(
      initError: 'Visual fixture complete',
    ));
    await tester.pump();
  });
}
