/// Dart-side room state helpers.
///
/// These helpers replace the legacy Rust FFI runner path for local room-state
/// bookkeeping and action handling. Curve and step math still delegate to the
/// local Rust curve engine so preview/output calculations stay aligned.
library;

import '../src/rust/api/curve.dart' as curve_api;
import '../src/rust/api/dto/curve.dart' show CurveConfigDto, LightingValuesDto;
import '../src/rust/api/dto/runner.dart' show RhythmActionDto;
import 'runner_models.dart';

const _noChange = Object();

/// Create an empty local room state.
RunnerStateDto emptyRunnerState() => const RunnerStateDto(rooms: []);

/// Add a room to state, replacing any existing room with the same ID.
RunnerStateDto addRoom({
  required RunnerStateDto state,
  required RoomDto room,
}) {
  final rooms = List<RoomDto>.of(state.rooms)
    ..removeWhere((r) => r.id == room.id);
  rooms.add(room);
  return RunnerStateDto(rooms: rooms);
}

/// Remove a room from state.
RunnerStateDto removeRoom({
  required RunnerStateDto state,
  required String roomId,
}) {
  final rooms = List<RoomDto>.of(state.rooms)
    ..removeWhere((r) => r.id == roomId);
  return RunnerStateDto(rooms: rooms);
}

/// Get a room by ID.
RoomDto? roomById({
  required RunnerStateDto state,
  required String roomId,
}) {
  for (final room in state.rooms) {
    if (room.id == roomId) return room;
  }
  return null;
}

/// Get all room IDs in state.
List<String> roomIds({required RunnerStateDto state}) {
  return state.rooms.map((room) => room.id).toList(growable: false);
}

/// Get all enabled rooms.
List<RoomDto> enabledRooms({required RunnerStateDto state}) {
  return state.rooms.where((room) => !room.disabled).toList(growable: false);
}

/// Get rooms by source.
List<RoomDto> roomsBySource({
  required RunnerStateDto state,
  required RoomSourceDto source,
}) {
  return state.rooms
      .where((room) => room.source == source)
      .toList(growable: false);
}

/// Update device IDs for a room.
RunnerStateDto setRoomDevices({
  required RunnerStateDto state,
  required String roomId,
  required List<String> deviceIds,
}) {
  return _updateRoom(
    state,
    roomId,
    (room) => _copyRoom(room, deviceIds: List<String>.of(deviceIds)),
  );
}

/// Update disabled state for a room.
RunnerStateDto setRoomDisabled({
  required RunnerStateDto state,
  required String roomId,
  required bool disabled,
}) {
  return _updateRoom(
    state,
    roomId,
    (room) => _copyRoom(room, disabled: disabled),
  );
}

/// Update light power state for a room.
RunnerStateDto setRoomLightsOn({
  required RunnerStateDto state,
  required String roomId,
  required bool lightsOn,
}) {
  return _updateRoom(
    state,
    roomId,
    (room) => _copyRoom(room, lightsOn: lightsOn),
  );
}

/// Update rhythm-enabled state for a room.
RunnerStateDto setRoomRhythmEnabled({
  required RunnerStateDto state,
  required String roomId,
  required bool rhythmEnabled,
}) {
  return _updateRoom(
    state,
    roomId,
    (room) => _copyRoom(room, rhythmEnabled: rhythmEnabled),
  );
}

/// Update time offset for a room.
RunnerStateDto setRoomTimeOffset({
  required RunnerStateDto state,
  required String roomId,
  required double timeOffsetMinutes,
}) {
  return _updateRoom(
    state,
    roomId,
    (room) => _copyRoom(room, timeOffsetMinutes: timeOffsetMinutes),
  );
}

/// Update brightness offset for a room.
RunnerStateDto setRoomBrightnessOffset({
  required RunnerStateDto state,
  required String roomId,
  required double brightnessOffset,
}) {
  return _updateRoom(
    state,
    roomId,
    (room) => _copyRoom(room, brightnessOffset: brightnessOffset),
  );
}

/// Update per-room curve config.
RunnerStateDto setRoomCurveConfig({
  required RunnerStateDto state,
  required String roomId,
  required CurveConfigDto? config,
}) {
  return _updateRoom(
    state,
    roomId,
    (room) => _copyRoom(room, curveConfig: config),
  );
}

/// Process a room action using Dart-side state and Rust curve math.
RunnerActionResultDto calculateRoomActionResult({
  required RunnerStateDto state,
  required CurveConfigDto config,
  required double latitude,
  required double longitude,
  required int year,
  required int month,
  required int day,
  required String timezone,
  required double currentHour,
  required String roomId,
  required RhythmActionDto action,
}) {
  final room = roomById(state: state, roomId: roomId);
  if (room == null) {
    return RunnerActionResultDto(
      state: state,
      commands: const [],
      stateChanged: false,
    );
  }

  final outcome = _applyAction(
    room: room,
    config: config,
    latitude: latitude,
    longitude: longitude,
    year: year,
    month: month,
    day: day,
    timezone: timezone,
    currentHour: currentHour,
    action: action,
  );

  final nextState = outcome.stateChanged
      ? _updateRoom(state, roomId, (_) => outcome.room)
      : state;

  return RunnerActionResultDto(
    state: nextState,
    commands: outcome.commands,
    stateChanged: outcome.stateChanged,
  );
}

RunnerStateDto _updateRoom(
  RunnerStateDto state,
  String roomId,
  RoomDto Function(RoomDto room) update,
) {
  final index = state.rooms.indexWhere((room) => room.id == roomId);
  if (index == -1) return state;

  final rooms = List<RoomDto>.of(state.rooms);
  rooms[index] = update(rooms[index]);
  return RunnerStateDto(rooms: rooms);
}

_ActionOutcome _applyAction({
  required RoomDto room,
  required CurveConfigDto config,
  required double latitude,
  required double longitude,
  required int year,
  required int month,
  required int day,
  required String timezone,
  required double currentHour,
  required RhythmActionDto action,
}) {
  switch (action) {
    case RhythmActionDto.onPress:
      if (room.lightsOn) {
        return _ActionOutcome(
          room: _copyRoom(room, lightsOn: false),
          commands: [_turnOffCommand(room.id)],
          stateChanged: true,
        );
      }

      final lighting = _lightingAt(
        config: config,
        latitude: latitude,
        longitude: longitude,
        year: year,
        month: month,
        day: day,
        timezone: timezone,
        hour: currentHour,
      );
      return _ActionOutcome(
        room: _copyRoom(
          room,
          rhythmEnabled: true,
          lightsOn: true,
          timeOffsetMinutes: 0,
          brightnessOffset: 0,
        ),
        commands: [
          _turnOnCommand(room.id, lighting.brightness, lighting.kelvin)
        ],
        stateChanged: true,
      );

    case RhythmActionDto.offPress:
      return _ActionOutcome(
        room: _copyRoom(room, lightsOn: false),
        commands: [_turnOffCommand(room.id)],
        stateChanged: room.lightsOn,
      );

    case RhythmActionDto.reset:
      final lighting = _lightingAt(
        config: config,
        latitude: latitude,
        longitude: longitude,
        year: year,
        month: month,
        day: day,
        timezone: timezone,
        hour: currentHour,
      );
      return _ActionOutcome(
        room: _copyRoom(
          room,
          rhythmEnabled: true,
          lightsOn: true,
          timeOffsetMinutes: 0,
          brightnessOffset: 0,
        ),
        commands: [
          _turnOnCommand(room.id, lighting.brightness, lighting.kelvin)
        ],
        stateChanged: true,
      );

    case RhythmActionDto.rhythmOn:
      return _ActionOutcome(
        room: _copyRoom(room, rhythmEnabled: true),
        commands: const [],
        stateChanged: !room.rhythmEnabled,
      );

    case RhythmActionDto.rhythmOff:
      return _ActionOutcome(
        room: _copyRoom(room, rhythmEnabled: false),
        commands: const [],
        stateChanged: room.rhythmEnabled,
      );

    case RhythmActionDto.stepUp:
    case RhythmActionDto.stepDown:
      final effectiveHour =
          _wrapHour(currentHour + room.timeOffsetMinutes / 60.0);
      final sequences = curve_api.calculateStepSequencesWithSunTimes(
        config: config,
        latitude: latitude,
        longitude: longitude,
        year: year,
        month: month,
        day: day,
        timezone: timezone,
        startHour: effectiveHour,
        maxSteps: 1,
      );
      final firstStep = action == RhythmActionDto.stepUp
          ? (sequences.stepUp.isEmpty ? null : sequences.stepUp.first)
          : (sequences.stepDown.isEmpty ? null : sequences.stepDown.first);
      final deltaMinutes =
          firstStep != null ? (firstStep.hour - effectiveHour) * 60.0 : 0.0;
      final newOffset = room.timeOffsetMinutes + deltaMinutes;
      final newEffectiveHour = _wrapHour(currentHour + newOffset / 60.0);
      final lighting = _lightingAt(
        config: config,
        latitude: latitude,
        longitude: longitude,
        year: year,
        month: month,
        day: day,
        timezone: timezone,
        hour: newEffectiveHour,
      );
      return _ActionOutcome(
        room: _copyRoom(
          room,
          lightsOn: true,
          timeOffsetMinutes: newOffset,
        ),
        commands: [
          _turnOnCommand(room.id, lighting.brightness, lighting.kelvin)
        ],
        stateChanged: true,
      );

    case RhythmActionDto.dimUp:
    case RhythmActionDto.dimDown:
      final delta = action == RhythmActionDto.dimUp ? 10.0 : -10.0;
      final nextBrightnessOffset =
          (room.brightnessOffset + delta).clamp(-100.0, 100.0);
      final effectiveHour =
          _wrapHour(currentHour + room.timeOffsetMinutes / 60.0);
      final lighting = _lightingAt(
        config: config,
        latitude: latitude,
        longitude: longitude,
        year: year,
        month: month,
        day: day,
        timezone: timezone,
        hour: effectiveHour,
      );
      final adjustedBrightness = (lighting.brightness + nextBrightnessOffset)
          .clamp(1.0, 100.0)
          .toInt();
      return _ActionOutcome(
        room: _copyRoom(
          room,
          lightsOn: true,
          brightnessOffset: nextBrightnessOffset,
        ),
        commands: [
          _turnOnCommand(room.id, adjustedBrightness, lighting.kelvin)
        ],
        stateChanged: true,
      );
  }
}

LightingValuesDto _lightingAt({
  required CurveConfigDto config,
  required double latitude,
  required double longitude,
  required int year,
  required int month,
  required int day,
  required String timezone,
  required double hour,
}) {
  return curve_api.calculateLightingWithSunTimes(
    config: config,
    latitude: latitude,
    longitude: longitude,
    year: year,
    month: month,
    day: day,
    timezone: timezone,
    currentHour: _wrapHour(hour),
  );
}

double _wrapHour(double hour) => ((hour % 24.0) + 24.0) % 24.0;

LightCommandDto _turnOnCommand(String roomId, int brightness, int kelvin) {
  return LightCommandDto(
    deviceId: roomId,
    roomId: roomId,
    commandType: LightCommandType.turnOn,
    brightness: brightness,
    kelvin: kelvin,
  );
}

LightCommandDto _turnOffCommand(String roomId) {
  return LightCommandDto(
    deviceId: roomId,
    roomId: roomId,
    commandType: LightCommandType.turnOff,
  );
}

RoomDto _copyRoom(
  RoomDto room, {
  String? id,
  String? name,
  RoomSourceDto? source,
  List<String>? deviceIds,
  bool? rhythmEnabled,
  bool? disabled,
  bool? lightsOn,
  double? timeOffsetMinutes,
  double? brightnessOffset,
  RoomNodeKind? kind,
  String? parentId,
  Object? placement = _noChange,
  Object? curveConfig = _noChange,
}) {
  return RoomDto(
    id: id ?? room.id,
    name: name ?? room.name,
    source: source ?? room.source,
    deviceIds: deviceIds ?? room.deviceIds,
    rhythmEnabled: rhythmEnabled ?? room.rhythmEnabled,
    disabled: disabled ?? room.disabled,
    lightsOn: lightsOn ?? room.lightsOn,
    timeOffsetMinutes: timeOffsetMinutes ?? room.timeOffsetMinutes,
    brightnessOffset: brightnessOffset ?? room.brightnessOffset,
    kind: kind ?? room.kind,
    parentId: parentId ?? room.parentId,
    placement: identical(placement, _noChange)
        ? room.placement
        : placement as RoomNodePlacement?,
    curveConfig: identical(curveConfig, _noChange)
        ? room.curveConfig
        : curveConfig as CurveConfigDto?,
  );
}

class _ActionOutcome {
  final RoomDto room;
  final List<LightCommandDto> commands;
  final bool stateChanged;

  const _ActionOutcome({
    required this.room,
    required this.commands,
    required this.stateChanged,
  });
}
