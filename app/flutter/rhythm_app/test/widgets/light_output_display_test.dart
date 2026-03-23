import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/widgets/light_output_display.dart';

import '../helpers/test_wrapper.dart';

void main() {
  group('LightOutputDisplay', () {
    group('rendering', () {
      testWidgets('displays brightness percentage', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          const LightOutputDisplay(
            brightness: 75,
            kelvin: 4000,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.text('75%'), findsOneWidget);
      });

      testWidgets('displays kelvin value', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          const LightOutputDisplay(
            brightness: 80,
            kelvin: 4500,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.text('4500K'), findsOneWidget);
      });

      testWidgets('displays brightness icon', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          const LightOutputDisplay(
            brightness: 80,
            kelvin: 4000,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.byIcon(Icons.brightness_6), findsOneWidget);
      });

      testWidgets('displays thermostat icon', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          const LightOutputDisplay(
            brightness: 80,
            kelvin: 4000,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.byIcon(Icons.thermostat), findsOneWidget);
      });

      testWidgets('displays color indicator', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          const LightOutputDisplay(
            brightness: 80,
            kelvin: 4000,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        // Should have a container for the color indicator
        expect(find.byType(Container), findsWidgets);
      });
    });

    group('temperature labels', () {
      testWidgets('shows "Candlelight" for very warm temperatures', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          const LightOutputDisplay(
            brightness: 80,
            kelvin: 2000,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.text('Candlelight'), findsOneWidget);
      });

      testWidgets('shows "Warm" for warm temperatures', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          const LightOutputDisplay(
            brightness: 80,
            kelvin: 2500,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.text('Warm'), findsOneWidget);
      });

      testWidgets('shows "Soft White" for soft white', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          const LightOutputDisplay(
            brightness: 80,
            kelvin: 3000,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.text('Soft White'), findsOneWidget);
      });

      testWidgets('shows "Neutral" for neutral temperatures', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          const LightOutputDisplay(
            brightness: 80,
            kelvin: 4000,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.text('Neutral'), findsOneWidget);
      });

      testWidgets('shows "Cool White" for cool temperatures', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          const LightOutputDisplay(
            brightness: 80,
            kelvin: 5000,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.text('Cool White'), findsOneWidget);
      });

      testWidgets('shows "Daylight" for very cool temperatures', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          const LightOutputDisplay(
            brightness: 80,
            kelvin: 6500,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.text('Daylight'), findsOneWidget);
      });
    });

    group('time display', () {
      testWidgets('shows time when selectedHour is provided', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          const LightOutputDisplay(
            brightness: 80,
            kelvin: 4000,
            selectedHour: 14.5,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        // Default test environment uses 12-hour format
        expect(find.text('02:30 PM'), findsOneWidget);
      });

      testWidgets('hides time when selectedHour is null', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          const LightOutputDisplay(
            brightness: 80,
            kelvin: 4000,
            selectedHour: null,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        // Should not have PM or AM text
        expect(find.textContaining('PM'), findsNothing);
        expect(find.textContaining('AM'), findsNothing);
      });

      testWidgets('formats times correctly in 12-hour format', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          const LightOutputDisplay(
            brightness: 80,
            kelvin: 4000,
            selectedHour: 14.0,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.text('02:00 PM'), findsOneWidget);
      });

      testWidgets('formats midnight correctly', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          const LightOutputDisplay(
            brightness: 80,
            kelvin: 4000,
            selectedHour: 0.0,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.text('12:00 AM'), findsOneWidget);
      });

      testWidgets('formats noon correctly', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          const LightOutputDisplay(
            brightness: 80,
            kelvin: 4000,
            selectedHour: 12.0,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.text('12:00 PM'), findsOneWidget);
      });
    });

    group('extreme values', () {
      testWidgets('handles 0% brightness', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          const LightOutputDisplay(
            brightness: 0,
            kelvin: 4000,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.text('0%'), findsOneWidget);
      });

      testWidgets('handles 100% brightness', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          const LightOutputDisplay(
            brightness: 100,
            kelvin: 4000,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.text('100%'), findsOneWidget);
      });

      testWidgets('handles minimum kelvin', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          const LightOutputDisplay(
            brightness: 80,
            kelvin: 1000,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.text('1000K'), findsOneWidget);
      });

      testWidgets('handles maximum kelvin', (tester) async {
        await tester.pumpWidget(buildMinimalWidget(
          const LightOutputDisplay(
            brightness: 80,
            kelvin: 10000,
          ),
        ));
        await tester.pump(const Duration(milliseconds: 100));

        expect(find.text('10000K'), findsOneWidget);
      });
    });
  });

  group('LightOutputCompact', () {
    testWidgets('displays brightness', (tester) async {
      await tester.pumpWidget(buildMinimalWidget(
        const LightOutputCompact(
          brightness: 85,
          kelvin: 4500,
        ),
      ));
      await tester.pump(const Duration(milliseconds: 100));

      expect(find.text('85%'), findsOneWidget);
    });

    testWidgets('displays kelvin', (tester) async {
      await tester.pumpWidget(buildMinimalWidget(
        const LightOutputCompact(
          brightness: 85,
          kelvin: 4500,
        ),
      ));
      await tester.pump(const Duration(milliseconds: 100));

      expect(find.text('4500K'), findsOneWidget);
    });

    testWidgets('has color indicator', (tester) async {
      await tester.pumpWidget(buildMinimalWidget(
        const LightOutputCompact(
          brightness: 85,
          kelvin: 4500,
        ),
      ));
      await tester.pump(const Duration(milliseconds: 100));

      // Should have containers for layout
      expect(find.byType(Container), findsWidgets);
    });

    testWidgets('renders in a Row', (tester) async {
      await tester.pumpWidget(buildMinimalWidget(
        const LightOutputCompact(
          brightness: 85,
          kelvin: 4500,
        ),
      ));
      await tester.pump(const Duration(milliseconds: 100));

      expect(find.byType(Row), findsWidgets);
    });
  });
}
