import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/settings/light_profile_screen.dart';

void main() {
  testWidgets('Day profile maximum brightness can be dragged to 2%',
      (tester) async {
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
}
