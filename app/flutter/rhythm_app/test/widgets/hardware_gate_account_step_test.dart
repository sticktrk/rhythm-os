import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/widgets/hardware_gate_screen.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:shared_preferences/shared_preferences.dart';

/// Just enough [HomeProvider] for [ConnectHubScreen] after the gate advances.
class _FakeHomeProvider extends HomeProvider {
  @override
  Home? get currentHome => null;

  @override
  List<Hub> get currentHomeHubs => const [];

  @override
  Hub? getFirstHubOfType(HubType type) => null;
}

Future<void> _pumpGate(
  WidgetTester tester, {
  required bool requiresAccount,
}) async {
  await tester.pumpWidget(
    ChangeNotifierProvider<HomeProvider>(
      create: (_) => _FakeHomeProvider(),
      child: MaterialApp(
        home: HardwareOnboardingGate(
          requiresAccountForConnect: () => requiresAccount,
        ),
      ),
    ),
  );
  await tester.pump(const Duration(seconds: 1));
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUp(() {
    SharedPreferences.setMockInitialValues({});
  });

  testWidgets('choosing "I have one" requires sign-in before the connect step',
      (tester) async {
    await _pumpGate(tester, requiresAccount: true);

    await tester.tap(find.text('I have one'));
    await tester.pump(const Duration(milliseconds: 600));

    // The account step (first-run copy) is shown instead of the connect step.
    expect(find.text('Create Your Account'), findsOneWidget);
    expect(find.text('Continue with Google'), findsOneWidget);

    // No way past it besides signing in — but backing out to the gate works.
    expect(find.text('Continue local-only'), findsNothing);
    await tester.tap(find.byIcon(Icons.arrow_back_rounded));
    // First pump swaps the switcher's child and starts the 380ms transition;
    // the next pumps run it to completion so the outgoing child is removed.
    await tester.pump(const Duration(milliseconds: 100));
    await tester.pump(const Duration(milliseconds: 500));
    await tester.pump(const Duration(milliseconds: 100));
    expect(find.text('I have one'), findsOneWidget);
    expect(find.text('Create Your Account'), findsNothing);
  });

  testWidgets('signed-in (or exempt) users go straight to the connect step',
      (tester) async {
    await _pumpGate(tester, requiresAccount: false);

    await tester.tap(find.text('I have one'));
    await tester.pump(const Duration(milliseconds: 600));

    expect(find.text('Create Your Account'), findsNothing);
    expect(find.text('I have one'), findsNothing);

    // The connect screen legitimately starts its bounded discovery sweep when
    // it becomes visible. Let the unsupported desktop test adapters hit their
    // deadlines so no discovery timers leak out of the widget test.
    await tester.pump(const Duration(seconds: 4));
    await tester.pump(const Duration(seconds: 4));
  });
}
