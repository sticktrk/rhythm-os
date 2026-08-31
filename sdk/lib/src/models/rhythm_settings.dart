import 'rhythm_curve_config.dart';
import 'rhythm_room.dart' show RhythmMode;

/// Human-readable solar anchor label for a schedule boundary.
///
/// Twilight anchors resolve on the morning side for Day boundaries and on the
/// evening side for Sleep boundaries, so the label must say dawn or dusk
/// instead of the ambiguous "twilight".
String lightScheduleSolarEventLabel(String event, RhythmMode targetMode) {
  final phase = targetMode == RhythmMode.day ? 'dawn' : 'dusk';
  return switch (event) {
    'sunrise' => 'Sunrise',
    'sunset' => 'Sunset',
    'civil_twilight' => 'Civil $phase',
    'nautical_twilight' => 'Nautical $phase',
    'astronomical_twilight' => 'Astronomical $phase',
    _ => event.replaceAll('_', ' '),
  };
}

const rhythmAdaptiveLightRuntimeId = 'rhythm-adaptive';

enum RhythmLightRuntime {
  rhythmAdaptive;

  String get id => switch (this) {
        RhythmLightRuntime.rhythmAdaptive => rhythmAdaptiveLightRuntimeId,
      };

  static RhythmLightRuntime fromId(String? value) => switch (value) {
        rhythmAdaptiveLightRuntimeId ||
        'rhythm' ||
        'rhythm_adaptive' =>
          RhythmLightRuntime.rhythmAdaptive,
        _ => RhythmLightRuntime.rhythmAdaptive,
      };
}

extension RhythmLightRuntimePresentation on RhythmLightRuntime {
  String get defaultDayProfileId => switch (this) {
        RhythmLightRuntime.rhythmAdaptive => 'rhythm',
      };
}

RhythmLightRuntime _lightRuntimeFromLegacyProfileId(String? _) {
  return RhythmLightRuntime.rhythmAdaptive;
}

class RhythmLightRuntimeState {
  final RhythmLightRuntime runtime;
  final List<RhythmLightRuntime> availableRuntimes;
  final RhythmLightRuntimeInitialApply? initialApply;

  const RhythmLightRuntimeState({
    required this.runtime,
    this.availableRuntimes = const [],
    this.initialApply,
  });

  factory RhythmLightRuntimeState.fromJson(Map<String, dynamic> json) {
    final runtimeId =
        json['runtime_id'] as String? ?? json['light_runtime'] as String?;
    return RhythmLightRuntimeState(
      runtime: RhythmLightRuntime.fromId(runtimeId),
      availableRuntimes: ((json['available_runtime_ids'] as List<dynamic>?) ??
              const <dynamic>[])
          .map((value) => RhythmLightRuntime.fromId(value as String?))
          .toSet()
          .toList(growable: false),
      initialApply: json['initial_apply'] is Map
          ? RhythmLightRuntimeInitialApply.fromJson(
              Map<String, dynamic>.from(json['initial_apply'] as Map),
            )
          : null,
    );
  }

  String get runtimeId => runtime.id;

  Map<String, dynamic> toJson() => {
        'runtime_id': runtime.id,
        if (availableRuntimes.isNotEmpty)
          'available_runtime_ids':
              availableRuntimes.map((runtime) => runtime.id).toList(),
        if (initialApply != null) 'initial_apply': initialApply!.toJson(),
      };
}

class RhythmLightRuntimeInitialApply {
  final bool queued;
  final int dispatchCount;
  final int dispatchSpacingMs;
  final int estimatedDispatchMs;
  final String? error;

  const RhythmLightRuntimeInitialApply({
    required this.queued,
    required this.dispatchCount,
    required this.dispatchSpacingMs,
    required this.estimatedDispatchMs,
    this.error,
  });

  factory RhythmLightRuntimeInitialApply.fromJson(Map<String, dynamic> json) {
    return RhythmLightRuntimeInitialApply(
      queued: json['queued'] as bool? ?? false,
      dispatchCount: (json['dispatch_count'] as num?)?.toInt() ?? 0,
      dispatchSpacingMs: (json['dispatch_spacing_ms'] as num?)?.toInt() ?? 0,
      estimatedDispatchMs:
          (json['estimated_dispatch_ms'] as num?)?.toInt() ?? 0,
      error: json['error'] as String?,
    );
  }

  Duration get estimatedDuration =>
      Duration(milliseconds: estimatedDispatchMs.clamp(0, 60000).toInt());

  Map<String, dynamic> toJson() => {
        'queued': queued,
        'dispatch_count': dispatchCount,
        'dispatch_spacing_ms': dispatchSpacingMs,
        'estimated_dispatch_ms': estimatedDispatchMs,
        if (error != null) 'error': error,
      };
}

class RoomDefault {
  final String roomId;
  final String state; // "active", "idle", "hard_off"

  const RoomDefault({required this.roomId, required this.state});

  factory RoomDefault.fromJson(Map<String, dynamic> json) {
    return RoomDefault(
      roomId: json['room_id'] as String? ?? '',
      state: json['state'] as String? ?? 'active',
    );
  }

  Map<String, dynamic> toJson() => {'room_id': roomId, 'state': state};
}

class RhythmModeConfig {
  final RhythmMode mode;
  final String activeProfileId;
  final String? idleProfileId;
  final String? wakeProfileId;
  final String? warningProfileId;
  final List<RoomDefault> roomDefaults;

  const RhythmModeConfig({
    required this.mode,
    required this.activeProfileId,
    this.idleProfileId,
    this.wakeProfileId,
    this.warningProfileId,
    this.roomDefaults = const [],
  });

  factory RhythmModeConfig.fromJson(Map<String, dynamic> json) {
    final idleProfileId = json['idle_profile_id'] as String?;
    return RhythmModeConfig(
      mode: RhythmMode.fromString(json['mode'] as String?) ?? RhythmMode.day,
      activeProfileId: json['active_profile_id'] as String? ?? '',
      idleProfileId:
          idleProfileId == null || idleProfileId.isEmpty ? null : idleProfileId,
      wakeProfileId: json['wake_profile_id'] as String?,
      warningProfileId: json['warning_profile_id'] as String?,
      roomDefaults:
          ((json['room_defaults'] as List<dynamic>?) ?? const <dynamic>[])
              .map((e) => RoomDefault.fromJson(e as Map<String, dynamic>))
              .toList(),
    );
  }

  RhythmModeConfig copyWith({
    RhythmMode? mode,
    String? activeProfileId,
    Object? idleProfileId = _sentinel,
    Object? wakeProfileId = _sentinel,
    Object? warningProfileId = _sentinel,
    List<RoomDefault>? roomDefaults,
  }) {
    return RhythmModeConfig(
      mode: mode ?? this.mode,
      activeProfileId: activeProfileId ?? this.activeProfileId,
      idleProfileId: identical(idleProfileId, _sentinel)
          ? this.idleProfileId
          : idleProfileId as String?,
      wakeProfileId: identical(wakeProfileId, _sentinel)
          ? this.wakeProfileId
          : wakeProfileId as String?,
      warningProfileId: identical(warningProfileId, _sentinel)
          ? this.warningProfileId
          : warningProfileId as String?,
      roomDefaults: roomDefaults ?? this.roomDefaults,
    );
  }

  Map<String, dynamic> toJson() {
    return {
      'mode': mode.wireValue,
      'active_profile_id': activeProfileId,
      'idle_profile_id': idleProfileId,
      'wake_profile_id': wakeProfileId,
      'warning_profile_id': warningProfileId,
      'room_defaults': roomDefaults.map((e) => e.toJson()).toList(),
    };
  }
}

const Object _sentinel = Object();
const int maxSolarScheduleOffsetMinutes = 720;

// ---------------------------------------------------------------------------
// Transition duration — fixed milliseconds or server-resolved auto
// ---------------------------------------------------------------------------

sealed class TransitionDuration {
  const TransitionDuration();

  const factory TransitionDuration.auto() = TransitionDurationAuto;
  const factory TransitionDuration.fixed(int ms) = TransitionDurationFixed;

  factory TransitionDuration.fromJson(dynamic json) {
    if (json is num) return TransitionDurationFixed(json.toInt());
    if (json is Map && json['mode'] == 'auto') {
      return const TransitionDurationAuto();
    }
    if (json is Map && json['mode'] == 'fixed') {
      final value = json['value'];
      if (value is num) return TransitionDurationFixed(value.toInt());
    }
    return const TransitionDurationAuto();
  }

  dynamic toJson();
  bool get isAuto;

  /// Milliseconds for fixed durations, 0 for auto.
  int get ms;
}

class TransitionDurationFixed extends TransitionDuration {
  final int milliseconds;
  const TransitionDurationFixed(this.milliseconds);

  @override
  dynamic toJson() => {'mode': 'fixed', 'value': milliseconds};
  @override
  bool get isAuto => false;
  @override
  int get ms => milliseconds;

  @override
  bool operator ==(Object other) =>
      other is TransitionDurationFixed && other.milliseconds == milliseconds;
  @override
  int get hashCode => milliseconds.hashCode;
}

class TransitionDurationAuto extends TransitionDuration {
  const TransitionDurationAuto();

  @override
  dynamic toJson() => {'mode': 'auto'};
  @override
  bool get isAuto => true;
  @override
  int get ms => 0;

  @override
  bool operator ==(Object other) => other is TransitionDurationAuto;
  @override
  int get hashCode => 0;
}

// ---------------------------------------------------------------------------
// Mode transition config
// ---------------------------------------------------------------------------

class RhythmModeTransitionConfig {
  final String id;
  final String label;
  final RhythmMode fromMode;
  final RhythmMode toMode;
  final RhythmTransitionTrigger trigger;
  final bool triggerEnabled;
  final TransitionDuration duration;

  const RhythmModeTransitionConfig({
    this.id = '',
    this.label = '',
    required this.fromMode,
    required this.toMode,
    this.trigger = const RhythmTransitionTrigger.manual(),
    this.triggerEnabled = true,
    required this.duration,
  });

  /// Convenience getter for backward-compatible int access.
  int get durationMs => duration.ms;

  factory RhythmModeTransitionConfig.fromJson(Map<String, dynamic> json) {
    final rawTrigger = json['trigger'];
    return RhythmModeTransitionConfig(
      id: json['id'] as String? ?? '',
      label: json['label'] as String? ?? '',
      fromMode:
          RhythmMode.fromString(json['from_mode'] as String?) ?? RhythmMode.day,
      toMode:
          RhythmMode.fromString(json['to_mode'] as String?) ?? RhythmMode.day,
      trigger: rawTrigger is Map
          ? RhythmTransitionTrigger.fromJson(rawTrigger.cast<String, dynamic>())
          : rawTrigger is String
              ? RhythmTransitionTrigger.fromLegacyValue(rawTrigger)
              : const RhythmTransitionTrigger.manual(),
      triggerEnabled: json['trigger_enabled'] as bool? ?? true,
      duration: TransitionDuration.fromJson(json['duration_ms']),
    );
  }

  Map<String, dynamic> toJson() {
    return {
      'id': id,
      'label': label,
      'from_mode': fromMode.wireValue,
      'to_mode': toMode.wireValue,
      'trigger': trigger.toJson(),
      'trigger_enabled': triggerEnabled,
      'duration_ms': duration.toJson(),
    };
  }

  RhythmModeTransitionConfig copyWith({
    String? id,
    String? label,
    RhythmMode? fromMode,
    RhythmMode? toMode,
    RhythmTransitionTrigger? trigger,
    bool? triggerEnabled,
    TransitionDuration? duration,
  }) {
    return RhythmModeTransitionConfig(
      id: id ?? this.id,
      label: label ?? this.label,
      fromMode: fromMode ?? this.fromMode,
      toMode: toMode ?? this.toMode,
      trigger: trigger ?? this.trigger,
      triggerEnabled: triggerEnabled ?? this.triggerEnabled,
      duration: duration ?? this.duration,
    );
  }
}

/// Reusable named automation lane for rooms and unassigned lights.
class RhythmLightScheduleConfig {
  final String id;
  final String name;
  final bool enabled;
  final RhythmMode activeMode;
  final List<RhythmModeTransitionConfig> transitions;
  /// Appliance-resolved local times for today's base transitions.
  /// Response metadata; intentionally omitted from writes.
  final Map<String, String> resolvedTransitions;
  /// Appliance-resolved effective local times keyed by node ID.
  final Map<String, Map<String, String>> resolvedTransitionsByNode;

  const RhythmLightScheduleConfig({
    required this.id,
    required this.name,
    this.enabled = true,
    this.activeMode = RhythmMode.day,
    this.transitions = const [],
    this.resolvedTransitions = const {},
    this.resolvedTransitionsByNode = const {},
  });

  factory RhythmLightScheduleConfig.fromJson(Map<String, dynamic> json) =>
      RhythmLightScheduleConfig(
        id: json['id'] as String? ?? '',
        name: json['name'] as String? ?? '',
        enabled: json['enabled'] as bool? ?? true,
        activeMode: RhythmMode.fromString(json['active_mode'] as String?) ??
            RhythmMode.day,
        transitions: ((json['transitions'] as List<dynamic>?) ?? const [])
            .whereType<Map>()
            .map(
              (value) => RhythmModeTransitionConfig.fromJson(
                value.cast<String, dynamic>(),
              ),
            )
            .toList(growable: false),
        resolvedTransitions: _lightScheduleResolvedTimes(
          json['resolved_transitions'],
        ),
        resolvedTransitionsByNode: _lightScheduleResolvedTimesByNode(
          json['resolved_transitions_by_node'],
        ),
      );

  Map<String, dynamic> toJson() => {
        'id': id,
        'name': name,
        'enabled': enabled,
        'active_mode': activeMode.wireValue,
        'transitions': transitions.map((value) => value.toJson()).toList(),
      };
}

Map<String, String> _lightScheduleResolvedTimes(Object? value) {
  if (value is! Map) return const {};
  return Map<String, String>.unmodifiable({
    for (final entry in value.entries)
      if (entry.key is String && entry.value is String)
        entry.key as String: entry.value as String,
  });
}

Map<String, Map<String, String>> _lightScheduleResolvedTimesByNode(
  Object? value,
) {
  if (value is! Map) return const {};
  return Map<String, Map<String, String>>.unmodifiable({
    for (final entry in value.entries)
      if (entry.key is String && entry.value is Map)
        entry.key as String: _lightScheduleResolvedTimes(entry.value),
  });
}

class RhythmTransitionTriggerOverride {
  final String? kind;
  final String? event;
  final int? offsetMinutes;
  final String? time;

  const RhythmTransitionTriggerOverride({
    this.kind,
    this.event,
    this.offsetMinutes,
    this.time,
  });

  factory RhythmTransitionTriggerOverride.fromJson(
    Map<String, dynamic> json,
  ) {
    final offset = json['offset_minutes'];
    if (offset != null && offset is! int) {
      throw const FormatException('offset_minutes must be a whole integer');
    }
    if (offset is int &&
        (offset < -maxSolarScheduleOffsetMinutes ||
            offset > maxSolarScheduleOffsetMinutes)) {
      throw const FormatException(
        'offset_minutes must be between -720 and 720',
      );
    }
    return RhythmTransitionTriggerOverride(
      kind: json['kind'] as String?,
      event: json['event'] as String?,
      offsetMinutes: offset as int?,
      time: json['time'] as String?,
    );
  }

  bool get isEmpty =>
      kind == null && event == null && offsetMinutes == null && time == null;

  Map<String, dynamic> toJson() => {
        if (kind != null) 'kind': kind,
        if (event != null) 'event': event,
        if (offsetMinutes != null) 'offset_minutes': offsetMinutes,
        if (time != null) 'time': time,
      };
}

class RhythmModeTransitionOverride {
  final RhythmTransitionTriggerOverride trigger;
  final bool? triggerEnabled;
  final TransitionDuration? duration;

  const RhythmModeTransitionOverride({
    this.trigger = const RhythmTransitionTriggerOverride(),
    this.triggerEnabled,
    this.duration,
  });

  factory RhythmModeTransitionOverride.fromJson(Map<String, dynamic> json) {
    final trigger = json['trigger'];
    return RhythmModeTransitionOverride(
      trigger: trigger is Map
          ? RhythmTransitionTriggerOverride.fromJson(
              trigger.cast<String, dynamic>(),
            )
          : const RhythmTransitionTriggerOverride(),
      triggerEnabled: json['trigger_enabled'] as bool?,
      duration: json.containsKey('duration_ms')
          ? TransitionDuration.fromJson(json['duration_ms'])
          : null,
    );
  }

  bool get isEmpty =>
      trigger.isEmpty &&
      triggerEnabled == null &&
      duration == null;

  Map<String, dynamic> toJson() => {
        if (!trigger.isEmpty) 'trigger': trigger.toJson(),
        if (triggerEnabled != null) 'trigger_enabled': triggerEnabled,
        if (duration != null) 'duration_ms': duration!.toJson(),
      };
}

class RhythmLightScheduleOverride {
  final Map<String, RhythmModeTransitionOverride> transitions;

  const RhythmLightScheduleOverride({this.transitions = const {}});

  factory RhythmLightScheduleOverride.fromJson(Map<String, dynamic> json) {
    final transitions = json['transitions'];
    if (transitions is! Map) {
      return const RhythmLightScheduleOverride();
    }
    return RhythmLightScheduleOverride(
      transitions: {
        for (final entry in transitions.entries)
          if (entry.key is String && entry.value is Map)
            entry.key as String: RhythmModeTransitionOverride.fromJson(
              (entry.value as Map).cast<String, dynamic>(),
            ),
      },
    );
  }

  bool get isEmpty => transitions.isEmpty;

  Map<String, dynamic> toJson() => {
        'transitions': {
          for (final entry in transitions.entries)
            entry.key: entry.value.toJson(),
        },
      };
}

class RhythmTransitionTrigger {
  final String kind;
  final String? event;
  final String? time;
  final int offsetMinutes;

  const RhythmTransitionTrigger._({
    required this.kind,
    this.event,
    this.time,
    this.offsetMinutes = 0,
  });

  const RhythmTransitionTrigger.manual() : this._(kind: 'manual');

  const RhythmTransitionTrigger.solar(
    String event, {
    int offsetMinutes = 0,
  }) : this._(
          kind: 'solar',
          event: event,
          offsetMinutes: offsetMinutes,
        );

  const RhythmTransitionTrigger.scheduled(String time)
      : this._(kind: 'scheduled', time: time);

  factory RhythmTransitionTrigger.fromJson(Map<String, dynamic> json) {
    final kind = json['kind'] as String? ?? 'manual';
    final event = json['event'] as String?;
    final time = json['time'] as String?;
    final rawOffset = json['offset_minutes'];
    if (rawOffset != null && rawOffset is! int) {
      throw const FormatException('offset_minutes must be a whole integer');
    }
    final offset = rawOffset as int? ?? 0;
    if (offset < -maxSolarScheduleOffsetMinutes ||
        offset > maxSolarScheduleOffsetMinutes) {
      throw const FormatException(
        'offset_minutes must be between -720 and 720',
      );
    }
    return switch (kind) {
      'solar' when event != null && event.isNotEmpty =>
        RhythmTransitionTrigger.solar(event, offsetMinutes: offset),
      'solar' => const RhythmTransitionTrigger._(kind: 'solar'),
      'manual' || 'scheduled' when offset != 0 => throw const FormatException(
          'offset_minutes is valid only for solar triggers',
        ),
      'scheduled' when time != null && time.isNotEmpty =>
        RhythmTransitionTrigger.scheduled(time),
      'scheduled' => const RhythmTransitionTrigger._(kind: 'scheduled'),
      _ => const RhythmTransitionTrigger.manual(),
    };
  }

  factory RhythmTransitionTrigger.fromLegacyValue(String value) {
    if (value.isEmpty || value == 'manual') {
      return const RhythmTransitionTrigger.manual();
    }
    return RhythmTransitionTrigger.solar(value);
  }

  bool get isManual => kind == 'manual';
  bool get isSolar => kind == 'solar';
  bool get isScheduled => kind == 'scheduled';

  Map<String, dynamic> toJson() {
    return {
      'kind': kind,
      if (isSolar && event != null && event!.isNotEmpty) 'event': event,
      if (isSolar && offsetMinutes != 0) 'offset_minutes': offsetMinutes,
      if (isScheduled && time != null && time!.isNotEmpty) 'time': time,
    };
  }
}

class RhythmModeLastChange {
  final String cause;
  final String? transitionId;
  final int epochMs;

  const RhythmModeLastChange({
    required this.cause,
    this.transitionId,
    required this.epochMs,
  });

  factory RhythmModeLastChange.fromJson(Map<String, dynamic> json) {
    return RhythmModeLastChange(
      cause: json['cause'] as String? ?? '',
      transitionId: json['transition_id'] as String?,
      epochMs: (json['epoch_ms'] as num?)?.toInt() ??
          (json['utc_ms'] as num?)?.toInt() ??
          0,
    );
  }

  Map<String, dynamic> toJson() {
    return {
      'cause': cause,
      if (transitionId != null) 'transition_id': transitionId,
      'epoch_ms': epochMs,
    };
  }
}

class RhythmModeResource {
  final RhythmMode active;
  final RhythmLightRuntime lightRuntime;
  final bool hasLightRuntime;
  final RhythmModeLastChange? lastChange;
  final List<RhythmModeConfig> configs;

  const RhythmModeResource({
    required this.active,
    this.lightRuntime = RhythmLightRuntime.rhythmAdaptive,
    this.hasLightRuntime = false,
    this.lastChange,
    this.configs = const [],
  });

  factory RhythmModeResource.fromJson(Map<String, dynamic> json) {
    final active =
        RhythmMode.fromString(json['active'] as String?) ?? RhythmMode.day;
    final lastChangeJson = json['last_change'];
    final hasFlatLastChange = json.containsKey('cause') ||
        json.containsKey('transition_id') ||
        json.containsKey('epoch_ms') ||
        json.containsKey('utc_ms');
    final configs = ((json['configs'] as List<dynamic>?) ?? const <dynamic>[])
        .map((e) => RhythmModeConfig.fromJson(e as Map<String, dynamic>))
        .toList();
    RhythmModeConfig? activeConfig;
    for (final config in configs) {
      if (config.mode == active) {
        activeConfig = config;
        break;
      }
    }
    final runtimeId =
        json['light_runtime'] as String? ?? json['runtime_id'] as String?;
    final lightRuntime = runtimeId == null
        ? _lightRuntimeFromLegacyProfileId(activeConfig?.activeProfileId)
        : RhythmLightRuntime.fromId(runtimeId);
    return RhythmModeResource(
      active: active,
      lightRuntime: lightRuntime,
      hasLightRuntime: runtimeId != null,
      lastChange: lastChangeJson is Map<String, dynamic>
          ? RhythmModeLastChange.fromJson(lastChangeJson)
          : hasFlatLastChange
              ? RhythmModeLastChange.fromJson(json)
              : null,
      configs: configs,
    );
  }

  RhythmModeConfig? get activeConfig => configFor(active);

  RhythmModeConfig? configFor(RhythmMode mode) {
    for (final config in configs) {
      if (config.mode == mode) return config;
    }
    return null;
  }
}

/// App-level settings from the Rhythm server.
class RhythmSettings {
  /// Legacy setting retained for older servers/callers.
  ///
  /// Newer servers no longer include this in settings payloads. Check
  /// [hasPowerSave] before treating [powerSave] as server-authoritative.
  final bool powerSave;
  final bool hasPowerSave;
  final bool autoUpdate;
  final RhythmLightRuntime lightRuntime;
  final bool hasLightRuntime;

  const RhythmSettings({
    required this.powerSave,
    this.hasPowerSave = true,
    this.autoUpdate = true,
    this.lightRuntime = RhythmLightRuntime.rhythmAdaptive,
    this.hasLightRuntime = false,
  });

  factory RhythmSettings.fromJson(Map<String, dynamic> json) {
    final hasPowerSave = json.containsKey('power_save');
    final runtimeId = json['light_runtime'] as String?;
    return RhythmSettings(
      powerSave: hasPowerSave ? json['power_save'] as bool? ?? true : true,
      hasPowerSave: hasPowerSave,
      autoUpdate: json['auto_update'] as bool? ?? true,
      lightRuntime: RhythmLightRuntime.fromId(runtimeId),
      hasLightRuntime: runtimeId != null,
    );
  }
}

/// Global switch controlling autonomous Rhythm light actions.
class RhythmLightBreaker {
  final bool enabled;

  const RhythmLightBreaker({required this.enabled});

  factory RhythmLightBreaker.fromJson(Map<String, dynamic> json) {
    return RhythmLightBreaker(enabled: json['enabled'] as bool? ?? true);
  }

  Map<String, dynamic> toJson() => {'enabled': enabled};
}

class RhythmProfiles {
  final List<RhythmCurveConfig> profiles;

  const RhythmProfiles({this.profiles = const []});

  factory RhythmProfiles.fromJson(Map<String, dynamic> json) {
    return RhythmProfiles(
      profiles: ((json['profiles'] as List<dynamic>?) ?? const <dynamic>[])
          .map((e) => RhythmCurveConfig.fromJson(e as Map<String, dynamic>))
          .toList(),
    );
  }
}
