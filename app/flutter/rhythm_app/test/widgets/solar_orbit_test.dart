import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/widgets/solar_orbit.dart';
import 'package:rhythm_core/rhythm_core.dart';

import '../helpers/test_wrapper.dart';
import '../mocks/mock_rhythm_api.dart';

void main() {
  group('SolarOrbit', () {
    late CurveData testCurveData;

    setUp(() {
      testCurveData = TestDataFactory.createCurveData(
        hours: List.generate(24, (i) => i.toDouble()),
        brightness: List.generate(24, (i) {
          final t = (i - 12).abs() / 12.0;
          return (100 * (1 - t * t)).round().clamp(10, 100);
        }),
        kelvin: List.generate(24, (i) {
          final t = (i - 12).abs() / 12.0;
          return (6500 - (4500 * t * t)).round().clamp(2000, 6500);
        }),
        solarNoon: 12.0,
      );
    });

    group('rendering', () {
      testWidgets('renders without curve data', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          SolarOrbit(
            curveData: null,
            selectedHour: 12.0,
            onHourChanged: (_) {},
          ),
        ));
        // Use pump() with duration instead of pumpAndSettle()
        // since the widget may have continuous animations
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.byType(SolarOrbit), findsOneWidget);
      });

      testWidgets('renders with curve data', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          SolarOrbit(
            curveData: testCurveData,
            selectedHour: 12.0,
            onHourChanged: (_) {},
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.byType(SolarOrbit), findsOneWidget);
      });

      testWidgets('renders at different hours', (tester) async {
        for (final hour in [0.0, 6.0, 12.0, 18.0, 23.0]) {
          await tester.pumpWidget(buildMinimalWidget(
            SolarOrbit(
              curveData: testCurveData,
              selectedHour: hour,
              onHourChanged: (_) {},
            ),
          ));
          await tester.pump(const Duration(milliseconds: 100));

          expect(find.byType(SolarOrbit), findsOneWidget);
        }
      });
    });

    group('light state', () {
      testWidgets('renders when light is on', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          SolarOrbit(
            curveData: testCurveData,
            selectedHour: 12.0,
            onHourChanged: (_) {},
            isLightOn: true,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.byType(SolarOrbit), findsOneWidget);
      });

      testWidgets('renders when light is off', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          SolarOrbit(
            curveData: testCurveData,
            selectedHour: 12.0,
            onHourChanged: (_) {},
            isLightOn: false,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.byType(SolarOrbit), findsOneWidget);
      });
    });

    group('callbacks', () {
      testWidgets('onSunTap is provided', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          SolarOrbit(
            curveData: testCurveData,
            selectedHour: 12.0,
            onHourChanged: (_) {},
            onSunTap: () {},
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        // Verify the widget renders with the callback
        expect(find.byType(SolarOrbit), findsOneWidget);
      });

      testWidgets('onHourChanged is required', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          SolarOrbit(
            curveData: testCurveData,
            selectedHour: 12.0,
            onHourChanged: (_) {},
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        // Verify the widget renders with the callback
        expect(find.byType(SolarOrbit), findsOneWidget);
      });
    });

    // Note: Interaction tests (drag, tap) require more complex setup
    // and may not work reliably with CustomPainter-based widgets.
    // These are better covered by integration tests.
  });
}
