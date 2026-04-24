import '../json_parsing.dart';

class RhythmTimerBreakpoint {
  final double hour;
  final int value;

  const RhythmTimerBreakpoint({
    required this.hour,
    required this.value,
  });

  factory RhythmTimerBreakpoint.fromJson(Map<String, dynamic> json) =>
      RhythmTimerBreakpoint(
        hour: jsonDouble(json['hour'], preferredKeys: const ['hour']) ?? 0.0,
        value: jsonInt(json['value'], preferredKeys: const ['value']) ?? 0,
      );

  Map<String, dynamic> toJson() => {
        'hour': hour,
        'value': value,
      };

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is RhythmTimerBreakpoint &&
          hour == other.hour &&
          value == other.value;

  @override
  int get hashCode => Object.hash(hour, value);
}

sealed class RhythmTimerSetting {
  const RhythmTimerSetting();

  const factory RhythmTimerSetting.auto() = RhythmAutoTimerSetting;
  const factory RhythmTimerSetting.fixed(int value) = RhythmFixedTimerSetting;
  const factory RhythmTimerSetting.scheduled(
    List<RhythmTimerBreakpoint> breakpoints,
  ) = RhythmScheduledTimerSetting;

  factory RhythmTimerSetting.fromJson(dynamic json) {
    if (json is num) return RhythmTimerSetting.fixed(json.toInt());
    if (json is String) {
      final parsed = int.tryParse(json);
      if (parsed != null) return RhythmTimerSetting.fixed(parsed);
    }

    final map = jsonMap(json);
    if (map == null) return const RhythmTimerSetting.auto();

    final mode = map['mode'] as String?;
    if (mode == 'scheduled' || map['breakpoints'] is List<dynamic>) {
      return RhythmTimerSetting.scheduled(
        (map['breakpoints'] as List<dynamic>? ?? const <dynamic>[])
            .map(jsonMap)
            .nonNulls
            .map(RhythmTimerBreakpoint.fromJson)
            .toList(growable: false),
      );
    }

    final fixedValue = jsonInt(map['value'], preferredKeys: const ['value']);
    if (mode == 'fixed' || fixedValue != null) {
      return RhythmTimerSetting.fixed(fixedValue ?? 0);
    }

    return const RhythmTimerSetting.auto();
  }

  Map<String, dynamic> toJson();

  bool get isAuto;
  bool get isFixed;
  bool get isScheduled;

  int? get fixedValue;
  List<RhythmTimerBreakpoint> get breakpoints;
}

class RhythmAutoTimerSetting extends RhythmTimerSetting {
  const RhythmAutoTimerSetting();

  @override
  Map<String, dynamic> toJson() => {
        'mode': 'auto',
      };

  @override
  bool get isAuto => true;

  @override
  bool get isFixed => false;

  @override
  bool get isScheduled => false;

  @override
  int? get fixedValue => null;

  @override
  List<RhythmTimerBreakpoint> get breakpoints =>
      const <RhythmTimerBreakpoint>[];

  @override
  bool operator ==(Object other) => other is RhythmAutoTimerSetting;

  @override
  int get hashCode => 0;
}

class RhythmFixedTimerSetting extends RhythmTimerSetting {
  final int value;

  const RhythmFixedTimerSetting(this.value);

  @override
  Map<String, dynamic> toJson() => {
        'mode': 'fixed',
        'value': value,
      };

  @override
  bool get isAuto => false;

  @override
  bool get isFixed => true;

  @override
  bool get isScheduled => false;

  @override
  int? get fixedValue => value;

  @override
  List<RhythmTimerBreakpoint> get breakpoints =>
      const <RhythmTimerBreakpoint>[];

  @override
  bool operator ==(Object other) =>
      other is RhythmFixedTimerSetting && other.value == value;

  @override
  int get hashCode => value.hashCode;
}

class RhythmScheduledTimerSetting extends RhythmTimerSetting {
  @override
  final List<RhythmTimerBreakpoint> breakpoints;

  const RhythmScheduledTimerSetting(this.breakpoints);

  @override
  Map<String, dynamic> toJson() => {
        'mode': 'scheduled',
        'breakpoints':
            breakpoints.map((breakpoint) => breakpoint.toJson()).toList(),
      };

  @override
  bool get isAuto => false;

  @override
  bool get isFixed => false;

  @override
  bool get isScheduled => true;

  @override
  int? get fixedValue => null;

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is RhythmScheduledTimerSetting &&
          _listEquals(breakpoints, other.breakpoints);

  @override
  int get hashCode => Object.hashAll(breakpoints);
}

/// RGB color payload from the server.
class RhythmRgbColor {
  final int r;
  final int g;
  final int b;

  const RhythmRgbColor({
    required this.r,
    required this.g,
    required this.b,
  });

  factory RhythmRgbColor.fromJson(Map<String, dynamic> json) => RhythmRgbColor(
        r: (json['r'] as num).toInt(),
        g: (json['g'] as num).toInt(),
        b: (json['b'] as num).toInt(),
      );

  Map<String, dynamic> toJson() => {
        'r': r,
        'g': g,
        'b': b,
      };

  RhythmRgbColor copyWith({int? r, int? g, int? b}) => RhythmRgbColor(
        r: r ?? this.r,
        g: g ?? this.g,
        b: b ?? this.b,
      );

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is RhythmRgbColor && r == other.r && g == other.g && b == other.b;

  @override
  int get hashCode => Object.hash(r, g, b);
}

/// XY color payload from the server.
class RhythmXyColor {
  final double x;
  final double y;

  const RhythmXyColor({
    required this.x,
    required this.y,
  });

  factory RhythmXyColor.fromJson(Map<String, dynamic> json) => RhythmXyColor(
        x: (json['x'] as num).toDouble(),
        y: (json['y'] as num).toDouble(),
      );

  Map<String, dynamic> toJson() => {
        'x': x,
        'y': y,
      };

  RhythmXyColor copyWith({double? x, double? y}) => RhythmXyColor(
        x: x ?? this.x,
        y: y ?? this.y,
      );

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is RhythmXyColor && x == other.x && y == other.y;

  @override
  int get hashCode => Object.hash(x, y);
}

/// Fixed RGB/XY color attached to a super-Gaussian profile.
class RhythmDirectColor {
  final RhythmRgbColor rgb;
  final RhythmXyColor xy;

  const RhythmDirectColor({
    required this.rgb,
    required this.xy,
  });

  factory RhythmDirectColor.fromJson(Map<String, dynamic> json) =>
      RhythmDirectColor(
        rgb: RhythmRgbColor.fromJson(json['rgb'] as Map<String, dynamic>),
        xy: RhythmXyColor.fromJson(json['xy'] as Map<String, dynamic>),
      );

  Map<String, dynamic> toJson() => {
        'rgb': rgb.toJson(),
        'xy': xy.toJson(),
      };

  RhythmDirectColor copyWith({
    RhythmRgbColor? rgb,
    RhythmXyColor? xy,
  }) =>
      RhythmDirectColor(
        rgb: rgb ?? this.rgb,
        xy: xy ?? this.xy,
      );

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is RhythmDirectColor && rgb == other.rgb && xy == other.xy;

  @override
  int get hashCode => Object.hash(rgb, xy);
}

/// Palette keyframe for palette-based profiles.
class RhythmPaletteKeyframe {
  final double hour;
  final int r;
  final int g;
  final int b;

  const RhythmPaletteKeyframe({
    required this.hour,
    required this.r,
    required this.g,
    required this.b,
  });

  factory RhythmPaletteKeyframe.fromJson(Map<String, dynamic> json) =>
      RhythmPaletteKeyframe(
        hour: (json['hour'] as num).toDouble(),
        r: (json['r'] as num).toInt(),
        g: (json['g'] as num).toInt(),
        b: (json['b'] as num).toInt(),
      );

  Map<String, dynamic> toJson() => {
        'hour': hour,
        'r': r,
        'g': g,
        'b': b,
      };

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is RhythmPaletteKeyframe &&
          hour == other.hour &&
          r == other.r &&
          g == other.g &&
          b == other.b;

  @override
  int get hashCode => Object.hash(hour, r, g, b);
}

/// Tagged union base type for profile curve shapes.
abstract class RhythmCurveShape {
  final String type;

  const RhythmCurveShape(this.type);

  factory RhythmCurveShape.fromJson(Map<String, dynamic> json) {
    final type = json['type'] as String? ?? 'super-gaussian';
    return switch (type) {
      'palette' => RhythmPaletteCurve.fromJson(json),
      'inherit-active' => const RhythmInheritActiveCurve(),
      'constant' => RhythmConstantCurve.fromJson(json),
      _ => RhythmSuperGaussianCurve.fromJson(json),
    };
  }

  Map<String, dynamic> toJson();
}

class RhythmSuperGaussianCurve extends RhythmCurveShape {
  final double widthLeftBri;
  final double widthRightBri;
  final double widthLeftCct;
  final double widthRightCct;
  final double shapeP;
  final RhythmDirectColor? directColor;

  const RhythmSuperGaussianCurve({
    this.widthLeftBri = RhythmCurveConfig.defaultWidthLeftBri,
    this.widthRightBri = RhythmCurveConfig.defaultWidthRightBri,
    this.widthLeftCct = RhythmCurveConfig.defaultWidthLeftCct,
    this.widthRightCct = RhythmCurveConfig.defaultWidthRightCct,
    this.shapeP = RhythmCurveConfig.defaultShapeP,
    this.directColor,
  }) : super('super-gaussian');

  factory RhythmSuperGaussianCurve.fromJson(Map<String, dynamic> json) =>
      RhythmSuperGaussianCurve(
        widthLeftBri: jsonDouble(
              json['width_left_bri'],
              preferredKeys: const ['width_left_bri'],
            ) ??
            RhythmCurveConfig.defaultWidthLeftBri,
        widthRightBri: jsonDouble(
              json['width_right_bri'],
              preferredKeys: const ['width_right_bri'],
            ) ??
            RhythmCurveConfig.defaultWidthRightBri,
        widthLeftCct: jsonDouble(
              json['width_left_cct'],
              preferredKeys: const ['width_left_cct'],
            ) ??
            RhythmCurveConfig.defaultWidthLeftCct,
        widthRightCct: jsonDouble(
              json['width_right_cct'],
              preferredKeys: const ['width_right_cct'],
            ) ??
            RhythmCurveConfig.defaultWidthRightCct,
        shapeP: jsonDouble(
              json['shape_p'],
              preferredKeys: const ['shape_p'],
            ) ??
            RhythmCurveConfig.defaultShapeP,
        directColor: jsonMap(json['direct_color']) == null
            ? null
            : RhythmDirectColor.fromJson(
                jsonMap(json['direct_color'])!,
              ),
      );

  @override
  Map<String, dynamic> toJson() => {
        'type': type,
        'width_left_bri': widthLeftBri,
        'width_right_bri': widthRightBri,
        'width_left_cct': widthLeftCct,
        'width_right_cct': widthRightCct,
        'shape_p': shapeP,
        if (directColor != null) 'direct_color': directColor!.toJson(),
      };

  RhythmSuperGaussianCurve copyWith({
    double? widthLeftBri,
    double? widthRightBri,
    double? widthLeftCct,
    double? widthRightCct,
    double? shapeP,
    RhythmDirectColor? directColor,
  }) =>
      RhythmSuperGaussianCurve(
        widthLeftBri: widthLeftBri ?? this.widthLeftBri,
        widthRightBri: widthRightBri ?? this.widthRightBri,
        widthLeftCct: widthLeftCct ?? this.widthLeftCct,
        widthRightCct: widthRightCct ?? this.widthRightCct,
        shapeP: shapeP ?? this.shapeP,
        directColor: directColor ?? this.directColor,
      );

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is RhythmSuperGaussianCurve &&
          widthLeftBri == other.widthLeftBri &&
          widthRightBri == other.widthRightBri &&
          widthLeftCct == other.widthLeftCct &&
          widthRightCct == other.widthRightCct &&
          shapeP == other.shapeP &&
          directColor == other.directColor;

  @override
  int get hashCode => Object.hash(
        widthLeftBri,
        widthRightBri,
        widthLeftCct,
        widthRightCct,
        shapeP,
        directColor,
      );
}

class RhythmPaletteCurve extends RhythmCurveShape {
  final List<RhythmPaletteKeyframe> keyframes;

  const RhythmPaletteCurve({
    required this.keyframes,
  }) : super('palette');

  factory RhythmPaletteCurve.fromJson(Map<String, dynamic> json) =>
      RhythmPaletteCurve(
        keyframes: (json['keyframes'] as List<dynamic>? ?? const <dynamic>[])
            .map((e) =>
                RhythmPaletteKeyframe.fromJson(e as Map<String, dynamic>))
            .toList(),
      );

  @override
  Map<String, dynamic> toJson() => {
        'type': type,
        'keyframes': keyframes.map((keyframe) => keyframe.toJson()).toList(),
      };

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is RhythmPaletteCurve && _listEquals(keyframes, other.keyframes);

  @override
  int get hashCode => Object.hashAll(keyframes);
}

class RhythmInheritActiveCurve extends RhythmCurveShape {
  const RhythmInheritActiveCurve() : super('inherit-active');

  @override
  Map<String, dynamic> toJson() => {
        'type': type,
      };

  @override
  bool operator ==(Object other) =>
      identical(this, other) || other is RhythmInheritActiveCurve;

  @override
  int get hashCode => type.hashCode;
}

class RhythmConstantCurve extends RhythmCurveShape {
  final int brightness;
  final int colorTemp;
  final RhythmDirectColor? directColor;

  const RhythmConstantCurve({
    required this.brightness,
    required this.colorTemp,
    this.directColor,
  }) : super('constant');

  factory RhythmConstantCurve.fromJson(Map<String, dynamic> json) =>
      RhythmConstantCurve(
        brightness: jsonInt(
              json['brightness'],
              preferredKeys: const ['brightness', 'brightness_pct'],
            ) ??
            1,
        colorTemp:
            jsonInt(json['color_temp'], preferredKeys: const ['color_temp']) ??
                0,
        directColor: jsonMap(json['direct_color']) == null
            ? null
            : RhythmDirectColor.fromJson(
                jsonMap(json['direct_color'])!,
              ),
      );

  @override
  Map<String, dynamic> toJson() => {
        'type': type,
        'brightness': brightness,
        'color_temp': colorTemp,
        if (directColor != null) 'direct_color': directColor!.toJson(),
      };

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is RhythmConstantCurve &&
          brightness == other.brightness &&
          colorTemp == other.colorTemp &&
          directColor == other.directColor;

  @override
  int get hashCode => Object.hash(brightness, colorTemp, directColor);
}

/// Full stored light profile config from the backend.
class RhythmCurveConfig {
  static const int defaultMinColorTemp = 2200;
  static const int defaultMaxColorTemp = 6500;
  static const int defaultMinBrightness = 1;
  static const int defaultMaxBrightness = 100;
  static const double defaultWidthLeftBri = 0.95;
  static const double defaultWidthRightBri = 0.85;
  static const double defaultWidthLeftCct = 0.95;
  static const double defaultWidthRightCct = 1.15;
  static const double defaultShapeP = 6.0;
  static const int defaultMaxDimSteps = 12;
  static const int defaultFadeMs = 500;
  static const int defaultMotionTimeoutSecs = 600;
  static const int defaultRhythmIntervalSecs = 60;

  final String id;
  final String name;
  final RhythmCurveShape? _curve;
  final double _widthLeftBri;
  final double _widthRightBri;
  final double _widthLeftCct;
  final double _widthRightCct;
  final double _shapeP;
  final RhythmDirectColor? _directColor;
  final int minColorTemp;
  final int maxColorTemp;
  final int minBrightness;
  final int maxBrightness;
  final int maxDimSteps;
  final RhythmTimerSetting? _fadeSetting;
  final RhythmTimerSetting? _motionTimeoutSetting;
  final RhythmTimerSetting? _rhythmIntervalSetting;
  final int? _fadeMsLegacy;
  final int? _motionTimeoutSecsLegacy;
  final int? _rhythmIntervalSecsLegacy;

  const RhythmCurveConfig({
    this.id = '',
    this.name = '',
    RhythmCurveShape? curve,
    this.minColorTemp = defaultMinColorTemp,
    this.maxColorTemp = defaultMaxColorTemp,
    this.minBrightness = defaultMinBrightness,
    this.maxBrightness = defaultMaxBrightness,
    double widthLeftBri = defaultWidthLeftBri,
    double widthRightBri = defaultWidthRightBri,
    double widthLeftCct = defaultWidthLeftCct,
    double widthRightCct = defaultWidthRightCct,
    double shapeP = defaultShapeP,
    RhythmDirectColor? directColor,
    this.maxDimSteps = defaultMaxDimSteps,
    RhythmTimerSetting? fadeSetting,
    RhythmTimerSetting? motionTimeoutSetting,
    RhythmTimerSetting? rhythmIntervalSetting,
    int? fadeMs = defaultFadeMs,
    int? motionTimeoutSecs = defaultMotionTimeoutSecs,
    int? rhythmIntervalSecs = defaultRhythmIntervalSecs,
  })  : _curve = curve,
        _widthLeftBri = widthLeftBri,
        _widthRightBri = widthRightBri,
        _widthLeftCct = widthLeftCct,
        _widthRightCct = widthRightCct,
        _shapeP = shapeP,
        _directColor = directColor,
        _fadeSetting = fadeSetting,
        _motionTimeoutSetting = motionTimeoutSetting,
        _rhythmIntervalSetting = rhythmIntervalSetting,
        _fadeMsLegacy = fadeMs,
        _motionTimeoutSecsLegacy = motionTimeoutSecs,
        _rhythmIntervalSecsLegacy = rhythmIntervalSecs;

  factory RhythmCurveConfig.fromJson(Map<String, dynamic> json) {
    final curveJson = jsonMap(json['curve']);
    return RhythmCurveConfig(
      id: json['id'] as String? ?? '',
      name: json['name'] as String? ?? '',
      curve: curveJson != null
          ? RhythmCurveShape.fromJson(curveJson)
          : RhythmSuperGaussianCurve.fromJson(json),
      minColorTemp: jsonInt(
            json['min_color_temp'],
            preferredKeys: const ['min_color_temp'],
          ) ??
          defaultMinColorTemp,
      maxColorTemp: jsonInt(
            json['max_color_temp'],
            preferredKeys: const ['max_color_temp'],
          ) ??
          defaultMaxColorTemp,
      minBrightness: jsonInt(
            json['min_brightness'],
            preferredKeys: const ['min_brightness'],
          ) ??
          defaultMinBrightness,
      maxBrightness: jsonInt(
            json['max_brightness'],
            preferredKeys: const ['max_brightness'],
          ) ??
          defaultMaxBrightness,
      maxDimSteps: jsonInt(json['max_dim_steps'],
              preferredKeys: const ['max_dim_steps']) ??
          defaultMaxDimSteps,
      fadeSetting: _timerSettingFromJson(
        json,
        'fade_ms',
      ),
      motionTimeoutSetting: _timerSettingFromJson(
        json,
        'motion_timeout_secs',
      ),
      rhythmIntervalSetting: _timerSettingFromJson(
        json,
        'rhythm_interval_secs',
      ),
      fadeMs: null,
      motionTimeoutSecs: null,
      rhythmIntervalSecs: null,
    );
  }

  RhythmSuperGaussianCurve? get superGaussianCurve =>
      curve is RhythmSuperGaussianCurve
          ? curve as RhythmSuperGaussianCurve
          : null;

  RhythmCurveShape get curve =>
      _curve ??
      RhythmSuperGaussianCurve(
        widthLeftBri: _widthLeftBri,
        widthRightBri: _widthRightBri,
        widthLeftCct: _widthLeftCct,
        widthRightCct: _widthRightCct,
        shapeP: _shapeP,
        directColor: _directColor,
      );

  double get widthLeftBri =>
      superGaussianCurve?.widthLeftBri ?? defaultWidthLeftBri;
  double get widthRightBri =>
      superGaussianCurve?.widthRightBri ?? defaultWidthRightBri;
  double get widthLeftCct =>
      superGaussianCurve?.widthLeftCct ?? defaultWidthLeftCct;
  double get widthRightCct =>
      superGaussianCurve?.widthRightCct ?? defaultWidthRightCct;
  double get shapeP => superGaussianCurve?.shapeP ?? defaultShapeP;
  RhythmDirectColor? get directColor => superGaussianCurve?.directColor;
  RhythmTimerSetting? get fadeSetting =>
      _fadeSetting ?? _timerSettingFromLegacyValue(_fadeMsLegacy);
  RhythmTimerSetting? get motionTimeoutSetting =>
      _motionTimeoutSetting ??
      _timerSettingFromLegacyValue(_motionTimeoutSecsLegacy);
  RhythmTimerSetting? get rhythmIntervalSetting =>
      _rhythmIntervalSetting ??
      _timerSettingFromLegacyValue(_rhythmIntervalSecsLegacy);
  int? get fadeMs => fadeSetting?.fixedValue;
  int? get motionTimeoutSecs => motionTimeoutSetting?.fixedValue;
  int? get rhythmIntervalSecs => rhythmIntervalSetting?.fixedValue;

  Map<String, dynamic> toJson() => {
        'id': id,
        'name': name,
        'curve': curve.toJson(),
        'min_color_temp': minColorTemp,
        'max_color_temp': maxColorTemp,
        'min_brightness': minBrightness,
        'max_brightness': maxBrightness,
        'max_dim_steps': maxDimSteps,
        if (fadeSetting != null) 'fade_ms': fadeSetting!.toJson(),
        if (motionTimeoutSetting != null)
          'motion_timeout_secs': motionTimeoutSetting!.toJson(),
        if (rhythmIntervalSetting != null)
          'rhythm_interval_secs': rhythmIntervalSetting!.toJson(),
      };

  Map<String, dynamic> toQueryParams() => {
        'id': id,
        'min_color_temp': minColorTemp,
        'max_color_temp': maxColorTemp,
        'min_brightness': minBrightness,
        'max_brightness': maxBrightness,
        'max_dim_steps': maxDimSteps,
        if (rhythmIntervalSecs != null)
          'rhythm_interval_secs': rhythmIntervalSecs,
      };

  RhythmCurveConfig copyWith({
    String? id,
    String? name,
    RhythmCurveShape? curve,
    int? minColorTemp,
    int? maxColorTemp,
    int? minBrightness,
    int? maxBrightness,
    double? widthLeftBri,
    double? widthRightBri,
    double? widthLeftCct,
    double? widthRightCct,
    double? shapeP,
    RhythmDirectColor? directColor,
    int? maxDimSteps,
    Object? fadeSetting = _curveConfigCopySentinel,
    Object? motionTimeoutSetting = _curveConfigCopySentinel,
    Object? rhythmIntervalSetting = _curveConfigCopySentinel,
    Object? fadeMs = _curveConfigCopySentinel,
    Object? motionTimeoutSecs = _curveConfigCopySentinel,
    Object? rhythmIntervalSecs = _curveConfigCopySentinel,
  }) {
    final nextCurve = curve ??
        (() {
          final currentSuperGaussian = superGaussianCurve;
          if (currentSuperGaussian != null ||
              widthLeftBri != null ||
              widthRightBri != null ||
              widthLeftCct != null ||
              widthRightCct != null ||
              shapeP != null ||
              directColor != null) {
            return (currentSuperGaussian ?? const RhythmSuperGaussianCurve())
                .copyWith(
              widthLeftBri: widthLeftBri,
              widthRightBri: widthRightBri,
              widthLeftCct: widthLeftCct,
              widthRightCct: widthRightCct,
              shapeP: shapeP,
              directColor: directColor,
            );
          }
          return this.curve;
        })();
    final nextFadeSetting = !identical(fadeSetting, _curveConfigCopySentinel)
        ? fadeSetting as RhythmTimerSetting?
        : !identical(fadeMs, _curveConfigCopySentinel)
            ? _timerSettingFromLegacyValue(fadeMs as int?)
            : this.fadeSetting;
    final nextMotionTimeoutSetting =
        !identical(motionTimeoutSetting, _curveConfigCopySentinel)
            ? motionTimeoutSetting as RhythmTimerSetting?
            : !identical(motionTimeoutSecs, _curveConfigCopySentinel)
                ? _timerSettingFromLegacyValue(motionTimeoutSecs as int?)
                : this.motionTimeoutSetting;
    final nextRhythmIntervalSetting =
        !identical(rhythmIntervalSetting, _curveConfigCopySentinel)
            ? rhythmIntervalSetting as RhythmTimerSetting?
            : !identical(rhythmIntervalSecs, _curveConfigCopySentinel)
                ? _timerSettingFromLegacyValue(rhythmIntervalSecs as int?)
                : this.rhythmIntervalSetting;
    return RhythmCurveConfig(
      id: id ?? this.id,
      name: name ?? this.name,
      curve: nextCurve,
      minColorTemp: minColorTemp ?? this.minColorTemp,
      maxColorTemp: maxColorTemp ?? this.maxColorTemp,
      minBrightness: minBrightness ?? this.minBrightness,
      maxBrightness: maxBrightness ?? this.maxBrightness,
      maxDimSteps: maxDimSteps ?? this.maxDimSteps,
      fadeSetting: nextFadeSetting,
      motionTimeoutSetting: nextMotionTimeoutSetting,
      rhythmIntervalSetting: nextRhythmIntervalSetting,
      fadeMs: null,
      motionTimeoutSecs: null,
      rhythmIntervalSecs: null,
    );
  }

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is RhythmCurveConfig &&
          id == other.id &&
          name == other.name &&
          curve == other.curve &&
          minColorTemp == other.minColorTemp &&
          maxColorTemp == other.maxColorTemp &&
          minBrightness == other.minBrightness &&
          maxBrightness == other.maxBrightness &&
          maxDimSteps == other.maxDimSteps &&
          fadeSetting == other.fadeSetting &&
          motionTimeoutSetting == other.motionTimeoutSetting &&
          rhythmIntervalSetting == other.rhythmIntervalSetting;

  @override
  int get hashCode => Object.hash(
        id,
        name,
        curve,
        minColorTemp,
        maxColorTemp,
        minBrightness,
        maxBrightness,
        maxDimSteps,
        fadeSetting,
        motionTimeoutSetting,
        rhythmIntervalSetting,
      );
}

RhythmTimerSetting _timerSettingFromLegacyValue(int? value) => value == null
    ? const RhythmTimerSetting.auto()
    : RhythmTimerSetting.fixed(value);

RhythmTimerSetting? _timerSettingFromJson(
  Map<String, dynamic> json,
  String key,
) {
  if (!json.containsKey(key)) return null;
  final value = json[key];
  if (value == null) return null;
  return RhythmTimerSetting.fromJson(value);
}

const Object _curveConfigCopySentinel = Object();

bool _listEquals<T>(List<T> a, List<T> b) {
  if (identical(a, b)) return true;
  if (a.length != b.length) return false;
  for (int i = 0; i < a.length; i++) {
    if (a[i] != b[i]) return false;
  }
  return true;
}
