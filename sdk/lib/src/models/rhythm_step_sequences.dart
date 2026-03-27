/// Step sequences for dimming visualization.
class RhythmStepSequences {
  final List<RhythmStepPoint> stepUp;
  final List<RhythmStepPoint> stepDown;

  RhythmStepSequences({required this.stepUp, required this.stepDown});

  factory RhythmStepSequences.fromJson(Map<String, dynamic> json) {
    return RhythmStepSequences(
      stepUp: (json['step_up']['steps'] as List)
          .map((e) => RhythmStepPoint.fromJson(e))
          .toList(),
      stepDown: (json['step_down']['steps'] as List)
          .map((e) => RhythmStepPoint.fromJson(e))
          .toList(),
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
      kelvin: (json['kelvin'] as num).toInt(),
      rgb: (json['rgb'] as List).map((e) => (e as num).toInt()).toList(),
    );
  }
}
