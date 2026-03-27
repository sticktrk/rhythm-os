/// Current time and lighting information.
class RhythmTimeInfo {
  final String currentTime;
  final double currentHour;
  final String? timezone;
  final int brightness;
  final int kelvin;
  final double solarPosition;

  RhythmTimeInfo({
    required this.currentTime,
    required this.currentHour,
    this.timezone,
    required this.brightness,
    required this.kelvin,
    required this.solarPosition,
  });

  factory RhythmTimeInfo.fromJson(Map<String, dynamic> json) {
    final lighting = json['lighting'] as Map<String, dynamic>;
    return RhythmTimeInfo(
      currentTime: json['current_time'] as String,
      currentHour: (json['current_hour'] as num).toDouble(),
      timezone: json['timezone'] as String?,
      brightness: (lighting['brightness'] as num).toInt(),
      kelvin: (lighting['kelvin'] as num).toInt(),
      solarPosition: (lighting['solar_position'] as num).toDouble(),
    );
  }
}
