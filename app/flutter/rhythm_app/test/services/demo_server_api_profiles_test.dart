import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/demo_server_api.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

/// The demo server subclasses [RhythmServerApi] over a host-less Dio, so any
/// profile/curve method it fails to override would surface as
/// "No host specified in URI api/…" from the Lighting screen. These tests pin
/// the demo-side implementations the Lighting screen and Time Simulator use.
void main() {
  final api = DemoServerApi.instance;

  setUp(api.reset);

  test('getProfiles returns the built-in profile set', () async {
    final profiles = await api.getProfiles();
    expect(
      profiles.map((profile) => profile.id),
      containsAll(['rhythm', 'sleep', 'day_idle', 'sleep_idle']),
    );
    final sleep = profiles.singleWhere((profile) => profile.id == 'sleep');
    expect(sleep.name, 'Sleep');
    expect(sleep.curve, isA<RhythmConstantCurve>());
    expect(sleep.minColorTemp, RhythmCurveConfig.defaultMinColorTemp);
    expect(sleep.maxColorTemp, RhythmCurveConfig.defaultMinColorTemp);
  });

  test('getConfig resolves the ids the mode resource advertises', () async {
    final mode = await api.getMode();
    for (final config in mode!.configs) {
      final profile = await api.getConfig(id: config.activeProfileId);
      expect(profile, isNotNull, reason: config.activeProfileId);
      expect(profile!.id, config.activeProfileId);
    }
    expect(await api.getConfig(id: 'missing'), isNull);
  });

  test('configSet persists and resetConfig restores defaults', () async {
    final day = (await api.getConfig(id: 'rhythm'))!;
    final edited = day.copyWith(maxColorTemp: 5000);
    expect(await api.configSet(edited, id: 'rhythm', apply: true), isTrue);
    expect((await api.getConfig(id: 'rhythm'))!.maxColorTemp, 5000);
    expect(
        (await api.getProfiles())
            .singleWhere((p) => p.id == 'rhythm')
            .maxColorTemp,
        5000);

    final restored = await api.resetConfig(id: 'rhythm');
    expect(restored!.maxColorTemp, RhythmCurveConfig.defaultMaxColorTemp);
    expect((await api.getConfig(id: 'rhythm'))!.maxColorTemp,
        RhythmCurveConfig.defaultMaxColorTemp);
  });

  test('getCurveData samples a day curve that peaks around solar noon',
      () async {
    final data = await api.getCurveData(id: 'rhythm', samplesPerHour: 4);
    expect(data, isNotNull);
    expect(data!.hours.length, 24 * 4 + 1);
    expect(data.brightness.length, data.hours.length);
    expect(data.kelvin.length, data.hours.length);
    expect(data.hours.first, 0);
    expect(data.hours.last, 24);

    final noonIndex = ((data.solar.solarNoon) * 4).round();
    expect(data.brightness[noonIndex], RhythmCurveConfig.defaultMaxBrightness);
    expect(data.kelvin[noonIndex], RhythmCurveConfig.defaultMaxColorTemp);
    // Midnight sits at the floor of both ranges.
    expect(data.brightness.first, RhythmCurveConfig.defaultMinBrightness);
    expect(data.kelvin.first, RhythmCurveConfig.defaultMinColorTemp);
    expect(data.solar.sunrise, lessThan(data.solar.solarNoon));
    expect(data.solar.sunset, greaterThan(data.solar.solarNoon));
  });

  test('getCurveData renders a constant profile as a flat line', () async {
    final data = await api.getCurveData(id: 'sleep');
    expect(data, isNotNull);
    expect(data!.brightness.toSet(), {1});
    expect(data.kelvin.toSet(), {RhythmCurveConfig.defaultMinColorTemp});
  });

  test('getCurveData honors overrides without touching the stored profile',
      () async {
    final day = (await api.getConfig(id: 'rhythm'))!;
    final data = await api.getCurveData(
      id: 'rhythm',
      overrides: day.copyWith(maxBrightness: 40),
    );
    expect(data!.brightness.reduce((a, b) => a > b ? a : b), 40);
    expect((await api.getConfig(id: 'rhythm'))!.maxBrightness,
        RhythmCurveConfig.defaultMaxBrightness);
  });

  test('constant preview maps normalized values into ordered profile ranges',
      () async {
    final sleep = (await api.getConfig(id: 'sleep'))!;
    final data = await api.getCurveData(
      id: 'sleep',
      overrides: sleep.copyWith(
        minBrightness: 60,
        maxBrightness: 10,
        minColorTemp: 2700,
        maxColorTemp: 5000,
        curve: const RhythmConstantCurve(brightness: 0, colorTemp: 1),
      ),
    );
    expect(data!.brightness.toSet(), {10});
    expect(data.kelvin.toSet(), {5000});
  });

  test('getCurveNow and absorbTimeOffsetResult answer locally', () async {
    final now = await api.getCurveNow(id: 'rhythm', hour: 13.0);
    expect(now, isNotNull);
    expect(now!.currentHour, 13.0);
    expect(now.brightness, greaterThan(50));

    final absorbed = await api.absorbTimeOffsetResult(30, id: 'rhythm');
    expect(absorbed.config?.id, 'rhythm');
  });
}
