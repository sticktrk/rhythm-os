import '../json_parsing.dart';

enum RhythmSkyCondition {
  clear,
  partlyCloudy,
  cloudy,
  overcast,
  fog,
  rain,
  storm;

  String get wireValue => switch (this) {
        RhythmSkyCondition.clear => 'clear',
        RhythmSkyCondition.partlyCloudy => 'partly_cloudy',
        RhythmSkyCondition.cloudy => 'cloudy',
        RhythmSkyCondition.overcast => 'overcast',
        RhythmSkyCondition.fog => 'fog',
        RhythmSkyCondition.rain => 'rain',
        RhythmSkyCondition.storm => 'storm',
      };

  static RhythmSkyCondition? fromString(String? value) => switch (value) {
        'clear' => RhythmSkyCondition.clear,
        'partly_cloudy' => RhythmSkyCondition.partlyCloudy,
        'cloudy' => RhythmSkyCondition.cloudy,
        'overcast' => RhythmSkyCondition.overcast,
        'fog' => RhythmSkyCondition.fog,
        'rain' => RhythmSkyCondition.rain,
        'storm' => RhythmSkyCondition.storm,
        _ => null,
      };
}

enum RhythmEnvironmentSource {
  manualOverride,
  luxSensor,
  weather,
  sunAngleFallback,
  unknown;

  static RhythmEnvironmentSource fromString(String? value) => switch (value) {
        'manual_override' => RhythmEnvironmentSource.manualOverride,
        'lux_sensor' => RhythmEnvironmentSource.luxSensor,
        'weather' => RhythmEnvironmentSource.weather,
        'sun_angle_fallback' => RhythmEnvironmentSource.sunAngleFallback,
        _ => RhythmEnvironmentSource.unknown,
      };
}

class RhythmEnvironmentSnapshot {
  final double outdoorFactor;
  final RhythmEnvironmentSource source;
  final String sourceRaw;
  final bool isFallback;
  final RhythmEnvironmentDiagnostics diagnostics;
  final Map<String, dynamic> raw;

  const RhythmEnvironmentSnapshot({
    required this.outdoorFactor,
    required this.source,
    required this.sourceRaw,
    required this.isFallback,
    required this.diagnostics,
    required this.raw,
  });

  factory RhythmEnvironmentSnapshot.fromJson(Map<String, dynamic> json) {
    final sourceRaw = json['source']?.toString() ?? '';
    return RhythmEnvironmentSnapshot(
      outdoorFactor: jsonDouble(
            json['outdoor_factor'],
            preferredKeys: const ['outdoor_factor'],
          ) ??
          0.0,
      source: RhythmEnvironmentSource.fromString(sourceRaw),
      sourceRaw: sourceRaw,
      isFallback: json['is_fallback'] == true,
      diagnostics: RhythmEnvironmentDiagnostics.fromJson(
        jsonMap(json['diagnostics']) ?? const <String, dynamic>{},
      ),
      raw: Map<String, dynamic>.from(json),
    );
  }
}

class RhythmEnvironmentDiagnostics {
  final double sunPosition;
  final double sunElevationDegrees;
  final double sunAngleFactor;
  final RhythmSkyCondition? skyCondition;
  final String? skyConditionRaw;
  final double skyMultiplier;
  final double? lux;
  final double? luxFactor;
  final double? luxLearnedFloor;
  final double? luxLearnedCeiling;
  final List<String> unavailableSources;
  final Map<String, dynamic> raw;

  const RhythmEnvironmentDiagnostics({
    required this.sunPosition,
    required this.sunElevationDegrees,
    required this.sunAngleFactor,
    this.skyCondition,
    this.skyConditionRaw,
    required this.skyMultiplier,
    this.lux,
    this.luxFactor,
    this.luxLearnedFloor,
    this.luxLearnedCeiling,
    this.unavailableSources = const [],
    this.raw = const <String, dynamic>{},
  });

  factory RhythmEnvironmentDiagnostics.fromJson(Map<String, dynamic> json) {
    final skyConditionRaw = json['sky_condition']?.toString();
    return RhythmEnvironmentDiagnostics(
      sunPosition: jsonDouble(
            json['sun_position'],
            preferredKeys: const ['sun_position'],
          ) ??
          0.0,
      sunElevationDegrees: jsonDouble(
            json['sun_elevation_degrees'],
            preferredKeys: const ['sun_elevation_degrees'],
          ) ??
          0.0,
      sunAngleFactor: jsonDouble(
            json['sun_angle_factor'],
            preferredKeys: const ['sun_angle_factor'],
          ) ??
          0.0,
      skyCondition: RhythmSkyCondition.fromString(skyConditionRaw),
      skyConditionRaw: skyConditionRaw,
      skyMultiplier: jsonDouble(
            json['sky_multiplier'],
            preferredKeys: const ['sky_multiplier'],
          ) ??
          0.0,
      lux: jsonDouble(json['lux'], preferredKeys: const ['lux']),
      luxFactor: jsonDouble(
        json['lux_factor'],
        preferredKeys: const ['lux_factor'],
      ),
      luxLearnedFloor: jsonDouble(
        json['lux_learned_floor'],
        preferredKeys: const ['lux_learned_floor'],
      ),
      luxLearnedCeiling: jsonDouble(
        json['lux_learned_ceiling'],
        preferredKeys: const ['lux_learned_ceiling'],
      ),
      unavailableSources: _stringList(json['unavailable_sources']),
      raw: Map<String, dynamic>.from(json),
    );
  }
}

class RhythmEnvironmentCalibration {
  final double? luxLearnedFloor;
  final double? luxLearnedCeiling;
  final Map<String, dynamic> raw;

  const RhythmEnvironmentCalibration({
    this.luxLearnedFloor,
    this.luxLearnedCeiling,
    this.raw = const <String, dynamic>{},
  });

  factory RhythmEnvironmentCalibration.fromJson(Map<String, dynamic> json) {
    return RhythmEnvironmentCalibration(
      luxLearnedFloor: jsonDouble(
        json['lux_learned_floor'],
        preferredKeys: const ['lux_learned_floor'],
      ),
      luxLearnedCeiling: jsonDouble(
        json['lux_learned_ceiling'],
        preferredKeys: const ['lux_learned_ceiling'],
      ),
      raw: Map<String, dynamic>.from(json),
    );
  }
}

class RhythmLearnBaselinesResponse {
  final bool learned;
  final RhythmEnvironmentCalibration calibration;
  final Map<String, dynamic> raw;

  const RhythmLearnBaselinesResponse({
    required this.learned,
    required this.calibration,
    required this.raw,
  });

  factory RhythmLearnBaselinesResponse.fromJson(Map<String, dynamic> json) {
    return RhythmLearnBaselinesResponse(
      learned: json['learned'] == true,
      calibration: RhythmEnvironmentCalibration.fromJson(
        jsonMap(json['calibration']) ?? const <String, dynamic>{},
      ),
      raw: Map<String, dynamic>.from(json),
    );
  }
}

List<String> _stringList(Object? value) {
  return ((value as List<dynamic>?) ?? const <dynamic>[])
      .map((item) => item.toString())
      .toList(growable: false);
}
