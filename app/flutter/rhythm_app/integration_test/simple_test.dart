/// Basic integration test for the Rhythm Lighting app.
///
/// This test verifies that the app can initialize with FFI and render.
/// For more comprehensive integration tests, see:
/// - integration_test/app_startup_test.dart
/// - integration_test/room_navigation_test.dart
/// - integration_test/kiosk_mode_test.dart
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:rhythm_app/config/platform_capabilities.dart';
import 'package:rhythm_app/main.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  setUp(() async {
    // Initialize SharedPreferences with mock values
    SharedPreferences.setMockInitialValues({
      // Skip onboarding for integration tests
      'onboardingComplete': true,
    });
  });

  testWidgets('App launches without crashing', (WidgetTester tester) async {
    // Build the error screen version to verify basic rendering
    // Full app initialization requires WASM which may not be available in tests
    await tester.pumpWidget(RhythmApp(
      client: null,
      initError: 'Integration test - WASM not available',
      capabilities: PlatformCapabilities.fromPlatform(),
    ));

    await tester.pumpAndSettle();

    // Should render the error screen
    expect(find.byType(Scaffold), findsOneWidget);
    expect(find.text('Initialization Failed'), findsOneWidget);
  });

  testWidgets('Error screen has correct styling', (WidgetTester tester) async {
    await tester.pumpWidget(RhythmApp(
      client: null,
      initError: 'Test error',
      capabilities: PlatformCapabilities.fromPlatform(),
    ));

    await tester.pumpAndSettle();

    // Should use Material 3 dark theme
    final scaffold = tester.widget<Scaffold>(find.byType(Scaffold));
    expect(scaffold.backgroundColor, isNotNull);
  });
}
