import '../src/rust/api/dto/curve.dart' show CurveConfigDto;
import '../models/config_state.dart';

/// Abstract interface for Rhythm Lighting API.
///
/// Implemented by:
/// - [RemoteApiClient] for any Rhythm server backend
/// - [HybridApiClient] for multi-source configuration
abstract class RhythmApi {
  /// Get current configuration state (config + solar + resolved).
  Future<ConfigState> getConfigState();

  /// Save raw configuration (supports dynamic midpoint values).
  Future<void> saveConfig(RawConfig config);

  /// Get curve data for visualization.
  Future<CurveData> getCurveData({int? month, CurveConfigDto? overrides});

  /// Get step sequences for visualization.
  Future<StepSequences> getStepSequences({
    required double hour,
    required int maxSteps,
    CurveConfigDto? overrides,
  });

  /// Get current time and lighting values.
  Future<TimeInfo> getTime();

  /// Health check.
  Future<bool> healthCheck();
}

/// Curve data for visualization.
class CurveData {
  final List<double> hours;
  final List<int> brightness;
  final List<int> kelvin;
  final SolarInfo solar;

  CurveData({
    required this.hours,
    required this.brightness,
    required this.kelvin,
    required this.solar,
  });

  factory CurveData.fromJson(Map<String, dynamic> json) {
    return CurveData(
      hours: (json['hours'] as List).map((e) => (e as num).toDouble()).toList(),
      brightness:
          (json['bris'] as List).map((e) => (e as num).toInt()).toList(),
      kelvin: (json['ccts'] as List).map((e) => (e as num).toInt()).toList(),
      solar: SolarInfo.fromJson(json['solar']),
    );
  }
}

/// Solar time information.
class SolarInfo {
  final double? sunrise;
  final double? sunset;
  final double solarNoon;
  final double solarMidnight;
  final double? dayLength;
  final TwilightPhase? dawn;
  final TwilightPhase? dusk;

  SolarInfo({
    this.sunrise,
    this.sunset,
    required this.solarNoon,
    required this.solarMidnight,
    this.dayLength,
    this.dawn,
    this.dusk,
  });

  factory SolarInfo.fromJson(Map<String, dynamic> json) {
    return SolarInfo(
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

/// Step sequences for dimming visualization.
class StepSequences {
  final List<StepPoint> stepUp;
  final List<StepPoint> stepDown;

  StepSequences({required this.stepUp, required this.stepDown});

  factory StepSequences.fromJson(Map<String, dynamic> json) {
    return StepSequences(
      stepUp: (json['step_up']['steps'] as List)
          .map((e) => StepPoint.fromJson(e))
          .toList(),
      stepDown: (json['step_down']['steps'] as List)
          .map((e) => StepPoint.fromJson(e))
          .toList(),
    );
  }
}

/// A single step point.
class StepPoint {
  final double hour;
  final int brightness;
  final int kelvin;
  final List<int> rgb;

  StepPoint({
    required this.hour,
    required this.brightness,
    required this.kelvin,
    required this.rgb,
  });

  factory StepPoint.fromJson(Map<String, dynamic> json) {
    return StepPoint(
      hour: (json['hour'] as num).toDouble(),
      brightness: (json['brightness'] as num).toInt(),
      kelvin: (json['kelvin'] as num).toInt(),
      rgb: (json['rgb'] as List).map((e) => (e as num).toInt()).toList(),
    );
  }
}

/// Current time and lighting information.
class TimeInfo {
  final String currentTime;
  final double currentHour;
  final String? timezone;
  final int brightness;
  final int kelvin;
  final double solarPosition;

  TimeInfo({
    required this.currentTime,
    required this.currentHour,
    this.timezone,
    required this.brightness,
    required this.kelvin,
    required this.solarPosition,
  });

  factory TimeInfo.fromJson(Map<String, dynamic> json) {
    final lighting = json['lighting'] as Map<String, dynamic>;
    final location = json['location'] as Map<String, dynamic>?;
    return TimeInfo(
      currentTime: (json['current_time'] ??
          json['current_local_time'] ??
          location?['current_local_time'] ??
          location?['current_time']) as String,
      currentHour: (json['current_hour'] as num).toDouble(),
      timezone: json['timezone'] as String?,
      brightness: (lighting['brightness'] as num).toInt(),
      kelvin: (lighting['kelvin'] as num).toInt(),
      solarPosition: (lighting['solar_position'] as num).toDouble(),
    );
  }
}
