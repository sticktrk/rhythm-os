import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/settings/light_profile_screen.dart';

void main() {
  testWidgets('Day profile maximum brightness can be dragged to 2%', (
    tester,
  ) async {
    var maxBrightness = 100.0;

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: StatefulBuilder(
            builder: (context, setState) {
              return Center(
                child: SizedBox(
                  width: 400,
                  child: LightProfileBrightnessRangeBar(
                    minValue: 1,
                    maxValue: maxBrightness,
                    tint: Colors.amber,
                    onMinChanged: (_) {},
                    onMaxChanged: (value) {
                      setState(() => maxBrightness = value);
                    },
                  ),
                ),
              );
            },
          ),
        ),
      ),
    );

    final gestureDetector = find.descendant(
      of: find.byType(LightProfileBrightnessRangeBar),
      matching: find.byType(GestureDetector),
    );
    final rangeRect = tester.getRect(gestureDetector);

    await tester.dragFrom(
      Offset(rangeRect.right - 11, rangeRect.center.dy),
      Offset(-rangeRect.width, 0),
    );
    await tester.pump();

    expect(maxBrightness, 2);
  });

  testWidgets('Day profile minimum brightness can be dragged above 50%', (
    tester,
  ) async {
    var minBrightness = 20.0;
    const maxBrightness = 90.0;

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Center(
            child: SizedBox(
              width: 400,
              child: StatefulBuilder(
                builder: (context, setState) {
                  return LightProfileBrightnessRangeBar(
                    minValue: minBrightness,
                    maxValue: maxBrightness,
                    tint: Colors.amber,
                    onMinChanged: (value) {
                      setState(() => minBrightness = value);
                    },
                    onMaxChanged: (_) {},
                  );
                },
              ),
            ),
          ),
        ),
      ),
    );

    final gestureDetector = find.descendant(
      of: find.byType(LightProfileBrightnessRangeBar),
      matching: find.byType(GestureDetector),
    );
    final topLeft = tester.getTopLeft(gestureDetector);
    final size = tester.getSize(gestureDetector);
    const thumbInset = 11.0;
    final usableWidth = size.width - thumbInset * 2;
    Offset positionFor(double value) =>
        topLeft +
        Offset(thumbInset + ((value - 1) / 99) * usableWidth, size.height / 2);

    final gesture = await tester.startGesture(positionFor(minBrightness));
    await gesture.moveTo(positionFor(80));
    await gesture.up();
    await tester.pump();

    expect(minBrightness, 80);
  });

  testWidgets('range bars pinpoint and label current room output', (
    tester,
  ) async {
    const evidenceKey = ValueKey('light-profile-current-value-evidence');
    final screenshotPath =
        Platform.environment['RHYTHM_LIGHT_PROFILE_RANGE_SCREENSHOT'];

    await tester.pumpWidget(
      const MaterialApp(
        home: Scaffold(
          backgroundColor: Color(0xFF111318),
          body: Center(
            child: RepaintBoundary(
              key: evidenceKey,
              child: ColoredBox(
                color: Color(0xFF1A1D24),
                child: SizedBox(
                  width: 360,
                  child: Padding(
                    padding: EdgeInsets.all(20),
                    child: Column(
                      mainAxisSize: MainAxisSize.min,
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        Text(
                          'Brightness range',
                          style: TextStyle(color: Colors.white),
                        ),
                        LightProfileBrightnessRangeBar(
                          minValue: 62,
                          maxValue: 92,
                          currentValue: 83,
                          tint: Colors.amber,
                          onMinChanged: _ignoreValue,
                          onMaxChanged: _ignoreValue,
                        ),
                        SizedBox(height: 18),
                        Text(
                          'Color temperature range',
                          style: TextStyle(color: Colors.white),
                        ),
                        LightProfileColorTemperatureRangeBar(
                          minValue: 2500,
                          maxValue: 4000,
                          currentValue: 3120,
                          hardMin: 1800,
                          hardMax: 6500,
                          tint: Colors.orange,
                          onMinChanged: _ignoreValue,
                          onMaxChanged: _ignoreValue,
                        ),
                      ],
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );

    expect(find.text('Current 83%'), findsOneWidget);
    expect(find.text('Current 3120K'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('light-profile-range-current-marker')),
      findsNWidgets(2),
    );

    if (screenshotPath != null && screenshotPath.isNotEmpty) {
      await expectLater(
        find.byKey(evidenceKey),
        matchesGoldenFile(Uri.file(screenshotPath)),
      );
    }
  });
}

void _ignoreValue(double _) {}
