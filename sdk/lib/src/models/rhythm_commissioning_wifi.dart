/// Ephemeral owner-only commissioning material. Never persist or log this DTO.
class RhythmCommissioningWifi {
  const RhythmCommissioningWifi({required this.ssid, required this.password});

  final String ssid;
  final String password;

  factory RhythmCommissioningWifi.fromJson(Map<String, dynamic> json) {
    final ssid = json['ssid'];
    final password = json['password'];
    if (ssid is! String || ssid.isEmpty || password is! String) {
      throw const FormatException('Invalid commissioning Wi-Fi response');
    }
    return RhythmCommissioningWifi(ssid: ssid, password: password);
  }

  @override
  String toString() => 'RhythmCommissioningWifi(<redacted>)';
}
