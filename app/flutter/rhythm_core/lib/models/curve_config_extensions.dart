import '../src/rust/api/dto/curve.dart' show CurveConfigDto;
import 'curve_defaults.dart' show curveConfigFromJson;

/// Extension on generated CurveConfigDto to add convenience methods.
///
/// This allows us to use the generated DTO directly without maintaining
/// a separate manual CurveConfig class.
extension CurveConfigDtoX on CurveConfigDto {
  /// Create a copy with updated fields (immutable update pattern).
  CurveConfigDto copyWith({
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
    return CurveConfigDto(
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

  /// Convert to JSON for REST API (snake_case keys).
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

  /// Convert to query parameters for API calls.
  Map<String, dynamic> toQueryParams() {
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
    };
  }

  /// Create from JSON (REST API response, snake_case keys).
  /// Uses shared Dart defaults for any missing fields.
  static CurveConfigDto fromJson(Map<String, dynamic> json) {
    return curveConfigFromJson(json);
  }
}
