import '../json_parsing.dart';
import 'rhythm_curve_config.dart' show RhythmTimerSetting;

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
  mood,
  standby,

  /// Compatibility alias for the old soft-off/idle API name.
  idle,
  wake,
  warning,
  hardOff;

  String get wireValue => switch (this) {
        RoomModeState.active => 'active',
        RoomModeState.mood => 'mood',
        RoomModeState.standby || RoomModeState.idle => 'standby',
        RoomModeState.wake => 'wake',
        RoomModeState.warning => 'warning',
        RoomModeState.hardOff => 'hard_off',
      };

  static RoomModeState fromString(String? value) => switch (value) {
        'mood' => RoomModeState.mood,
        'idle' || 'standby' || 'soft_off' => RoomModeState.standby,
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
    if (json['mood_active'] as bool? ?? false) {
      return RoomModeState.mood;
    }
    if (json['soft_off'] as bool? ?? false) {
      return RoomModeState.standby;
    }
    if (json['standby_active'] as bool? ?? false) {
      return RoomModeState.standby;
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

  static RhythmDeviceType? fromNodeKind(RhythmNodeKind kind) => switch (kind) {
        RhythmNodeKind.lightDevice => RhythmDeviceType.light,
        RhythmNodeKind.motionSensor ||
        RhythmNodeKind.sensor =>
          RhythmDeviceType.motion,
        RhythmNodeKind.button ||
        RhythmNodeKind.switchDevice =>
          RhythmDeviceType.button,
        _ => null,
      };
}

enum RhythmNodeKind {
  room,
  lightDevice,
  switchDevice,
  motionSensor,
  sensor,
  button,
  otherDevice;

  static RhythmNodeKind fromString(String? value) => switch (value) {
        'light_device' => RhythmNodeKind.lightDevice,
        'switch_device' => RhythmNodeKind.switchDevice,
        'motion_sensor' => RhythmNodeKind.motionSensor,
        'sensor' => RhythmNodeKind.sensor,
        'button' => RhythmNodeKind.button,
        'other_device' => RhythmNodeKind.otherDevice,
        _ => RhythmNodeKind.room,
      };

  bool get isRoom => this == RhythmNodeKind.room;

  bool get isLightDevice => this == RhythmNodeKind.lightDevice;

  bool get isDevice => !isRoom;

  bool get isLightAddressable => isRoom || isLightDevice;
}

enum RhythmNodePlacement {
  hubDefault,
  userOverride,
  standalone;

  static RhythmNodePlacement? fromString(String? value) => switch (value) {
        'hub_default' => RhythmNodePlacement.hubDefault,
        'user_override' => RhythmNodePlacement.userOverride,
        'standalone' => RhythmNodePlacement.standalone,
        _ => null,
      };
}

class RhythmTopologyControlLink {
  final String kind;
  final String? targetId;
  final bool inherited;

  const RhythmTopologyControlLink({
    required this.kind,
    this.targetId,
    this.inherited = false,
  });

  bool get isMotion => kind == 'motion';

  factory RhythmTopologyControlLink.fromJson(Map<String, dynamic> json) =>
      RhythmTopologyControlLink(
        kind: json['kind'] as String? ?? '',
        targetId: json['target_id'] as String?,
        inherited: json['inherited'] as bool? ?? false,
      );
}

class RhythmHubRoomBinding {
  final Map<String, dynamic>? hubKey;
  final String hubRoomId;
  final String controlId;
  final List<String> lightDeviceIds;

  const RhythmHubRoomBinding({
    this.hubKey,
    required this.hubRoomId,
    required this.controlId,
    this.lightDeviceIds = const [],
  });

  factory RhythmHubRoomBinding.fromJson(Map<String, dynamic> json) =>
      RhythmHubRoomBinding(
        hubKey: jsonMap(json['hub_key']),
        hubRoomId: json['hub_room_id'] as String? ?? '',
        controlId: json['control_id'] as String? ?? '',
        lightDeviceIds: (json['light_device_ids'] as List<dynamic>?)
                ?.map((id) => id as String)
                .toList() ??
            const [],
      );
}

class RhythmNodeProfileSettings {
  final String? profileId;
  final bool? moodEnabled;
  final String? moodProfileId;
  final RhythmTimerSetting? fadeSetting;
  final RhythmTimerSetting? motionTimeoutSetting;
  final Map<String, dynamic> raw;

  const RhythmNodeProfileSettings({
    this.profileId,
    this.moodEnabled,
    this.moodProfileId,
    this.fadeSetting,
    this.motionTimeoutSetting,
    this.raw = const <String, dynamic>{},
  });

  int? get fadeMs => fadeSetting?.fixedValue;
  int? get motionTimeoutSecs => motionTimeoutSetting?.fixedValue;

  bool get isEmpty =>
      profileId == null &&
      moodEnabled == null &&
      moodProfileId == null &&
      fadeSetting == null &&
      motionTimeoutSetting == null &&
      raw.isEmpty;

  factory RhythmNodeProfileSettings.fromJson(Map<String, dynamic> json) {
    final raw = Map<String, dynamic>.from(json)
      ..remove('profile_id')
      ..remove('mood_enabled')
      ..remove('mood_profile_id')
      ..remove('idle_profile_id')
      ..remove('fade_ms')
      ..remove('motion_timeout_secs');
    final moodProfileId = json['mood_profile_id'] as String? ??
        json['idle_profile_id'] as String?;
    return RhythmNodeProfileSettings(
      profileId: json['profile_id'] as String?,
      moodEnabled: json['mood_enabled'] as bool?,
      moodProfileId:
          moodProfileId == null || moodProfileId.isEmpty ? null : moodProfileId,
      fadeSetting: _timerSettingFromJson(json, 'fade_ms'),
      motionTimeoutSetting: _timerSettingFromJson(json, 'motion_timeout_secs'),
      raw: raw,
    );
  }

  Map<String, dynamic> toJson() => {
        ...raw,
        if (profileId != null) 'profile_id': profileId,
        if (moodEnabled != null) 'mood_enabled': moodEnabled,
        if (moodProfileId != null) 'mood_profile_id': moodProfileId,
        if (fadeSetting != null) 'fade_ms': fadeSetting!.toJson(),
        if (motionTimeoutSetting != null)
          'motion_timeout_secs': motionTimeoutSetting!.toJson(),
      };
}

RhythmTimerSetting? _timerSettingFromJson(
  Map<String, dynamic> json,
  String key,
) {
  if (!json.containsKey(key)) return null;
  final value = json[key];
  if (value == null) return null;
  return RhythmTimerSetting.fromJson(value);
}

class RhythmObservedPower {
  final bool? lightsOn;
  final bool? fresh;
  final String? source;

  const RhythmObservedPower({
    this.lightsOn,
    this.fresh,
    this.source,
  });

  factory RhythmObservedPower.fromJson(Map<String, dynamic> json) {
    return RhythmObservedPower(
      lightsOn: json['lights_on'] as bool?,
      fresh: json['fresh'] as bool?,
      source: json['source'] as String?,
    );
  }

  static RhythmObservedPower? maybeFromJson(
    Map<String, dynamic> json, {
    bool synthesizeLegacy = false,
  }) {
    final observedPowerJson = jsonMap(json['observed_power']);
    if (observedPowerJson != null) {
      return RhythmObservedPower.fromJson(observedPowerJson);
    }
    if (!synthesizeLegacy || !json.containsKey('lights_on')) {
      return null;
    }
    return RhythmObservedPower(
      lightsOn: json['lights_on'] as bool?,
      fresh: true,
      source: 'legacy_lights_on',
    );
  }
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

  factory RhythmDevice.fromTopologyNode(RhythmTopologyNode node) {
    return RhythmDevice(
      id: node.id,
      type: RhythmDeviceType.fromNodeKind(node.kind) ?? RhythmDeviceType.button,
      name: node.name,
      manufacturer: node.manufacturer,
      model: node.model,
    );
  }
}

/// A topology/control node as reported by the Rhythm server.
///
/// The class name stays `RhythmRoom` for compatibility with existing callers,
/// but it now models any light-addressable node returned by the server.
class RhythmRoom {
  final String id;
  final String name;
  final RhythmNodeKind kind;
  final String? parentId;
  final RhythmNodePlacement? placement;
  final String groupedLightId;
  final RoomModeState state;
  final bool transitioning;
  final bool rhythmEnabled;
  final bool disabled;
  final double timeOffset;
  final double brightnessOffset;
  final List<String> hubTypes;
  final String? manufacturer;
  final String? model;
  final List<String> deviceIds;
  final List<RhythmDevice> devices;
  final RhythmNodeProfileSettings? profileSettings;
  final RhythmObservedPower? observedPower;
  final bool? lightsOn;
  final int? brightness;
  final int? kelvin;
  final bool moodEnabled;
  final bool moodActive;
  final bool standbyEnabled;
  final bool standbyActive;
  final bool? motionActive;
  final bool? motionOwned;
  final int? remainingSecs;
  final int? timeoutSecs;
  final bool? warningActive;

  const RhythmRoom({
    required this.id,
    required this.name,
    this.kind = RhythmNodeKind.room,
    this.parentId,
    this.placement,
    required this.groupedLightId,
    required this.state,
    this.transitioning = false,
    required this.rhythmEnabled,
    required this.disabled,
    required this.timeOffset,
    required this.brightnessOffset,
    this.hubTypes = const [],
    this.manufacturer,
    this.model,
    this.deviceIds = const [],
    this.devices = const [],
    RhythmNodeProfileSettings? profileSettings,
    RhythmNodeProfileSettings? roomProfile,
    this.observedPower,
    this.lightsOn,
    this.brightness,
    this.kelvin,
    this.moodEnabled = false,
    this.moodActive = false,
    this.standbyEnabled = false,
    this.standbyActive = false,
    this.motionActive,
    this.motionOwned,
    this.remainingSecs,
    this.timeoutSecs,
    this.warningActive,
  })  : assert(profileSettings == null || roomProfile == null),
        profileSettings = profileSettings ?? roomProfile;

  bool get hasMotionSensor =>
      kind == RhythmNodeKind.motionSensor ||
      kind == RhythmNodeKind.sensor ||
      devices.any((d) => d.type == RhythmDeviceType.motion);

  String? get hubType => hubTypes.isEmpty ? null : hubTypes.first;

  bool get isRoomNode => kind.isRoom;

  bool get isLightDeviceNode => kind.isLightDevice;

  bool get isLightAddressable => kind.isLightAddressable;

  RhythmNodeProfileSettings? get roomProfile => profileSettings;

  bool? get powerFresh => observedPower?.fresh;

  String? get powerSource => observedPower?.source;

  List<RhythmDevice> get lights =>
      devices.where((d) => d.type == RhythmDeviceType.light).toList();

  List<RhythmDevice> get buttons =>
      devices.where((d) => d.type == RhythmDeviceType.button).toList();

  List<RhythmDevice> get motionSensors =>
      devices.where((d) => d.type == RhythmDeviceType.motion).toList();

  int get lightCount {
    final typed = lights.length;
    if (typed > 0) return typed;
    if (kind == RhythmNodeKind.lightDevice) return 1;
    final inferred = deviceIds.length - devices.length;
    return inferred > 0 ? inferred : 0;
  }

  int get deviceCount => devices.length;

  bool get softOff =>
      state == RoomModeState.standby || state == RoomModeState.idle;

  String get deviceSummary {
    final parts = <String>[];
    final l = lights.length;
    final b = buttons.length;
    final m = motionSensors.length;
    if (l > 0) parts.add('$l light${l > 1 ? 's' : ''}');
    if (b > 0) parts.add('$b button${b > 1 ? 's' : ''}');
    if (m > 0) parts.add('$m sensor${m > 1 ? 's' : ''}');
    if (parts.isEmpty && kind == RhythmNodeKind.lightDevice) {
      parts.add('1 light');
    }
    return parts.isEmpty ? 'No devices' : parts.join(', ');
  }

  factory RhythmRoom.fromJson(Map<String, dynamic> json) {
    final observedPower = RhythmObservedPower.maybeFromJson(
      json,
      synthesizeLegacy: json.containsKey('lights_on'),
    );
    // Prefer hub_types array; fall back to legacy singular hub_type.
    var hubTypes = (json['hub_types'] as List<dynamic>?)?.cast<String>() ?? [];
    if (hubTypes.isEmpty) {
      final legacy = json['hub_type'] as String?;
      if (legacy != null) hubTypes = [legacy];
    }
    final profileSettings = switch (json['profile_settings']) {
      Map<String, dynamic> value => RhythmNodeProfileSettings.fromJson(value),
      _ when json['room_profile'] is Map<String, dynamic> =>
        RhythmNodeProfileSettings.fromJson(
          json['room_profile'] as Map<String, dynamic>,
        ),
      _ => null,
    };
    final state = RoomModeState.fromJson(json);
    return RhythmRoom(
      id: json['id'] as String? ?? json['node_id'] as String? ?? '',
      name: json['name'] as String? ?? '',
      kind: RhythmNodeKind.fromString(json['kind'] as String?),
      parentId: json['parent_id'] as String?,
      placement: RhythmNodePlacement.fromString(json['placement'] as String?),
      groupedLightId: json['grouped_light_id'] as String? ?? '',
      state: state,
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
      manufacturer: json['manufacturer'] as String?,
      model: json['model'] as String?,
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
      profileSettings: profileSettings,
      observedPower: observedPower,
      lightsOn: observedPower?.lightsOn ?? json['lights_on'] as bool?,
      brightness: jsonInt(
        json['brightness'],
        preferredKeys: const ['brightness', 'brightness_pct'],
      ),
      kelvin: jsonInt(
        json['kelvin'],
        preferredKeys: const ['kelvin', 'color_temp'],
      ),
      moodEnabled: json['mood_enabled'] as bool? ??
          profileSettings?.moodEnabled ??
          state == RoomModeState.mood,
      moodActive: json['mood_active'] as bool? ?? state == RoomModeState.mood,
      standbyEnabled: json['standby_enabled'] as bool? ?? false,
      standbyActive:
          json['standby_active'] as bool? ?? state == RoomModeState.standby,
      motionActive: json['motion_active'] as bool?,
      motionOwned: json['motion_owned'] as bool?,
      remainingSecs: jsonInt(
        json['remaining_secs'],
        preferredKeys: const ['remaining_secs', 'motion_remaining'],
      ),
      timeoutSecs: jsonInt(
        json['timeout_secs'],
        preferredKeys: const ['timeout_secs', 'motion_timeout'],
      ),
      warningActive: json['warning_active'] as bool?,
    );
  }
}

class RhythmTopologyNode {
  final String id;
  final String name;
  final RhythmNodeKind kind;
  final String? parentId;
  final RhythmNodePlacement? placement;
  final List<RhythmTopologyControlLink> controls;
  final List<RhythmHubRoomBinding> hubRoomBindings;
  final String? manufacturer;
  final String? model;
  final bool? userCustomized;
  final String? bootstrapName;

  const RhythmTopologyNode({
    required this.id,
    required this.name,
    required this.kind,
    this.parentId,
    this.placement,
    this.controls = const [],
    this.hubRoomBindings = const [],
    this.manufacturer,
    this.model,
    this.userCustomized,
    this.bootstrapName,
  });

  bool get isRoom => kind.isRoom;

  bool get isDevice => kind.isDevice;

  RhythmTopologyControlLink? controlForKind(String kind) {
    for (final control in controls) {
      if (control.kind == kind) return control;
    }
    return null;
  }

  String? controlTargetId(String kind) => controlForKind(kind)?.targetId;

  String? get motionTargetId => controlTargetId('motion');

  factory RhythmTopologyNode.fromJson(Map<String, dynamic> json) =>
      RhythmTopologyNode(
        id: json['id'] as String? ?? '',
        name: json['name'] as String? ?? '',
        kind: RhythmNodeKind.fromString(json['kind'] as String?),
        parentId: json['parent_id'] as String?,
        placement: RhythmNodePlacement.fromString(json['placement'] as String?),
        controls: (json['controls'] as List<dynamic>?)
                ?.map(jsonMap)
                .nonNulls
                .map(RhythmTopologyControlLink.fromJson)
                .toList() ??
            const [],
        hubRoomBindings: (json['hub_room_bindings'] as List<dynamic>?)
                ?.map(jsonMap)
                .nonNulls
                .map(RhythmHubRoomBinding.fromJson)
                .toList() ??
            const [],
        manufacturer: json['manufacturer'] as String?,
        model: json['model'] as String?,
        userCustomized: json['user_customized'] as bool?,
        bootstrapName: json['bootstrap_name'] as String?,
      );
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

/// Node state from Rhythm server poll diffs and SSE events.
class RhythmRoomState {
  final String nodeId;
  final RhythmMode? mode;
  final RoomModeState state;
  final bool transitioning;
  final bool rhythmEnabled;
  final double timeOffset;
  final double brightnessOffset;
  final RhythmNodeKind? kind;
  final String? name;
  final String? parentId;
  final RhythmNodePlacement? placement;
  final List<String> hubTypes;
  final String? manufacturer;
  final String? model;
  final RhythmObservedPower? observedPower;
  final bool? lightsOn;
  final int? brightness;
  final int? kelvin;
  final RhythmRoomColor? color;
  final RhythmNodeProfileSettings? profileSettings;
  final bool? moodEnabled;
  final bool? moodActive;
  final bool? standbyEnabled;
  final bool? standbyActive;
  final bool? motionActive;
  final bool? motionOwned;
  final int? remainingSecs;
  final int? timeoutSecs;
  final bool? warningActive;
  final bool tick;

  const RhythmRoomState({
    required this.nodeId,
    this.mode,
    required this.state,
    this.transitioning = false,
    required this.rhythmEnabled,
    required this.timeOffset,
    required this.brightnessOffset,
    this.kind,
    this.name,
    this.parentId,
    this.placement,
    this.hubTypes = const [],
    this.manufacturer,
    this.model,
    this.observedPower,
    this.lightsOn,
    this.brightness,
    this.kelvin,
    this.color,
    RhythmNodeProfileSettings? profileSettings,
    RhythmNodeProfileSettings? roomProfile,
    this.moodEnabled,
    this.moodActive,
    this.standbyEnabled,
    this.standbyActive,
    this.motionActive,
    this.motionOwned,
    this.remainingSecs,
    this.timeoutSecs,
    this.warningActive,
    this.tick = false,
  })  : assert(profileSettings == null || roomProfile == null),
        profileSettings = profileSettings ?? roomProfile;

  String get roomId => nodeId;

  RhythmNodeProfileSettings? get roomProfile => profileSettings;

  bool? get powerFresh => observedPower?.fresh;

  String? get powerSource => observedPower?.source;

  bool get softOff =>
      state == RoomModeState.standby || state == RoomModeState.idle;

  factory RhythmRoomState.fromJson(Map<String, dynamic> json) {
    final observedPower = RhythmObservedPower.maybeFromJson(
      json,
      synthesizeLegacy: json.containsKey('lights_on'),
    );
    var hubTypes = (json['hub_types'] as List<dynamic>?)?.cast<String>() ?? [];
    if (hubTypes.isEmpty) {
      final legacy = json['hub_type'] as String?;
      if (legacy != null) hubTypes = [legacy];
    }
    final profileSettings = switch (json['profile_settings']) {
      Map<String, dynamic> value => RhythmNodeProfileSettings.fromJson(value),
      _ when json['room_profile'] is Map<String, dynamic> =>
        RhythmNodeProfileSettings.fromJson(
          json['room_profile'] as Map<String, dynamic>,
        ),
      _ => null,
    };
    final state = RoomModeState.fromJson(json);
    return RhythmRoomState(
      nodeId: json['node_id'] as String? ??
          json['room_id'] as String? ??
          json['id'] as String? ??
          '',
      mode: RhythmMode.fromString(json['mode'] as String?),
      state: state,
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
      kind: json.containsKey('kind')
          ? RhythmNodeKind.fromString(json['kind'] as String?)
          : null,
      name: json['name'] as String?,
      parentId: json['parent_id'] as String?,
      placement: RhythmNodePlacement.fromString(json['placement'] as String?),
      hubTypes: hubTypes,
      manufacturer: json['manufacturer'] as String?,
      model: json['model'] as String?,
      observedPower: observedPower,
      lightsOn: observedPower?.lightsOn ?? json['lights_on'] as bool?,
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
      profileSettings: profileSettings,
      moodEnabled: json['mood_enabled'] as bool? ??
          profileSettings?.moodEnabled ??
          (state == RoomModeState.mood ? true : null),
      moodActive: json['mood_active'] as bool? ??
          (state == RoomModeState.mood ? true : null),
      standbyEnabled: json['standby_enabled'] as bool?,
      standbyActive: json['standby_active'] as bool? ??
          (state == RoomModeState.standby ? true : null),
      motionActive: json['motion_active'] as bool?,
      motionOwned: json['motion_owned'] as bool?,
      remainingSecs: jsonInt(
        json['remaining_secs'],
        preferredKeys: const ['remaining_secs', 'motion_remaining'],
      ),
      timeoutSecs: jsonInt(
        json['timeout_secs'],
        preferredKeys: const ['timeout_secs', 'motion_timeout'],
      ),
      warningActive: json['warning_active'] as bool?,
      tick: json['tick'] as bool? ?? false,
    );
  }
}

/// Motion timer state from Rhythm server poll diffs and SSE events.
class RhythmMotionTimer {
  final String nodeId;
  final bool motionActive;
  final bool motionOwned;
  final int? remainingSecs;
  final int timeoutSecs;
  final bool warningActive;

  const RhythmMotionTimer({
    required String roomId,
    required this.motionActive,
    required this.motionOwned,
    this.remainingSecs,
    required this.timeoutSecs,
    this.warningActive = false,
  }) : nodeId = roomId;

  const RhythmMotionTimer.node({
    required this.nodeId,
    required this.motionActive,
    required this.motionOwned,
    this.remainingSecs,
    required this.timeoutSecs,
    this.warningActive = false,
  });

  const RhythmMotionTimer.cleared(this.nodeId)
      : motionActive = false,
        motionOwned = false,
        remainingSecs = null,
        timeoutSecs = 0,
        warningActive = false;

  String get roomId => nodeId;

  bool get isCleared => timeoutSecs == 0 && !motionActive;
}
