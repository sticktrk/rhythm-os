/// Dart-side JSON serialization for RunnerStateDto.
///
/// Replaces the hand-rolled Rust JSON serializer/parser that was in
/// runner.rs. Uses dart:convert for reliability and maintainability.
/// JSON keys use snake_case for backwards compatibility with existing
/// persisted data.
library;

import 'dart:convert';

import '../src/rust/api/dto/curve.dart';
import '../src/rust/api/dto/runner.dart';

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
  return RoomDto.raw(
    id: map['id'] as String,
    name: (map['name'] as String?) ?? (map['id'] as String),
    source: _sourceFromString(map['source'] as String? ?? 'unknown'),
    deviceIds: (map['device_ids'] as List<dynamic>?)?.cast<String>() ?? [],
    rhythmEnabled: map['rhythm_enabled'] as bool? ?? false,
    disabled: map['disabled'] as bool? ?? false,
    lightsOn: map['lights_on'] as bool? ?? false,
    timeOffsetMinutes:
        (map['time_offset_minutes'] as num?)?.toDouble() ?? 0.0,
    brightnessOffset: (map['brightness_offset'] as num?)?.toDouble() ?? 0.0,
    curveConfig: map['curve_config'] != null
        ? _curveConfigFromMap(map['curve_config'] as Map<String, dynamic>)
        : null,
  );
}

String _sourceToString(RoomSourceDto source) {
  return switch (source) {
    RoomSourceDto.hue => 'hue',
    RoomSourceDto.homeAssistant => 'home_assistant',
    RoomSourceDto.esp32 => 'esp32',
    RoomSourceDto.unknown => 'unknown',
  };
}

RoomSourceDto _sourceFromString(String s) {
  return switch (s.toLowerCase()) {
    'hue' => RoomSourceDto.hue,
    'home_assistant' || 'homeassistant' => RoomSourceDto.homeAssistant,
    'esp32' => RoomSourceDto.esp32,
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
  };
}

CurveConfigDto _curveConfigFromMap(Map<String, dynamic> map) {
  // Use CurveConfigDto.default_() for missing fields to match Rust behavior.
  final d = CurveConfigDto.default_();
  return CurveConfigDto(
    minColorTemp: (map['min_color_temp'] as num?)?.toInt() ?? d.minColorTemp,
    maxColorTemp: (map['max_color_temp'] as num?)?.toInt() ?? d.maxColorTemp,
    minBrightness:
        (map['min_brightness'] as num?)?.toInt() ?? d.minBrightness,
    maxBrightness:
        (map['max_brightness'] as num?)?.toInt() ?? d.maxBrightness,
    widthLeftBri:
        (map['width_left_bri'] as num?)?.toDouble() ?? d.widthLeftBri,
    widthRightBri:
        (map['width_right_bri'] as num?)?.toDouble() ?? d.widthRightBri,
    widthLeftCct:
        (map['width_left_cct'] as num?)?.toDouble() ?? d.widthLeftCct,
    widthRightCct:
        (map['width_right_cct'] as num?)?.toDouble() ?? d.widthRightCct,
    shapeP: (map['shape_p'] as num?)?.toDouble() ?? d.shapeP,
    maxDimSteps: (map['max_dim_steps'] as num?)?.toInt() ?? d.maxDimSteps,
  );
}
