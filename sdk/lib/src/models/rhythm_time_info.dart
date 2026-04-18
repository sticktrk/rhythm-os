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
    final lighting = _asMap(json['lighting']);
    final location = _asMap(json['location']);
    final currentHour =
        ((lighting?['hour'] ?? json['current_hour'] ?? json['hour']) as num?)
                ?.toDouble() ??
            0.0;
    return RhythmTimeInfo(
      currentTime: (json['current_time'] ??
              json['current_local_time'] ??
              location?['current_local_time'] ??
              location?['current_time']) as String? ??
          _formatHour(currentHour),
      currentHour: currentHour,
      timezone: json['timezone'] as String?,
      brightness:
          ((lighting?['brightness'] ?? json['brightness']) as num?)?.toInt() ??
              0,
      kelvin: ((lighting?['kelvin'] ?? json['kelvin']) as num?)?.toInt() ?? 0,
      solarPosition:
          ((lighting?['solar_position'] ?? json['sun_position']) as num?)
                  ?.toDouble() ??
              0.0,
    );
  }
}

Map<String, dynamic>? _asMap(dynamic value) {
  if (value is Map<String, dynamic>) return value;
  if (value is Map) return value.cast<String, dynamic>();
  return null;
}

String _formatHour(double hour) {
  final whole = hour.floor() % 24;
  final minutes = ((hour - whole) * 60).round();
  return '${whole.toString().padLeft(2, '0')}:${minutes.toString().padLeft(2, '0')}';
}
