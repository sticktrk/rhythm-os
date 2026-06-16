import 'rhythm_curve_config.dart';
import 'rhythm_room.dart' show RhythmMode;

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

  Map<String, dynamic> toJson() => {
        'room_id': roomId,
        'state': state,
      };
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
  dynamic toJson() => {
        'mode': 'fixed',
        'value': milliseconds,
      };
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
  final bool preserveHardOff;

  const RhythmModeTransitionConfig({
    this.id = '',
    this.label = '',
    required this.fromMode,
    required this.toMode,
    this.trigger = const RhythmTransitionTrigger.manual(),
    this.triggerEnabled = true,
    required this.duration,
    required this.preserveHardOff,
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
          ? RhythmTransitionTrigger.fromJson(
              rawTrigger.cast<String, dynamic>(),
            )
          : rawTrigger is String
              ? RhythmTransitionTrigger.fromLegacyValue(rawTrigger)
              : const RhythmTransitionTrigger.manual(),
      triggerEnabled: json['trigger_enabled'] as bool? ?? true,
      duration: TransitionDuration.fromJson(json['duration_ms']),
      preserveHardOff: json['preserve_hard_off'] as bool? ?? true,
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
      'preserve_hard_off': preserveHardOff,
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
    bool? preserveHardOff,
  }) {
    return RhythmModeTransitionConfig(
      id: id ?? this.id,
      label: label ?? this.label,
      fromMode: fromMode ?? this.fromMode,
      toMode: toMode ?? this.toMode,
      trigger: trigger ?? this.trigger,
      triggerEnabled: triggerEnabled ?? this.triggerEnabled,
      duration: duration ?? this.duration,
      preserveHardOff: preserveHardOff ?? this.preserveHardOff,
    );
  }
}

class RhythmTransitionTrigger {
  final String kind;
  final String? event;
  final String? time;

  const RhythmTransitionTrigger._({
    required this.kind,
    this.event,
    this.time,
  });

  const RhythmTransitionTrigger.manual() : this._(kind: 'manual');

  const RhythmTransitionTrigger.solar(String event)
      : this._(kind: 'solar', event: event);

  const RhythmTransitionTrigger.scheduled(String time)
      : this._(kind: 'scheduled', time: time);

  factory RhythmTransitionTrigger.fromJson(Map<String, dynamic> json) {
    final kind = json['kind'] as String? ?? 'manual';
    final event = json['event'] as String?;
    final time = json['time'] as String?;
    return switch (kind) {
      'solar' when event != null && event.isNotEmpty =>
        RhythmTransitionTrigger.solar(event),
      'solar' => const RhythmTransitionTrigger._(kind: 'solar'),
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
  final RhythmModeLastChange? lastChange;
  final List<RhythmModeConfig> configs;

  const RhythmModeResource({
    required this.active,
    this.lastChange,
    this.configs = const [],
  });

  factory RhythmModeResource.fromJson(Map<String, dynamic> json) {
    final lastChangeJson = json['last_change'];
    final hasFlatLastChange = json.containsKey('cause') ||
        json.containsKey('transition_id') ||
        json.containsKey('epoch_ms') ||
        json.containsKey('utc_ms');
    return RhythmModeResource(
      active:
          RhythmMode.fromString(json['active'] as String?) ?? RhythmMode.day,
      lastChange: lastChangeJson is Map<String, dynamic>
          ? RhythmModeLastChange.fromJson(
              lastChangeJson,
            )
          : hasFlatLastChange
              ? RhythmModeLastChange.fromJson(json)
              : null,
      configs: ((json['configs'] as List<dynamic>?) ?? const <dynamic>[])
          .map((e) => RhythmModeConfig.fromJson(e as Map<String, dynamic>))
          .toList(),
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

  const RhythmSettings({
    required this.powerSave,
    this.hasPowerSave = true,
    this.autoUpdate = true,
  });

  factory RhythmSettings.fromJson(Map<String, dynamic> json) {
    final hasPowerSave = json.containsKey('power_save');
    return RhythmSettings(
      powerSave: hasPowerSave ? json['power_save'] as bool? ?? true : true,
      hasPowerSave: hasPowerSave,
      autoUpdate: json['auto_update'] as bool? ?? true,
    );
  }
}

/// Global switch controlling autonomous Rhythm light actions.
class RhythmLightBreaker {
  final bool enabled;

  const RhythmLightBreaker({
    required this.enabled,
  });

  factory RhythmLightBreaker.fromJson(Map<String, dynamic> json) {
    return RhythmLightBreaker(
      enabled: json['enabled'] as bool? ?? true,
    );
  }

  Map<String, dynamic> toJson() => {
        'enabled': enabled,
      };
}

class RhythmProfiles {
  final List<RhythmCurveConfig> profiles;

  const RhythmProfiles({
    this.profiles = const [],
  });

  factory RhythmProfiles.fromJson(Map<String, dynamic> json) {
    return RhythmProfiles(
      profiles: ((json['profiles'] as List<dynamic>?) ?? const <dynamic>[])
          .map((e) => RhythmCurveConfig.fromJson(e as Map<String, dynamic>))
          .toList(),
    );
  }
}
