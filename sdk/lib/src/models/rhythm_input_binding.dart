import 'rhythm_room.dart' show RhythmMode;

class RhythmInputBindingPreset {
  final String value;

  const RhythmInputBindingPreset(this.value);

  static const daySleepToggle = RhythmInputBindingPreset('day_sleep_toggle');

  static RhythmInputBindingPreset? fromString(String? value) {
    if (value == null || value.isEmpty) return null;
    return switch (value) {
      'day_sleep_toggle' => daySleepToggle,
      _ => RhythmInputBindingPreset(value),
    };
  }

  String get wireValue => value;

  @override
  bool operator ==(Object other) =>
      other is RhythmInputBindingPreset && other.value == value;

  @override
  int get hashCode => value.hashCode;

  @override
  String toString() => value;
}

class RhythmButtonAction {
  final String value;

  const RhythmButtonAction(this.value);

  static const onPress = RhythmButtonAction('on_press');
  static const toggle = RhythmButtonAction('toggle');
  static const offPress = RhythmButtonAction('off_press');
  static const reset = RhythmButtonAction('reset');
  static const upPress = RhythmButtonAction('up_press');
  static const downPress = RhythmButtonAction('down_press');
  static const upHold = RhythmButtonAction('up_hold');
  static const downHold = RhythmButtonAction('down_hold');
  static const stop = RhythmButtonAction('stop');
  static const rhythmOn = RhythmButtonAction('rhythm_on');
  static const rhythmOff = RhythmButtonAction('rhythm_off');
  static const lightsOff = RhythmButtonAction('lights_off');
  static const sleepOn = RhythmButtonAction('sleep_on');
  static const sleepOff = RhythmButtonAction('sleep_off');

  static RhythmButtonAction? fromString(String? value) {
    if (value == null || value.isEmpty) return null;
    return switch (value) {
      'on' || 'on_press' => onPress,
      'toggle' => toggle,
      'off' || 'off_press' => offPress,
      'reset' => reset,
      'up_press' => upPress,
      'down_press' => downPress,
      'up_hold' => upHold,
      'down_hold' => downHold,
      'stop' => stop,
      'rhythm_on' => rhythmOn,
      'rhythm_off' => rhythmOff,
      'lights_off' => lightsOff,
      'sleep_on' => sleepOn,
      'sleep_off' => sleepOff,
      _ => RhythmButtonAction(value),
    };
  }

  String get wireValue => value;

  @override
  bool operator ==(Object other) =>
      other is RhythmButtonAction && other.value == value;

  @override
  int get hashCode => value.hashCode;

  @override
  String toString() => value;
}

class RhythmInputBindingTrigger {
  final String kind;
  final RhythmButtonAction? buttonAction;

  const RhythmInputBindingTrigger({
    required this.kind,
    this.buttonAction,
  });

  const RhythmInputBindingTrigger.button({
    this.buttonAction,
  }) : kind = 'button';

  bool get isButton => kind == 'button';

  factory RhythmInputBindingTrigger.fromJson(Map<String, dynamic> json) {
    return RhythmInputBindingTrigger(
      kind: json['kind'] as String? ?? 'button',
      buttonAction:
          RhythmButtonAction.fromString(json['button_action'] as String?),
    );
  }

  Map<String, dynamic> toJson() => {
        'kind': kind,
        if (buttonAction != null) 'button_action': buttonAction!.wireValue,
      };
}

sealed class RhythmModeTransitionSelection {
  const RhythmModeTransitionSelection();

  const factory RhythmModeTransitionSelection.auto() = RhythmModeTransitionAuto;
  const factory RhythmModeTransitionSelection.none() = RhythmModeTransitionNone;
  const factory RhythmModeTransitionSelection.exact(String id) =
      RhythmModeTransitionExact;

  factory RhythmModeTransitionSelection.fromJson(dynamic json) {
    if (json is! Map) return const RhythmModeTransitionAuto();
    final kind = json['kind'] as String? ?? 'auto';
    return switch (kind) {
      'none' => const RhythmModeTransitionNone(),
      'exact' => RhythmModeTransitionExact(json['id'] as String? ?? ''),
      _ => const RhythmModeTransitionAuto(),
    };
  }

  Map<String, dynamic> toJson();
}

class RhythmModeTransitionAuto extends RhythmModeTransitionSelection {
  const RhythmModeTransitionAuto();

  @override
  Map<String, dynamic> toJson() => {'kind': 'auto'};
}

class RhythmModeTransitionNone extends RhythmModeTransitionSelection {
  const RhythmModeTransitionNone();

  @override
  Map<String, dynamic> toJson() => {'kind': 'none'};
}

class RhythmModeTransitionExact extends RhythmModeTransitionSelection {
  final String id;

  const RhythmModeTransitionExact(this.id);

  @override
  Map<String, dynamic> toJson() => {
        'kind': 'exact',
        'id': id,
      };
}

sealed class RhythmAutomationAction {
  const RhythmAutomationAction();

  factory RhythmAutomationAction.fromJson(Map<String, dynamic> json) {
    final kind = json['kind'] as String? ?? '';
    return switch (kind) {
      'mode_cycle' => RhythmModeCycleAction.fromJson(json),
      'mode_set' => RhythmModeSetAction.fromJson(json),
      'mode_toggle' => RhythmModeToggleAction.fromJson(json),
      _ => RhythmRawAutomationAction(kind: kind, raw: json),
    };
  }

  Map<String, dynamic> toJson();
}

class RhythmModeCycleAction extends RhythmAutomationAction {
  final List<RhythmMode> modes;
  final RhythmModeTransitionSelection transition;

  const RhythmModeCycleAction({
    required this.modes,
    this.transition = const RhythmModeTransitionSelection.auto(),
  });

  factory RhythmModeCycleAction.fromJson(Map<String, dynamic> json) {
    return RhythmModeCycleAction(
      modes: ((json['modes'] as List<dynamic>?) ?? const <dynamic>[])
          .map((value) => RhythmMode.fromString(value as String?))
          .nonNulls
          .toList(),
      transition: RhythmModeTransitionSelection.fromJson(json['transition']),
    );
  }

  @override
  Map<String, dynamic> toJson() => {
        'kind': 'mode_cycle',
        'modes': modes.map((mode) => mode.wireValue).toList(),
        'transition': transition.toJson(),
      };
}

class RhythmModeSetAction extends RhythmAutomationAction {
  final RhythmMode mode;
  final RhythmModeTransitionSelection transition;

  const RhythmModeSetAction({
    required this.mode,
    this.transition = const RhythmModeTransitionSelection.auto(),
  });

  factory RhythmModeSetAction.fromJson(Map<String, dynamic> json) {
    return RhythmModeSetAction(
      mode: RhythmMode.fromString(json['mode'] as String?) ?? RhythmMode.day,
      transition: RhythmModeTransitionSelection.fromJson(json['transition']),
    );
  }

  @override
  Map<String, dynamic> toJson() => {
        'kind': 'mode_set',
        'mode': mode.wireValue,
        'transition': transition.toJson(),
      };
}

class RhythmModeToggleAction extends RhythmAutomationAction {
  final RhythmMode firstMode;
  final RhythmMode secondMode;
  final RhythmModeTransitionSelection transition;

  const RhythmModeToggleAction({
    required this.firstMode,
    required this.secondMode,
    this.transition = const RhythmModeTransitionSelection.auto(),
  });

  factory RhythmModeToggleAction.fromJson(Map<String, dynamic> json) {
    return RhythmModeToggleAction(
      firstMode: RhythmMode.fromString(json['first_mode'] as String?) ??
          RhythmMode.day,
      secondMode: RhythmMode.fromString(json['second_mode'] as String?) ??
          RhythmMode.sleep,
      transition: RhythmModeTransitionSelection.fromJson(json['transition']),
    );
  }

  @override
  Map<String, dynamic> toJson() => {
        'kind': 'mode_toggle',
        'first_mode': firstMode.wireValue,
        'second_mode': secondMode.wireValue,
        'transition': transition.toJson(),
      };
}

class RhythmRawAutomationAction extends RhythmAutomationAction {
  final String kind;
  final Map<String, dynamic> raw;

  const RhythmRawAutomationAction({
    required this.kind,
    required this.raw,
  });

  @override
  Map<String, dynamic> toJson() => Map<String, dynamic>.from(raw);
}

class RhythmInputBinding {
  final String id;
  final RhythmInputBindingPreset? preset;
  final String sourceNodeId;
  final RhythmInputBindingTrigger trigger;
  final RhythmAutomationAction action;
  final bool enabled;

  const RhythmInputBinding({
    required this.id,
    this.preset,
    required this.sourceNodeId,
    required this.trigger,
    required this.action,
    this.enabled = true,
  });

  factory RhythmInputBinding.fromJson(Map<String, dynamic> json) {
    final triggerJson = json['trigger'];
    final actionJson = json['action'];
    return RhythmInputBinding(
      id: json['id'] as String? ?? '',
      preset: RhythmInputBindingPreset.fromString(json['preset'] as String?),
      sourceNodeId: json['source_node_id'] as String? ?? '',
      trigger: triggerJson is Map<String, dynamic>
          ? RhythmInputBindingTrigger.fromJson(triggerJson)
          : const RhythmInputBindingTrigger.button(),
      action: actionJson is Map<String, dynamic>
          ? RhythmAutomationAction.fromJson(actionJson)
          : const RhythmRawAutomationAction(kind: '', raw: {}),
      enabled: json['enabled'] as bool? ?? true,
    );
  }

  Map<String, dynamic> toJson() => {
        'id': id,
        if (preset != null) 'preset': preset!.wireValue,
        'source_node_id': sourceNodeId,
        'trigger': trigger.toJson(),
        'action': action.toJson(),
        'enabled': enabled,
      };

  RhythmInputBinding copyWith({
    String? id,
    Object? preset = _sentinel,
    String? sourceNodeId,
    RhythmInputBindingTrigger? trigger,
    RhythmAutomationAction? action,
    bool? enabled,
  }) {
    return RhythmInputBinding(
      id: id ?? this.id,
      preset: identical(preset, _sentinel)
          ? this.preset
          : preset as RhythmInputBindingPreset?,
      sourceNodeId: sourceNodeId ?? this.sourceNodeId,
      trigger: trigger ?? this.trigger,
      action: action ?? this.action,
      enabled: enabled ?? this.enabled,
    );
  }
}

class RhythmInputBindings {
  final List<RhythmInputBinding> bindings;

  const RhythmInputBindings({
    this.bindings = const [],
  });

  factory RhythmInputBindings.fromJson(Map<String, dynamic> json) {
    return RhythmInputBindings(
      bindings: ((json['bindings'] as List<dynamic>?) ?? const <dynamic>[])
          .whereType<Map<String, dynamic>>()
          .map(RhythmInputBinding.fromJson)
          .toList(),
    );
  }
}

const Object _sentinel = Object();
