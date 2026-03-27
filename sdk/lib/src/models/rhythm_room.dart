/// Device type in the Rhythm device model.
enum RhythmDeviceType {
  light,
  button,
  motion;

  static RhythmDeviceType fromString(String value) => switch (value) {
        'light' => RhythmDeviceType.light,
        'motion' => RhythmDeviceType.motion,
        _ => RhythmDeviceType.button,
      };
}

/// A device from the Rhythm device registry.
class RhythmDevice {
  final String id;
  final RhythmDeviceType type;
  final String? name;
  final String? manufacturer;
  final String? model;

  const RhythmDevice({
    required this.id,
    required this.type,
    this.name,
    this.manufacturer,
    this.model,
  });

  String get displayName => name ?? id.substring(0, id.length.clamp(0, 8));

  String? get productInfo {
    if (manufacturer == null && model == null) return null;
    final parts = [manufacturer, model].whereType<String>();
    return parts.join(' \u00b7 ');
  }

  factory RhythmDevice.fromJson(Map<String, dynamic> json) => RhythmDevice(
        id: json['id'] as String? ?? '',
        type:
            RhythmDeviceType.fromString(json['type'] as String? ?? 'button'),
        name: json['name'] as String?,
        manufacturer: json['manufacturer'] as String?,
        model: json['model'] as String?,
      );
}

/// A room as reported by the Rhythm server.
class RhythmRoom {
  final String id;
  final String name;
  final String groupedLightId;
  final bool rhythmEnabled;
  final bool disabled;
  final double timeOffset;
  final double brightnessOffset;
  final bool softOff;
  final String? hubType;
  final List<String> deviceIds;
  final List<RhythmDevice> devices;
  final bool? lightsOn;
  final int? brightness;
  final int? kelvin;

  const RhythmRoom({
    required this.id,
    required this.name,
    required this.groupedLightId,
    required this.rhythmEnabled,
    required this.disabled,
    required this.timeOffset,
    required this.brightnessOffset,
    required this.softOff,
    this.hubType,
    this.deviceIds = const [],
    this.devices = const [],
    this.lightsOn,
    this.brightness,
    this.kelvin,
  });

  bool get hasMotionSensor =>
      devices.any((d) => d.type == RhythmDeviceType.motion);

  List<RhythmDevice> get lights =>
      devices.where((d) => d.type == RhythmDeviceType.light).toList();

  List<RhythmDevice> get buttons =>
      devices.where((d) => d.type == RhythmDeviceType.button).toList();

  List<RhythmDevice> get motionSensors =>
      devices.where((d) => d.type == RhythmDeviceType.motion).toList();

  int get lightCount {
    final typed = lights.length;
    if (typed > 0) return typed;
    final inferred = deviceIds.length - devices.length;
    return inferred > 0 ? inferred : 0;
  }

  int get deviceCount => devices.length;

  String get deviceSummary {
    final parts = <String>[];
    final l = lights.length;
    final b = buttons.length;
    final m = motionSensors.length;
    if (l > 0) parts.add('$l light${l > 1 ? 's' : ''}');
    if (b > 0) parts.add('$b button${b > 1 ? 's' : ''}');
    if (m > 0) parts.add('$m sensor${m > 1 ? 's' : ''}');
    return parts.isEmpty ? 'No devices' : parts.join(', ');
  }

  factory RhythmRoom.fromJson(Map<String, dynamic> json) {
    return RhythmRoom(
      id: json['id'] as String? ?? '',
      name: json['name'] as String? ?? '',
      groupedLightId: json['grouped_light_id'] as String? ?? '',
      rhythmEnabled: json['rhythm_enabled'] as bool? ?? false,
      disabled: json['disabled'] as bool? ?? false,
      timeOffset: (json['time_offset'] as num?)?.toDouble() ?? 0.0,
      brightnessOffset:
          (json['brightness_offset'] as num?)?.toDouble() ?? 0.0,
      softOff: json['soft_off'] as bool? ?? false,
      hubType: json['hub_type'] as String?,
      deviceIds: (json['device_ids'] as List<dynamic>?)
              ?.map((e) => e as String)
              .toList() ??
          [],
      devices: (json['devices'] as List<dynamic>?)
              ?.map((e) => RhythmDevice.fromJson(e as Map<String, dynamic>))
              .toList() ??
          [],
      lightsOn: json['lights_on'] as bool?,
      brightness: (json['brightness'] as num?)?.toInt(),
      kelvin: (json['kelvin'] as num?)?.toInt(),
    );
  }
}

/// Room state from Rhythm server poll diffs and SSE events.
class RhythmRoomState {
  final String roomId;
  final bool rhythmEnabled;
  final double timeOffset;
  final double brightnessOffset;
  final bool softOff;
  final bool? lightsOn;
  final int? brightness;
  final int? kelvin;
  final bool tick;

  const RhythmRoomState({
    required this.roomId,
    required this.rhythmEnabled,
    required this.timeOffset,
    required this.brightnessOffset,
    required this.softOff,
    this.lightsOn,
    this.brightness,
    this.kelvin,
    this.tick = false,
  });

  factory RhythmRoomState.fromJson(Map<String, dynamic> json) {
    return RhythmRoomState(
      roomId: json['room_id'] as String? ?? json['id'] as String? ?? '',
      rhythmEnabled: json['rhythm_enabled'] as bool? ?? false,
      timeOffset: (json['time_offset'] as num?)?.toDouble() ?? 0.0,
      brightnessOffset:
          (json['brightness_offset'] as num?)?.toDouble() ?? 0.0,
      softOff: json['soft_off'] as bool? ?? false,
      lightsOn: json['lights_on'] as bool?,
      brightness: (json['brightness'] as num?)?.toInt(),
      kelvin: (json['kelvin'] as num?)?.toInt(),
      tick: json['tick'] as bool? ?? false,
    );
  }
}

/// Motion timer state from Rhythm server poll diffs.
class RhythmMotionTimer {
  final String roomId;
  final bool motionActive;
  final bool motionOwned;
  final int? remainingSecs;
  final int timeoutSecs;

  const RhythmMotionTimer({
    required this.roomId,
    required this.motionActive,
    required this.motionOwned,
    this.remainingSecs,
    required this.timeoutSecs,
  });

  const RhythmMotionTimer.cleared(this.roomId)
      : motionActive = false,
        motionOwned = false,
        remainingSecs = null,
        timeoutSecs = 0;

  bool get isCleared => timeoutSecs == 0 && !motionActive;
}
