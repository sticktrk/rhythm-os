import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/settings/light_profile_screen.dart';

void main() {
  testWidgets('minimum color temperature can meet maximum at 6500K', (
    tester,
  ) async {
    var minValue = 1800.0;
    const maxValue = 6500.0;

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Center(
            child: SizedBox(
              width: 300,
              child: StatefulBuilder(
                builder: (context, setState) =>
                    LightProfileColorTemperatureRangeBar(
                      minValue: minValue,
                      maxValue: maxValue,
                      hardMin: 500,
                      hardMax: 6500,
                      tint: Colors.orange,
                      divisions: 60,
                      onMinChanged: (value) => setState(() => minValue = value),
                      onMaxChanged: (_) {},
                    ),
              ),
            ),
          ),
        ),
      ),
    );

    final rangeBar = find.byType(LightProfileColorTemperatureRangeBar);
    final gestureDetector = find.descendant(
      of: rangeBar,
      matching: find.byType(GestureDetector),
    );
    final topLeft = tester.getTopLeft(gestureDetector);
    final size = tester.getSize(gestureDetector);
    const thumbInset = 11.0;
    final usableWidth = size.width - thumbInset * 2;
    final minFraction = (minValue - 500) / (6500 - 500);
    final start =
        topLeft +
        Offset(thumbInset + minFraction * usableWidth, size.height / 2);
    final end = topLeft + Offset(size.width - thumbInset, size.height / 2);

    final gesture = await tester.startGesture(start);
    await gesture.moveTo(end);
    await gesture.up();
    await tester.pump();

    expect(minValue, 6500);
  });
}
