import '../json_parsing.dart';

enum RhythmSceneSourceKind {
  user,
  imported;

  String get wireValue => switch (this) {
        RhythmSceneSourceKind.user => 'user',
        RhythmSceneSourceKind.imported => 'imported',
      };

  static RhythmSceneSourceKind fromString(String? value) => switch (value) {
        'imported' => RhythmSceneSourceKind.imported,
        _ => RhythmSceneSourceKind.user,
      };
}

class RhythmSceneSource {
  final RhythmSceneSourceKind kind;
  final String? provider;
  final String? externalId;
  final Map<String, dynamic> raw;

  const RhythmSceneSource({
    required this.kind,
    this.provider,
    this.externalId,
    this.raw = const <String, dynamic>{},
  });

  const RhythmSceneSource.user()
      : kind = RhythmSceneSourceKind.user,
        provider = null,
        externalId = null,
        raw = const <String, dynamic>{};

  const RhythmSceneSource.imported({
    required this.provider,
    required this.externalId,
    this.raw = const <String, dynamic>{},
  }) : kind = RhythmSceneSourceKind.imported;

  factory RhythmSceneSource.fromJson(Map<String, dynamic> json) {
    final raw = Map<String, dynamic>.from(json)
      ..remove('kind')
      ..remove('provider')
      ..remove('external_id');
    return RhythmSceneSource(
      kind: RhythmSceneSourceKind.fromString(json['kind'] as String?),
      provider: json['provider'] as String?,
      externalId: json['external_id'] as String?,
      raw: raw,
    );
  }

  Map<String, dynamic> toJson() => {
        ...raw,
        'kind': kind.wireValue,
        if (provider != null) 'provider': provider,
        if (externalId != null) 'external_id': externalId,
      };
}

enum RhythmLightTargetKind {
  node;

  String get wireValue => switch (this) {
        RhythmLightTargetKind.node => 'node',
      };

  static RhythmLightTargetKind fromString(String? value) => switch (value) {
        _ => RhythmLightTargetKind.node,
      };
}

class RhythmLightTarget {
  final RhythmLightTargetKind kind;
  final String nodeId;
  final Map<String, dynamic> raw;

  const RhythmLightTarget({
    required this.kind,
    required this.nodeId,
    this.raw = const <String, dynamic>{},
  });

  const RhythmLightTarget.node(this.nodeId)
      : kind = RhythmLightTargetKind.node,
        raw = const <String, dynamic>{};

  factory RhythmLightTarget.fromJson(Map<String, dynamic> json) {
    final raw = Map<String, dynamic>.from(json)
      ..remove('kind')
      ..remove('node_id');
    return RhythmLightTarget(
      kind: RhythmLightTargetKind.fromString(json['kind'] as String?),
      nodeId: json['node_id'] as String? ?? '',
      raw: raw,
    );
  }

  bool get isValid => kind == RhythmLightTargetKind.node && nodeId.isNotEmpty;

  Map<String, dynamic> toJson() => {
        ...raw,
        'kind': kind.wireValue,
        'node_id': nodeId,
      };
}

class RhythmSceneRgbColor {
  final int r;
  final int g;
  final int b;

  const RhythmSceneRgbColor({
    required this.r,
    required this.g,
    required this.b,
  });

  factory RhythmSceneRgbColor.fromJson(Map<String, dynamic> json) {
    return RhythmSceneRgbColor(
      r: jsonInt(json['r'], preferredKeys: const ['r']) ?? 0,
      g: jsonInt(json['g'], preferredKeys: const ['g']) ?? 0,
      b: jsonInt(json['b'], preferredKeys: const ['b']) ?? 0,
    );
  }

  Map<String, dynamic> toJson() => {
        'r': r,
        'g': g,
        'b': b,
      };
}

class RhythmSceneXyColor {
  final double x;
  final double y;

  const RhythmSceneXyColor({
    required this.x,
    required this.y,
  });

  factory RhythmSceneXyColor.fromJson(Map<String, dynamic> json) {
    return RhythmSceneXyColor(
      x: jsonDouble(json['x'], preferredKeys: const ['x']) ?? 0.0,
      y: jsonDouble(json['y'], preferredKeys: const ['y']) ?? 0.0,
    );
  }

  Map<String, dynamic> toJson() => {
        'x': x,
        'y': y,
      };
}

enum RhythmLightColorKind {
  kelvin,
  rgb,
  xy,
  rgbXy;

  String get wireValue => switch (this) {
        RhythmLightColorKind.kelvin => 'kelvin',
        RhythmLightColorKind.rgb => 'rgb',
        RhythmLightColorKind.xy => 'xy',
        RhythmLightColorKind.rgbXy => 'rgb_xy',
      };

  static RhythmLightColorKind fromString(String? value) => switch (value) {
        'rgb' => RhythmLightColorKind.rgb,
        'xy' => RhythmLightColorKind.xy,
        'rgb_xy' => RhythmLightColorKind.rgbXy,
        _ => RhythmLightColorKind.kelvin,
      };
}

class RhythmLightColor {
  final RhythmLightColorKind kind;
  final int? kelvin;
  final RhythmSceneRgbColor? rgb;
  final RhythmSceneXyColor? xy;
  final Map<String, dynamic> raw;

  const RhythmLightColor({
    required this.kind,
    this.kelvin,
    this.rgb,
    this.xy,
    this.raw = const <String, dynamic>{},
  });

  const RhythmLightColor.kelvin(this.kelvin)
      : kind = RhythmLightColorKind.kelvin,
        rgb = null,
        xy = null,
        raw = const <String, dynamic>{};

  const RhythmLightColor.rgb(this.rgb)
      : kind = RhythmLightColorKind.rgb,
        kelvin = null,
        xy = null,
        raw = const <String, dynamic>{};

  const RhythmLightColor.xy(this.xy)
      : kind = RhythmLightColorKind.xy,
        kelvin = null,
        rgb = null,
        raw = const <String, dynamic>{};

  const RhythmLightColor.rgbXy({
    required this.rgb,
    required this.xy,
  })  : kind = RhythmLightColorKind.rgbXy,
        kelvin = null,
        raw = const <String, dynamic>{};

  factory RhythmLightColor.fromJson(Map<String, dynamic> json) {
    final raw = Map<String, dynamic>.from(json)
      ..remove('kind')
      ..remove('kelvin')
      ..remove('rgb')
      ..remove('xy');
    final rgbJson = jsonMap(json['rgb']);
    final xyJson = jsonMap(json['xy']);
    return RhythmLightColor(
      kind: RhythmLightColorKind.fromString(json['kind'] as String?),
      kelvin: jsonInt(json['kelvin'], preferredKeys: const ['kelvin']),
      rgb: rgbJson == null ? null : RhythmSceneRgbColor.fromJson(rgbJson),
      xy: xyJson == null ? null : RhythmSceneXyColor.fromJson(xyJson),
      raw: raw,
    );
  }

  bool get isComplete => switch (kind) {
        RhythmLightColorKind.kelvin => kelvin != null,
        RhythmLightColorKind.rgb => rgb != null,
        RhythmLightColorKind.xy => xy != null,
        RhythmLightColorKind.rgbXy => rgb != null && xy != null,
      };

  Map<String, dynamic> toJson() => {
        ...raw,
        'kind': kind.wireValue,
        if (kelvin != null) 'kelvin': kelvin,
        if (rgb != null) 'rgb': rgb!.toJson(),
        if (xy != null) 'xy': xy!.toJson(),
      };
}

enum RhythmScenePower {
  on,
  off;

  String get wireValue => switch (this) {
        RhythmScenePower.on => 'on',
        RhythmScenePower.off => 'off',
      };

  static RhythmScenePower fromString(String? value) => switch (value) {
        'off' => RhythmScenePower.off,
        _ => RhythmScenePower.on,
      };
}

class RhythmLightSceneOutput {
  final RhythmScenePower power;
  final int? brightness;
  final RhythmLightColor? color;
  final int? transitionMs;
  final Map<String, dynamic> raw;

  const RhythmLightSceneOutput({
    this.power = RhythmScenePower.on,
    this.brightness,
    this.color,
    this.transitionMs,
    this.raw = const <String, dynamic>{},
  });

  const RhythmLightSceneOutput.on({
    this.brightness,
    required this.color,
    this.transitionMs,
    this.raw = const <String, dynamic>{},
  }) : power = RhythmScenePower.on;

  const RhythmLightSceneOutput.off({
    this.transitionMs,
    this.raw = const <String, dynamic>{},
  })  : power = RhythmScenePower.off,
        brightness = null,
        color = null;

  factory RhythmLightSceneOutput.fromJson(Map<String, dynamic> json) {
    final raw = Map<String, dynamic>.from(json)
      ..remove('power')
      ..remove('brightness')
      ..remove('color')
      ..remove('transition_ms');
    final colorJson = jsonMap(json['color']);
    return RhythmLightSceneOutput(
      power: RhythmScenePower.fromString(json['power'] as String?),
      brightness:
          jsonInt(json['brightness'], preferredKeys: const ['brightness']),
      color: colorJson == null ? null : RhythmLightColor.fromJson(colorJson),
      transitionMs: jsonInt(json['transition_ms'],
          preferredKeys: const ['transition_ms']),
      raw: raw,
    );
  }

  bool get isOff => power == RhythmScenePower.off;

  bool get isOn => power == RhythmScenePower.on;

  bool get hasValidBrightness =>
      brightness == null || (brightness! >= 1 && brightness! <= 100);

  bool get isValid =>
      isOff || (color?.isComplete == true && hasValidBrightness);

  List<String> get validationErrors {
    if (isOff) return const [];
    final errors = <String>[];
    if (color?.isComplete != true) {
      errors.add('power=on requires a color');
    }
    if (!hasValidBrightness) {
      errors.add('brightness must be between 1 and 100');
    }
    return errors;
  }

  Map<String, dynamic> toJson() {
    if (isOff) {
      return {
        ...raw,
        'power': power.wireValue,
        if (transitionMs != null) 'transition_ms': transitionMs,
      };
    }
    return {
      ...raw,
      'power': power.wireValue,
      if (brightness != null) 'brightness': brightness,
      if (color != null) 'color': color!.toJson(),
      if (transitionMs != null) 'transition_ms': transitionMs,
    };
  }
}

class RhythmLightSceneEntry {
  final RhythmLightTarget target;
  final RhythmLightSceneOutput output;
  final Map<String, dynamic> raw;

  const RhythmLightSceneEntry({
    required this.target,
    required this.output,
    this.raw = const <String, dynamic>{},
  });

  factory RhythmLightSceneEntry.fromJson(Map<String, dynamic> json) {
    final raw = Map<String, dynamic>.from(json)
      ..remove('target')
      ..remove('output');
    final targetJson = jsonMap(json['target']);
    final outputJson = jsonMap(json['output']);
    return RhythmLightSceneEntry(
      target: targetJson == null
          ? const RhythmLightTarget.node('')
          : RhythmLightTarget.fromJson(targetJson),
      output: outputJson == null
          ? const RhythmLightSceneOutput()
          : RhythmLightSceneOutput.fromJson(outputJson),
      raw: raw,
    );
  }

  bool get isValid => target.isValid && output.isValid;

  List<String> get validationErrors {
    final errors = <String>[];
    if (!target.isValid) {
      errors.add('entry target requires a node_id');
    }
    errors.addAll(output.validationErrors);
    return errors;
  }

  Map<String, dynamic> toJson() => {
        ...raw,
        'target': target.toJson(),
        'output': output.toJson(),
      };
}

class RhythmLightScene {
  final int? defaultTransitionMs;
  final RhythmLightSceneOutput? defaultOutput;
  final List<RhythmLightSceneOutput> palette;
  final List<RhythmLightSceneEntry> entries;
  final Map<String, dynamic> raw;

  const RhythmLightScene({
    this.defaultTransitionMs,
    this.defaultOutput,
    this.palette = const [],
    this.entries = const [],
    this.raw = const <String, dynamic>{},
  });

  factory RhythmLightScene.fromJson(Map<String, dynamic> json) {
    final raw = Map<String, dynamic>.from(json)
      ..remove('default_transition_ms')
      ..remove('default_output')
      ..remove('palette')
      ..remove('entries');
    final defaultOutputJson = jsonMap(json['default_output']);
    return RhythmLightScene(
      defaultTransitionMs: jsonInt(
        json['default_transition_ms'],
        preferredKeys: const ['default_transition_ms'],
      ),
      defaultOutput: defaultOutputJson == null
          ? null
          : RhythmLightSceneOutput.fromJson(defaultOutputJson),
      palette: ((json['palette'] as List<dynamic>?) ?? const <dynamic>[])
          .map(jsonMap)
          .nonNulls
          .map(RhythmLightSceneOutput.fromJson)
          .toList(),
      entries: ((json['entries'] as List<dynamic>?) ?? const <dynamic>[])
          .map(jsonMap)
          .nonNulls
          .map(RhythmLightSceneEntry.fromJson)
          .toList(),
      raw: raw,
    );
  }

  bool get isValid =>
      (defaultOutput?.isValid ?? true) &&
      palette.every((output) => output.isValid) &&
      entries.every((entry) => entry.isValid);

  List<String> get validationErrors {
    final errors = <String>[];
    for (final error in defaultOutput?.validationErrors ?? const <String>[]) {
      errors.add('default_output: $error');
    }
    for (var i = 0; i < palette.length; i += 1) {
      for (final error in palette[i].validationErrors) {
        errors.add('palette[$i]: $error');
      }
    }
    for (var i = 0; i < entries.length; i += 1) {
      for (final error in entries[i].validationErrors) {
        errors.add('entries[$i]: $error');
      }
    }
    return errors;
  }

  Map<String, dynamic> toJson() => {
        ...raw,
        if (defaultTransitionMs != null)
          'default_transition_ms': defaultTransitionMs,
        'default_output': defaultOutput?.toJson(),
        if (palette.isNotEmpty)
          'palette': palette.map((output) => output.toJson()).toList(),
        'entries': entries.map((entry) => entry.toJson()).toList(),
      };
}

class RhythmSceneDefinition {
  final String id;
  final String name;
  final String? description;
  final RhythmSceneSource source;
  final RhythmLightScene light;
  final Map<String, dynamic> extensions;
  final Map<String, dynamic> raw;

  const RhythmSceneDefinition({
    required this.id,
    required this.name,
    this.description,
    this.source = const RhythmSceneSource.user(),
    this.light = const RhythmLightScene(),
    this.extensions = const <String, dynamic>{},
    this.raw = const <String, dynamic>{},
  });

  factory RhythmSceneDefinition.fromJson(Map<String, dynamic> json) {
    final raw = Map<String, dynamic>.from(json)
      ..remove('id')
      ..remove('name')
      ..remove('description')
      ..remove('source')
      ..remove('light')
      ..remove('extensions');
    return RhythmSceneDefinition(
      id: json['id'] as String? ?? '',
      name: json['name'] as String? ?? '',
      description: json['description'] as String?,
      source: switch (jsonMap(json['source'])) {
        final sourceJson? => RhythmSceneSource.fromJson(sourceJson),
        _ => const RhythmSceneSource.user(),
      },
      light: switch (jsonMap(json['light'])) {
        final lightJson? => RhythmLightScene.fromJson(lightJson),
        _ => const RhythmLightScene(),
      },
      extensions: Map<String, dynamic>.from(
        jsonMap(json['extensions']) ?? const <String, dynamic>{},
      ),
      raw: raw,
    );
  }

  bool get isValidForSave => id.isNotEmpty && name.isNotEmpty && light.isValid;

  List<String> get validationErrors {
    final errors = <String>[];
    if (id.isEmpty) errors.add('id is required');
    if (name.isEmpty) errors.add('name is required');
    errors.addAll(light.validationErrors);
    return errors;
  }

  RhythmSceneDefinition copyWith({
    String? id,
    String? name,
    String? description,
    RhythmSceneSource? source,
    RhythmLightScene? light,
    Map<String, dynamic>? extensions,
    Map<String, dynamic>? raw,
  }) {
    return RhythmSceneDefinition(
      id: id ?? this.id,
      name: name ?? this.name,
      description: description ?? this.description,
      source: source ?? this.source,
      light: light ?? this.light,
      extensions: extensions ?? this.extensions,
      raw: raw ?? this.raw,
    );
  }

  Map<String, dynamic> toJson() => {
        ...raw,
        'id': id,
        'name': name,
        'description': description,
        'source': source.toJson(),
        'light': light.toJson(),
        'extensions': extensions,
      };
}

class RhythmSceneActionResult {
  final String sceneId;
  final String targetId;
  final List<String> affectedNodeIds;
  final List<String> unresolvedNodeIds;
  final String? previewId;
  final Map<String, dynamic> raw;

  const RhythmSceneActionResult({
    required this.sceneId,
    required this.targetId,
    this.affectedNodeIds = const [],
    this.unresolvedNodeIds = const [],
    this.previewId,
    this.raw = const <String, dynamic>{},
  });

  factory RhythmSceneActionResult.fromJson(Map<String, dynamic> json) {
    final raw = Map<String, dynamic>.from(json)
      ..remove('scene_id')
      ..remove('target_id')
      ..remove('affected_node_ids')
      ..remove('unresolved_node_ids')
      ..remove('preview_id');
    return RhythmSceneActionResult(
      sceneId: json['scene_id'] as String? ?? '',
      targetId: json['target_id'] as String? ?? '',
      affectedNodeIds: _stringList(json['affected_node_ids']),
      unresolvedNodeIds: _stringList(json['unresolved_node_ids']),
      previewId: json['preview_id'] as String?,
      raw: raw,
    );
  }

  bool get hasPreviewId => previewId != null && previewId!.isNotEmpty;
}

List<String> _stringList(dynamic value) {
  return ((value as List<dynamic>?) ?? const <dynamic>[])
      .whereType<String>()
      .toList();
}
