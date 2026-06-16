/// Basic smoke test for the Rhythm Lighting app.
///
/// This test verifies that the app can start without crashing.
/// For more comprehensive tests, see the test/ subdirectories:
/// - test/unit/ - Unit tests for models and utilities
/// - test/providers/ - Provider state management tests
/// - test/widgets/ - Widget rendering and interaction tests
/// - test/ffi/ - FFI binding tests
library;
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

  testWidgets('App shows error screen when client is null', (WidgetTester tester) async {
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

  testWidgets('App shows error icon on initialization failure', (WidgetTester tester) async {
    await tester.pumpWidget(RhythmApp(
      client: null,
      initError: 'WASM brain failed to initialize',
      capabilities: PlatformCapabilities.fromPlatform(),
    ));
    await tester.pumpAndSettle();

    // Should show error icon
    expect(find.byIcon(Icons.error_outline), findsOneWidget);
  });

  testWidgets('Error screen shows troubleshooting steps', (WidgetTester tester) async {
    await tester.pumpWidget(RhythmApp(
      client: null,
      initError: 'Test error',
      capabilities: PlatformCapabilities.fromPlatform(),
    ));
    await tester.pumpAndSettle();

    // Should show troubleshooting information
    expect(find.textContaining('wasm-pack'), findsOneWidget);
  });
}
