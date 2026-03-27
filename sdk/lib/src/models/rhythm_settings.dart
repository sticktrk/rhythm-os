/// Global device settings from the Rhythm server.
class RhythmSettings {
  final int bulbFadeMs;
  final int rhythmIntervalSecs;
  final int defaultMotionTimeoutSecs;
  final bool powerSave;
  final int softOffBrightness;

  const RhythmSettings({
    required this.bulbFadeMs,
    required this.rhythmIntervalSecs,
    required this.defaultMotionTimeoutSecs,
    required this.powerSave,
    required this.softOffBrightness,
  });

  factory RhythmSettings.fromJson(Map<String, dynamic> json) {
    return RhythmSettings(
      bulbFadeMs: (json['bulb_fade_ms'] as num?)?.toInt() ?? 500,
      rhythmIntervalSecs:
          (json['rhythm_interval_secs'] as num?)?.toInt() ?? 60,
      defaultMotionTimeoutSecs:
          (json['default_motion_timeout_secs'] as num?)?.toInt() ?? 600,
      powerSave: json['power_save'] as bool? ?? false,
      softOffBrightness:
          (json['soft_off_brightness'] as num?)?.toInt() ?? 1,
    );
  }
}
