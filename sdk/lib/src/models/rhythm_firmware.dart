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

/// User-visible stage of a server-side OTA update flow (mirrors the server
/// `OtaUpdateStage` enum). These stages come from `ota_update_progress` SSE
/// events emitted while the Rhythm server is updating itself.
enum RhythmOtaUpdateStage {
  checking,
  updateAvailable,
  upToDate,
  downloading,
  verifying,
  staging,
  installing,
  finalizing,
  restarting,
  failed,
}

RhythmOtaUpdateStage rhythmOtaUpdateStageFromWire(String? value) {
  return switch (value) {
    'checking' => RhythmOtaUpdateStage.checking,
    'update_available' => RhythmOtaUpdateStage.updateAvailable,
    'up_to_date' => RhythmOtaUpdateStage.upToDate,
    'downloading' => RhythmOtaUpdateStage.downloading,
    'verifying' => RhythmOtaUpdateStage.verifying,
    'staging' => RhythmOtaUpdateStage.staging,
    'installing' => RhythmOtaUpdateStage.installing,
    'finalizing' => RhythmOtaUpdateStage.finalizing,
    'restarting' => RhythmOtaUpdateStage.restarting,
    'failed' => RhythmOtaUpdateStage.failed,
    _ => RhythmOtaUpdateStage.checking,
  };
}

/// An `ota_update_progress` SSE event from the server.
class RhythmOtaUpdateProgress {
  final RhythmOtaUpdateStage stage;
  final String message;
  final String? currentVersion;
  final String? targetVersion;
  final bool? updateAvailable;
  final int? downloadedBytes;
  final int? totalBytes;
  final int? percent;
  final bool? checksumVerified;
  final List<String> installedTargets;
  final String? error;

  const RhythmOtaUpdateProgress({
    required this.stage,
    required this.message,
    this.currentVersion,
    this.targetVersion,
    this.updateAvailable,
    this.downloadedBytes,
    this.totalBytes,
    this.percent,
    this.checksumVerified,
    this.installedTargets = const [],
    this.error,
  });

  factory RhythmOtaUpdateProgress.fromJson(Map<String, dynamic> json) {
    final installedRaw = json['installed_targets'];
    return RhythmOtaUpdateProgress(
      stage: rhythmOtaUpdateStageFromWire(json['stage'] as String?),
      message: json['message'] as String? ?? '',
      currentVersion: json['current_version'] as String?,
      targetVersion: json['target_version'] as String?,
      updateAvailable: json['update_available'] as bool?,
      downloadedBytes: _readInt(json['downloaded_bytes']),
      totalBytes: _readInt(json['total_bytes']),
      percent: _readInt(json['percent']),
      checksumVerified: json['checksum_verified'] as bool?,
      installedTargets: installedRaw is List
          ? installedRaw.map((e) => e.toString()).toList(growable: false)
          : const [],
      error: json['error'] as String?,
    );
  }

  bool get isTerminal =>
      stage == RhythmOtaUpdateStage.upToDate ||
      stage == RhythmOtaUpdateStage.restarting ||
      stage == RhythmOtaUpdateStage.failed;

  static int? _readInt(Object? value) {
    if (value is int) return value;
    if (value is num) return value.toInt();
    if (value is String) return int.tryParse(value);
    return null;
  }
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
