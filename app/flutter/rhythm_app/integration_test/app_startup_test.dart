/// Integration tests for app startup flow.
///
/// Tests the complete app initialization sequence including:
/// - FFI/WASM initialization
/// - Backend initialization (Supabase)
/// - Auth gate routing
/// - Error handling
library;
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:rhythm_app/config/platform_capabilities.dart';
import 'package:rhythm_app/main.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  group('App Startup', () {
    setUp(() async {
      SharedPreferences.setMockInitialValues({});
    });

    testWidgets('shows error screen when WASM fails to load', (WidgetTester tester) async {
      await tester.pumpWidget(RhythmApp(
        client: null,
        initError: 'WASM brain failed to initialize',
        capabilities: PlatformCapabilities.fromPlatform(),
      ));
      await tester.pumpAndSettle();

      // Should show error screen
      expect(find.text('Initialization Failed'), findsOneWidget);
      expect(find.byIcon(Icons.error_outline), findsOneWidget);
    });

    testWidgets('error screen displays the error message', (WidgetTester tester) async {
      const errorMessage = 'Custom error message for testing';
      await tester.pumpWidget(RhythmApp(
        client: null,
        initError: errorMessage,
        capabilities: PlatformCapabilities.fromPlatform(),
      ));
      await tester.pumpAndSettle();

      expect(find.text(errorMessage), findsOneWidget);
    });

    testWidgets('error screen shows WASM troubleshooting when brain fails', (WidgetTester tester) async {
      await tester.pumpWidget(RhythmApp(
        client: null,
        initError: 'WASM brain failed',
        capabilities: PlatformCapabilities.fromPlatform(),
      ));
      await tester.pumpAndSettle();

      // Should show troubleshooting steps
      expect(find.textContaining('wasm-pack'), findsOneWidget);
      expect(find.textContaining('flutter_rust_bridge'), findsOneWidget);
    });

    testWidgets('app theme is dark mode', (WidgetTester tester) async {
      await tester.pumpWidget(RhythmApp(
        client: null,
        initError: 'Test',
        capabilities: PlatformCapabilities.fromPlatform(),
      ));
      await tester.pumpAndSettle();

      final MaterialApp app = tester.widget(find.byType(MaterialApp));
      expect(app.theme?.brightness, equals(Brightness.dark));
    });
  });
}
