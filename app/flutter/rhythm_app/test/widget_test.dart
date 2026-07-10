/// Basic smoke test for the Rhythm Lighting app.
///
/// This test verifies that the app can start without crashing.
/// For more comprehensive tests, see the test/ subdirectories:
/// - test/unit/ - Unit tests for models and utilities
/// - test/providers/ - Provider state management tests
/// - test/widgets/ - Widget rendering and interaction tests
/// - test/ffi/ - FFI binding tests
library;

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:rhythm_app/config/platform_capabilities.dart';
import 'package:rhythm_app/main.dart';

void main() {
  setUp(() async {
    // Initialize SharedPreferences with mock values
    SharedPreferences.setMockInitialValues({});
  });

  testWidgets('App shows error screen when client is null',
      (WidgetTester tester) async {
    // Build the app with no client (simulates initialization failure)
    await tester.pumpWidget(RhythmApp(
      client: null,
      initError: 'Test initialization error',
      capabilities: PlatformCapabilities.fromPlatform(),
    ));

    // Wait for any async operations
    await tester.pumpAndSettle();

    // Should show error screen with error message
    expect(find.text('Initialization Failed'), findsOneWidget);
    expect(find.text('Test initialization error'), findsOneWidget);
  });

  testWidgets('App shows error icon on initialization failure',
      (WidgetTester tester) async {
    await tester.pumpWidget(RhythmApp(
      client: null,
      initError: 'WASM brain failed to initialize',
      capabilities: PlatformCapabilities.fromPlatform(),
    ));
    await tester.pumpAndSettle();

    // Should show error icon
    expect(find.byIcon(Icons.error_outline), findsOneWidget);
  });

  testWidgets('Error screen shows troubleshooting steps',
      (WidgetTester tester) async {
    await tester.pumpWidget(RhythmApp(
      client: null,
      initError: 'Test error',
      capabilities: PlatformCapabilities.fromPlatform(),
    ));
    await tester.pumpAndSettle();

    // Should show troubleshooting information
    expect(find.textContaining('wasm-pack'), findsOneWidget);
  });

  testWidgets(
      'Bootstrap releases the native launch screen before startup completes',
      (WidgetTester tester) async {
    final startup = Completer<RhythmStartupResult>();

    await tester.pumpWidget(RhythmBootstrap(
      capabilities: PlatformCapabilities.fromPlatform(),
      initializer: (_) => startup.future,
    ));

    expect(find.byKey(const Key('rhythm_startup_loading')), findsOneWidget);
    expect(find.byKey(const Key('hub_connection_loading')), findsOneWidget);
    expect(find.text('Setting up...'), findsOneWidget);
    expect(find.text('Connecting to your lights'), findsOneWidget);
    expect(find.byType(CircularProgressIndicator), findsNothing);

    startup.complete(const RhythmStartupResult(
      initError: 'Test initialization complete',
    ));
    await tester.pump();

    expect(find.text('Initialization Failed'), findsOneWidget);
    expect(find.text('Test initialization complete'), findsOneWidget);
  });
}
