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

  factory RhythmPairedDevice.strictFromJson(Map<String, dynamic> json) {
    final deviceId = _requiredBoundedString(json, 'device_id', 256);
    final name = _requiredBoundedString(json, 'name', 256);
    final deviceType = _requiredBoundedString(json, 'device_type', 64);
    final manufacturer = _optionalBoundedString(json, 'manufacturer', 128);
    final model = _optionalBoundedString(json, 'model', 128);
    return RhythmPairedDevice(
      deviceId: deviceId,
      name: name,
      deviceType: deviceType,
      manufacturer: manufacturer,
      model: model,
    );
  }
}

/// Durable polling state for a client-generated pairing session ID.
enum RhythmPairingResultState { pending, terminal, notFound }

RhythmPairingResultState rhythmPairingResultStateFromWire(String? value) {
  return switch (value) {
    'terminal' => RhythmPairingResultState.terminal,
    'not_found' => RhythmPairingResultState.notFound,
    _ => RhythmPairingResultState.pending,
  };
}

RhythmPairingResultState _strictPairingResultState(Object? value) {
  return switch (value) {
    'pending' => RhythmPairingResultState.pending,
    'terminal' => RhythmPairingResultState.terminal,
    'not_found' => RhythmPairingResultState.notFound,
    _ => throw const FormatException('Invalid pairing result state'),
  };
}

RhythmPairingStatus _strictTerminalPairingStatus(Object? value) {
  return switch (value) {
    'complete' => RhythmPairingStatus.complete,
    'failed' => RhythmPairingStatus.failed,
    _ => throw const FormatException('Invalid terminal pairing status'),
  };
}

String _requiredBoundedString(
  Map<String, dynamic> json,
  String key,
  int maxLength,
) {
  final value = json[key];
  if (value is! String || value.trim().isEmpty || value.length > maxLength) {
    throw FormatException('Invalid $key');
  }
  return value;
}

String? _optionalBoundedString(
  Map<String, dynamic> json,
  String key,
  int maxLength,
) {
  final value = json[key];
  if (value == null) return null;
  if (value is! String || value.length > maxLength) {
    throw FormatException('Invalid $key');
  }
  return value;
}

/// Terminal result persisted by the server for pairing reconciliation.
class RhythmPairingSessionResult {
  final String hubType;
  final RhythmPairingStatus status;
  final RhythmPairedDevice? device;
  final List<RhythmPairedDevice> devices;
  final List<String> warnings;
  final String? error;

  const RhythmPairingSessionResult({
    required this.hubType,
    required this.status,
    this.device,
    this.devices = const [],
    this.warnings = const [],
    this.error,
  });

  factory RhythmPairingSessionResult.fromJson(Map<String, dynamic> json) {
    final rawDevice = json['device'];
    if (rawDevice != null && rawDevice is! Map) {
      throw const FormatException('Invalid paired device');
    }
    final device = rawDevice is Map
        ? RhythmPairedDevice.strictFromJson(rawDevice.cast<String, dynamic>())
        : null;
    final devices = <RhythmPairedDevice>[];
    final rawDevices = json['devices'];
    if (rawDevices != null && rawDevices is! List) {
      throw const FormatException('Invalid pairing devices');
    }
    if (rawDevices is List) {
      if (rawDevices.length > 64) {
        throw const FormatException('Too many pairing devices');
      }
      for (final raw in rawDevices) {
        if (raw is! Map) {
          throw const FormatException('Invalid paired device');
        }
        devices.add(
          RhythmPairedDevice.strictFromJson(raw.cast<String, dynamic>()),
        );
      }
    }
    final rawWarnings = json['warnings'];
    if (rawWarnings != null && rawWarnings is! List) {
      throw const FormatException('Invalid pairing warnings');
    }
    if (rawWarnings is List && rawWarnings.length > 32) {
      throw const FormatException('Too many pairing warnings');
    }
    final warnings = <String>[];
    if (rawWarnings is List) {
      for (final raw in rawWarnings) {
        if (raw is! String || raw.length > 512) {
          throw const FormatException('Invalid pairing warning');
        }
        final warning = raw.trim();
        if (warning.isNotEmpty) warnings.add(warning);
      }
    }
    final hubType = _requiredBoundedString(json, 'hub_type', 64);
    final status = _strictTerminalPairingStatus(json['status']);
    final error = _optionalBoundedString(json, 'error', 512);
    if (device != null && devices.isNotEmpty) {
      final first = devices.first;
      if (device.deviceId != first.deviceId ||
          device.name != first.name ||
          device.deviceType != first.deviceType ||
          device.manufacturer != first.manufacturer ||
          device.model != first.model) {
        throw const FormatException(
          'Inconsistent first-device pairing projections',
        );
      }
    }
    final result = RhythmPairingSessionResult(
      hubType: hubType,
      status: status,
      device: device,
      devices: List.unmodifiable(devices),
      warnings: List.unmodifiable(warnings),
      error: error,
    );
    switch (status) {
      case RhythmPairingStatus.complete:
        if (result.completedDevices.isEmpty || error != null) {
          throw const FormatException('Invalid completed pairing result');
        }
        break;
      case RhythmPairingStatus.failed:
        if (device != null ||
            devices.isNotEmpty ||
            (rawWarnings is List && rawWarnings.isNotEmpty)) {
          throw const FormatException('Invalid failed pairing result');
        }
        break;
      default:
        throw const FormatException('Invalid terminal pairing status');
    }
    return result;
  }

  List<RhythmPairedDevice> get completedDevices =>
      devices.isNotEmpty ? devices : [if (device != null) device!];

  bool get succeeded => status == RhythmPairingStatus.complete;
}

/// Response from `GET /api/devices/pair/:session_id`.
class RhythmPairingResultStatus {
  final String sessionId;
  final RhythmPairingResultState state;
  final String? hubType;
  final RhythmPairingSessionResult? result;

  const RhythmPairingResultStatus({
    required this.sessionId,
    required this.state,
    this.hubType,
    this.result,
  });

  factory RhythmPairingResultStatus.fromJson(Map<String, dynamic> json) {
    final rawResult = json['result'];
    if (rawResult != null && rawResult is! Map) {
      throw const FormatException('Invalid pairing result');
    }
    final sessionId = _requiredBoundedString(json, 'session_id', 96);
    final state = _strictPairingResultState(json['state']);
    final hubType = _optionalBoundedString(json, 'hub_type', 64);
    if (hubType != null && hubType.trim().isEmpty) {
      throw const FormatException('Invalid hub_type');
    }
    final result = RhythmPairingResultStatus(
      sessionId: sessionId,
      state: state,
      hubType: hubType,
      result: rawResult is Map
          ? RhythmPairingSessionResult.fromJson(
              rawResult.cast<String, dynamic>(),
            )
          : null,
    );
    switch (state) {
      case RhythmPairingResultState.pending:
        if (hubType == null || result.result != null) {
          throw const FormatException('Invalid pending pairing result');
        }
      case RhythmPairingResultState.terminal:
        if (hubType == null ||
            result.result == null ||
            result.result!.hubType != hubType) {
          throw const FormatException('Invalid terminal pairing result');
        }
      case RhythmPairingResultState.notFound:
        if (hubType != null || result.result != null) {
          throw const FormatException('Invalid missing pairing result');
        }
    }
    return result;
  }

  bool get isTerminal => state == RhythmPairingResultState.terminal;
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
