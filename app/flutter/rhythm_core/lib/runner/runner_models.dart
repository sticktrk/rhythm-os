library;

import 'package:flutter/foundation.dart';

import '../src/rust/api/dto/curve.dart' show CurveConfigDto;

enum RoomSourceDto {
  unknown,
  hue,
  homeAssistant,
  esp32,
}

class RoomDto {
  final String id;
  final String name;
  final RoomSourceDto source;
  final List<String> deviceIds;
  final bool rhythmEnabled;
  final bool disabled;
  final bool lightsOn;
  final double timeOffsetMinutes;
  final double brightnessOffset;
  final CurveConfigDto? curveConfig;

  const RoomDto({
    required this.id,
    required this.name,
    required this.source,
    required this.deviceIds,
    required this.rhythmEnabled,
    required this.disabled,
    required this.lightsOn,
    required this.timeOffsetMinutes,
    required this.brightnessOffset,
    this.curveConfig,
  });

  @override
  int get hashCode => Object.hash(
        id,
        name,
        source,
        Object.hashAll(deviceIds),
        rhythmEnabled,
        disabled,
        lightsOn,
        timeOffsetMinutes,
        brightnessOffset,
        curveConfig,
      );

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is RoomDto &&
          runtimeType == other.runtimeType &&
          id == other.id &&
          name == other.name &&
          source == other.source &&
          listEquals(deviceIds, other.deviceIds) &&
          rhythmEnabled == other.rhythmEnabled &&
          disabled == other.disabled &&
          lightsOn == other.lightsOn &&
          timeOffsetMinutes == other.timeOffsetMinutes &&
          brightnessOffset == other.brightnessOffset &&
          curveConfig == other.curveConfig;
}

enum LightCommandType {
  turnOn,
  turnOff,
}

class LightCommandDto {
  final String deviceId;
  final String roomId;
  final LightCommandType commandType;
  final int? brightness;
  final int? kelvin;

  const LightCommandDto({
    required this.deviceId,
    required this.roomId,
    required this.commandType,
    this.brightness,
    this.kelvin,
  });

  @override
  int get hashCode =>
      Object.hash(deviceId, roomId, commandType, brightness, kelvin);

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is LightCommandDto &&
          runtimeType == other.runtimeType &&
          deviceId == other.deviceId &&
          roomId == other.roomId &&
          commandType == other.commandType &&
          brightness == other.brightness &&
          kelvin == other.kelvin;
}

class RunnerStateDto {
  final List<RoomDto> rooms;

  const RunnerStateDto({
    required this.rooms,
  });

  @override
  int get hashCode => Object.hashAll(rooms);

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is RunnerStateDto &&
          runtimeType == other.runtimeType &&
          listEquals(rooms, other.rooms);
}

class RunnerActionResultDto {
  final RunnerStateDto state;
  final List<LightCommandDto> commands;
  final bool stateChanged;

  const RunnerActionResultDto({
    required this.state,
    required this.commands,
    required this.stateChanged,
  });

  @override
  int get hashCode => Object.hash(state, Object.hashAll(commands), stateChanged);

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is RunnerActionResultDto &&
          runtimeType == other.runtimeType &&
          state == other.state &&
          listEquals(commands, other.commands) &&
          stateChanged == other.stateChanged;
}
