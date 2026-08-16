import 'dart:ui' show SemanticsAction, Tristate;

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/settings/light_profile_screen.dart';
import 'package:rhythm_app/widgets/color_wheel_picker.dart';
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

  group('room Day brightness contract', () {
    test('equal endpoints reopen as Fixed and a range reopens as Auto', () {
      expect(
        roomDayBrightnessModeForConfig(
          const RhythmCurveConfig(
            id: 'rhythm',
            minBrightness: 42,
            maxBrightness: 42,
          ),
        ),
        RoomDayBrightnessMode.fixed,
      );
      expect(
        roomDayBrightnessModeForConfig(
          const RhythmCurveConfig(
            id: 'rhythm',
            minBrightness: 8,
            maxBrightness: 82,
          ),
        ),
        RoomDayBrightnessMode.automatic,
      );
    });

    test('fixed brightness saves through existing additive override fields',
        () {
      const global = RhythmCurveConfig(
        id: 'rhythm',
        minBrightness: 8,
        maxBrightness: 82,
      );
      final endpoints = roomDayBrightnessEndpointsForMode(
        mode: RoomDayBrightnessMode.fixed,
        automaticMinBrightness: global.minBrightness.toDouble(),
        automaticMaxBrightness: global.maxBrightness.toDouble(),
        fixedBrightness: 42,
      );
      final fixed = global.copyWith(
        minBrightness: endpoints.minBrightness,
        maxBrightness: endpoints.maxBrightness,
      );
      final override = RhythmLightProfileNodeOverride.between(global, fixed);
      final reopened = override.applyTo(global);

      expect(override.toJson(), {
        'min_brightness': 42,
        'max_brightness': 42,
      });
      expect(reopened, fixed);
      expect(
        roomDayBrightnessModeForConfig(reopened),
        RoomDayBrightnessMode.fixed,
      );
    });

    test('switching back to Auto restores its range without stale endpoints',
        () {
      final endpoints = roomDayBrightnessEndpointsForMode(
        mode: RoomDayBrightnessMode.automatic,
        automaticMinBrightness: 8,
        automaticMaxBrightness: 82,
        fixedBrightness: 42,
      );

      expect(endpoints.minBrightness, 8);
      expect(endpoints.maxBrightness, 82);
      expect(endpoints.minBrightness, isNot(endpoints.maxBrightness));
    });
  });

  testWidgets('brightness selector is accessible and switches modes',
      (tester) async {
    var mode = RoomDayBrightnessMode.automatic;

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Center(
            child: SizedBox(
              width: 300,
              child: StatefulBuilder(
                builder: (context, setState) =>
                    RoomDayBrightnessModeSelector(
                  mode: mode,
                  onChanged: (value) => setState(() => mode = value),
                ),
              ),
            ),
          ),
        ),
      ),
    );

    final automatic = find.byKey(
      const ValueKey('room-day-brightness-mode-automatic'),
    );
    final fixed = find.byKey(
      const ValueKey('room-day-brightness-mode-fixed'),
    );
    expect(find.text('Auto'), findsOneWidget);
    expect(find.text('Fixed'), findsOneWidget);
    expect(tester.getRect(automatic).left, lessThan(tester.getRect(fixed).left));
    expect(tester.getSemantics(automatic).label, 'Auto');
    expect(
      tester
          .getSemantics(automatic)
          .getSemanticsData()
          .hasAction(SemanticsAction.tap),
      isTrue,
    );

    await tester.tap(fixed);
    await tester.pumpAndSettle();
    expect(mode, RoomDayBrightnessMode.fixed);
    expect(
      tester.getSemantics(fixed).getSemanticsData().flagsCollection.isSelected,
      Tristate.isTrue,
    );
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

  testWidgets('fixed Color uses the large hue and saturation wheel',
      (tester) async {
    var selected = const HSVColor.fromAHSV(1, 35, 0.85, 1);
    final commits = <bool>[];

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Center(
            child: SizedBox(
              width: 300,
              child: StatefulBuilder(
                builder: (context, setState) => ColorWheelPicker(
                  value: selected,
                  maxSide: 252,
                  semanticsLabel: 'Fixed daytime color wheel',
                  interactionKey: const ValueKey('room-day-color-spectrum'),
                  wheelKey: const ValueKey('room-day-color-wheel'),
                  onChanged: (color, {required commit}) {
                    setState(() => selected = color);
                    commits.add(commit);
                  },
                ),
              ),
            ),
          ),
        ),
      ),
    );

    final wheel = find.byKey(const ValueKey('room-day-color-wheel'));
    final wheelRect = tester.getRect(wheel);
    expect(wheelRect.width, 252);
    expect(wheelRect.height, 252);
    expect(
      tester
          .getSemantics(
            find.byKey(const ValueKey('room-day-color-spectrum')),
          )
          .label,
      'Fixed daytime color wheel',
    );

    await tester.tapAt(wheelRect.center + const Offset(112, 0));
    await tester.pump();

    expect(selected.hue, closeTo(0, 0.1));
    expect(selected.saturation, greaterThan(0.85));
    expect(commits.last, isTrue);

    final gesture = await tester.startGesture(wheelRect.center);
    await gesture.moveTo(wheelRect.center + const Offset(0, -100));
    await gesture.up();
    await tester.pump();

    expect(selected.hue, closeTo(270, 0.1));
    expect(selected.saturation, greaterThan(0.75));
    expect(commits, contains(false));
    expect(commits.last, isTrue);
  });
}
