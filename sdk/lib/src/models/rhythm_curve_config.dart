/// Pure Dart curve configuration, replacing the FRB-generated RhythmCurveConfigDto.
///
/// Defaults match the Rust constants in `rhythm-curve/src/config.rs` and
/// `rhythm-core/src/config.rs`.
class RhythmCurveConfig {
  static const int defaultMinColorTemp = 1800;
  static const int defaultMaxColorTemp = 5500;
  static const int defaultMinBrightness = 2;
  static const int defaultMaxBrightness = 100;
  static const double defaultWidthLeftBri = 0.95;
  static const double defaultWidthRightBri = 0.85;
  static const double defaultWidthLeftCct = 0.95;
  static const double defaultWidthRightCct = 1.15;
  static const double defaultShapeP = 6.0;
  static const int defaultMaxDimSteps = 6;

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

  const RhythmCurveConfig({
    this.minColorTemp = defaultMinColorTemp,
    this.maxColorTemp = defaultMaxColorTemp,
    this.minBrightness = defaultMinBrightness,
    this.maxBrightness = defaultMaxBrightness,
    this.widthLeftBri = defaultWidthLeftBri,
    this.widthRightBri = defaultWidthRightBri,
    this.widthLeftCct = defaultWidthLeftCct,
    this.widthRightCct = defaultWidthRightCct,
    this.shapeP = defaultShapeP,
    this.maxDimSteps = defaultMaxDimSteps,
  });

  factory RhythmCurveConfig.fromJson(Map<String, dynamic> json) {
    return RhythmCurveConfig(
      minColorTemp:
          (json['min_color_temp'] as num?)?.toInt() ?? defaultMinColorTemp,
      maxColorTemp:
          (json['max_color_temp'] as num?)?.toInt() ?? defaultMaxColorTemp,
      minBrightness:
          (json['min_brightness'] as num?)?.toInt() ?? defaultMinBrightness,
      maxBrightness:
          (json['max_brightness'] as num?)?.toInt() ?? defaultMaxBrightness,
      widthLeftBri:
          (json['width_left_bri'] as num?)?.toDouble() ?? defaultWidthLeftBri,
      widthRightBri:
          (json['width_right_bri'] as num?)?.toDouble() ?? defaultWidthRightBri,
      widthLeftCct:
          (json['width_left_cct'] as num?)?.toDouble() ?? defaultWidthLeftCct,
      widthRightCct:
          (json['width_right_cct'] as num?)?.toDouble() ?? defaultWidthRightCct,
      shapeP: (json['shape_p'] as num?)?.toDouble() ?? defaultShapeP,
      maxDimSteps:
          (json['max_dim_steps'] as num?)?.toInt() ?? defaultMaxDimSteps,
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

  /// Convert to query parameters for API calls (excludes max_dim_steps).
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

  RhythmCurveConfig copyWith({
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
    return RhythmCurveConfig(
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

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is RhythmCurveConfig &&
          minColorTemp == other.minColorTemp &&
          maxColorTemp == other.maxColorTemp &&
          minBrightness == other.minBrightness &&
          maxBrightness == other.maxBrightness &&
          widthLeftBri == other.widthLeftBri &&
          widthRightBri == other.widthRightBri &&
          widthLeftCct == other.widthLeftCct &&
          widthRightCct == other.widthRightCct &&
          shapeP == other.shapeP &&
          maxDimSteps == other.maxDimSteps;

  @override
  int get hashCode => Object.hash(
        minColorTemp,
        maxColorTemp,
        minBrightness,
        maxBrightness,
        widthLeftBri,
        widthRightBri,
        widthLeftCct,
        widthRightCct,
        shapeP,
        maxDimSteps,
      );
}
