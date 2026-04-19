library;

import 'package:flutter/foundation.dart';

import '../src/rust/api/dto/curve.dart' show CurveConfigDto;

enum RoomSourceDto {
  unknown,
  matter,
  hue,
  homeAssistant,
  esp32,
}

enum RoomNodeKind {
  room,
  lightDevice,
  switchDevice,
  motionSensor,
  sensor,
  button,
  otherDevice;

  String get wireValue => switch (this) {
        RoomNodeKind.room => 'room',
        RoomNodeKind.lightDevice => 'light_device',
        RoomNodeKind.switchDevice => 'switch_device',
        RoomNodeKind.motionSensor => 'motion_sensor',
        RoomNodeKind.sensor => 'sensor',
        RoomNodeKind.button => 'button',
        RoomNodeKind.otherDevice => 'other_device',
      };

  static RoomNodeKind fromWireValue(String? value) => switch (value) {
        'light_device' => RoomNodeKind.lightDevice,
        'switch_device' => RoomNodeKind.switchDevice,
        'motion_sensor' => RoomNodeKind.motionSensor,
        'sensor' => RoomNodeKind.sensor,
        'button' => RoomNodeKind.button,
        'other_device' => RoomNodeKind.otherDevice,
        _ => RoomNodeKind.room,
      };

  bool get isRoom => this == RoomNodeKind.room;

  bool get isLightDevice => this == RoomNodeKind.lightDevice;

  bool get isLightAddressable => isRoom || isLightDevice;
}

enum RoomNodePlacement {
  hubDefault,
  userOverride,
  standalone;

  String get wireValue => switch (this) {
        RoomNodePlacement.hubDefault => 'hub_default',
        RoomNodePlacement.userOverride => 'user_override',
        RoomNodePlacement.standalone => 'standalone',
      };

  static RoomNodePlacement? fromWireValue(String? value) => switch (value) {
        'hub_default' => RoomNodePlacement.hubDefault,
        'user_override' => RoomNodePlacement.userOverride,
        'standalone' => RoomNodePlacement.standalone,
        _ => null,
      };
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
  final RoomNodeKind kind;
  final String? parentId;
  final RoomNodePlacement? placement;

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
    this.kind = RoomNodeKind.room,
    this.parentId,
    this.placement,
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
        kind,
        parentId,
        placement,
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
          curveConfig == other.curveConfig &&
          kind == other.kind &&
          parentId == other.parentId &&
          placement == other.placement;
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
  int get hashCode =>
      Object.hash(state, Object.hashAll(commands), stateChanged);

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is RunnerActionResultDto &&
          runtimeType == other.runtimeType &&
          state == other.state &&
          listEquals(commands, other.commands) &&
          stateChanged == other.stateChanged;
}
