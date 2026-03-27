/// Curve data for visualization.
class RhythmCurveData {
  final List<double> hours;
  final List<int> brightness;
  final List<int> kelvin;
  final RhythmSolarInfo solar;

  RhythmCurveData({
    required this.hours,
    required this.brightness,
    required this.kelvin,
    required this.solar,
  });

  factory RhythmCurveData.fromJson(Map<String, dynamic> json) {
    return RhythmCurveData(
      hours:
          (json['hours'] as List).map((e) => (e as num).toDouble()).toList(),
      brightness:
          (json['bris'] as List).map((e) => (e as num).toInt()).toList(),
      kelvin:
          (json['ccts'] as List).map((e) => (e as num).toInt()).toList(),
      solar: RhythmSolarInfo.fromJson(json['solar']),
    );
  }
}

/// Solar time information.
class RhythmSolarInfo {
  final double? sunrise;
  final double? sunset;
  final double solarNoon;
  final double solarMidnight;
  final double? dayLength;
  final TwilightPhase? dawn;
  final TwilightPhase? dusk;

  RhythmSolarInfo({
    this.sunrise,
    this.sunset,
    required this.solarNoon,
    required this.solarMidnight,
    this.dayLength,
    this.dawn,
    this.dusk,
  });

  factory RhythmSolarInfo.fromJson(Map<String, dynamic> json) {
    return RhythmSolarInfo(
      sunrise: (json['sunrise'] as num?)?.toDouble(),
      sunset: (json['sunset'] as num?)?.toDouble(),
      solarNoon: (json['solarNoon'] as num).toDouble(),
      solarMidnight: (json['solarMidnight'] as num).toDouble(),
      dayLength: (json['dayLength'] as num?)?.toDouble(),
      dawn: json['dawn'] == null
          ? null
          : TwilightPhase.fromJson(json['dawn'] as Map<String, dynamic>),
      dusk: json['dusk'] == null
          ? null
          : TwilightPhase.fromJson(json['dusk'] as Map<String, dynamic>),
    );
  }
}

class TwilightPhase {
  final double? civil;
  final double? nautical;
  final double? astronomical;

  const TwilightPhase({
    this.civil,
    this.nautical,
    this.astronomical,
  });

  factory TwilightPhase.fromJson(Map<String, dynamic> json) {
    return TwilightPhase(
      civil: (json['civil'] as num?)?.toDouble(),
      nautical: (json['nautical'] as num?)?.toDouble(),
      astronomical: (json['astronomical'] as num?)?.toDouble(),
    );
  }
}
