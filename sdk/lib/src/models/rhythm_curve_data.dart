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
    final curve = _asMap(json['curve']) ?? json;
    final hours = _doubleList(curve['hours']);
    final brightness = _intList(curve['brightness'] ?? curve['bris']);
    final kelvin = _intList(curve['kelvin'] ?? curve['ccts']);
    final solarJson = _asMap(json['solar']) ??
        _asMap(curve['solar']) ??
        const <String, dynamic>{};
    return RhythmCurveData(
      hours: hours,
      brightness: brightness,
      kelvin: kelvin,
      solar: RhythmSolarInfo.fromJson(solarJson),
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
    final twilight = _asMap(json['twilight']);
    return RhythmSolarInfo(
      sunrise: (json['sunrise'] as num?)?.toDouble(),
      sunset: (json['sunset'] as num?)?.toDouble(),
      solarNoon: (json['solarNoon'] as num?)?.toDouble() ??
          (json['solar_noon'] as num?)?.toDouble() ??
          12.0,
      solarMidnight: (json['solarMidnight'] as num?)?.toDouble() ??
          (json['solar_midnight'] as num?)?.toDouble() ??
          0.0,
      dayLength: (json['dayLength'] as num?)?.toDouble() ??
          (json['day_length'] as num?)?.toDouble(),
      dawn: _twilightPhaseFromJson(json['dawn'] ?? twilight?['dawn']),
      dusk: _twilightPhaseFromJson(json['dusk'] ?? twilight?['dusk']),
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

Map<String, dynamic>? _asMap(dynamic value) {
  if (value is Map<String, dynamic>) return value;
  if (value is Map) return value.cast<String, dynamic>();
  return null;
}

TwilightPhase? _twilightPhaseFromJson(dynamic value) {
  final json = _asMap(value);
  return json == null ? null : TwilightPhase.fromJson(json);
}

List<double> _doubleList(dynamic value) {
  if (value is! List) return const [];
  return value.map((e) => (e as num).toDouble()).toList();
}

List<int> _intList(dynamic value) {
  if (value is! List) return const [];
  return value.map((e) => (e as num).toInt()).toList();
}
