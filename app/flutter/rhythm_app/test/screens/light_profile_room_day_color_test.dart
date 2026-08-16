import 'dart:ui' show SemanticsAction;

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/settings/light_profile_screen.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

void main() {
  const directColor = RhythmDirectColor(
    rgb: RhythmRgbColor(r: 255, g: 149, b: 41),
    xy: RhythmXyColor(x: 0.61, y: 0.37),
  );

  group('room Day color contract', () {
    test('derives legacy fixed white and direct-color modes', () {
      expect(
        roomDayColorModeForConfig(
          const RhythmCurveConfig(
            id: 'rhythm',
            minColorTemp: 3500,
            maxColorTemp: 3500,
          ),
        ),
        RoomDayColorMode.white,
      );
      expect(
        roomDayColorModeForConfig(
          const RhythmCurveConfig(
            id: 'rhythm',
            minColorTemp: 2200,
            maxColorTemp: 6500,
            curve: RhythmSuperGaussianCurve(directColor: directColor),
          ),
        ),
        RoomDayColorMode.color,
      );
      expect(
        roomDayColorModeForConfig(
          const RhythmCurveConfig(
            id: 'rhythm',
            minColorTemp: 2200,
            maxColorTemp: 6500,
          ),
        ),
        RoomDayColorMode.natural,
      );
    });

    test('mode curve preserves the daytime shape and clears stale color', () {
      final natural = roomDayCurveForMode(
        mode: RoomDayColorMode.natural,
        selectedColor: directColor,
        widthLeftBri: 0.71,
        widthRightBri: 0.82,
        widthLeftCct: 0.93,
        widthRightCct: 1.04,
        shapeP: 5.5,
      );
      final color = roomDayCurveForMode(
        mode: RoomDayColorMode.color,
        selectedColor: directColor,
        widthLeftBri: 0.71,
        widthRightBri: 0.82,
        widthLeftCct: 0.93,
        widthRightCct: 1.04,
        shapeP: 5.5,
      );

      expect(natural.directColor, isNull);
      expect(color.directColor, directColor);
      expect(color.widthLeftBri, 0.71);
      expect(color.widthRightBri, 0.82);
      expect(color.widthLeftCct, 0.93);
      expect(color.widthRightCct, 1.04);
      expect(color.shapeP, 5.5);
    });

    test('fixed White and Color use additive existing room override fields',
        () {
      const global = RhythmCurveConfig(
        id: 'rhythm',
        minColorTemp: 2200,
        maxColorTemp: 6500,
        minBrightness: 2,
        maxBrightness: 88,
        curve: RhythmSuperGaussianCurve(),
      );
      final fixedWhite = global.copyWith(
        minColorTemp: 3500,
        maxColorTemp: 3500,
      );
      final whiteOverride = RhythmLightProfileNodeOverride.between(
        global,
        fixedWhite,
      );
      final fixedColor = global.copyWith(
        curve: roomDayCurveForMode(
          mode: RoomDayColorMode.color,
          selectedColor: directColor,
          widthLeftBri: global.widthLeftBri,
          widthRightBri: global.widthRightBri,
          widthLeftCct: global.widthLeftCct,
          widthRightCct: global.widthRightCct,
          shapeP: global.shapeP,
        ),
      );
      final colorOverride = RhythmLightProfileNodeOverride.between(
        global,
        fixedColor,
      );

      expect(whiteOverride.toJson(), {
        'min_color_temp': 3500,
        'max_color_temp': 3500,
      });
      expect(whiteOverride.applyTo(global), fixedWhite);
      expect(colorOverride.minBrightness, isNull);
      expect(colorOverride.maxBrightness, isNull);
      expect(colorOverride.curve, isA<RhythmSuperGaussianCurve>());
      expect(
        (colorOverride.curve as RhythmSuperGaussianCurve).directColor,
        directColor,
      );
      expect(colorOverride.applyTo(global), fixedColor);
    });
  });

  testWidgets('selector activates White and Color on a compact room card',
      (tester) async {
    var mode = RoomDayColorMode.natural;

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Center(
            child: SizedBox(
              width: 300,
              child: StatefulBuilder(
                builder: (context, setState) => RoomDayColorModeSelector(
                  mode: mode,
                  onChanged: (value) => setState(() => mode = value),
                ),
              ),
            ),
          ),
        ),
      ),
    );

    expect(find.text('Auto'), findsOneWidget);
    expect(find.text('White'), findsOneWidget);
    expect(find.text('Color'), findsOneWidget);

    final naturalRect = tester.getRect(
      find.byKey(const ValueKey('room-day-color-mode-natural')),
    );
    final whiteRect = tester.getRect(
      find.byKey(const ValueKey('room-day-color-mode-white')),
    );
    final colorRect = tester.getRect(
      find.byKey(const ValueKey('room-day-color-mode-color')),
    );
    expect(naturalRect.left, lessThan(whiteRect.left));
    expect(whiteRect.left, lessThan(colorRect.left));
    expect(naturalRect.top, whiteRect.top);
    expect(whiteRect.top, colorRect.top);
    expect((naturalRect.width - whiteRect.width).abs(), lessThan(1));
    expect((whiteRect.width - colorRect.width).abs(), lessThan(1));

    final naturalSemantics = tester.getSemantics(
      find.byKey(const ValueKey('room-day-color-mode-natural')),
    );
    expect(naturalSemantics.label, 'Auto');
    expect(
      naturalSemantics.getSemanticsData().hasAction(SemanticsAction.tap),
      isTrue,
    );

    await tester.tap(
      find.byKey(const ValueKey('room-day-color-mode-white')),
    );
    await tester.pumpAndSettle();
    expect(mode, RoomDayColorMode.white);

    await tester.tap(
      find.byKey(const ValueKey('room-day-color-mode-color')),
    );
    await tester.pumpAndSettle();
    expect(mode, RoomDayColorMode.color);

    // Repeated taps keep the selected detail mode stable.
    await tester.tap(
      find.byKey(const ValueKey('room-day-color-mode-color')),
    );
    await tester.pumpAndSettle();
    expect(mode, RoomDayColorMode.color);
    expect(tester.takeException(), isNull);
  });

  testWidgets('selector disables Color when the profile cannot carry it',
      (tester) async {
    var mode = RoomDayColorMode.natural;

    await tester.pumpWidget(
      MaterialApp(
        home: RoomDayColorModeSelector(
          mode: mode,
          colorEnabled: false,
          onChanged: (value) => mode = value,
        ),
      ),
    );

    await tester.tap(
      find.byKey(const ValueKey('room-day-color-mode-color')),
    );
    await tester.pump();
    expect(mode, RoomDayColorMode.natural);
  });
}
