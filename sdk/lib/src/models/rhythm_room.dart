import '../json_parsing.dart';
import 'rhythm_curve_config.dart'
    show RhythmCurveConfig, RhythmCurveShape, RhythmTimerSetting;
import 'rhythm_settings.dart' show RhythmLightScheduleOverride;

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
  motion,
  contact;

  static RhythmDeviceType fromString(String value) => switch (value) {
        'light' => RhythmDeviceType.light,
        'motion' => RhythmDeviceType.motion,
        'contact' => RhythmDeviceType.contact,
        _ => RhythmDeviceType.button,
      };

  static RhythmDeviceType? fromNodeKind(RhythmNodeKind kind) => switch (kind) {
        RhythmNodeKind.lightDevice => RhythmDeviceType.light,
        RhythmNodeKind.motionSensor => RhythmDeviceType.motion,
        RhythmNodeKind.sensor => RhythmDeviceType.contact,
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

class RhythmLightScheduleAssignment {
  final String kind;
  final String? scheduleId;
  final RhythmMode? activeMode;

  const RhythmLightScheduleAssignment.unscheduled({
    this.activeMode = RhythmMode.day,
  })  : kind = 'unscheduled',
        scheduleId = null;

  const RhythmLightScheduleAssignment.named({
    required this.scheduleId,
    required this.activeMode,
  }) : kind = 'named';

  bool get isUnscheduled => kind == 'unscheduled';

  factory RhythmLightScheduleAssignment.fromJson(Map<String, dynamic> json) {
    if (json['kind'] == 'named') {
      return RhythmLightScheduleAssignment.named(
        scheduleId: json['schedule_id'] as String? ?? '',
        activeMode: RhythmMode.fromString(json['active_mode'] as String?) ??
            RhythmMode.day,
      );
    }
    return RhythmLightScheduleAssignment.unscheduled(
      activeMode: RhythmMode.fromString(json['active_mode'] as String?) ??
          RhythmMode.day,
    );
  }

  Map<String, dynamic> toJson() => {
        'kind': kind,
        if (scheduleId != null) 'schedule_id': scheduleId,
        if (activeMode != null) 'active_mode': activeMode!.wireValue,
      };
}

class RhythmNodeProfileSettings {
  final String? profileId;
  final bool? moodEnabled;
  final String? moodProfileId;
  final String? moodSceneId;
  final RhythmTimerSetting? fadeSetting;
  final RhythmTimerSetting? motionTimeoutSetting;
  final bool? motionActivationEnabled;
  final RhythmLightScheduleAssignment? lightSchedule;
  final Map<String, RhythmLightScheduleOverride> lightScheduleOverrides;
  final RhythmRoomSchedule? roomSchedule;
  final Map<String, RhythmLightProfileNodeOverride> profileOverrides;
  final Map<String, dynamic> raw;

  const RhythmNodeProfileSettings({
    this.profileId,
    this.moodEnabled,
    this.moodProfileId,
    this.moodSceneId,
    this.fadeSetting,
    this.motionTimeoutSetting,
    this.motionActivationEnabled,
    this.lightSchedule,
    this.lightScheduleOverrides = const {},
    this.roomSchedule,
    this.profileOverrides = const {},
    this.raw = const <String, dynamic>{},
  });

  int? get fadeMs => fadeSetting?.fixedValue;
  int? get motionTimeoutSecs => motionTimeoutSetting?.fixedValue;
  bool get isMotionActivationEnabled => motionActivationEnabled ?? true;
  String? get lightScheduleId => lightSchedule?.scheduleId;
  RhythmMode? get lightScheduleMode => lightSchedule?.activeMode;
  bool get isExplicitlyUnscheduled => lightSchedule?.isUnscheduled ?? false;

  bool get isEmpty =>
      profileId == null &&
      moodEnabled == null &&
      moodProfileId == null &&
      moodSceneId == null &&
      fadeSetting == null &&
      motionTimeoutSetting == null &&
      motionActivationEnabled == null &&
      lightSchedule == null &&
      lightScheduleOverrides.isEmpty &&
      roomSchedule == null &&
      profileOverrides.isEmpty &&
      raw.isEmpty;

  factory RhythmNodeProfileSettings.fromJson(Map<String, dynamic> json) {
    final raw = Map<String, dynamic>.from(json)
      ..remove('profile_id')
      ..remove('mood_enabled')
      ..remove('mood_profile_id')
      ..remove('mood_scene_id')
      ..remove('active_light_scene_id')
      ..remove('idle_profile_id')
      ..remove('fade_ms')
      ..remove('motion_timeout_secs')
      ..remove('motion_activation_enabled')
      ..remove('light_schedule')
      ..remove('light_schedule_overrides')
      ..remove('room_schedule')
      ..remove('profile_overrides');
    final moodProfileId = json['mood_profile_id'] as String? ??
        json['idle_profile_id'] as String?;
    final moodSceneId = json['mood_scene_id'] as String? ??
        json['active_light_scene_id'] as String?;
    return RhythmNodeProfileSettings(
      profileId: json['profile_id'] as String?,
      moodEnabled: json['mood_enabled'] as bool?,
      moodProfileId:
          moodProfileId == null || moodProfileId.isEmpty ? null : moodProfileId,
      moodSceneId:
          moodSceneId == null || moodSceneId.isEmpty ? null : moodSceneId,
      fadeSetting: _timerSettingFromJson(json, 'fade_ms'),
      motionTimeoutSetting: _timerSettingFromJson(json, 'motion_timeout_secs'),
      motionActivationEnabled: json['motion_activation_enabled'] as bool?,
      lightSchedule: jsonMap(json['light_schedule']) == null
          ? null
          : RhythmLightScheduleAssignment.fromJson(
              jsonMap(json['light_schedule'])!,
            ),
      lightScheduleOverrides:
          _lightScheduleOverridesFromJson(json['light_schedule_overrides']),
      roomSchedule: jsonMap(json['room_schedule']) == null
          ? null
          : RhythmRoomSchedule.fromJson(jsonMap(json['room_schedule'])!),
      profileOverrides: _profileOverridesFromJson(json['profile_overrides']),
      raw: raw,
    );
  }

  Map<String, dynamic> toJson() => {
        ...raw,
        if (profileId != null) 'profile_id': profileId,
        if (moodEnabled != null) 'mood_enabled': moodEnabled,
        if (moodProfileId != null) 'mood_profile_id': moodProfileId,
        if (moodSceneId != null) 'mood_scene_id': moodSceneId,
        if (fadeSetting != null) 'fade_ms': fadeSetting!.toJson(),
        if (motionTimeoutSetting != null)
          'motion_timeout_secs': motionTimeoutSetting!.toJson(),
        if (motionActivationEnabled != null)
          'motion_activation_enabled': motionActivationEnabled,
        if (lightSchedule != null) 'light_schedule': lightSchedule!.toJson(),
        if (lightScheduleOverrides.isNotEmpty)
          'light_schedule_overrides': {
            for (final entry in lightScheduleOverrides.entries)
              entry.key: entry.value.toJson(),
          },
        if (roomSchedule != null) 'room_schedule': roomSchedule!.toJson(),
        if (profileOverrides.isNotEmpty)
          'profile_overrides': {
            for (final entry in profileOverrides.entries)
              entry.key: entry.value.toJson(),
          },
      };
}

Map<String, RhythmLightScheduleOverride> _lightScheduleOverridesFromJson(
  dynamic value,
) {
  if (value is! Map) return const {};
  return {
    for (final entry in value.entries)
      if (entry.key is String && entry.value is Map)
        entry.key as String: RhythmLightScheduleOverride.fromJson(
          (entry.value as Map).cast<String, dynamic>(),
        ),
  };
}

enum RhythmRoomScheduleSource {
  wakeSleepPresets('wake_sleep_presets'),
  followTime('follow_time');

  const RhythmRoomScheduleSource(this.wireValue);
  final String wireValue;

  static RhythmRoomScheduleSource fromWire(String? value) => values.firstWhere(
        (source) => source.wireValue == value,
        orElse: () => wakeSleepPresets,
      );
}

/// Appliance-authoritative local schedule for one stable room ID.
class RhythmRoomSchedule {
  final RhythmRoomScheduleSource source;
  final String wakeTime;
  final String sleepTime;

  const RhythmRoomSchedule({
    this.source = RhythmRoomScheduleSource.wakeSleepPresets,
    this.wakeTime = '06:30',
    this.sleepTime = '22:30',
  });

  factory RhythmRoomSchedule.fromJson(Map<String, dynamic> json) =>
      RhythmRoomSchedule(
        source: RhythmRoomScheduleSource.fromWire(json['source'] as String?),
        wakeTime: json['wake_time'] as String? ?? '06:30',
        sleepTime: json['sleep_time'] as String? ?? '22:30',
      );

  Map<String, dynamic> toJson() => {
        'source': source.wireValue,
        'wake_time': wakeTime,
        'sleep_time': sleepTime,
      };

  RhythmRoomSchedule copyWith({
    RhythmRoomScheduleSource? source,
    String? wakeTime,
    String? sleepTime,
  }) =>
      RhythmRoomSchedule(
        source: source ?? this.source,
        wakeTime: wakeTime ?? this.wakeTime,
        sleepTime: sleepTime ?? this.sleepTime,
      );
}

class RhythmLightProfileNodeOverride {
  final RhythmCurveShape? curve;
  final int? minColorTemp;
  final int? maxColorTemp;
  final int? minBrightness;
  final int? maxBrightness;
  final int? maxDimSteps;
  final RhythmTimerSetting? fadeSetting;
  final RhythmTimerSetting? motionTimeoutSetting;
  final RhythmTimerSetting? rhythmIntervalSetting;
  final Map<String, dynamic> raw;

  const RhythmLightProfileNodeOverride({
    this.curve,
    this.minColorTemp,
    this.maxColorTemp,
    this.minBrightness,
    this.maxBrightness,
    this.maxDimSteps,
    this.fadeSetting,
    this.motionTimeoutSetting,
    this.rhythmIntervalSetting,
    this.raw = const <String, dynamic>{},
  });

  int? get fadeMs => fadeSetting?.fixedValue;
  int? get motionTimeoutSecs => motionTimeoutSetting?.fixedValue;
  int? get rhythmIntervalSecs => rhythmIntervalSetting?.fixedValue;

  bool get isEmpty =>
      curve == null &&
      minColorTemp == null &&
      maxColorTemp == null &&
      minBrightness == null &&
      maxBrightness == null &&
      maxDimSteps == null &&
      fadeSetting == null &&
      motionTimeoutSetting == null &&
      rhythmIntervalSetting == null &&
      raw.isEmpty;

  bool get hasVisualOverrides =>
      curve != null ||
      minColorTemp != null ||
      maxColorTemp != null ||
      minBrightness != null ||
      maxBrightness != null ||
      maxDimSteps != null ||
      rhythmIntervalSetting != null;

  List<String> get changedFields => [
        if (curve != null) 'curve',
        if (minColorTemp != null) 'min_color_temp',
        if (maxColorTemp != null) 'max_color_temp',
        if (minBrightness != null) 'min_brightness',
        if (maxBrightness != null) 'max_brightness',
        if (maxDimSteps != null) 'max_dim_steps',
        if (fadeSetting != null) 'fade_ms',
        if (motionTimeoutSetting != null) 'motion_timeout_secs',
        if (rhythmIntervalSetting != null) 'rhythm_interval_secs',
      ];

  factory RhythmLightProfileNodeOverride.fromJson(Map<String, dynamic> json) {
    final curveJson = jsonMap(json['curve']);
    final raw = Map<String, dynamic>.from(json)
      ..remove('curve')
      ..remove('min_color_temp')
      ..remove('max_color_temp')
      ..remove('min_brightness')
      ..remove('max_brightness')
      ..remove('max_dim_steps')
      ..remove('fade_ms')
      ..remove('motion_timeout_secs')
      ..remove('rhythm_interval_secs');
    return RhythmLightProfileNodeOverride(
      curve: curveJson == null ? null : RhythmCurveShape.fromJson(curveJson),
      minColorTemp: jsonInt(
        json['min_color_temp'],
        preferredKeys: const ['min_color_temp'],
      ),
      maxColorTemp: jsonInt(
        json['max_color_temp'],
        preferredKeys: const ['max_color_temp'],
      ),
      minBrightness: jsonInt(
        json['min_brightness'],
        preferredKeys: const ['min_brightness'],
      ),
      maxBrightness: jsonInt(
        json['max_brightness'],
        preferredKeys: const ['max_brightness'],
      ),
      maxDimSteps: jsonInt(
        json['max_dim_steps'],
        preferredKeys: const ['max_dim_steps'],
      ),
      fadeSetting: _timerSettingFromJson(json, 'fade_ms'),
      motionTimeoutSetting: _timerSettingFromJson(json, 'motion_timeout_secs'),
      rhythmIntervalSetting: _timerSettingFromJson(
        json,
        'rhythm_interval_secs',
      ),
      raw: raw,
    );
  }

  Map<String, dynamic> toJson() => {
        ...raw,
        if (curve != null) 'curve': curve!.toJson(),
        if (minColorTemp != null) 'min_color_temp': minColorTemp,
        if (maxColorTemp != null) 'max_color_temp': maxColorTemp,
        if (minBrightness != null) 'min_brightness': minBrightness,
        if (maxBrightness != null) 'max_brightness': maxBrightness,
        if (maxDimSteps != null) 'max_dim_steps': maxDimSteps,
        if (fadeSetting != null) 'fade_ms': fadeSetting!.toJson(),
        if (motionTimeoutSetting != null)
          'motion_timeout_secs': motionTimeoutSetting!.toJson(),
        if (rhythmIntervalSetting != null)
          'rhythm_interval_secs': rhythmIntervalSetting!.toJson(),
      };

  RhythmCurveConfig applyTo(RhythmCurveConfig base) {
    return base.copyWith(
      curve: curve ?? base.curve,
      minColorTemp: minColorTemp ?? base.minColorTemp,
      maxColorTemp: maxColorTemp ?? base.maxColorTemp,
      minBrightness: minBrightness ?? base.minBrightness,
      maxBrightness: maxBrightness ?? base.maxBrightness,
      maxDimSteps: maxDimSteps ?? base.maxDimSteps,
      fadeSetting: fadeSetting ?? base.fadeSetting,
      motionTimeoutSetting: motionTimeoutSetting ?? base.motionTimeoutSetting,
      rhythmIntervalSetting:
          rhythmIntervalSetting ?? base.rhythmIntervalSetting,
    );
  }

  factory RhythmLightProfileNodeOverride.between(
    RhythmCurveConfig global,
    RhythmCurveConfig effective, {
    Map<String, dynamic> raw = const <String, dynamic>{},
  }) {
    return RhythmLightProfileNodeOverride(
      curve: effective.curve == global.curve ? null : effective.curve,
      minColorTemp: effective.minColorTemp == global.minColorTemp
          ? null
          : effective.minColorTemp,
      maxColorTemp: effective.maxColorTemp == global.maxColorTemp
          ? null
          : effective.maxColorTemp,
      minBrightness: effective.minBrightness == global.minBrightness
          ? null
          : effective.minBrightness,
      maxBrightness: effective.maxBrightness == global.maxBrightness
          ? null
          : effective.maxBrightness,
      maxDimSteps: effective.maxDimSteps == global.maxDimSteps
          ? null
          : effective.maxDimSteps,
      fadeSetting: effective.fadeSetting == global.fadeSetting
          ? null
          : effective.fadeSetting,
      motionTimeoutSetting:
          effective.motionTimeoutSetting == global.motionTimeoutSetting
              ? null
              : effective.motionTimeoutSetting,
      rhythmIntervalSetting:
          effective.rhythmIntervalSetting == global.rhythmIntervalSetting
              ? null
              : effective.rhythmIntervalSetting,
      raw: raw,
    );
  }
}

Map<String, RhythmLightProfileNodeOverride> _profileOverridesFromJson(
  Object? value,
) {
  final map = value is Map<String, dynamic>
      ? value
      : value is Map
          ? value.cast<String, dynamic>()
          : null;
  if (map == null) return const {};

  final overrides = <String, RhythmLightProfileNodeOverride>{};
  for (final entry in map.entries) {
    final profileId = entry.key.trim();
    if (profileId.isEmpty) continue;
    final overrideValue = entry.value;
    final overrideMap = overrideValue is Map<String, dynamic>
        ? overrideValue
        : overrideValue is Map
            ? overrideValue.cast<String, dynamic>()
            : null;
    if (overrideMap == null) continue;
    overrides[profileId] = RhythmLightProfileNodeOverride.fromJson(overrideMap);
  }
  return Map.unmodifiable(overrides);
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

  const RhythmObservedPower({this.lightsOn, this.fresh, this.source});

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

/// Color-temperature range the server has proven safe for a light node.
class RhythmColorTemperatureCapabilities {
  final int minKelvin;
  final int maxKelvin;

  const RhythmColorTemperatureCapabilities({
    required this.minKelvin,
    required this.maxKelvin,
  });

  bool supports(int kelvin) => kelvin >= minKelvin && kelvin <= maxKelvin;

  int clamp(int kelvin) => kelvin.clamp(minKelvin, maxKelvin).toInt();

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is RhythmColorTemperatureCapabilities &&
          minKelvin == other.minKelvin &&
          maxKelvin == other.maxKelvin;

  @override
  int get hashCode => Object.hash(minKelvin, maxKelvin);

  static RhythmColorTemperatureCapabilities? maybeFromJson(Object? value) {
    final json = jsonMap(value);
    if (json == null) return null;

    final minKelvin = jsonInt(
      json['min_kelvin'],
      preferredKeys: const ['min_kelvin'],
    );
    final maxKelvin = jsonInt(
      json['max_kelvin'],
      preferredKeys: const ['max_kelvin'],
    );
    if (minKelvin == null ||
        maxKelvin == null ||
        minKelvin <= 0 ||
        minKelvin > maxKelvin) {
      return null;
    }
    return RhythmColorTemperatureCapabilities(
      minKelvin: minKelvin,
      maxKelvin: maxKelvin,
    );
  }
}

/// Extensible, normalized light-control capabilities for a node.
class RhythmLightCapabilities {
  final RhythmColorTemperatureCapabilities? colorTemperature;
  final bool? individualProfileOverrides;

  const RhythmLightCapabilities({
    this.colorTemperature,
    this.individualProfileOverrides,
  });

  bool get supportsColorTemperature => colorTemperature != null;

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is RhythmLightCapabilities &&
          colorTemperature == other.colorTemperature &&
          individualProfileOverrides == other.individualProfileOverrides;

  @override
  int get hashCode => Object.hash(colorTemperature, individualProfileOverrides);

  static RhythmLightCapabilities? maybeFromJson(Object? value) {
    final json = jsonMap(value);
    if (json == null) return null;
    return RhythmLightCapabilities(
      colorTemperature: RhythmColorTemperatureCapabilities.maybeFromJson(
        json['color_temperature'],
      ),
      individualProfileOverrides: json['individual_profile_overrides'] as bool?,
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
  final bool pendingDispatch;
  final bool rhythmEnabled;
  final bool disabled;
  final double timeOffset;
  final double brightnessOffset;
  final List<String> hubTypes;
  final String? manufacturer;
  final String? model;
  final RhythmLightCapabilities? lightCapabilities;
  final List<String> deviceIds;
  final List<RhythmDevice> devices;
  final RhythmNodeProfileSettings? profileSettings;

  /// Node-local settings before parent inheritance is applied.
  final RhythmNodeProfileSettings? localProfileSettings;
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
    bool? pendingDispatch,
    required this.rhythmEnabled,
    required this.disabled,
    required this.timeOffset,
    required this.brightnessOffset,
    this.hubTypes = const [],
    this.manufacturer,
    this.model,
    this.lightCapabilities,
    this.deviceIds = const [],
    this.devices = const [],
    RhythmNodeProfileSettings? profileSettings,
    RhythmNodeProfileSettings? roomProfile,
    this.localProfileSettings,
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
        pendingDispatch = pendingDispatch ?? false,
        profileSettings = profileSettings ?? roomProfile;

  bool get hasMotionSensor =>
      kind == RhythmNodeKind.motionSensor ||
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

  List<RhythmDevice> get contactSensors =>
      devices.where((d) => d.type == RhythmDeviceType.contact).toList();

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
    final c = contactSensors.length;
    if (l > 0) parts.add('$l light${l > 1 ? 's' : ''}');
    if (b > 0) parts.add('$b button${b > 1 ? 's' : ''}');
    if (m > 0) parts.add('$m sensor${m > 1 ? 's' : ''}');
    if (c > 0) parts.add('$c contact sensor${c > 1 ? 's' : ''}');
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
    final localProfileSettings = json['room_profile'] is Map<String, dynamic>
        ? RhythmNodeProfileSettings.fromJson(
            json['room_profile'] as Map<String, dynamic>,
          )
        : profileSettings;
    final state = RoomModeState.fromJson(json);
    return RhythmRoom(
      id: json['id'] as String? ?? json['node_id'] as String? ?? '',
      name: json['name'] as String? ?? '',
      kind: RhythmNodeKind.fromString(json['kind'] as String?),
      parentId: json['parent_id'] as String?,
      placement: RhythmNodePlacement.fromString(json['placement'] as String?),
      groupedLightId: json['grouped_light_id'] as String? ?? '',
      state: state,
      transitioning: jsonBool(json['transitioning']),
      pendingDispatch: jsonBool(json['pending_dispatch']),
      rhythmEnabled: json['rhythm_enabled'] as bool? ?? false,
      disabled: json['disabled'] as bool? ?? false,
      timeOffset: jsonDouble(
            json['time_offset'],
            preferredKeys: const ['time_offset'],
          ) ??
          0.0,
      brightnessOffset: jsonDouble(
            json['brightness_offset'],
            preferredKeys: const ['brightness_offset'],
          ) ??
          0.0,
      hubTypes: hubTypes,
      manufacturer: json['manufacturer'] as String?,
      model: json['model'] as String?,
      lightCapabilities: RhythmLightCapabilities.maybeFromJson(
        json['light_capabilities'],
      ),
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
      localProfileSettings: localProfileSettings,
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
  final bool pendingDispatch;
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
  final RhythmLightCapabilities? lightCapabilities;
  final RhythmObservedPower? observedPower;
  final bool? lightsOn;
  final int? brightness;
  final int? kelvin;
  final RhythmRoomColor? color;
  final RhythmNodeProfileSettings? profileSettings;

  /// Node-local settings before parent inheritance is applied.
  final RhythmNodeProfileSettings? localProfileSettings;
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
    bool? pendingDispatch,
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
    this.lightCapabilities,
    this.observedPower,
    this.lightsOn,
    this.brightness,
    this.kelvin,
    this.color,
    RhythmNodeProfileSettings? profileSettings,
    RhythmNodeProfileSettings? roomProfile,
    this.localProfileSettings,
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
        pendingDispatch = pendingDispatch ?? false,
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
    final localProfileSettings = json['room_profile'] is Map<String, dynamic>
        ? RhythmNodeProfileSettings.fromJson(
            json['room_profile'] as Map<String, dynamic>,
          )
        : profileSettings;
    final state = RoomModeState.fromJson(json);
    return RhythmRoomState(
      nodeId: json['node_id'] as String? ??
          json['room_id'] as String? ??
          json['id'] as String? ??
          '',
      mode: RhythmMode.fromString(json['mode'] as String?),
      state: state,
      transitioning: jsonBool(json['transitioning']),
      pendingDispatch: jsonBool(json['pending_dispatch']),
      rhythmEnabled: json['rhythm_enabled'] as bool? ?? false,
      timeOffset: jsonDouble(
            json['time_offset'],
            preferredKeys: const ['time_offset'],
          ) ??
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
      lightCapabilities: RhythmLightCapabilities.maybeFromJson(
        json['light_capabilities'],
      ),
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
      localProfileSettings: localProfileSettings,
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
