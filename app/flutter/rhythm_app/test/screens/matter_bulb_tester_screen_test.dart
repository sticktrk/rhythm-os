import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/hubs/matter_bulb_tester_screen.dart';
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
        home: MatterBulbTesterScreen(
          device: device,
          nativeDeviceId: 'matter-1-1',
        ),
      ),
    );
  }

  testWidgets('shows a binary, state-owned 21-step plan', (tester) async {
    await pumpTester(tester);

    expect(find.text('1/21'), findsOneWidget);
    expect(find.text('Yes'), findsOneWidget);
    expect(find.text('No'), findsOneWidget);
    expect(find.text('Unsure'), findsNothing);

    await tester.drag(find.byType(ListView), const Offset(0, -600));
    await tester.pumpAndSettle();
    final brightnessStep = find
        .byKey(const ValueKey('matter-bulb-test-step-brightness_without_on'));
    await tester.ensureVisible(brightnessStep);
    await tester.pumpAndSettle();
    await tester.tap(brightnessStep);
    await tester.pump();
    tester
        .state<ScrollableState>(find.byType(Scrollable).first)
        .position
        .jumpTo(0);
    await tester.pumpAndSettle();

    expect(find.text('3/21'), findsOneWidget);
    expect(
      find.text('The tester turns the bulb off first.'),
      findsAtLeastNWidgets(1),
    );
  });

  testWidgets('No on XY red removes every later XY-dependent step',
      (tester) async {
    await pumpTester(tester);

    await tester.drag(find.byType(ListView), const Offset(0, -600));
    await tester.pumpAndSettle();
    final xyStep = find.byKey(const ValueKey('matter-bulb-test-step-xy_red'));
    await tester.ensureVisible(xyStep);
    await tester.pumpAndSettle();
    await tester.tap(xyStep);
    await tester.pump();
    tester
        .state<ScrollableState>(find.byType(Scrollable).first)
        .position
        .jumpTo(0);
    await tester.pumpAndSettle();
    expect(
      find.textContaining('no more XY commands will run'),
      findsOneWidget,
    );
    await tester.tap(find.text('No'));
    await tester.pump();

    expect(find.text('19/19'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('matter-bulb-test-step-ct_to_xy')),
      findsNothing,
    );
    expect(
      find.byKey(const ValueKey('matter-bulb-test-step-xy_to_ct')),
      findsNothing,
    );

    await tester.tap(find.text('Yes'));
    await tester.pump();
    expect(find.text('20/21'), findsOneWidget);
    expect(find.text('CT to XY'), findsAtLeastNWidgets(1));
  });
}
