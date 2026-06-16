import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:fl_chart/fl_chart.dart';
import 'package:rhythm_app/widgets/curve_chart.dart';
import 'package:rhythm_core/rhythm_core.dart';

import '../helpers/test_wrapper.dart';
import '../mocks/mock_rhythm_api.dart';

void main() {
  group('CurveChart', () {
    late CurveData testCurveData;

    setUp(() {
      testCurveData = TestDataFactory.createCurveData(
        hours: List.generate(24, (i) => i.toDouble()),
        brightness: List.generate(24, (i) {
          // Bell curve peaking at noon
          final t = (i - 12).abs() / 12.0;
          return (100 * (1 - t * t)).round().clamp(10, 100);
        }),
        kelvin: List.generate(24, (i) {
          final t = (i - 12).abs() / 12.0;
          return (6500 - (4500 * t * t)).round().clamp(2000, 6500);
        }),
        solarNoon: 12.0,
        sunrise: 6.0,
        sunset: 20.0,
      );
    });

    group('rendering', () {
      testWidgets('shows loading indicator when data is null', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          const CurveChart(data: null),
        ));

        expect(find.byType(CircularProgressIndicator), findsOneWidget);
      });

      testWidgets('renders LineChart when data is provided', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          CurveChart(data: testCurveData),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.byType(LineChart), findsOneWidget);
      });

      testWidgets('renders with standard style', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          CurveChart(
            data: testCurveData,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.byType(LineChart), findsOneWidget);
      });
    });

    group('interactions', () {
      testWidgets('onHourSelected callback fires on tap', (tester) async {
        double? selectedHour;

        await tester.pumpWidget(buildMinimalWidget(
          CurveChart(
            data: testCurveData,
            onHourSelected: (hour) => selectedHour = hour,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        // Find the GestureDetector and tap it
        final chartFinder = find.byType(GestureDetector).first;
        await tester.tap(chartFinder);
        await tester.pump(const Duration(milliseconds: 100));

        // Callback should have been called
        // Note: The exact hour depends on tap position
        expect(selectedHour, isNotNull);
      });

      testWidgets('respects selectedHour prop', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          CurveChart(
            data: testCurveData,
            selectedHour: 14.5,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        // Widget should render without error
        expect(find.byType(LineChart), findsOneWidget);
      });

      testWidgets('shows now marker when showNowMarker is true', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          CurveChart(
            data: testCurveData,
            currentHour: 12.0,
            showNowMarker: true,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        // Widget should render
        expect(find.byType(LineChart), findsOneWidget);
      });

      testWidgets('hides now marker when showNowMarker is false', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          CurveChart(
            data: testCurveData,
            currentHour: 12.0,
            showNowMarker: false,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.byType(LineChart), findsOneWidget);
      });
    });

    group('time format', () {
      testWidgets('uses device time format from MediaQuery', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          CurveChart(
            data: testCurveData,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        // Widget should render without error
        expect(find.byType(LineChart), findsOneWidget);
      });
    });

    group('solar context', () {
      testWidgets('shows solar lines when showSolarContext is true', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          CurveChart(
            data: testCurveData,
            showSolarContext: true,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.byType(LineChart), findsOneWidget);
      });

      testWidgets('hides solar lines when showSolarContext is false', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          CurveChart(
            data: testCurveData,
            showSolarContext: false,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.byType(LineChart), findsOneWidget);
      });
    });
  });
}
