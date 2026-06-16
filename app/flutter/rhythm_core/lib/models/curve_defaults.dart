import '../src/rust/api/dto/curve.dart' show CurveConfigDto;

const CurveConfigDto _defaultCurveConfig = CurveConfigDto(
  minColorTemp: 1800,
  maxColorTemp: 5500,
  minBrightness: 2,
  maxBrightness: 100,
  widthLeftBri: 0.95,
  widthRightBri: 0.85,
  widthLeftCct: 0.95,
  widthRightCct: 1.15,
  shapeP: 6.0,
  maxDimSteps: 6,
  fadeMs: 500,
  motionTimeoutSecs: 1200,
);

/// Shared curve defaults copied from the server-side Rust profile defaults.
CurveConfigDto get defaultCurveConfig => _defaultCurveConfig;

/// Parse a partial snake_case JSON map using shared default values.
CurveConfigDto curveConfigFromJson(Map<String, dynamic> json) {
  final d = defaultCurveConfig;
  return CurveConfigDto(
    minColorTemp: (json['min_color_temp'] as num?)?.toInt() ?? d.minColorTemp,
    maxColorTemp: (json['max_color_temp'] as num?)?.toInt() ?? d.maxColorTemp,
    minBrightness: (json['min_brightness'] as num?)?.toInt() ?? d.minBrightness,
    maxBrightness: (json['max_brightness'] as num?)?.toInt() ?? d.maxBrightness,
    widthLeftBri: (json['width_left_bri'] as num?)?.toDouble() ?? d.widthLeftBri,
    widthRightBri:
        (json['width_right_bri'] as num?)?.toDouble() ?? d.widthRightBri,
    widthLeftCct: (json['width_left_cct'] as num?)?.toDouble() ?? d.widthLeftCct,
    widthRightCct:
        (json['width_right_cct'] as num?)?.toDouble() ?? d.widthRightCct,
    shapeP: (json['shape_p'] as num?)?.toDouble() ?? d.shapeP,
    maxDimSteps: (json['max_dim_steps'] as num?)?.toInt() ?? d.maxDimSteps,
    fadeMs: (json['fade_ms'] as num?)?.toInt() ?? d.fadeMs,
    motionTimeoutSecs:
        (json['motion_timeout_secs'] as num?)?.toInt() ?? d.motionTimeoutSecs,
  );
}
