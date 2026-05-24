/// Dart-side JSON serialization for RunnerStateDto.
///
/// Replaces the hand-rolled serializer/parser from the legacy Rust runner
/// module. Uses dart:convert for reliability and maintainability.
/// JSON keys use snake_case for backwards compatibility with existing
/// persisted data.
library;

import 'dart:convert';

import '../src/rust/api/dto/curve.dart';
import '../models/curve_defaults.dart' show curveConfigFromJson;
import 'runner_models.dart';

/// Serialize RunnerStateDto to a JSON string.
String runnerStateToJson(RunnerStateDto state) {
  return jsonEncode(_stateToMap(state));
}

/// Deserialize a JSON string to RunnerStateDto.
/// Returns null if the JSON is invalid.
RunnerStateDto? runnerStateFromJson(String json) {
  try {
    final map = jsonDecode(json) as Map<String, dynamic>;
    return _stateFromMap(map);
  } catch (_) {
    return null;
  }
}

Map<String, dynamic> _stateToMap(RunnerStateDto state) {
  return {
    'rooms': state.rooms.map(_roomToMap).toList(),
  };
}

RunnerStateDto _stateFromMap(Map<String, dynamic> map) {
  final rooms = (map['rooms'] as List<dynamic>? ?? [])
      .map((r) => _roomFromMap(r as Map<String, dynamic>))
      .toList();
  return RunnerStateDto(rooms: rooms);
}

Map<String, dynamic> _roomToMap(RoomDto room) {
  return {
    'id': room.id,
    'name': room.name,
    'source': _sourceToString(room.source),
    'kind': room.kind.wireValue,
    'parent_id': room.parentId,
    'placement': room.placement?.wireValue,
    'device_ids': room.deviceIds,
    'rhythm_enabled': room.rhythmEnabled,
    'disabled': room.disabled,
    'lights_on': room.lightsOn,
    'time_offset_minutes': room.timeOffsetMinutes,
    'brightness_offset': room.brightnessOffset,
    'curve_config':
        room.curveConfig != null ? _curveConfigToMap(room.curveConfig!) : null,
  };
}

RoomDto _roomFromMap(Map<String, dynamic> map) {
  return RoomDto(
    id: map['id'] as String,
    name: (map['name'] as String?) ?? (map['id'] as String),
    source: _sourceFromString(map['source'] as String? ?? 'unknown'),
    kind: RoomNodeKind.fromWireValue(map['kind'] as String?),
    parentId: map['parent_id'] as String?,
    placement: RoomNodePlacement.fromWireValue(map['placement'] as String?),
    deviceIds: (map['device_ids'] as List<dynamic>?)?.cast<String>() ?? [],
    rhythmEnabled: map['rhythm_enabled'] as bool? ?? false,
    disabled: map['disabled'] as bool? ?? false,
    lightsOn: map['lights_on'] as bool? ?? false,
    timeOffsetMinutes: (map['time_offset_minutes'] as num?)?.toDouble() ?? 0.0,
    brightnessOffset: (map['brightness_offset'] as num?)?.toDouble() ?? 0.0,
    curveConfig: map['curve_config'] != null
        ? _curveConfigFromMap(map['curve_config'] as Map<String, dynamic>)
        : null,
  );
}

String _sourceToString(RoomSourceDto source) {
  return switch (source) {
    RoomSourceDto.matter => 'matter',
    RoomSourceDto.hue => 'hue',
    RoomSourceDto.homeAssistant => 'home_assistant',
    RoomSourceDto.bridge => 'bridge',
    RoomSourceDto.unknown => 'unknown',
  };
}

RoomSourceDto _sourceFromString(String s) {
  return switch (s.toLowerCase()) {
    'matter' => RoomSourceDto.matter,
    'hue' => RoomSourceDto.hue,
    'home_assistant' || 'homeassistant' => RoomSourceDto.homeAssistant,
    'bridge' || 'embedded' || 'esp32' => RoomSourceDto.bridge,
    _ => RoomSourceDto.unknown,
  };
}

Map<String, dynamic> _curveConfigToMap(CurveConfigDto config) {
  return {
    'min_color_temp': config.minColorTemp,
    'max_color_temp': config.maxColorTemp,
    'min_brightness': config.minBrightness,
    'max_brightness': config.maxBrightness,
    'width_left_bri': config.widthLeftBri,
    'width_right_bri': config.widthRightBri,
    'width_left_cct': config.widthLeftCct,
    'width_right_cct': config.widthRightCct,
    'shape_p': config.shapeP,
    'max_dim_steps': config.maxDimSteps,
    'fade_ms': config.fadeMs,
    'motion_timeout_secs': config.motionTimeoutSecs,
  };
}

CurveConfigDto _curveConfigFromMap(Map<String, dynamic> map) {
  return curveConfigFromJson(map);
}
