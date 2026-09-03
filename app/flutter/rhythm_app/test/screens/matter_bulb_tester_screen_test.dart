import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/hubs/bulb_audition_screen.dart';
import 'package:rhythm_app/screens/hubs/matter_bulb_tester_screen.dart'
    show preferredMatterColorCommand;
import 'package:rhythm_sdk/rhythm_sdk.dart';

void main() {
  const device = RhythmDevice(
    id: 'matter-light-1',
    type: RhythmDeviceType.light,
    name: 'Test bulb',
    manufacturer: 'Example',
    model: 'Color A19',
  );

  Future<void> pumpTester(WidgetTester tester) async {
    await tester.pumpWidget(
      const MaterialApp(
        home: BulbAuditionScreen(
          device: device,
          nativeDeviceId: 'matter-1-1',
        ),
      ),
    );
  }

  group('preferred Matter color command', () {
    test('keeps adaptive whites on native CT when direct color also works', () {
      expect(
        preferredMatterColorCommand(
          colorTemperatureWorked: true,
          hueSaturationWorked: true,
          xyWorked: false,
        ),
        'color_temperature',
      );
    });

    test('falls back through HS, XY, then dimming only', () {
      expect(
        preferredMatterColorCommand(
          colorTemperatureWorked: false,
          hueSaturationWorked: true,
          xyWorked: true,
        ),
        'hue_saturation',
      );
      expect(
        preferredMatterColorCommand(
          colorTemperatureWorked: false,
          hueSaturationWorked: false,
          xyWorked: true,
        ),
        'xy',
      );
      expect(
        preferredMatterColorCommand(
          colorTemperatureWorked: false,
          hueSaturationWorked: false,
          xyWorked: false,
        ),
        'onoff_or_dimming_only',
      );
    });
  });

  testWidgets('shows the 15-scenario audition and skips answers for preflight',
      (tester) async {
    await pumpTester(tester);

    expect(find.text('1/15'), findsOneWidget);
    expect(find.text('Preflight'), findsAtLeastNWidgets(1));
    expect(find.text('Yes'), findsNothing);
    expect(find.text('No'), findsNothing);

    await tester.drag(find.byType(ListView), const Offset(0, -600));
    await tester.pumpAndSettle();
    final fromOffStep =
        find.byKey(const ValueKey('matter-bulb-test-step-turn_on_from_off'));
    await tester.ensureVisible(fromOffStep);
    await tester.pumpAndSettle();
    await tester.tap(fromOffStep);
    await tester.pump();
    tester
        .state<ScrollableState>(find.byType(Scrollable).first)
        .position
        .jumpTo(0);
    await tester.pumpAndSettle();

    expect(find.text('3/15'), findsOneWidget);
    expect(find.text('Yes'), findsOneWidget);
    expect(find.text('No'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('bulb-audition-try-with')),
      findsOneWidget,
    );
    await tester.tap(find.byKey(const ValueKey('bulb-audition-try-with')));
    await tester.pumpAndSettle();
    expect(find.text('Explicit On first'), findsOneWidget);
    expect(
      find.text('Audition turns the bulb off, then runs the runtime plan.'),
      findsAtLeastNWidgets(1),
    );
  });

  testWidgets('includes subscription and power-cycle scenarios',
      (tester) async {
    await pumpTester(tester);

    await tester.drag(find.byType(ListView), const Offset(0, -600));
    await tester.pumpAndSettle();
    expect(
      find.byKey(
        const ValueKey('matter-bulb-test-step-subscription_establish'),
      ),
      findsOneWidget,
    );
    expect(
      find.byKey(
        const ValueKey('matter-bulb-test-step-power_cycle_then_tick'),
      ),
      findsOneWidget,
    );
  });
}
