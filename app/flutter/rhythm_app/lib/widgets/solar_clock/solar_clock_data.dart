import 'package:flutter/material.dart';
import 'package:rhythm_core/rhythm_core.dart';

class SolarEvent {
  final String label;
  final double hour;
  final Color color;

  const SolarEvent({
    required this.label,
    required this.hour,
    required this.color,
  });
}

class SolarCurveSamples {
  final List<double> hours;
  final List<int> brightness;
  final List<int> kelvin;

  const SolarCurveSamples({
    required this.hours,
    required this.brightness,
    required this.kelvin,
  });

  factory SolarCurveSamples.fromCurveData(CurveData data) {
    return SolarCurveSamples(
      hours: data.hours,
      brightness: data.brightness,
      kelvin: data.kelvin,
    );
  }

  factory SolarCurveSamples.fromCurveDataDto(CurveDataDto data) {
    return SolarCurveSamples(
      hours: data.hours,
      brightness: data.brightness,
      kelvin: data.kelvin,
    );
  }

  bool get isEmpty => hours.isEmpty || brightness.isEmpty || kelvin.isEmpty;
}

class SolarClockData {
  static const dawnColor = Color(0xFFF0A830);
  static const duskColor = Color(0xFF7898D0);

  final SunTimesDto sunTimes;
  final TwilightTimesDto twilightTimes;

  const SolarClockData({
    required this.sunTimes,
    required this.twilightTimes,
  });

  double get solarNoon => sunTimes.solarNoon;
  double get sunrise => sunTimes.sunrise;
  double get sunset => sunTimes.sunset;
  bool get isPolarDay => sunTimes.dayLength >= 24.0;
  bool get isPolarNight => sunTimes.dayLength <= 0.0;

  List<SolarEvent> get events {
    final events = <SolarEvent>[
      if (twilightTimes.dawn.astronomical != null)
        SolarEvent(
          label: 'ASTRO DAWN',
          hour: twilightTimes.dawn.astronomical!,
          color: dawnColor,
        ),
      if (twilightTimes.dawn.nautical != null)
        SolarEvent(
          label: 'NAUTICAL DAWN',
          hour: twilightTimes.dawn.nautical!,
          color: dawnColor,
        ),
      if (twilightTimes.dawn.civil != null)
        SolarEvent(
          label: 'CIVIL DAWN',
          hour: twilightTimes.dawn.civil!,
          color: dawnColor,
        ),
      SolarEvent(
        label: 'SUNRISE',
        hour: sunTimes.sunrise,
        color: dawnColor,
      ),
      SolarEvent(
        label: 'SUNSET',
        hour: sunTimes.sunset,
        color: duskColor,
      ),
      if (twilightTimes.dusk.civil != null)
        SolarEvent(
          label: 'CIVIL DUSK',
          hour: twilightTimes.dusk.civil!,
          color: duskColor,
        ),
      if (twilightTimes.dusk.nautical != null)
        SolarEvent(
          label: 'NAUTICAL DUSK',
          hour: twilightTimes.dusk.nautical!,
          color: duskColor,
        ),
      if (twilightTimes.dusk.astronomical != null)
        SolarEvent(
          label: 'ASTRO DUSK',
          hour: twilightTimes.dusk.astronomical!,
          color: duskColor,
        ),
    ];
    events.sort((a, b) => a.hour.compareTo(b.hour));
    return events;
  }
}
