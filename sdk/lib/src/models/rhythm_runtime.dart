import '../json_parsing.dart';
import 'rhythm_room.dart';

class RhythmNodesPollResponse {
  final bool hubConnected;
  final List<RhythmRoomState> nodes;
  final Map<String, dynamic> raw;

  const RhythmNodesPollResponse({
    required this.hubConnected,
    this.nodes = const [],
    this.raw = const <String, dynamic>{},
  });

  const RhythmNodesPollResponse.empty()
      : hubConnected = false,
        nodes = const [],
        raw = const <String, dynamic>{};

  factory RhythmNodesPollResponse.fromJson(Map<String, dynamic> json) {
    return RhythmNodesPollResponse(
      hubConnected: json['hub_connected'] == true,
      nodes: _parseNodeStates(json['nodes']),
      raw: Map<String, dynamic>.from(json),
    );
  }
}

class RhythmNodeNow {
  final RhythmRoomState node;
  final RhythmPipelineTrace pipelineTrace;
  final Map<String, dynamic> raw;

  const RhythmNodeNow({
    required this.node,
    required this.pipelineTrace,
    required this.raw,
  });

  factory RhythmNodeNow.fromJson(Map<String, dynamic> json) {
    return RhythmNodeNow(
      node: RhythmRoomState.fromJson(
        jsonMap(json['node']) ?? const <String, dynamic>{},
      ),
      pipelineTrace: RhythmPipelineTrace.fromJson(
        jsonMap(json['pipeline_trace']) ?? const <String, dynamic>{},
      ),
      raw: Map<String, dynamic>.from(json),
    );
  }
}

class RhythmPipelineTrace {
  final List<RhythmPipelineTraceEntry> entries;
  final List<RhythmPipelineDispatchDecision> dispatchDecisions;
  final Map<String, dynamic> raw;

  const RhythmPipelineTrace({
    this.entries = const [],
    this.dispatchDecisions = const [],
    this.raw = const <String, dynamic>{},
  });

  factory RhythmPipelineTrace.fromJson(Map<String, dynamic> json) {
    return RhythmPipelineTrace(
      entries: ((json['entries'] as List<dynamic>?) ?? const <dynamic>[])
          .map(jsonMap)
          .nonNulls
          .map(RhythmPipelineTraceEntry.fromJson)
          .toList(growable: false),
      dispatchDecisions:
          ((json['dispatch_decisions'] as List<dynamic>?) ?? const <dynamic>[])
              .map(jsonMap)
              .nonNulls
              .map(RhythmPipelineDispatchDecision.fromJson)
              .toList(growable: false),
      raw: Map<String, dynamic>.from(json),
    );
  }
}

class RhythmPipelineTraceEvent {
  final String nodeId;
  final RhythmPipelineTrace trace;
  final Map<String, dynamic> raw;

  const RhythmPipelineTraceEvent({
    required this.nodeId,
    required this.trace,
    this.raw = const <String, dynamic>{},
  });

  factory RhythmPipelineTraceEvent.fromJson(Map<String, dynamic> json) {
    return RhythmPipelineTraceEvent(
      nodeId: json['node_id']?.toString() ?? '',
      trace: RhythmPipelineTrace.fromJson(
        jsonMap(json['trace']) ?? const <String, dynamic>{},
      ),
      raw: Map<String, dynamic>.from(json),
    );
  }
}

class RhythmPipelineTraceEntry {
  final String stageId;
  final String phase;
  final String value;
  final double input;
  final double output;
  final String? skipReason;
  final String? quantizeMode;
  final String? timingWriter;
  final Map<String, dynamic> raw;

  const RhythmPipelineTraceEntry({
    required this.stageId,
    required this.phase,
    required this.value,
    required this.input,
    required this.output,
    this.skipReason,
    this.quantizeMode,
    this.timingWriter,
    this.raw = const <String, dynamic>{},
  });

  factory RhythmPipelineTraceEntry.fromJson(Map<String, dynamic> json) {
    return RhythmPipelineTraceEntry(
      stageId: json['stage_id']?.toString() ?? '',
      phase: json['phase']?.toString() ?? '',
      value: json['value']?.toString() ?? '',
      input: jsonDouble(json['input'], preferredKeys: const ['input']) ?? 0.0,
      output:
          jsonDouble(json['output'], preferredKeys: const ['output']) ?? 0.0,
      skipReason: json['skip_reason']?.toString(),
      quantizeMode: json['quantize_mode']?.toString(),
      timingWriter: json['timing_writer']?.toString(),
      raw: Map<String, dynamic>.from(json),
    );
  }
}

class RhythmPipelineDispatchDecision {
  final String stageId;
  final String targetId;
  final bool shouldDispatch;
  final String reason;
  final Map<String, dynamic> raw;

  const RhythmPipelineDispatchDecision({
    required this.stageId,
    required this.targetId,
    required this.shouldDispatch,
    required this.reason,
    this.raw = const <String, dynamic>{},
  });

  factory RhythmPipelineDispatchDecision.fromJson(Map<String, dynamic> json) {
    return RhythmPipelineDispatchDecision(
      stageId: json['stage_id']?.toString() ?? '',
      targetId: json['target_id']?.toString() ?? '',
      shouldDispatch: json['should_dispatch'] == true,
      reason: json['reason']?.toString() ?? '',
      raw: Map<String, dynamic>.from(json),
    );
  }
}

class RhythmScopeLayerWritesResult {
  final List<RhythmScopeLayerWriteNode> nodes;
  final Map<String, dynamic> raw;

  const RhythmScopeLayerWritesResult({
    this.nodes = const [],
    this.raw = const <String, dynamic>{},
  });

  const RhythmScopeLayerWritesResult.empty()
      : nodes = const [],
        raw = const <String, dynamic>{};

  factory RhythmScopeLayerWritesResult.fromJson(Map<String, dynamic> json) {
    return RhythmScopeLayerWritesResult(
      nodes: ((json['nodes'] as List<dynamic>?) ?? const <dynamic>[])
          .map(jsonMap)
          .nonNulls
          .map(RhythmScopeLayerWriteNode.fromJson)
          .toList(growable: false),
      raw: Map<String, dynamic>.from(json),
    );
  }
}

class RhythmScopeLayerWriteNode {
  final String nodeId;
  final List<Map<String, dynamic>> writes;
  final Map<String, dynamic> raw;

  const RhythmScopeLayerWriteNode({
    required this.nodeId,
    this.writes = const [],
    this.raw = const <String, dynamic>{},
  });

  factory RhythmScopeLayerWriteNode.fromJson(Map<String, dynamic> json) {
    return RhythmScopeLayerWriteNode(
      nodeId: json['node_id']?.toString() ?? '',
      writes: ((json['writes'] as List<dynamic>?) ?? const <dynamic>[])
          .map(jsonMap)
          .nonNulls
          .map(Map<String, dynamic>.from)
          .toList(growable: false),
      raw: Map<String, dynamic>.from(json),
    );
  }
}

class RhythmInputActionsCatalog {
  final List<RhythmInputAction> actions;
  final List<RhythmInputActionDynamicPattern> dynamicPatterns;
  final Map<String, dynamic> raw;

  const RhythmInputActionsCatalog({
    this.actions = const [],
    this.dynamicPatterns = const [],
    this.raw = const <String, dynamic>{},
  });

  const RhythmInputActionsCatalog.empty()
      : actions = const [],
        dynamicPatterns = const [],
        raw = const <String, dynamic>{};

  factory RhythmInputActionsCatalog.fromJson(Map<String, dynamic> json) {
    return RhythmInputActionsCatalog(
      actions: ((json['actions'] as List<dynamic>?) ?? const <dynamic>[])
          .map(jsonMap)
          .nonNulls
          .map(RhythmInputAction.fromJson)
          .toList(growable: false),
      dynamicPatterns:
          ((json['dynamic_patterns'] as List<dynamic>?) ?? const <dynamic>[])
              .map(jsonMap)
              .nonNulls
              .map(RhythmInputActionDynamicPattern.fromJson)
              .toList(growable: false),
      raw: Map<String, dynamic>.from(json),
    );
  }
}

class RhythmInputActionParam {
  final String key;
  final String value;

  const RhythmInputActionParam({
    required this.key,
    required this.value,
  });

  factory RhythmInputActionParam.fromJson(Map<String, dynamic> json) {
    return RhythmInputActionParam(
      key: json['key']?.toString() ?? '',
      value: json['value']?.toString() ?? '',
    );
  }
}

class RhythmInputAction {
  final String? pythonActionId;
  final String? migratedId;
  final String category;
  final String label;
  final String rustAction;
  final List<RhythmInputActionParam> parameters;
  final List<String> allowedTargetGrains;
  final bool supportsWhenOff;
  final List<String> allowedWhenOffValues;
  final String dispatchPrecondition;
  final String importBehavior;
  final Map<String, dynamic> raw;

  const RhythmInputAction({
    this.pythonActionId,
    this.migratedId,
    required this.category,
    required this.label,
    required this.rustAction,
    this.parameters = const [],
    this.allowedTargetGrains = const [],
    required this.supportsWhenOff,
    this.allowedWhenOffValues = const [],
    required this.dispatchPrecondition,
    required this.importBehavior,
    this.raw = const <String, dynamic>{},
  });

  factory RhythmInputAction.fromJson(Map<String, dynamic> json) {
    return RhythmInputAction(
      pythonActionId: json['python_action_id']?.toString(),
      migratedId: json['migrated_id']?.toString(),
      category: json['category']?.toString() ?? '',
      label: json['label']?.toString() ?? '',
      rustAction: json['rust_action']?.toString() ?? '',
      parameters: ((json['parameters'] as List<dynamic>?) ?? const <dynamic>[])
          .map(jsonMap)
          .nonNulls
          .map(RhythmInputActionParam.fromJson)
          .toList(growable: false),
      allowedTargetGrains: _stringList(json['allowed_target_grains']),
      supportsWhenOff: json['supports_when_off'] == true,
      allowedWhenOffValues: _stringList(json['allowed_when_off_values']),
      dispatchPrecondition: json['dispatch_precondition']?.toString() ?? '',
      importBehavior: json['import_behavior']?.toString() ?? '',
      raw: Map<String, dynamic>.from(json),
    );
  }
}

class RhythmInputActionDynamicPattern {
  final String pattern;
  final String rustAction;
  final String paramTemplate;
  final List<String> allowedTargetGrains;
  final String dispatchPrecondition;
  final String importBehavior;
  final Map<String, dynamic> raw;

  const RhythmInputActionDynamicPattern({
    required this.pattern,
    required this.rustAction,
    required this.paramTemplate,
    this.allowedTargetGrains = const [],
    required this.dispatchPrecondition,
    required this.importBehavior,
    this.raw = const <String, dynamic>{},
  });

  factory RhythmInputActionDynamicPattern.fromJson(
    Map<String, dynamic> json,
  ) {
    return RhythmInputActionDynamicPattern(
      pattern: json['pattern']?.toString() ?? '',
      rustAction: json['rust_action']?.toString() ?? '',
      paramTemplate: json['param_template']?.toString() ?? '',
      allowedTargetGrains: _stringList(json['allowed_target_grains']),
      dispatchPrecondition: json['dispatch_precondition']?.toString() ?? '',
      importBehavior: json['import_behavior']?.toString() ?? '',
      raw: Map<String, dynamic>.from(json),
    );
  }
}

class RhythmControlPause {
  final String controlId;
  final bool active;
  final int? expiresAtEpochMs;
  final Map<String, dynamic> raw;

  const RhythmControlPause({
    required this.controlId,
    required this.active,
    this.expiresAtEpochMs,
    this.raw = const <String, dynamic>{},
  });

  factory RhythmControlPause.fromJson(Map<String, dynamic> json) {
    return RhythmControlPause(
      controlId: json['control_id']?.toString() ?? '',
      active: json['active'] == true,
      expiresAtEpochMs: jsonInt(
        json['expires_at_epoch_ms'],
        preferredKeys: const ['expires_at_epoch_ms'],
      ),
      raw: Map<String, dynamic>.from(json),
    );
  }
}

class RhythmPowerSchedules {
  final List<Map<String, dynamic>> schedules;
  final Map<String, dynamic> raw;

  const RhythmPowerSchedules({
    this.schedules = const [],
    this.raw = const <String, dynamic>{},
  });

  const RhythmPowerSchedules.empty()
      : schedules = const [],
        raw = const <String, dynamic>{};

  factory RhythmPowerSchedules.fromJson(Map<String, dynamic> json) {
    return RhythmPowerSchedules(
      schedules: ((json['schedules'] as List<dynamic>?) ?? const <dynamic>[])
          .map(jsonMap)
          .nonNulls
          .map(Map<String, dynamic>.from)
          .toList(growable: false),
      raw: Map<String, dynamic>.from(json),
    );
  }

  factory RhythmPowerSchedules.fromList(List<dynamic> schedules) {
    return RhythmPowerSchedules(
      schedules: schedules
          .map(jsonMap)
          .nonNulls
          .map(Map<String, dynamic>.from)
          .toList(growable: false),
      raw: <String, dynamic>{'schedules': schedules},
    );
  }
}

class RhythmSyncRequired {
  final String reason;
  final String? lastEventId;
  final Map<String, dynamic> raw;

  const RhythmSyncRequired({
    required this.reason,
    this.lastEventId,
    this.raw = const <String, dynamic>{},
  });

  factory RhythmSyncRequired.fromJson(Map<String, dynamic> json) {
    return RhythmSyncRequired(
      reason: json['reason']?.toString() ?? '',
      lastEventId: json['last_event_id']?.toString(),
      raw: Map<String, dynamic>.from(json),
    );
  }
}

class RhythmHistory {
  final List<RhythmActivityEvent> activities;
  final Map<String, dynamic> raw;

  const RhythmHistory({
    this.activities = const [],
    this.raw = const <String, dynamic>{},
  });

  const RhythmHistory.empty()
      : activities = const [],
        raw = const <String, dynamic>{};

  factory RhythmHistory.fromJson(Map<String, dynamic> json) {
    return RhythmHistory(
      activities: ((json['activities'] as List<dynamic>?) ?? const <dynamic>[])
          .map(jsonMap)
          .nonNulls
          .map(RhythmActivityEvent.fromJson)
          .toList(growable: false),
      raw: Map<String, dynamic>.from(json),
    );
  }
}

class RhythmActivityEvent {
  final String id;
  final String nodeId;
  final String actionId;
  final RhythmActivitySource source;
  final int epochMs;
  final RhythmValueChange? change;
  final int count;
  final String? correlationId;
  final String? fanoutOf;
  final RhythmPipelineTrace? trace;
  final Object? payload;
  final Map<String, dynamic> raw;

  const RhythmActivityEvent({
    required this.id,
    required this.nodeId,
    required this.actionId,
    required this.source,
    required this.epochMs,
    this.change,
    this.count = 1,
    this.correlationId,
    this.fanoutOf,
    this.trace,
    this.payload,
    this.raw = const <String, dynamic>{},
  });

  factory RhythmActivityEvent.fromJson(Map<String, dynamic> json) {
    final changeJson = jsonMap(json['change']);
    final traceJson = jsonMap(json['trace']);
    return RhythmActivityEvent(
      id: json['id']?.toString() ?? '',
      nodeId: json['node_id']?.toString() ?? '',
      actionId: json['action_id']?.toString() ?? '',
      source: RhythmActivitySource.fromJson(
        jsonMap(json['source']) ?? const <String, dynamic>{},
      ),
      epochMs:
          jsonInt(json['epoch_ms'], preferredKeys: const ['epoch_ms']) ?? 0,
      change:
          changeJson == null ? null : RhythmValueChange.fromJson(changeJson),
      count: jsonInt(json['count'], preferredKeys: const ['count']) ?? 1,
      correlationId: json['correlation_id']?.toString(),
      fanoutOf: json['fanout_of']?.toString(),
      trace: traceJson == null ? null : RhythmPipelineTrace.fromJson(traceJson),
      payload: json['payload'],
      raw: Map<String, dynamic>.from(json),
    );
  }
}

class RhythmActivitySource {
  final String rawSource;
  final String kind;
  final bool marksTouched;
  final String? controlId;
  final Map<String, dynamic> raw;

  const RhythmActivitySource({
    required this.rawSource,
    required this.kind,
    required this.marksTouched,
    this.controlId,
    this.raw = const <String, dynamic>{},
  });

  factory RhythmActivitySource.fromJson(Map<String, dynamic> json) {
    return RhythmActivitySource(
      rawSource: json['raw']?.toString() ?? '',
      kind: _enumValueString(json['kind']),
      marksTouched: json['marks_touched'] == true,
      controlId: json['control_id']?.toString(),
      raw: Map<String, dynamic>.from(json),
    );
  }
}

class RhythmValueChange {
  final String? axis;
  final Object? before;
  final Object? after;
  final Map<String, dynamic> raw;

  const RhythmValueChange({
    this.axis,
    this.before,
    this.after,
    this.raw = const <String, dynamic>{},
  });

  factory RhythmValueChange.fromJson(Map<String, dynamic> json) {
    return RhythmValueChange(
      axis: json['axis']?.toString(),
      before: json['before'],
      after: json['after'],
      raw: Map<String, dynamic>.from(json),
    );
  }
}

class RhythmFilterPresetDocument {
  final int schemaVersion;
  final List<Map<String, dynamic>> presets;
  final Map<String, dynamic> extra;
  final Map<String, dynamic> raw;

  const RhythmFilterPresetDocument({
    required this.schemaVersion,
    this.presets = const [],
    this.extra = const <String, dynamic>{},
    this.raw = const <String, dynamic>{},
  });

  const RhythmFilterPresetDocument.empty()
      : schemaVersion = 1,
        presets = const [],
        extra = const <String, dynamic>{},
        raw = const <String, dynamic>{};

  factory RhythmFilterPresetDocument.fromJson(Map<String, dynamic> json) {
    return RhythmFilterPresetDocument(
      schemaVersion: jsonInt(
            json['schema_version'],
            preferredKeys: const ['schema_version'],
          ) ??
          1,
      presets: ((json['presets'] as List<dynamic>?) ?? const <dynamic>[])
          .map(jsonMap)
          .nonNulls
          .map(Map<String, dynamic>.from)
          .toList(growable: false),
      extra: Map<String, dynamic>.from(
        jsonMap(json['extra']) ?? const <String, dynamic>{},
      ),
      raw: Map<String, dynamic>.from(json),
    );
  }

  Map<String, dynamic> toJson() {
    return <String, dynamic>{
      ...raw,
      'schema_version': schemaVersion,
      'presets': presets,
      if (extra.isNotEmpty) 'extra': extra,
    };
  }
}

List<RhythmRoomState> _parseNodeStates(Object? value) {
  return ((value as List<dynamic>?) ?? const <dynamic>[])
      .map(jsonMap)
      .nonNulls
      .map(RhythmRoomState.fromJson)
      .where((node) => node.nodeId.isNotEmpty)
      .toList(growable: false);
}

List<String> _stringList(Object? value) {
  return ((value as List<dynamic>?) ?? const <dynamic>[])
      .map((item) => item.toString())
      .toList(growable: false);
}

String _enumValueString(Object? value) {
  if (value is String) return value;
  if (value is Map) {
    final map = value.cast<String, dynamic>();
    final unknown = map['unknown'];
    if (unknown != null) return unknown.toString();
    if (map.length == 1) return map.keys.single;
  }
  return '';
}
