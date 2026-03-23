/// Room management provider for Rhythm Lighting.
///
/// Manages rooms across multiple sources (Hue, Home Assistant, ESP32)
/// with Rust-based state management via FFI.
library;

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';
import '../services/analytics_service.dart';
import '../services/settings_service.dart';

/// Replace rooms from a source while preserving runtime state.
///
/// Preserves:
/// - `disabled` (user preference)
/// - `rhythmEnabled`, `timeOffset`, `brightnessOffset` (ESP32-authoritative)
/// - `lightsOn` (may have been set by Hue fetch or ESP32)
/// - `curveConfig` (per-room override)
///
/// Fresh rooms provide updated metadata (name, deviceIds) from the hub.
RunnerStateDto replaceRoomsPreservingUserState(
  RunnerStateDto state, {
  required RoomSourceDto source,
  required List<RoomDto> freshRooms,
}) {
  final existing = runnerGetRoomsBySource(state: state, source: source);
  final existingById = <String, RoomDto>{
    for (final r in existing) r.id: r,
  };

  // Remove existing rooms from this source
  for (final room in existing) {
    state = runnerRemoveRoom(state: state, roomId: room.id);
  }

  // Add fresh rooms, restoring preserved state from existing
  for (final room in freshRooms) {
    final prev = existingById[room.id];
    final toAdd = prev != null
        ? RoomDto.raw(
            id: room.id,
            name: room.name,
            source: room.source,
            deviceIds: room.deviceIds,
            // Preserve runtime state from previous
            rhythmEnabled: prev.rhythmEnabled,
            disabled: prev.disabled,
            lightsOn: room.lightsOn,
            timeOffsetMinutes: prev.timeOffsetMinutes,
            brightnessOffset: prev.brightnessOffset,
            curveConfig: prev.curveConfig,
          )
        : room;
    state = runnerAddRoom(state: state, room: toAdd);
  }

  return state;
}

/// Motion timer info for a room, stored separately from RoomDto.
class MotionTimerInfo {
  final bool motionActive;
  final bool motionOwned;
  final int? remainingSecs;
  final int timeoutSecs;

  /// When this info was received — used for local countdown interpolation.
  final DateTime receivedAt;

  const MotionTimerInfo({
    required this.motionActive,
    required this.motionOwned,
    this.remainingSecs,
    required this.timeoutSecs,
    required this.receivedAt,
  });

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is MotionTimerInfo &&
          motionActive == other.motionActive &&
          motionOwned == other.motionOwned &&
          remainingSecs == other.remainingSecs &&
          timeoutSecs == other.timeoutSecs;

  @override
  int get hashCode => Object.hash(
        motionActive,
        motionOwned,
        remainingSecs,
        timeoutSecs,
      );
}

/// Manages room state across all connected hubs.
///
/// Features:
/// - Syncs rooms from Hue, Home Assistant, ESP32
/// - Persists state via SettingsService (Hive)
/// - Tracks current room index for swipeable UI
/// - Provides enabled/disabled room filtering
class RoomProvider extends ChangeNotifier {
  RunnerStateDto _state = createRunnerState();
  int _currentIndex = 0;
  bool _initialized = false;
  int _resetGeneration = 0;

  /// Monotonic counter incremented on every reset action.
  ///
  /// Widgets watching this can detect external resets (e.g. Fix My Lights)
  /// and drop stale local overrides so the curve-computed values show through.
  int get resetGeneration => _resetGeneration;

  /// Bump the reset generation counter so room cards clear stale slider state.
  void bumpResetGeneration() {
    _resetGeneration++;
    notifyListeners();
  }

  /// Per-room motion timer state from ESP32 (not part of RoomDto to avoid FRB regen).
  final Map<String, MotionTimerInfo> _motionTimers = {};

  /// Rooms that have at least one motion sensor configured (sticky).
  ///
  /// Set when a motion timer event arrives for a room; never cleared until
  /// the server connection resets. This lets the UI show a stable sensor icon
  /// even when no motion is currently active.
  final Set<String> _roomsWithSensors = {};

  /// Per-room lock timestamps to suppress stale external lightsOn overrides.
  ///
  /// When the user toggles a light locally, the room is locked for 3s so that
  /// incoming ESP32/Hue state (which may still reflect the old value) doesn't
  /// flicker the UI back.
  final Map<String, DateTime> _lightsOnLockedUntil = {};

  /// Per-room lock timestamps to suppress stale external soft_off overrides.
  ///
  /// Same pattern as [_lightsOnLockedUntil] — when the user toggles idle mode
  /// locally, the room is locked for 3s so that a stale periodic tick SSE event
  /// (still carrying the old soft_off value) doesn't immediately flip the UI.
  final Map<String, DateTime> _softOffLockedUntil = {};

  /// Per-room idle (soft-off) state. Tracked in Dart only — not in RoomDto.
  ///
  /// When idle, lights stay physically on at a very low brightness (soft-off)
  /// with rhythm disabled. This allows the lights to maintain color temperature
  /// readiness while appearing nearly off.
  final Map<String, bool> _roomIdleState = {};

  /// Per-room brightness from server (effective brightness after offsets).
  final Map<String, int> _roomBrightness = {};

  /// Per-room kelvin from server (effective color temperature after offsets).
  final Map<String, int> _roomKelvin = {};

  /// Fires after rooms from a source are added or cleared.
  final StreamController<RoomSourceDto> _sourceChangedController =
      StreamController<RoomSourceDto>.broadcast();

  /// Stream that emits after [addRoomsFromSource] or [clearRoomsBySource].
  Stream<RoomSourceDto> get onSourceRoomsChanged =>
      _sourceChangedController.stream;

  // Idle state getters
  /// Whether a room is in idle (soft-off) mode.
  bool isRoomIdle(String roomId) => _roomIdleState[roomId] ?? false;

  /// Set idle state for a room (local UI toggle).
  ///
  /// Locks the room for 3s to prevent stale SSE events from overwriting
  /// the optimistic value before the server has processed the preference.
  void setRoomIdle(String roomId, bool idle) {
    final wasIdle = _roomIdleState[roomId] ?? false;
    if (wasIdle == idle) return;
    _softOffLockedUntil[roomId] = DateTime.now().add(const Duration(seconds: 3));
    if (idle) {
      _roomIdleState[roomId] = true;
    } else {
      _roomIdleState.remove(roomId);
    }
    notifyListeners();
  }

  // Display value getters (from server)
  /// Get server-computed brightness for a room, or null if not available.
  int? getBrightness(String roomId) => _roomBrightness[roomId];

  /// Get server-computed kelvin for a room, or null if not available.
  int? getKelvin(String roomId) => _roomKelvin[roomId];

  /// Whether lights are on in a room (falls back to room.lightsOn).
  bool isLightsOn(String roomId) {
    final room = getRoom(roomId);
    return room?.lightsOn ?? false;
  }

  // Motion timer getters
  /// Get motion timer info for a room, or null if no motion tracking active.
  MotionTimerInfo? getMotionTimer(String roomId) => _motionTimers[roomId];

  /// Whether a room has a motion sensor configured (sticky).
  bool hasMotionSensor(String roomId) => _roomsWithSensors.contains(roomId);

  /// Mark a room as having a motion sensor. Only notifies if newly added.
  void markRoomHasSensor(String roomId) {
    if (_roomsWithSensors.add(roomId)) {
      notifyListeners();
    }
  }

  /// Remove rooms from [_roomsWithSensors] that are no longer reported by
  /// the server. Also clears stale motion timers for those rooms.
  void reconcileMotionSensors(Set<String> serverRoomsWithSensors) {
    final stale = _roomsWithSensors.difference(serverRoomsWithSensors);
    if (stale.isEmpty) return;
    for (final roomId in stale) {
      _roomsWithSensors.remove(roomId);
      _motionTimers.remove(roomId);
    }
    notifyListeners();
  }

  /// Update motion timer info for a room. Only notifies if values changed.
  void updateMotionTimer(String roomId, MotionTimerInfo info) {
    final existing = _motionTimers[roomId];
    if (existing == info) return;
    _motionTimers[roomId] = info;
    notifyListeners();
  }

  /// Clear motion timer for a room. Only notifies if it was present.
  void clearMotionTimer(String roomId) {
    if (_motionTimers.remove(roomId) != null) {
      notifyListeners();
    }
  }

  // Getters
  List<RoomDto> get rooms => _state.rooms;
  List<RoomDto> get enabledRooms => runnerGetEnabledRooms(state: _state);
  RoomDto? get currentRoom => rooms.isNotEmpty && _currentIndex < rooms.length
      ? rooms[_currentIndex]
      : null;
  int get currentIndex => _currentIndex;
  bool get initialized => _initialized;
  RunnerStateDto get state => _state;

  /// Initialize the provider by loading saved state.
  Future<void> initialize() async {
    if (_initialized) return;

    try {
      final loaded = SettingsService.instance.getRunnerState();
      if (loaded != null) {
        _state = loaded;
      }
    } catch (e) {
      debugPrint('Failed to load room state: $e');
    }

    _initialized = true;
    notifyListeners();
  }

  /// Save current state to persistent storage.
  Future<void> _save() async {
    try {
      await SettingsService.instance.saveRunnerState(_state);
    } catch (e) {
      debugPrint('Failed to save room state: $e');
    }
  }

  // ============================================================================
  // Room Sync
  // ============================================================================

  /// Add rooms from a specific source.
  ///
  /// Removes all existing rooms from this source first, then adds the new ones.
  /// This ensures a clean sync without duplicates.
  Future<void> addRoomsFromSource(RoomSourceDto source, List<RoomDto> rooms) async {
    _state = replaceRoomsPreservingUserState(
      _state,
      source: source,
      freshRooms: rooms,
    );

    // Reset index if needed
    if (_currentIndex >= _state.rooms.length) {
      _currentIndex = _state.rooms.isEmpty ? 0 : _state.rooms.length - 1;
    }

    await _save();
    // Update room count analytics property
    AnalyticsService().setRoomCount(roomCount);
    notifyListeners();
    _sourceChangedController.add(source);
  }

  /// Add a single room.
  Future<void> addRoom(RoomDto room) async {
    _state = runnerAddRoom(state: _state, room: room);
    await _save();
    notifyListeners();
  }

  /// Remove a room by ID.
  Future<void> removeRoom(String roomId) async {
    _state = runnerRemoveRoom(state: _state, roomId: roomId);

    // Adjust current index if needed
    if (_currentIndex >= _state.rooms.length && _state.rooms.isNotEmpty) {
      _currentIndex = _state.rooms.length - 1;
    }

    await _save();
    notifyListeners();
  }

  /// Get rooms from a specific source.
  List<RoomDto> getRoomsBySource(RoomSourceDto source) {
    return runnerGetRoomsBySource(state: _state, source: source);
  }

  /// Clear all rooms from a specific source.
  Future<void> clearRoomsBySource(RoomSourceDto source) async {
    final roomsToRemove = runnerGetRoomsBySource(state: _state, source: source);
    for (final room in roomsToRemove) {
      _state = runnerRemoveRoom(state: _state, roomId: room.id);
    }
    if (_currentIndex >= _state.rooms.length && _state.rooms.isNotEmpty) {
      _currentIndex = _state.rooms.length - 1;
    } else if (_state.rooms.isEmpty) {
      _currentIndex = 0;
    }
    await _save();
    // Update room count analytics property
    AnalyticsService().setRoomCount(roomCount);
    notifyListeners();
    _sourceChangedController.add(source);
  }

  // ============================================================================
  // Navigation
  // ============================================================================

  /// Set the current room index (for swipeable PageView).
  void setCurrentIndex(int index) {
    if (index >= 0 && index < rooms.length && index != _currentIndex) {
      _currentIndex = index;
      notifyListeners();
    }
  }

  /// Move to the next room.
  void nextRoom() {
    if (_currentIndex < rooms.length - 1) {
      _currentIndex++;
      notifyListeners();
    }
  }

  /// Move to the previous room.
  void previousRoom() {
    if (_currentIndex > 0) {
      _currentIndex--;
      notifyListeners();
    }
  }

  // ============================================================================
  // Room Control
  // ============================================================================

  /// Toggle the disabled state of a room.
  Future<void> toggleDisabled(String roomId) async {
    final room = rooms.firstWhere((r) => r.id == roomId, orElse: () => throw Exception('Room not found'));
    _state = runnerSetRoomDisabled(state: _state, roomId: roomId, disabled: !room.disabled);
    await _save();
    notifyListeners();
    _sourceChangedController.add(room.source);
  }

  /// Set the disabled state of a room.
  Future<void> setRoomDisabled(String roomId, bool disabled) async {
    final room = rooms.firstWhere((r) => r.id == roomId, orElse: () => throw Exception('Room not found'));
    _state = runnerSetRoomDisabled(state: _state, roomId: roomId, disabled: disabled);
    await _save();
    notifyListeners();
    _sourceChangedController.add(room.source);
  }

  /// Set the per-room curve configuration.
  ///
  /// Pass `null` to use the global configuration.
  Future<void> setRoomCurveConfig(String roomId, CurveConfigDto? config) async {
    _state = runnerSetRoomCurveConfig(state: _state, roomId: roomId, config: config);
    await _save();
    notifyListeners();
  }

  /// Update room devices.
  Future<void> setRoomDevices(String roomId, List<String> deviceIds) async {
    _state = runnerSetRoomDevices(state: _state, roomId: roomId, deviceIds: deviceIds);
    await _save();
    notifyListeners();
  }

  // ============================================================================
  // Room Control (Rust brain wrappers)
  // ============================================================================

  /// Apply all server-authoritative room state in a single atomic update.
  ///
  /// One [_save] + one [notifyListeners] instead of per-field updates.
  /// This prevents intermediate states from being visible to widgets.
  Future<void> applyServerRoomState(
    String roomId, {
    required bool rhythmEnabled,
    required double timeOffset,
    required double brightnessOffset,
    required bool softOff,
    bool? lightsOn,
    int? brightness,
    int? kelvin,
  }) async {
    bool changed = false;
    final room = getRoom(roomId);
    if (room == null) return;

    if (room.rhythmEnabled != rhythmEnabled) {
      _state = runnerSetRoomRhythmEnabled(state: _state, roomId: roomId, rhythmEnabled: rhythmEnabled);
      changed = true;
    }
    if (room.timeOffsetMinutes != timeOffset) {
      _state = runnerSetRoomTimeOffset(state: _state, roomId: roomId, timeOffsetMinutes: timeOffset);
      changed = true;
    }
    if (room.brightnessOffset != brightnessOffset) {
      _state = runnerSetRoomBrightnessOffset(state: _state, roomId: roomId, brightnessOffset: brightnessOffset);
      changed = true;
    }
    // Update soft_off / idle state (respecting the 3s lock for optimistic UI)
    final softOffLocked = _softOffLockedUntil[roomId];
    final softOffUnlocked = softOffLocked == null || DateTime.now().isAfter(softOffLocked);
    if (softOffUnlocked) {
      final wasIdle = _roomIdleState[roomId] ?? false;
      if (wasIdle != softOff) {
        if (softOff) {
          _roomIdleState[roomId] = true;
        } else {
          _roomIdleState.remove(roomId);
        }
        changed = true;
      }
    }
    // Update lights_on (respecting the 3s lock for optimistic UI)
    if (lightsOn != null) {
      final lockedUntil = _lightsOnLockedUntil[roomId];
      if (lockedUntil == null || DateTime.now().isAfter(lockedUntil)) {
        if (room.lightsOn != lightsOn) {
          _state = runnerSetRoomLightsOn(state: _state, roomId: roomId, lightsOn: lightsOn);
          // Only clear idle if soft_off is also unlocked — don't let a stale
          // lightsOn:false clobber an optimistic idle toggle.
          if (!lightsOn && softOffUnlocked && !softOff) _roomIdleState.remove(roomId);
          changed = true;
        }
      }
    }
    // Update display values
    if (brightness != null && _roomBrightness[roomId] != brightness) {
      _roomBrightness[roomId] = brightness;
      changed = true;
    }
    if (kelvin != null && _roomKelvin[roomId] != kelvin) {
      _roomKelvin[roomId] = kelvin;
      changed = true;
    }
    if (changed) {
      await _save();
      notifyListeners();
    }
  }

  /// Set rhythm enabled/disabled for a room.
  Future<void> setRoomRhythmEnabled(String roomId, bool enabled) async {
    _state = runnerSetRoomRhythmEnabled(state: _state, roomId: roomId, rhythmEnabled: enabled);
    await _save();
    notifyListeners();
  }

  /// Set lights_on state for a room (sync with actual device state).
  ///
  /// Skips the update if the room is currently locked by a recent
  /// [setRoomLightsOnLocal] call, preventing stale external state from
  /// overriding the user's optimistic toggle.
  Future<void> setRoomLightsOn(String roomId, bool lightsOn) async {
    final lockedUntil = _lightsOnLockedUntil[roomId];
    if (lockedUntil != null && DateTime.now().isBefore(lockedUntil)) {
      return; // suppress stale external override
    }
    // Clear idle only when lights turn off externally (e.g. physical switch).
    // When lights are on, idle state is managed by ESP32 sync / UI toggle.
    if (!lightsOn) {
      _roomIdleState.remove(roomId);
    }
    _state = runnerSetRoomLightsOn(state: _state, roomId: roomId, lightsOn: lightsOn);
    await _save();
    notifyListeners();
  }

  /// Set lights_on from a local UI toggle with a 3s lock.
  ///
  /// Locks the room so that incoming ESP32/Hue state events don't
  /// immediately overwrite the optimistic value before the bridge
  /// has processed the command.
  Future<void> setRoomLightsOnLocal(String roomId, bool lightsOn) async {
    _lightsOnLockedUntil[roomId] = DateTime.now().add(const Duration(seconds: 3));
    _state = runnerSetRoomLightsOn(state: _state, roomId: roomId, lightsOn: lightsOn);
    await _save();
    notifyListeners();
  }

  /// Set time offset for a room (from dragging the blue dot).
  Future<void> setRoomTimeOffset(String roomId, double offsetMinutes) async {
    _state = runnerSetRoomTimeOffset(state: _state, roomId: roomId, timeOffsetMinutes: offsetMinutes);
    await _save();
    notifyListeners();
  }

  /// Handle a rhythm action for any room by ID.
  ///
  /// Returns the action result with commands to execute.
  RunnerActionResultDto? handleRoomAction({
    required String roomId,
    required RhythmActionDto action,
    required CurveConfigDto config,
    required double solarNoonHour,
    required double latitude,
    required int dayOfYear,
    required double currentHour,
  }) {
    final result = runnerHandleAction(
      state: _state,
      config: config,
      solarNoonHour: solarNoonHour,
      latitude: latitude,
      dayOfYear: dayOfYear,
      currentHour: currentHour,
      roomId: roomId,
      action: action,
    );

    if (result.stateChanged) {
      _state = result.state;
      if (action == RhythmActionDto.reset) {
        _resetGeneration++;
      }
      _save();
      notifyListeners();
    }

    return result;
  }

  // ============================================================================
  // Rhythm Mode (legacy - for current room)
  // ============================================================================

  /// Handle a rhythm action for the current room.
  ///
  /// Returns the light commands to execute.
  Future<RunnerActionResultDto?> handleAction({
    required CurveConfigDto config,
    required double solarNoonHour,
    required double latitude,
    required int dayOfYear,
    required double currentHour,
    required RhythmActionDto action,
  }) async {
    final room = currentRoom;
    if (room == null) return null;

    final result = runnerHandleAction(
      state: _state,
      config: config,
      solarNoonHour: solarNoonHour,
      latitude: latitude,
      dayOfYear: dayOfYear,
      currentHour: currentHour,
      roomId: room.id,
      action: action,
    );

    if (result.stateChanged) {
      _state = result.state;
      await _save();
      notifyListeners();
    }

    return result;
  }

  // ============================================================================
  // Utilities
  // ============================================================================

  /// Get a room by ID.
  RoomDto? getRoom(String roomId) {
    return runnerGetRoom(state: _state, roomId: roomId);
  }

  /// Check if there are any rooms.
  bool get hasRooms => rooms.isNotEmpty;

  /// Get the count of rooms.
  int get roomCount => rooms.length;

  /// Get the count of enabled rooms.
  int get enabledRoomCount => enabledRooms.length;

  /// Clear transient per-room state without removing rooms.
  ///
  /// Called when switching to a different server so stale motion timers,
  /// display values, and optimistic UI locks don't bleed across servers.
  void clearTransientState() {
    _motionTimers.clear();
    _roomsWithSensors.clear();
    _lightsOnLockedUntil.clear();
    _softOffLockedUntil.clear();
    _roomIdleState.clear();
    _roomBrightness.clear();
    _roomKelvin.clear();
    notifyListeners();
  }

  @override
  void dispose() {
    _sourceChangedController.close();
    super.dispose();
  }

  /// Clear all rooms and associated transient state.
  Future<void> clearAllRooms() async {
    _state = createRunnerState();
    _currentIndex = 0;
    _motionTimers.clear();
    _roomsWithSensors.clear();
    _lightsOnLockedUntil.clear();
    _softOffLockedUntil.clear();
    _roomIdleState.clear();
    _roomBrightness.clear();
    _roomKelvin.clear();
    await _save();
    // Update room count analytics property
    AnalyticsService().setRoomCount(0);
    notifyListeners();
  }
}
