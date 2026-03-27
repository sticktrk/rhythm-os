/// OTA update state machine.
enum RhythmOtaState {
  idle,
  checking,
  available,
  upToDate,
  downloading,
  uploading,
  flashing,
  rebooting,
  complete,
  error,
}

/// Firmware release metadata from the manifest.
class RhythmFirmwareRelease {
  final String version;
  final String url;
  final int? size;
  final String? changelog;

  const RhythmFirmwareRelease({
    required this.version,
    required this.url,
    this.size,
    this.changelog,
  });

  factory RhythmFirmwareRelease.fromJson(Map<String, dynamic> json) {
    final relativeUrl = json['url'] as String? ?? '';
    return RhythmFirmwareRelease(
      version: json['version'] as String? ?? '0.0.0',
      url: 'https://dl.rhythm.lighting/esp32/$relativeUrl',
      size: json['size'] as int?,
      changelog: json['changelog'] as String?,
    );
  }
}

/// OTA progress event emitted during firmware update.
class RhythmOtaProgress {
  final RhythmOtaState state;
  final int progressPercent;
  final String? errorMessage;

  const RhythmOtaProgress({
    required this.state,
    this.progressPercent = 0,
    this.errorMessage,
  });
}
