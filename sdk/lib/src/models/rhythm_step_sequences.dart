/// Step sequences for dimming visualization.
class RhythmStepSequences {
  final List<RhythmStepPoint> stepUp;
  final List<RhythmStepPoint> stepDown;

  RhythmStepSequences({required this.stepUp, required this.stepDown});

  factory RhythmStepSequences.fromJson(Map<String, dynamic> json) {
    final steps = _asMap(json['steps']) ?? json;
    final rawStepUp = steps['step_up'];
    final rawStepDown = steps['step_down'];
    final stepUp = _stepList(rawStepUp);
    final stepDown = _stepList(rawStepDown);
    return RhythmStepSequences(
      stepUp: stepUp.map((e) => RhythmStepPoint.fromJson(e)).toList(),
      stepDown: stepDown.map((e) => RhythmStepPoint.fromJson(e)).toList(),
    );
  }
}

/// A single step point.
class RhythmStepPoint {
  final double hour;
  final int brightness;
  final int kelvin;
  final List<int> rgb;

  RhythmStepPoint({
    required this.hour,
    required this.brightness,
    required this.kelvin,
    required this.rgb,
  });

  factory RhythmStepPoint.fromJson(Map<String, dynamic> json) {
    return RhythmStepPoint(
      hour: (json['hour'] as num).toDouble(),
      brightness: (json['brightness'] as num).toInt(),
      kelvin: (json['kelvin'] as num?)?.toInt() ?? 0,
      rgb: _rgbList(json['rgb']),
    );
  }
}

Map<String, dynamic>? _asMap(dynamic value) {
  if (value is Map<String, dynamic>) return value;
  if (value is Map) return value.cast<String, dynamic>();
  return null;
}

List<Map<String, dynamic>> _stepList(dynamic value) {
  if (value is List) {
    return value.cast<Map<String, dynamic>>();
  }
  if (value is Map) {
    final map = value.cast<String, dynamic>();
    final steps = map['steps'];
    if (steps is List) return steps.cast<Map<String, dynamic>>();
  }
  return const [];
}

List<int> _rgbList(dynamic value) {
  if (value is List) {
    return value.map((e) => (e as num).toInt()).toList();
  }
  if (value is Map) {
    final map = value.cast<String, dynamic>();
    return [
      (map['r'] as num?)?.toInt() ?? 0,
      (map['g'] as num?)?.toInt() ?? 0,
      (map['b'] as num?)?.toInt() ?? 0,
    ];
  }
  return const [0, 0, 0];
}
