/// User-visible coarse status of a pairing flow (mirrors the server enum).
enum RhythmPairingStatus {
  searching,
  commissioning,
  complete,
  failed,
}

RhythmPairingStatus rhythmPairingStatusFromWire(String? value) {
  return switch (value) {
    'searching' => RhythmPairingStatus.searching,
    'commissioning' => RhythmPairingStatus.commissioning,
    'complete' => RhythmPairingStatus.complete,
    'failed' => RhythmPairingStatus.failed,
    _ => RhythmPairingStatus.searching,
  };
}

/// More specific user-visible stage of a pairing flow.
enum RhythmPairingStage {
  requested,
  hubConnecting,
  searching,
  connecting,
  commissioning,
  finalizing,
  complete,
  failed,
}

RhythmPairingStage rhythmPairingStageFromWire(String? value) {
  return switch (value) {
    'requested' => RhythmPairingStage.requested,
    'hub_connecting' => RhythmPairingStage.hubConnecting,
    'searching' => RhythmPairingStage.searching,
    'connecting' => RhythmPairingStage.connecting,
    'commissioning' => RhythmPairingStage.commissioning,
    'finalizing' => RhythmPairingStage.finalizing,
    'complete' => RhythmPairingStage.complete,
    'failed' => RhythmPairingStage.failed,
    _ => RhythmPairingStage.requested,
  };
}

/// Information about a successfully paired device.
class RhythmPairedDevice {
  final String deviceId;
  final String name;
  final String deviceType;
  final String? manufacturer;
  final String? model;

  const RhythmPairedDevice({
    required this.deviceId,
    required this.name,
    required this.deviceType,
    this.manufacturer,
    this.model,
  });

  factory RhythmPairedDevice.fromJson(Map<String, dynamic> json) {
    return RhythmPairedDevice(
      deviceId: json['device_id'] as String? ?? '',
      name: json['name'] as String? ?? 'Device',
      deviceType: json['device_type'] as String? ?? 'light',
      manufacturer: json['manufacturer'] as String?,
      model: json['model'] as String?,
    );
  }
}

/// A `pairing_progress` SSE event from the server.
class RhythmPairingProgress {
  final String hubType;
  final String? sessionId;
  final RhythmPairingStatus status;
  final RhythmPairingStage stage;
  final String message;
  final RhythmPairedDevice? device;
  final List<RhythmPairedDevice> devices;
  final List<String> warnings;
  final String? error;

  const RhythmPairingProgress({
    required this.hubType,
    required this.status,
    required this.stage,
    required this.message,
    this.sessionId,
    this.device,
    this.devices = const [],
    this.warnings = const [],
    this.error,
  });

  factory RhythmPairingProgress.fromJson(Map<String, dynamic> json) {
    final deviceJson = json['device'];
    final device = deviceJson is Map<String, dynamic>
        ? RhythmPairedDevice.fromJson(deviceJson)
        : deviceJson is Map
            ? RhythmPairedDevice.fromJson(deviceJson.cast<String, dynamic>())
            : null;
    final devicesJson = json['devices'];
    final devices = <RhythmPairedDevice>[];
    if (devicesJson is List) {
      for (final value in devicesJson) {
        if (value is Map<String, dynamic>) {
          devices.add(RhythmPairedDevice.fromJson(value));
        } else if (value is Map) {
          devices.add(
            RhythmPairedDevice.fromJson(value.cast<String, dynamic>()),
          );
        }
      }
    }
    final warningsJson = json['warnings'];
    final warnings = warningsJson is List
        ? warningsJson
            .whereType<String>()
            .map((warning) => warning.trim())
            .where((warning) => warning.isNotEmpty)
        : const Iterable<String>.empty();
    return RhythmPairingProgress(
      hubType: json['hub_type'] as String? ?? 'unknown',
      sessionId: json['session_id'] as String?,
      status: rhythmPairingStatusFromWire(json['status'] as String?),
      stage: rhythmPairingStageFromWire(json['stage'] as String?),
      message: json['message'] as String? ?? '',
      device: device,
      devices: List.unmodifiable(devices),
      warnings: List.unmodifiable(warnings),
      error: json['error'] as String?,
    );
  }

  /// Every paired device carried by a terminal event, with the legacy
  /// singular payload used as a fallback for older servers.
  List<RhythmPairedDevice> get completedDevices =>
      devices.isNotEmpty ? devices : [if (device != null) device!];

  bool get isTerminal =>
      stage == RhythmPairingStage.complete ||
      stage == RhythmPairingStage.failed;
}
