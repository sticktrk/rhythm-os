import 'rhythm_curve_config.dart';

/// Solar context with sunrise/sunset times for the current day.
class RhythmSolarContext {
  final double sunrise;
  final double sunset;
  final double solarNoon;
  final double solarMidnight;
  final double dayLength;

  const RhythmSolarContext({
    required this.sunrise,
    required this.sunset,
    required this.solarNoon,
    required this.solarMidnight,
    required this.dayLength,
  });

  factory RhythmSolarContext.fromJson(Map<String, dynamic> json) {
    return RhythmSolarContext(
      sunrise: (json['sunrise'] as num).toDouble(),
      sunset: (json['sunset'] as num).toDouble(),
      solarNoon: (json['solar_noon'] as num).toDouble(),
      solarMidnight: (json['solar_midnight'] as num).toDouble(),
      dayLength: (json['day_length'] as num).toDouble(),
    );
  }

  factory RhythmSolarContext.defaults() {
    return const RhythmSolarContext(
      sunrise: 6.0,
      sunset: 20.0,
      solarNoon: 12.0,
      solarMidnight: 0.0,
      dayLength: 14.0,
    );
  }

  Map<String, dynamic> toJson() {
    return {
      'sunrise': sunrise,
      'sunset': sunset,
      'solar_noon': solarNoon,
      'solar_midnight': solarMidnight,
      'day_length': dayLength,
    };
  }
}

/// Raw user configuration using super-Gaussian curve parameters.
class RhythmRawConfig {
  final int minColorTemp;
  final int maxColorTemp;
  final int minBrightness;
  final int maxBrightness;
  final double widthLeftBri;
  final double widthRightBri;
  final double widthLeftCct;
  final double widthRightCct;
  final double shapeP;
  final int maxDimSteps;

  const RhythmRawConfig({
    required this.minColorTemp,
    required this.maxColorTemp,
    required this.minBrightness,
    required this.maxBrightness,
    required this.widthLeftBri,
    required this.widthRightBri,
    required this.widthLeftCct,
    required this.widthRightCct,
    required this.shapeP,
    required this.maxDimSteps,
  });

  factory RhythmRawConfig.fromJson(Map<String, dynamic> json) {
    return RhythmRawConfig(
      minColorTemp: (json['min_color_temp'] as num?)?.toInt() ??
          RhythmCurveConfig.defaultMinColorTemp,
      maxColorTemp: (json['max_color_temp'] as num?)?.toInt() ??
          RhythmCurveConfig.defaultMaxColorTemp,
      minBrightness: (json['min_brightness'] as num?)?.toInt() ??
          RhythmCurveConfig.defaultMinBrightness,
      maxBrightness: (json['max_brightness'] as num?)?.toInt() ??
          RhythmCurveConfig.defaultMaxBrightness,
      widthLeftBri: (json['width_left_bri'] as num?)?.toDouble() ??
          RhythmCurveConfig.defaultWidthLeftBri,
      widthRightBri: (json['width_right_bri'] as num?)?.toDouble() ??
          RhythmCurveConfig.defaultWidthRightBri,
      widthLeftCct: (json['width_left_cct'] as num?)?.toDouble() ??
          RhythmCurveConfig.defaultWidthLeftCct,
      widthRightCct: (json['width_right_cct'] as num?)?.toDouble() ??
          RhythmCurveConfig.defaultWidthRightCct,
      shapeP:
          (json['shape_p'] as num?)?.toDouble() ?? RhythmCurveConfig.defaultShapeP,
      maxDimSteps: (json['max_dim_steps'] as num?)?.toInt() ??
          RhythmCurveConfig.defaultMaxDimSteps,
    );
  }

  factory RhythmRawConfig.defaults() {
    return const RhythmRawConfig(
      minColorTemp: RhythmCurveConfig.defaultMinColorTemp,
      maxColorTemp: RhythmCurveConfig.defaultMaxColorTemp,
      minBrightness: RhythmCurveConfig.defaultMinBrightness,
      maxBrightness: RhythmCurveConfig.defaultMaxBrightness,
      widthLeftBri: RhythmCurveConfig.defaultWidthLeftBri,
      widthRightBri: RhythmCurveConfig.defaultWidthRightBri,
      widthLeftCct: RhythmCurveConfig.defaultWidthLeftCct,
      widthRightCct: RhythmCurveConfig.defaultWidthRightCct,
      shapeP: RhythmCurveConfig.defaultShapeP,
      maxDimSteps: RhythmCurveConfig.defaultMaxDimSteps,
    );
  }

  Map<String, dynamic> toJson() {
    return {
      'min_color_temp': minColorTemp,
      'max_color_temp': maxColorTemp,
      'min_brightness': minBrightness,
      'max_brightness': maxBrightness,
      'width_left_bri': widthLeftBri,
      'width_right_bri': widthRightBri,
      'width_left_cct': widthLeftCct,
      'width_right_cct': widthRightCct,
      'shape_p': shapeP,
      'max_dim_steps': maxDimSteps,
    };
  }

  RhythmRawConfig copyWith({
    int? minColorTemp,
    int? maxColorTemp,
    int? minBrightness,
    int? maxBrightness,
    double? widthLeftBri,
    double? widthRightBri,
    double? widthLeftCct,
    double? widthRightCct,
    double? shapeP,
    int? maxDimSteps,
  }) {
    return RhythmRawConfig(
      minColorTemp: minColorTemp ?? this.minColorTemp,
      maxColorTemp: maxColorTemp ?? this.maxColorTemp,
      minBrightness: minBrightness ?? this.minBrightness,
      maxBrightness: maxBrightness ?? this.maxBrightness,
      widthLeftBri: widthLeftBri ?? this.widthLeftBri,
      widthRightBri: widthRightBri ?? this.widthRightBri,
      widthLeftCct: widthLeftCct ?? this.widthLeftCct,
      widthRightCct: widthRightCct ?? this.widthRightCct,
      shapeP: shapeP ?? this.shapeP,
      maxDimSteps: maxDimSteps ?? this.maxDimSteps,
    );
  }
}

/// Full configuration state combining raw config and solar context.
class RhythmConfigState {
  final RhythmRawConfig config;
  final RhythmSolarContext solar;
  final double? latitude;
  final double? longitude;
  final String? timezone;

  const RhythmConfigState({
    required this.config,
    required this.solar,
    this.latitude,
    this.longitude,
    this.timezone,
  });

  factory RhythmConfigState.fromJson(Map<String, dynamic> json) {
    final configMap = json['config'] as Map<String, dynamic>?;
    final solarMap = json['solar'] as Map<String, dynamic>?;

    return RhythmConfigState(
      config: RhythmRawConfig.fromJson(configMap ?? json),
      solar: solarMap != null
          ? RhythmSolarContext.fromJson(solarMap)
          : RhythmSolarContext.defaults(),
      latitude: (json['latitude'] as num?)?.toDouble(),
      longitude: (json['longitude'] as num?)?.toDouble(),
      timezone: json['timezone'] as String?,
    );
  }

  factory RhythmConfigState.defaults() {
    return RhythmConfigState(
      config: RhythmRawConfig.defaults(),
      solar: RhythmSolarContext.defaults(),
    );
  }

  RhythmConfigState withConfig(RhythmRawConfig newConfig) {
    return RhythmConfigState(
      config: newConfig,
      solar: solar,
      latitude: latitude,
      longitude: longitude,
      timezone: timezone,
    );
  }

  /// Convert RhythmRawConfig to RhythmCurveConfig for API interop.
  static RhythmCurveConfig rawConfigToCurveConfig(RhythmRawConfig config) {
    return RhythmCurveConfig(
      minColorTemp: config.minColorTemp,
      maxColorTemp: config.maxColorTemp,
      minBrightness: config.minBrightness,
      maxBrightness: config.maxBrightness,
      widthLeftBri: config.widthLeftBri,
      widthRightBri: config.widthRightBri,
      widthLeftCct: config.widthLeftCct,
      widthRightCct: config.widthRightCct,
      shapeP: config.shapeP,
      maxDimSteps: config.maxDimSteps,
    );
  }
}
