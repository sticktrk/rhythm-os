import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/widgets/bottom_nav_overlay.dart';

void main() {
  testWidgets('shows direct settings and sun buttons by default', (
    tester,
  ) async {
    var settingsTapCount = 0;
    var sunTapCount = 0;

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Stack(
            children: [
              BottomNavOverlay(
                currentPage: 0,
                totalPages: 1,
                onSettingsTap: () => settingsTapCount++,
                onSunPositionTap: () => sunTapCount++,
                onFixMyLights: () {},
              ),
            ],
          ),
        ),
      ),
    );

    expect(find.byIcon(Icons.settings), findsOneWidget);
    expect(find.byIcon(Icons.wb_sunny_rounded), findsOneWidget);
    expect(find.byIcon(Icons.auto_fix_high), findsNothing);

    await tester.tap(find.byIcon(Icons.settings));
    await tester.pump();
    expect(settingsTapCount, 1);

    await tester.tap(find.byIcon(Icons.wb_sunny_rounded));
    await tester.pump();
    expect(sunTapCount, 1);
  });
}
