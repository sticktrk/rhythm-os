import '../json_parsing.dart';

enum RhythmMode {
  day,
  sleep;

  String get wireValue => switch (this) {
        RhythmMode.day => 'day',
        RhythmMode.sleep => 'sleep',
      };

  static RhythmMode? fromString(String? value) => switch (value) {
        'day' => RhythmMode.day,
        'sleep' => RhythmMode.sleep,
        _ => null,
      };
}

enum RoomModeState {
  active,
  idle,
  wake,
  warning,
  hardOff;

  String get wireValue => switch (this) {
        RoomModeState.active => 'active',
        RoomModeState.idle => 'idle',
        RoomModeState.wake => 'wake',
        RoomModeState.warning => 'warning',
        RoomModeState.hardOff => 'hard_off',
      };

  static RoomModeState fromString(String? value) => switch (value) {
        'idle' => RoomModeState.idle,
        'wake' => RoomModeState.wake,
        'warning' => RoomModeState.warning,
        'hard_off' => RoomModeState.hardOff,
        _ => RoomModeState.active,
      };

  static RoomModeState fromJson(Map<String, dynamic> json) {
    final explicit = json['state'] as String?;
    if (explicit != null && explicit.isNotEmpty) {
      return fromString(explicit);
    }
    if (json['hard_off'] as bool? ?? false) {
      return RoomModeState.hardOff;
    }
    if (json['soft_off'] as bool? ?? false) {
      return RoomModeState.idle;
    }
    return RoomModeState.active;
  }
}

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
        type: RhythmDeviceType.fromString(json['type'] as String? ?? 'button'),
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
  final RoomModeState state;
  final bool transitioning;
  final bool rhythmEnabled;
  final bool disabled;
  final double timeOffset;
  final double brightnessOffset;
  final List<String> hubTypes;
  final List<String> deviceIds;
  final List<RhythmDevice> devices;
  final bool? lightsOn;
  final int? brightness;
  final int? kelvin;

  const RhythmRoom({
    required this.id,
    required this.name,
    required this.groupedLightId,
    required this.state,
    this.transitioning = false,
    required this.rhythmEnabled,
    required this.disabled,
    required this.timeOffset,
    required this.brightnessOffset,
    this.hubTypes = const [],
    this.deviceIds = const [],
    this.devices = const [],
    this.lightsOn,
    this.brightness,
    this.kelvin,
  });

  bool get hasMotionSensor =>
      devices.any((d) => d.type == RhythmDeviceType.motion);

  String? get hubType => hubTypes.isEmpty ? null : hubTypes.first;

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

  bool get softOff => state == RoomModeState.idle;

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
    // Prefer hub_types array; fall back to legacy singular hub_type.
    var hubTypes = (json['hub_types'] as List<dynamic>?)?.cast<String>() ?? [];
    if (hubTypes.isEmpty) {
      final legacy = json['hub_type'] as String?;
      if (legacy != null) hubTypes = [legacy];
    }
    return RhythmRoom(
      id: json['id'] as String? ?? '',
      name: json['name'] as String? ?? '',
      groupedLightId: json['grouped_light_id'] as String? ?? '',
      state: RoomModeState.fromJson(json),
      transitioning: json['transitioning'] as bool? ?? false,
      rhythmEnabled: json['rhythm_enabled'] as bool? ?? false,
      disabled: json['disabled'] as bool? ?? false,
      timeOffset: jsonDouble(json['time_offset'],
              preferredKeys: const ['time_offset']) ??
          0.0,
      brightnessOffset: jsonDouble(
            json['brightness_offset'],
            preferredKeys: const ['brightness_offset'],
          ) ??
          0.0,
      hubTypes: hubTypes,
      deviceIds: (json['device_ids'] as List<dynamic>?)
              ?.map((e) => e as String)
              .toList() ??
          [],
      devices: (json['devices'] as List<dynamic>?)
              ?.map(jsonMap)
              .nonNulls
              .map(RhythmDevice.fromJson)
              .toList() ??
          [],
      lightsOn: json['lights_on'] as bool?,
      brightness: jsonInt(
        json['brightness'],
        preferredKeys: const ['brightness', 'brightness_pct'],
      ),
      kelvin: jsonInt(
        json['kelvin'],
        preferredKeys: const ['kelvin', 'color_temp'],
      ),
    );
  }
}

/// RGB color from server room state events (for direct-color profiles).
class RhythmRoomColor {
  final int r;
  final int g;
  final int b;

  const RhythmRoomColor({required this.r, required this.g, required this.b});

  factory RhythmRoomColor.fromJson(Map<String, dynamic> json) =>
      RhythmRoomColor(
        r: (json['r'] as num?)?.toInt() ?? 0,
        g: (json['g'] as num?)?.toInt() ?? 0,
        b: (json['b'] as num?)?.toInt() ?? 0,
      );
}

/// Room state from Rhythm server poll diffs and SSE events.
class RhythmRoomState {
  final String roomId;
  final RhythmMode? mode;
  final RoomModeState state;
  final bool transitioning;
  final bool rhythmEnabled;
  final double timeOffset;
  final double brightnessOffset;
  final bool? lightsOn;
  final int? brightness;
  final int? kelvin;
  final RhythmRoomColor? color;
  final bool tick;

  const RhythmRoomState({
    required this.roomId,
    this.mode,
    required this.state,
    this.transitioning = false,
    required this.rhythmEnabled,
    required this.timeOffset,
    required this.brightnessOffset,
    this.lightsOn,
    this.brightness,
    this.kelvin,
    this.color,
    this.tick = false,
  });

  bool get softOff => state == RoomModeState.idle;

  factory RhythmRoomState.fromJson(Map<String, dynamic> json) {
    return RhythmRoomState(
      roomId: json['room_id'] as String? ?? json['id'] as String? ?? '',
      mode: RhythmMode.fromString(json['mode'] as String?),
      state: RoomModeState.fromJson(json),
      transitioning: json['transitioning'] as bool? ?? false,
      rhythmEnabled: json['rhythm_enabled'] as bool? ?? false,
      timeOffset: jsonDouble(json['time_offset'],
              preferredKeys: const ['time_offset']) ??
          0.0,
      brightnessOffset: jsonDouble(
            json['brightness_offset'],
            preferredKeys: const ['brightness_offset'],
          ) ??
          0.0,
      lightsOn: json['lights_on'] as bool?,
      brightness: jsonInt(
        json['brightness'],
        preferredKeys: const ['brightness', 'brightness_pct'],
      ),
      kelvin: jsonInt(
        json['kelvin'],
        preferredKeys: const ['kelvin', 'color_temp'],
      ),
      color: json['color'] is Map<String, dynamic>
          ? RhythmRoomColor.fromJson(json['color'] as Map<String, dynamic>)
          : null,
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
