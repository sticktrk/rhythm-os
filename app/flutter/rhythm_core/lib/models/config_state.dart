import '../src/rust/api/dto/curve.dart' show CurveConfigDto;

/// Solar context with sunrise/sunset times for the current day.
class SolarContext {
  final double sunrise;
  final double sunset;
  final double solarNoon;
  final double solarMidnight;
  final double dayLength;

  const SolarContext({
    required this.sunrise,
    required this.sunset,
    required this.solarNoon,
    required this.solarMidnight,
    required this.dayLength,
  });

  factory SolarContext.fromJson(Map<String, dynamic> json) {
    return SolarContext(
      sunrise: (json['sunrise'] as num).toDouble(),
      sunset: (json['sunset'] as num).toDouble(),
      solarNoon: (json['solar_noon'] as num).toDouble(),
      solarMidnight: (json['solar_midnight'] as num).toDouble(),
      dayLength: (json['day_length'] as num).toDouble(),
    );
  }

  /// Default solar context for when location is not configured.
  factory SolarContext.defaults() {
    return const SolarContext(
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
///
/// Uses flat-topped bell curve with configurable peak flatness
/// and asymmetric morning/evening ramp speeds.
class RawConfig {
  final int minColorTemp;
  final int maxColorTemp;
  final int minBrightness;
  final int maxBrightness;
  /// Morning ramp speed for brightness (<1 = faster, >1 = slower)
  final double widthLeftBri;
  /// Evening ramp speed for brightness (<1 = faster, >1 = slower)
  final double widthRightBri;
  /// Morning ramp speed for CCT (<1 = faster, >1 = slower)
  final double widthLeftCct;
  /// Evening ramp speed for CCT (<1 = faster, >1 = slower)
  final double widthRightCct;
  /// Shape exponent for super-Gaussian curve (2 = round, 6 = flat plateau)
  final double shapeP;
  final int maxDimSteps;
  final int fadeMs;
  final int motionTimeoutSecs;

  const RawConfig({
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
    required this.fadeMs,
    required this.motionTimeoutSecs,
  });

  factory RawConfig.fromJson(Map<String, dynamic> json) {
    // Use Rust defaults for fallback values
    final d = CurveConfigDto.default_();
    return RawConfig(
      minColorTemp: (json['min_color_temp'] as num?)?.toInt() ?? d.minColorTemp,
      maxColorTemp: (json['max_color_temp'] as num?)?.toInt() ?? d.maxColorTemp,
      minBrightness: (json['min_brightness'] as num?)?.toInt() ?? d.minBrightness,
      maxBrightness: (json['max_brightness'] as num?)?.toInt() ?? d.maxBrightness,
      widthLeftBri: (json['width_left_bri'] as num?)?.toDouble() ?? d.widthLeftBri,
      widthRightBri: (json['width_right_bri'] as num?)?.toDouble() ?? d.widthRightBri,
      widthLeftCct: (json['width_left_cct'] as num?)?.toDouble() ?? d.widthLeftCct,
      widthRightCct: (json['width_right_cct'] as num?)?.toDouble() ?? d.widthRightCct,
      shapeP: (json['shape_p'] as num?)?.toDouble() ?? d.shapeP,
      maxDimSteps: (json['max_dim_steps'] as num?)?.toInt() ?? d.maxDimSteps,
      fadeMs: (json['fade_ms'] as num?)?.toInt() ?? d.fadeMs,
      motionTimeoutSecs: (json['motion_timeout_secs'] as num?)?.toInt() ?? d.motionTimeoutSecs,
    );
  }

  /// Create default config using values from Rust's CurveConfigDto.default_().
  factory RawConfig.defaults() {
    final d = CurveConfigDto.default_();
    return RawConfig(
      minColorTemp: d.minColorTemp,
      maxColorTemp: d.maxColorTemp,
      minBrightness: d.minBrightness,
      maxBrightness: d.maxBrightness,
      widthLeftBri: d.widthLeftBri,
      widthRightBri: d.widthRightBri,
      widthLeftCct: d.widthLeftCct,
      widthRightCct: d.widthRightCct,
      shapeP: d.shapeP,
      maxDimSteps: d.maxDimSteps,
      fadeMs: d.fadeMs,
      motionTimeoutSecs: d.motionTimeoutSecs,
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
      'fade_ms': fadeMs,
      'motion_timeout_secs': motionTimeoutSecs,
    };
  }

  /// Create a copy with updated fields.
  RawConfig copyWith({
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
    int? fadeMs,
    int? motionTimeoutSecs,
  }) {
    return RawConfig(
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
      fadeMs: fadeMs ?? this.fadeMs,
      motionTimeoutSecs: motionTimeoutSecs ?? this.motionTimeoutSecs,
    );
  }
}

/// Full configuration state combining raw config and solar context.
///
/// This is what the API returns and what Flutter uses for editing and calculations.
class ConfigState {
  /// Raw user configuration
  final RawConfig config;

  /// Today's solar times for reference
  final SolarContext solar;

  /// Location metadata
  final double? latitude;
  final double? longitude;
  final String? timezone;

  const ConfigState({
    required this.config,
    required this.solar,
    this.latitude,
    this.longitude,
    this.timezone,
  });

  factory ConfigState.fromJson(Map<String, dynamic> json) {
    // Handle both nested format {"config": {...}, "solar": {...}}
    // and flat format (just the CurveConfig fields directly)
    final configMap = json['config'] as Map<String, dynamic>?;
    final solarMap = json['solar'] as Map<String, dynamic>?;

    return ConfigState(
      config: RawConfig.fromJson(configMap ?? json),
      solar: solarMap != null ? SolarContext.fromJson(solarMap) : SolarContext.defaults(),
      latitude: (json['latitude'] as num?)?.toDouble(),
      longitude: (json['longitude'] as num?)?.toDouble(),
      timezone: json['timezone'] as String?,
    );
  }

  /// Create default config state.
  factory ConfigState.defaults() {
    final solar = SolarContext.defaults();
    final rawConfig = RawConfig.defaults();
    return ConfigState(
      config: rawConfig,
      solar: solar,
    );
  }

  /// Create updated state with new raw config.
  ConfigState withConfig(RawConfig newConfig) {
    return ConfigState(
      config: newConfig,
      solar: solar,
      latitude: latitude,
      longitude: longitude,
      timezone: timezone,
    );
  }

  /// Convert RawConfig to CurveConfigDto for Rust interop.
  static CurveConfigDto rawConfigToDto(RawConfig config) {
    return CurveConfigDto(
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
      fadeMs: config.fadeMs,
      motionTimeoutSecs: config.motionTimeoutSecs,
    );
  }
}
