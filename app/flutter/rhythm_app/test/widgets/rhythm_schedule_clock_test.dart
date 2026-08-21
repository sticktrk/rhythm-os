import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/widgets/rhythm_clock/rhythm_schedule_clock.dart';
import 'package:rhythm_app/widgets/solar_clock/solar_clock_exports.dart';
import 'package:rhythm_core/rhythm_core.dart' hide Home, Hub, HubType;
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmMode;

SolarClockData _fakeSolarData() => const SolarClockData(
      sunTimes: SunTimesDto(
        sunrise: 6.5,
        sunset: 18.5,
        solarNoon: 12.5,
        solarMidnight: 0.5,
        dayLength: 12.0,
      ),
      twilightTimes: TwilightTimesDto(
        dawn: TwilightPhaseDto(civil: 6.0, nautical: 5.5, astronomical: 5.0),
        dusk: TwilightPhaseDto(civil: 19.0, nautical: 19.5, astronomical: 20.0),
      ),
    );

/// Mirrors the geometry the clock builds internally (same SolarClock layout
/// parameters) so tests can convert an hour to an on-screen tick position.
SolarClockGeometry _geometryFor(Size size, SolarClockData data) {
  return SolarClockGeometry.fromConstraints(
    BoxConstraints.tight(size),
    solarNoon: data.solarNoon,
    orientation: SolarClockOrientation.standard24,
    horizonFactor: 0.58,
    radiusWidthFactor: 0.35,
    radiusHeightFactor: 0.92,
  );
}

void main() {
  const clockSize = Size(320, 320);

  Future<void> pumpClock(
    WidgetTester tester, {
    required SolarClockData data,
    required RhythmMode selectedMode,
    required void Function(RhythmMode, double, TriggerAnchor?) onCommitted,
    void Function(RhythmMode)? onModeSelected,
    double dayHour = 9.0,
    double sleepHour = 22.0,
  }) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Center(
            child: SizedBox.fromSize(
              size: clockSize,
              child: RhythmScheduleClock(
                data: data,
                dayHour: dayHour,
                sleepHour: sleepHour,
                dayColor: const Color(0xFFF9A825),
                sleepColor: const Color(0xFFE57373),
                dayVisual:
                    const ModeCurveVisual(fallbackColor: Color(0xFFF9A825)),
                sleepVisual:
                    const ModeCurveVisual(fallbackColor: Color(0xFFE57373)),
                selectedMode: selectedMode,
                onModeSelected: onModeSelected ?? (_) {},
                dayAnchors: solarAnchorsForMode(RhythmMode.day, data),
                sleepAnchors: solarAnchorsForMode(RhythmMode.sleep, data),
                onHourCommitted: onCommitted,
              ),
            ),
          ),
        ),
      ),
    );
    await tester.pump();
  }

  testWidgets('tapping a solar marker pins the selected orb to it',
      (tester) async {
    final data = _fakeSolarData();
    final commits = <(RhythmMode, double, TriggerAnchor?)>[];

    await pumpClock(
      tester,
      data: data,
      selectedMode: RhythmMode.day,
      onCommitted: (mode, hour, anchor) => commits.add((mode, hour, anchor)),
    );

    final origin = tester.getTopLeft(find.byType(RhythmScheduleClock));
    final geometry = _geometryFor(clockSize, data);
    final sunriseTick = geometry.positionForHour(data.sunrise);

    await tester.tapAt(origin + sunriseTick);
    await tester.pump();

    expect(commits, hasLength(1));
    final (mode, hour, anchor) = commits.single;
    expect(mode, RhythmMode.day);
    expect(hour, data.sunrise);
    expect(anchor?.event, 'sunrise');
  });

  testWidgets('marker taps pin the SELECTED mode and respect the day gap',
      (tester) async {
    final data = _fakeSolarData();
    final commits = <(RhythmMode, double, TriggerAnchor?)>[];

    await pumpClock(
      tester,
      data: data,
      selectedMode: RhythmMode.sleep,
      onCommitted: (mode, hour, anchor) => commits.add((mode, hour, anchor)),
    );

    final origin = tester.getTopLeft(find.byType(RhythmScheduleClock));
    final geometry = _geometryFor(clockSize, data);

    // Sleep is selected, so the ladder shows dusk markers — tap sunset.
    await tester.tapAt(origin + geometry.positionForHour(data.sunset));
    await tester.pump();

    expect(commits, hasLength(1));
    expect(commits.single.$1, RhythmMode.sleep);
    expect(commits.single.$3?.event, 'sunset');

    // A tap far from every marker and orb commits nothing.
    await tester.tapAt(origin + geometry.center);
    await tester.pump();
    expect(commits, hasLength(1));
  });

  testWidgets('tapping the other orb selects its mode without committing',
      (tester) async {
    final data = _fakeSolarData();
    final commits = <(RhythmMode, double, TriggerAnchor?)>[];
    final selections = <RhythmMode>[];

    await pumpClock(
      tester,
      data: data,
      selectedMode: RhythmMode.day,
      onCommitted: (mode, hour, anchor) => commits.add((mode, hour, anchor)),
      onModeSelected: selections.add,
    );

    final origin = tester.getTopLeft(find.byType(RhythmScheduleClock));
    final geometry = _geometryFor(clockSize, data);

    await tester.tapAt(origin + geometry.positionForHour(22.0));
    await tester.pump();

    expect(selections, [RhythmMode.sleep]);
    expect(commits, isEmpty);
  });
}
