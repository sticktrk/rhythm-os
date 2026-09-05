/// Room management provider for Rhythm Lighting.
///
/// Manages rooms across multiple sources (Hue, Home Assistant, Rhythm bridge)
/// with Dart-side room state and local Rust curve math.
library;

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_core/runner/room_state_store.dart' as room_state;
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmMode, RoomModeState;
import '../services/analytics_service.dart';
import '../services/settings_service.dart';

const _roomTransitionFallbackTimeout = Duration(seconds: 15);

/// Cap on how long one `pending_dispatch=true` assertion can show the card
/// spinner without the server re-asserting it. A missed clear (SSE event lost
/// in a connection blip, server restart mid-action) must not spin forever.
const _nodeDispatchPendingFallbackTimeout = Duration(seconds: 25);

/// Replace rooms from a source while preserving runtime state.
///
/// Preserves:
/// - `disabled` (user preference)
/// - `rhythmEnabled`, `timeOffset`, `brightnessOffset` (bridge-authoritative)
/// - `lightsOn` (replaced from server-observed power on refresh)
/// - `curveConfig` (per-room override)
///
/// Fresh rooms provide updated metadata (name, deviceIds) from the hub.
RunnerStateDto replaceRoomsPreservingUserState(
  RunnerStateDto state, {
  required RoomSourceDto source,
  required List<RoomDto> freshRooms,
}) {
  final existing = room_state.roomsBySource(state: state, source: source);
  final existingById = <String, RoomDto>{
    for (final r in existing) r.id: r,
  };

  final nextById = <String, RoomDto>{
    for (final room in state.rooms)
      if (room.source != source) room.id: room,
  };
  // Add fresh rooms, restoring preserved state from existing
  for (final room in freshRooms) {
    final prev = existingById[room.id];
    final toAdd = prev != null
        ? RoomDto(
            id: room.id,
            name: room.name,
            source: room.source,
            kind: room.kind,
            parentId: room.parentId,
            placement: room.placement,
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
    nextById[toAdd.id] = toAdd;
  }

  state = RunnerStateDto(rooms: nextById.values.toList(growable: false));

  return state;
}

/// Motion timer info for a room, stored separately from RoomDto.
class MotionTimerInfo {
  final bool motionActive;
  final bool motionOwned;
  final int? remainingSecs;
  final int timeoutSecs;
  final bool warningActive;

  /// When this info was received — used for local countdown interpolation.
  final DateTime receivedAt;

  const MotionTimerInfo({
    required this.motionActive,
    required this.motionOwned,
    this.remainingSecs,
    required this.timeoutSecs,
    this.warningActive = false,
    required this.receivedAt,
  });

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is MotionTimerInfo &&
          motionActive == other.motionActive &&
          motionOwned == other.motionOwned &&
          remainingSecs == other.remainingSecs &&
          timeoutSecs == other.timeoutSecs &&
          warningActive == other.warningActive &&
          receivedAt == other.receivedAt;

  @override
  int get hashCode => Object.hash(
        motionActive,
        motionOwned,
        remainingSecs,
        timeoutSecs,
        warningActive,
        receivedAt,
      );
}

/// Manages room state across all connected hubs.
///
/// Features:
/// - Syncs rooms from Hue, Home Assistant, Rhythm bridge
/// - Persists state via SettingsService (Hive)
/// - Tracks current room index for swipeable UI
/// - Provides enabled/disabled room filtering
class RoomProvider extends ChangeNotifier {
  RunnerStateDto _state = room_state.emptyRunnerState();
  RunnerStateDto? _indexedState;
  Map<String, RoomDto> _nodeIndex = {};
  Map<String, RoomDto>? _snapshotNodes;
  bool _snapshotChanged = false;

  @override
  void notifyListeners() {
    if (_snapshotNodes != null) {
      _snapshotChanged = true;
      return;
    }
    super.notifyListeners();
  }

  /// Apply membership and all live state synchronously, then publish once.
  /// The callback must only reconcile runtime state; it must not await I/O.
  void applyServerSnapshot(
      List<RoomDto> nodes, void Function() applyRuntimeState) {
    assert(_snapshotNodes == null);
    final previous = {for (final node in _state.rooms) node.id: node};
    final next = <String, RoomDto>{};
    for (final node in nodes) {
      final old = previous[node.id];
      next[node.id] = RoomDto(
        id: node.id, name: node.name, source: node.source, kind: node.kind,
        parentId: node.parentId, placement: node.placement,
        deviceIds: node.deviceIds, rhythmEnabled: node.rhythmEnabled,
        disabled: old?.disabled ?? node.disabled,
        // The runtime update below applies power through the optimistic locks.
        lightsOn: old?.lightsOn ?? node.lightsOn,
        timeOffsetMinutes: node.timeOffsetMinutes,
        brightnessOffset: node.brightnessOffset,
        curveConfig: old?.curveConfig ?? node.curveConfig,
      );
    }
    _snapshotNodes = next;
    _snapshotChanged = false;
    try {
      for (final id in previous.keys) {
        if (!next.containsKey(id)) _forgetNode(id);
      }
      applyRuntimeState();
    } finally {
      final nextRooms = next.values.toList(growable: false);
      final persistedChanged = !listEquals(_state.rooms, nextRooms);
      _state = RunnerStateDto(rooms: nextRooms);
      _snapshotNodes = null;
      if (_currentIndex >= nextRooms.length) {
        _currentIndex = nextRooms.isEmpty ? 0 : nextRooms.length - 1;
      }
      if (persistedChanged || SettingsService.instance.runnerStateSaveFailed) {
        unawaited(_save());
      }
      if (persistedChanged || _snapshotChanged) notifyListeners();
      if (previous.length != next.length) {
        unawaited(AnalyticsService().setRoomCount(next.length));
      }
    }
  }

  void _forgetNode(String id) {
    _clearRoomTransitioningState(id);
    _lockExpiryTimers.remove(id)?.cancel();
    _lightsOnLockedUntil.remove(id);
    _roomStateLockedUntil.remove(id);
    _suppressedRoomStates.remove(id);
    _suppressedLightsOn.remove(id);
    _acknowledgedRoomStates.remove(id);
    _acknowledgedLightsOn.remove(id);
    _motionTimers.remove(id);
    _roomsWithSensors.remove(id);
    _roomStates.remove(id);
    _roomModes.remove(id);
    _roomBrightness.remove(id);
    _roomKelvin.remove(id);
    _roomColor.remove(id);
    _roomMoodColor.remove(id);
    _roomMoodBrightness.remove(id);
    _roomMoodEnabled.remove(id);
    _roomMoodActive.remove(id);
    _lastTickTime.remove(id);
  }

  int _currentIndex = 0;
  bool _initialized = false;
  int _resetGeneration = 0;

  /// Monotonic counter incremented on every reset action.
  ///
  /// Widgets watching this can detect reset actions and drop stale local
  /// overrides so the curve-computed values show through.
  int get resetGeneration => _resetGeneration;

  /// Bump the reset generation counter so room cards clear stale slider state.
  void bumpResetGeneration() {
    _resetGeneration++;
    notifyListeners();
  }

  /// Per-room motion timer state from the Rhythm bridge (not part of RoomDto to avoid FRB regen).
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
  /// incoming hub state (which may still reflect the old value) doesn't
  /// flicker the UI back.
  final Map<String, DateTime> _lightsOnLockedUntil = {};

  /// Per-room lock timestamps to suppress stale external room-state overrides.
  ///
  /// Same pattern as [_lightsOnLockedUntil] — when the user toggles idle or
  /// hard-off locally, the room is locked for 3s so stale server state does
  /// not immediately flip the UI back.
  final Map<String, DateTime> _roomStateLockedUntil = {};

  /// Server values suppressed by an active optimistic lock.
  ///
  /// Re-applied when the lock expires: if the hub dropped the command, no
  /// further server event arrives (nothing changed server-side), so without
  /// this the optimistic value would stay on screen indefinitely.
  final Map<String, RoomModeState> _suppressedRoomStates = {};
  final Map<String, bool> _suppressedLightsOn = {};
  final Map<String, Timer> _lockExpiryTimers = {};
  final Map<String, RoomModeState> _acknowledgedRoomStates = {};
  final Map<String, bool> _acknowledgedLightsOn = {};

  /// Latest server-reported power, separate from optimistic local locks.
  ///
  /// A rejected command can leave [RoomDto.lightsOn] temporarily protected by
  /// its optimistic lock while the server has already restored `hardOff`.
  /// Only a matching server report may therefore turn a raw hard-off state
  /// into an observed-on display.
  final Map<String, bool> _serverReportedLightsOn = {};

  /// Per-room mode state from the server, tracked separately from RoomDto.
  final Map<String, RoomModeState> _roomStates = {};

  /// Per-room live mode from SSE (`day` / `sleep`).
  final Map<String, RhythmMode> _roomModes = {};

  /// Per-room global mode-transition flag from the server.
  final Map<String, bool> _roomTransitioning = {};
  final Map<String, Timer> _roomTransitionTimers = {};

  /// Per-room command dispatch flag from the server.
  final Map<String, bool> _nodeDispatchPending = {};
  final Map<String, Timer> _nodeDispatchPendingTimers = {};

  /// Per-room brightness from server (effective brightness after offsets).
  final Map<String, int> _roomBrightness = {};

  /// Per-room kelvin from server (effective color temperature after offsets).
  final Map<String, int> _roomKelvin = {};

  /// Per-room direct color from server (for direct-color profiles like idle).
  final Map<String, (int r, int g, int b)> _roomColor = {};

  /// Last known Mood color for each room.
  ///
  /// This is separate from [_roomColor] because the live direct color is
  /// cleared when a room returns to Kelvin/CCT mode, while Mood should still
  /// remember the last selected color when the user comes back to it.
  final Map<String, (int r, int g, int b)> _roomMoodColor = {};

  /// Last known Mood brightness for each room.
  ///
  /// This is separate from [_roomBrightness] because the generic brightness
  /// cache can hold the active-mode runtime brightness.
  final Map<String, int> _roomMoodBrightness = {};

  /// Whether Mood lighting is enabled for this room.
  final Map<String, bool> _roomMoodEnabled = {};

  /// Whether this room is currently in its Mood state.
  final Map<String, bool> _roomMoodActive = {};

  /// Per-room timestamp of the last rhythm tick from the server.
  final Map<String, DateTime> _lastTickTime = {};

  /// Fires after rooms from a source are added or cleared.
  final StreamController<RoomSourceDto> _sourceChangedController =
      StreamController<RoomSourceDto>.broadcast();

  /// Stream that emits after [addRoomsFromSource] or [clearRoomsBySource].
  Stream<RoomSourceDto> get onSourceRoomsChanged =>
      _sourceChangedController.stream;

  /// Raw room mode state from the server/local controls.
  ///
  /// This represents Rhythm's automation intent (`active`, `mood`, `standby`,
  /// `hardOff`, etc.) and may remain `active` even when the physical lights
  /// are currently off due to an external wall switch, dimmer, or hub action.
  ///
  /// Until the first authoritative server state arrives, default to `active`
  /// rather than inferring semantics from observed power alone.
  RoomModeState getRoomState(String roomId) {
    final state = _roomStates[roomId];
    if (state != null) return state;
    return RoomModeState.active;
  }

  /// Visual room state for cards and other UI that reflects actual power.
  ///
  /// The backend can legitimately report `state=active` while `lightsOn=false`
  /// when a room is configured to participate in Rhythm but was turned off
  /// outside the app. It can likewise retain `state=hardOff` after a physical
  /// integration reports the light on. For "is this room on right now?" UI,
  /// `lightsOn` wins in both directions while [getRoomState] preserves the
  /// automation intent.
  RoomModeState getDisplayRoomState(String roomId) {
    final state = getRoomState(roomId);
    final room = getRoom(roomId);
    if (room == null) return state;
    if (!room.lightsOn) return RoomModeState.hardOff;
    if (state == RoomModeState.hardOff &&
        _serverReportedLightsOn[roomId] == true) {
      return RoomModeState.active;
    }
    return state;
  }

  /// Whether a room is in Standby mode.
  bool isRoomIdle(String roomId) {
    final state = getRoomState(roomId);
    return state == RoomModeState.standby || state == RoomModeState.idle;
  }

  /// Whether a room has per-room Mood lighting enabled.
  bool isMoodEnabled(String roomId) => _roomMoodEnabled[roomId] ?? false;

  /// Whether a room is actively showing Mood lighting.
  bool isMoodActive(String roomId) =>
      _roomMoodActive[roomId] ?? getRoomState(roomId) == RoomModeState.mood;

  /// Latest live mode for a room from SSE, when available.
  RhythmMode? getRoomMode(String roomId) => _roomModes[roomId];

  /// Whether the server reports an active mode-transition fade for a room.
  bool isRoomTransitioning(String roomId) =>
      _roomTransitioning[roomId] ?? false;

  /// Whether the server still has queued or in-flight work for this node.
  bool isNodeDispatchPending(String nodeId) =>
      _nodeDispatchPending[nodeId] ?? false;

  /// Whether any current room is inside a server-side mode transition.
  bool get anyRoomTransitioning =>
      rooms.any((room) => _roomTransitioning[room.id] ?? false);

  bool _setNodeDispatchPending(String nodeId, bool pending) {
    _nodeDispatchPendingTimers.remove(nodeId)?.cancel();

    final current = _nodeDispatchPending[nodeId] ?? false;
    if (pending) {
      _nodeDispatchPending[nodeId] = true;
      _nodeDispatchPendingTimers[nodeId] =
          Timer(_nodeDispatchPendingFallbackTimeout, () {
        _nodeDispatchPendingTimers.remove(nodeId);
        if (_nodeDispatchPending.remove(nodeId) != null) {
          notifyListeners();
        }
      });
      return !current;
    }
    if (!current) {
      _nodeDispatchPending.remove(nodeId);
      return false;
    }
    _nodeDispatchPending.remove(nodeId);
    return true;
  }

  void _cancelNodeDispatchPendingTimers() {
    for (final timer in _nodeDispatchPendingTimers.values) {
      timer.cancel();
    }
    _nodeDispatchPendingTimers.clear();
  }

  bool _setRoomTransitioning(
    String roomId,
    bool transitioning, {
    Duration timeout = _roomTransitionFallbackTimeout,
  }) {
    _roomTransitionTimers.remove(roomId)?.cancel();

    final current = _roomTransitioning[roomId] ?? false;
    if (transitioning) {
      _roomTransitioning[roomId] = true;
      _roomTransitionTimers[roomId] = Timer(timeout, () {
        if ((_roomTransitioning[roomId] ?? false) == false) return;
        _roomTransitioning.remove(roomId);
        _roomTransitionTimers.remove(roomId);
        notifyListeners();
      });
      return !current;
    }

    if (!current) {
      _roomTransitioning.remove(roomId);
      return false;
    }
    _roomTransitioning.remove(roomId);
    return true;
  }

  void _cancelRoomTransitionTimers() {
    for (final timer in _roomTransitionTimers.values) {
      timer.cancel();
    }
    _roomTransitionTimers.clear();
  }

  void _clearRoomTransitioningState(String roomId) {
    _roomTransitionTimers.remove(roomId)?.cancel();
    _roomTransitioning.remove(roomId);
    _nodeDispatchPendingTimers.remove(roomId)?.cancel();
    _nodeDispatchPending.remove(roomId);
    _serverReportedLightsOn.remove(roomId);
  }

  /// Set room state locally with a 3s optimistic lock.
  void setRoomStateLocal(String roomId, RoomModeState state) {
    final previous = getRoomState(roomId);
    if (previous == state) return;
    _roomStateLockedUntil[roomId] =
        DateTime.now().add(const Duration(seconds: 3));
    // A fresh optimistic action supersedes whatever the previous lock
    // suppressed — re-applying it now would fight this toggle.
    _suppressedRoomStates.remove(roomId);
    _acknowledgedRoomStates.remove(roomId);
    _roomStates[roomId] = state;
    notifyListeners();
  }

  /// Mark an optimistic local command as accepted by the server.
  ///
  /// Server responses can still carry stale observed-power snapshots while the
  /// physical hub command is queued. Once the write is acknowledged, those
  /// suppressed stale values should not be replayed when the short UI lock
  /// expires; a later unlocked server observation can still correct the UI.
  void acknowledgeOptimisticNodeState(
    String nodeId, {
    RoomModeState? state,
    bool? lightsOn,
  }) {
    if (state != null) {
      _acknowledgedRoomStates[nodeId] = state;
      if (_roomStates[nodeId] == state &&
          _suppressedRoomStates[nodeId] != state) {
        _suppressedRoomStates.remove(nodeId);
      }
    }

    if (lightsOn != null) {
      _acknowledgedLightsOn[nodeId] = lightsOn;
      final room = getRoom(nodeId);
      if (room != null &&
          room.lightsOn == lightsOn &&
          _suppressedLightsOn[nodeId] != lightsOn) {
        _suppressedLightsOn.remove(nodeId);
      }
    }
  }

  /// Restore the presentation captured before a rejected optimistic command.
  ///
  /// Unlike an ordinary local setter, this clears the optimistic locks so a
  /// server refusal cannot leave the failed state protected as acknowledged.
  Future<void> restoreRejectedOptimisticNodeState(
    String nodeId, {
    required bool rhythmEnabled,
    required RoomModeState state,
    required bool lightsOn,
    required bool moodEnabled,
    required bool moodActive,
  }) async {
    _lockExpiryTimers.remove(nodeId)?.cancel();
    _roomStateLockedUntil.remove(nodeId);
    _lightsOnLockedUntil.remove(nodeId);
    _suppressedRoomStates.remove(nodeId);
    _suppressedLightsOn.remove(nodeId);
    _acknowledgedRoomStates.remove(nodeId);
    _acknowledgedLightsOn.remove(nodeId);
    _roomStates[nodeId] = state;
    _roomMoodEnabled[nodeId] = moodEnabled;
    _roomMoodActive[nodeId] = moodActive;
    _state = room_state.setRoomRhythmEnabled(
      state: _state,
      roomId: nodeId,
      rhythmEnabled: rhythmEnabled,
    );
    _state = room_state.setRoomLightsOn(
      state: _state,
      roomId: nodeId,
      lightsOn: lightsOn,
    );
    await _save();
    notifyListeners();
  }

  /// Re-apply server values a lock suppressed once that lock expires.
  ///
  /// Fires slightly after the latest known expiry; each value re-checks its
  /// own lock so a re-lock in the meantime wins.
  void _armLockExpiryTimer(String roomId, DateTime lockedUntil) {
    final delay = lockedUntil.difference(DateTime.now()) +
        const Duration(milliseconds: 50);
    _lockExpiryTimers[roomId]?.cancel();
    _lockExpiryTimers[roomId] =
        Timer(delay.isNegative ? Duration.zero : delay, () {
      _lockExpiryTimers.remove(roomId);
      var changed = false;
      var persist = false;

      final suppressedState = _suppressedRoomStates.remove(roomId);
      if (suppressedState != null) {
        final lock = _roomStateLockedUntil[roomId];
        final acknowledgedState = _acknowledgedRoomStates[roomId];
        final shouldKeepAcknowledgedState = acknowledgedState != null &&
            _roomStates[roomId] == acknowledgedState &&
            suppressedState != acknowledgedState;
        if (!shouldKeepAcknowledgedState &&
            (lock == null || DateTime.now().isAfter(lock)) &&
            _roomStates[roomId] != suppressedState) {
          _roomStates[roomId] = suppressedState;
          changed = true;
        }
      }

      final suppressedLights = _suppressedLightsOn.remove(roomId);
      if (suppressedLights != null) {
        final lock = _lightsOnLockedUntil[roomId];
        final room = getRoom(roomId);
        final acknowledgedLightsOn = _acknowledgedLightsOn[roomId];
        final shouldKeepAcknowledgedLights = acknowledgedLightsOn != null &&
            room != null &&
            room.lightsOn == acknowledgedLightsOn &&
            suppressedLights != acknowledgedLightsOn;
        if (!shouldKeepAcknowledgedLights &&
            (lock == null || DateTime.now().isAfter(lock)) &&
            room != null &&
            room.lightsOn != suppressedLights) {
          _state = room_state.setRoomLightsOn(
              state: _state, roomId: roomId, lightsOn: suppressedLights);
          changed = true;
          persist = true;
        }
      }

      if (persist) unawaited(_save());
      if (changed) notifyListeners();
    });
  }

  void _cancelLockExpiryTimers() {
    for (final timer in _lockExpiryTimers.values) {
      timer.cancel();
    }
    _lockExpiryTimers.clear();
    _suppressedRoomStates.clear();
    _suppressedLightsOn.clear();
    _acknowledgedRoomStates.clear();
    _acknowledgedLightsOn.clear();
  }

  // Display value getters (from server)
  /// Get server-computed brightness for a room, or null if not available.
  int? getBrightness(String roomId) => _roomBrightness[roomId];

  /// Get server-computed kelvin for a room, or null if not available.
  int? getKelvin(String roomId) => _roomKelvin[roomId];

  /// Get direct color for a room (from direct-color profiles), or null.
  (int r, int g, int b)? getRoomColor(String roomId) => _roomColor[roomId];

  /// Get the last known Mood color for a room, or null.
  (int r, int g, int b)? getMoodColor(String roomId) => _roomMoodColor[roomId];

  /// Get the last known Mood brightness for a room, or null.
  int? getMoodBrightness(String roomId) => _roomMoodBrightness[roomId];

  /// Update cached Mood color from the server's persisted mood profile.
  void setMoodColorFromServer(String roomId, (int r, int g, int b)? color) {
    var changed = false;
    if (color == null) {
      changed = _roomMoodColor.remove(roomId) != null;
    } else if (_roomMoodColor[roomId] != color) {
      _roomMoodColor[roomId] = color;
      changed = true;
    }
    if (changed) notifyListeners();
  }

  /// Update cached Mood brightness from the server's persisted mood profile.
  void setMoodBrightnessFromServer(String roomId, int? brightness) {
    var changed = false;
    if (brightness == null) {
      changed = _roomMoodBrightness.remove(roomId) != null;
    } else {
      final clamped = brightness.clamp(1, 100).toInt();
      if (_roomMoodBrightness[roomId] != clamped) {
        _roomMoodBrightness[roomId] = clamped;
        changed = true;
      }
    }
    if (changed) notifyListeners();
  }

  /// Set Mood brightness locally with a 3s optimistic lock.
  void setMoodBrightnessLocal(String roomId, int brightness) {
    final clamped = brightness.clamp(1, 100).toInt();
    if (_roomMoodBrightness[roomId] == clamped) return;
    _roomMoodBrightness[roomId] = clamped;
    _roomStateLockedUntil[roomId] =
        DateTime.now().add(const Duration(seconds: 3));
    notifyListeners();
  }

  /// Set room color locally with a 3s optimistic lock.
  void setRoomColorLocal(
    String roomId,
    int r,
    int g,
    int b, {
    bool rememberAsMood = false,
  }) {
    _roomColor[roomId] = (r, g, b);
    if (rememberAsMood) {
      _roomMoodColor[roomId] = (r, g, b);
    }
    _roomStateLockedUntil[roomId] =
        DateTime.now().add(const Duration(seconds: 3));
    notifyListeners();
  }

  /// Set Mood enablement locally for optimistic UI.
  void setMoodEnabledLocal(String roomId, bool enabled) {
    if (_roomMoodEnabled[roomId] == enabled) return;
    _roomMoodEnabled[roomId] = enabled;
    notifyListeners();
  }

  /// Restore the local room presentation after a rejected optimistic scene.
  void restoreMoodPresentationLocal(
    String roomId, {
    required (int r, int g, int b)? roomColor,
    required (int r, int g, int b)? moodColor,
    required int? moodBrightness,
    required bool moodEnabled,
  }) {
    if (roomColor == null) {
      _roomColor.remove(roomId);
    } else {
      _roomColor[roomId] = roomColor;
    }
    if (moodColor == null) {
      _roomMoodColor.remove(roomId);
    } else {
      _roomMoodColor[roomId] = moodColor;
    }
    if (moodBrightness == null) {
      _roomMoodBrightness.remove(roomId);
    } else {
      _roomMoodBrightness[roomId] = moodBrightness;
    }
    _roomMoodEnabled[roomId] = moodEnabled;
    _roomStateLockedUntil.remove(roomId);
    notifyListeners();
  }

  /// Get the timestamp of the last rhythm tick for a room.
  DateTime? getLastTickTime(String roomId) => _lastTickTime[roomId];

  /// Set the last tick time for a room (bootstrap from server hello).
  void setLastTickTime(String roomId, DateTime time) {
    _lastTickTime[roomId] = time;
  }

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

  void markNodeHasSensor(String nodeId) => markRoomHasSensor(nodeId);

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

  /// Replace the known motion-target node set with a fresh authoritative set.
  ///
  /// This is used during hello/topology refresh when the backend graph is the
  /// source of truth for automation wiring.
  void setMotionSensorNodes(Set<String> nodeIds) {
    final stale = _roomsWithSensors.difference(nodeIds);
    final added = nodeIds.difference(_roomsWithSensors);
    if (stale.isEmpty && added.isEmpty) return;

    for (final nodeId in stale) {
      _roomsWithSensors.remove(nodeId);
      _motionTimers.remove(nodeId);
    }
    _roomsWithSensors.addAll(added);
    notifyListeners();
  }

  /// Update motion timer info for a room. Only notifies if values changed.
  void updateMotionTimer(String roomId, MotionTimerInfo info) {
    final existing = _motionTimers[roomId];
    if (existing == info) return;
    _motionTimers[roomId] = info;
    notifyListeners();
  }

  void updateNodeMotionTimer(String nodeId, MotionTimerInfo info) =>
      updateMotionTimer(nodeId, info);

  /// Clear motion timer for a room. Only notifies if it was present.
  void clearMotionTimer(String roomId) {
    if (_motionTimers.remove(roomId) != null) {
      notifyListeners();
    }
  }

  void clearNodeMotionTimer(String nodeId) => clearMotionTimer(nodeId);

  // Getters
  List<RoomDto> get rooms => _state.rooms;
  List<RoomDto> get enabledRooms => room_state.enabledRooms(state: _state);
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
  Future<void> addRoomsFromSource(
      RoomSourceDto source, List<RoomDto> rooms) async {
    final previousIds = room_state
        .roomsBySource(state: _state, source: source)
        .map((r) => r.id);
    final nextIds = rooms.map((r) => r.id).toSet();
    for (final roomId in previousIds) {
      if (!nextIds.contains(roomId)) {
        _clearRoomTransitioningState(roomId);
      }
    }

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
    _state = room_state.addRoom(state: _state, room: room);
    await _save();
    notifyListeners();
  }

  /// Remove a room by ID.
  Future<void> removeRoom(String roomId) async {
    _state = room_state.removeRoom(state: _state, roomId: roomId);
    _clearRoomTransitioningState(roomId);

    // Adjust current index if needed
    if (_currentIndex >= _state.rooms.length && _state.rooms.isNotEmpty) {
      _currentIndex = _state.rooms.length - 1;
    }

    await _save();
    notifyListeners();
  }

  /// Get rooms from a specific source.
  List<RoomDto> getRoomsBySource(RoomSourceDto source) {
    return room_state.roomsBySource(state: _state, source: source);
  }

  /// Clear all rooms from a specific source.
  Future<void> clearRoomsBySource(RoomSourceDto source) async {
    final roomsToRemove =
        room_state.roomsBySource(state: _state, source: source);
    for (final room in roomsToRemove) {
      _state = room_state.removeRoom(state: _state, roomId: room.id);
      _clearRoomTransitioningState(room.id);
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
    final room = rooms.firstWhere((r) => r.id == roomId,
        orElse: () => throw Exception('Room not found'));
    _state = room_state.setRoomDisabled(
        state: _state, roomId: roomId, disabled: !room.disabled);
    await _save();
    notifyListeners();
    _sourceChangedController.add(room.source);
  }

  /// Set the disabled state of a room.
  Future<void> setRoomDisabled(String roomId, bool disabled) async {
    final room = rooms.firstWhere((r) => r.id == roomId,
        orElse: () => throw Exception('Room not found'));
    _state = room_state.setRoomDisabled(
        state: _state, roomId: roomId, disabled: disabled);
    await _save();
    notifyListeners();
    _sourceChangedController.add(room.source);
  }

  /// Set the per-room curve configuration.
  ///
  /// Pass `null` to use the global configuration.
  Future<void> setRoomCurveConfig(String roomId, CurveConfigDto? config) async {
    _state = room_state.setRoomCurveConfig(
        state: _state, roomId: roomId, config: config);
    await _save();
    notifyListeners();
  }

  /// Update room devices.
  Future<void> setRoomDevices(String roomId, List<String> deviceIds) async {
    _state = room_state.setRoomDevices(
        state: _state, roomId: roomId, deviceIds: deviceIds);
    await _save();
    notifyListeners();
  }

  // ============================================================================
  // Room Control
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
    required RoomModeState state,
    bool transitioning = false,
    bool pendingDispatch = false,
    RhythmMode? mode,
    bool? lightsOn,
    int? brightness,
    int? kelvin,
    (int r, int g, int b)? color,
    bool? moodEnabled,
    bool? moodActive,
    bool tick = false,
  }) async {
    bool changed = false;
    final room = getRoom(roomId);
    if (room == null) return;

    var nextLightsOn = room.lightsOn;
    if (lightsOn != null) {
      _serverReportedLightsOn[roomId] = lightsOn;
    }

    if (tick) {
      _lastTickTime[roomId] = DateTime.now();
      changed = true;
    }

    if (room.rhythmEnabled != rhythmEnabled) {
      changed = true;
    }
    if (room.timeOffsetMinutes != timeOffset) {
      changed = true;
    }
    if (room.brightnessOffset != brightnessOffset) {
      changed = true;
    }
    final roomStateLocked = _roomStateLockedUntil[roomId];
    final roomStateUnlocked =
        roomStateLocked == null || DateTime.now().isAfter(roomStateLocked);
    final acknowledgedState = _acknowledgedRoomStates[roomId];
    final pendingProtectsState = pendingDispatch &&
        acknowledgedState != null &&
        _roomStates[roomId] == acknowledgedState &&
        state != acknowledgedState;
    if (pendingProtectsState) {
      // A pending command can outlive the short optimistic lock. Do not let a
      // pre-command snapshot flip the card while physical delivery is still
      // unresolved; the pending=false outcome remains authoritative.
      _suppressedRoomStates.remove(roomId);
    } else if (roomStateUnlocked) {
      _suppressedRoomStates.remove(roomId);
      _acknowledgedRoomStates.remove(roomId);
      if (_roomStates[roomId] != state) {
        _roomStates[roomId] = state;
        changed = true;
      }
    } else if (_roomStates[roomId] != state) {
      // Locked: remember the server value and re-apply it on lock expiry.
      _suppressedRoomStates[roomId] = state;
      _armLockExpiryTimer(roomId, roomStateLocked);
    } else {
      _suppressedRoomStates.remove(roomId);
    }
    if (_setRoomTransitioning(roomId, transitioning)) {
      changed = true;
    }
    if (_setNodeDispatchPending(roomId, pendingDispatch)) {
      changed = true;
    }
    if (mode != null && _roomModes[roomId] != mode) {
      _roomModes[roomId] = mode;
      changed = true;
    }
    // Update observed lights_on (respecting the 3s lock for optimistic UI).
    if (lightsOn != null) {
      final lockedUntil = _lightsOnLockedUntil[roomId];
      final acknowledgedLightsOn = _acknowledgedLightsOn[roomId];
      final pendingProtectsLights = pendingDispatch &&
          acknowledgedLightsOn != null &&
          room.lightsOn == acknowledgedLightsOn &&
          lightsOn != acknowledgedLightsOn;
      if (pendingProtectsLights) {
        _suppressedLightsOn.remove(roomId);
      } else if (lockedUntil == null || DateTime.now().isAfter(lockedUntil)) {
        _suppressedLightsOn.remove(roomId);
        _acknowledgedLightsOn.remove(roomId);
        if (room.lightsOn != lightsOn) {
          nextLightsOn = lightsOn;
          changed = true;
        }
      } else if (room.lightsOn != lightsOn) {
        // Locked: remember the server value and re-apply it on lock expiry.
        _suppressedLightsOn[roomId] = lightsOn;
        _armLockExpiryTimer(roomId, lockedUntil);
      } else {
        _suppressedLightsOn.remove(roomId);
      }
    }
    // Update display values
    if (brightness != null && _roomBrightness[roomId] != brightness) {
      _roomBrightness[roomId] = brightness;
      changed = true;
    }
    if (brightness != null &&
        (state == RoomModeState.mood || moodActive == true)) {
      final clamped = brightness.clamp(1, 100).toInt();
      if (_roomMoodBrightness[roomId] != clamped) {
        _roomMoodBrightness[roomId] = clamped;
        changed = true;
      }
    }
    if (kelvin != null && _roomKelvin[roomId] != kelvin) {
      _roomKelvin[roomId] = kelvin;
      changed = true;
    }
    if (color != null) {
      _roomColor[roomId] = color;
      if (state == RoomModeState.mood || moodActive == true) {
        _roomMoodColor[roomId] = color;
      }
      changed = true;
    } else if (kelvin != null && kelvin > 0) {
      // Clear direct color when receiving a real kelvin value.
      if (_roomColor.remove(roomId) != null) changed = true;
    }
    if (moodEnabled != null && _roomMoodEnabled[roomId] != moodEnabled) {
      _roomMoodEnabled[roomId] = moodEnabled;
      changed = true;
    }
    if (moodActive != null && _roomMoodActive[roomId] != moodActive) {
      _roomMoodActive[roomId] = moodActive;
      changed = true;
    }
    final updated = RoomDto(
      id: room.id,
      name: room.name,
      source: room.source,
      kind: room.kind,
      parentId: room.parentId,
      placement: room.placement,
      deviceIds: room.deviceIds,
      disabled: room.disabled,
      curveConfig: room.curveConfig,
      rhythmEnabled: rhythmEnabled,
      timeOffsetMinutes: timeOffset,
      brightnessOffset: brightnessOffset,
      lightsOn: nextLightsOn,
    );
    final persistedChanged = updated != room;
    if (persistedChanged) {
      if (_snapshotNodes != null) {
        _snapshotNodes![roomId] = updated;
      } else {
        _state = RunnerStateDto(rooms: [
          for (final node in _state.rooms) node.id == roomId ? updated : node,
        ]);
      }
    }
    if (_snapshotNodes != null) {
      if (changed) notifyListeners();
      return;
    }
    if (persistedChanged) await _save();
    if (changed) notifyListeners();
  }

  Future<void> applyServerNodeState(
    String nodeId, {
    required bool rhythmEnabled,
    required double timeOffset,
    required double brightnessOffset,
    required RoomModeState state,
    bool transitioning = false,
    bool pendingDispatch = false,
    RhythmMode? mode,
    bool? lightsOn,
    int? brightness,
    int? kelvin,
    (int r, int g, int b)? color,
    bool? moodEnabled,
    bool? moodActive,
    bool tick = false,
  }) {
    return applyServerRoomState(
      nodeId,
      rhythmEnabled: rhythmEnabled,
      timeOffset: timeOffset,
      brightnessOffset: brightnessOffset,
      state: state,
      transitioning: transitioning,
      pendingDispatch: pendingDispatch,
      mode: mode,
      lightsOn: lightsOn,
      brightness: brightness,
      kelvin: kelvin,
      color: color,
      moodEnabled: moodEnabled,
      moodActive: moodActive,
      tick: tick,
    );
  }

  /// Set rhythm enabled/disabled for a room.
  Future<void> setRoomRhythmEnabled(String roomId, bool enabled) async {
    _state = room_state.setRoomRhythmEnabled(
        state: _state, roomId: roomId, rhythmEnabled: enabled);
    await _save();
    notifyListeners();
  }

  /// Set lights_on state for a room (sync with actual device state).
  ///
  /// Skips the update if the room is currently locked by a recent
  /// [setRoomLightsOnLocal] call, preventing stale external state from
  /// overriding the user's optimistic toggle.
  ///
  /// Power-only updates do not rewrite semantic room mode.
  Future<void> setRoomLightsOn(String roomId, bool lightsOn) async {
    final lockedUntil = _lightsOnLockedUntil[roomId];
    if (lockedUntil != null && DateTime.now().isBefore(lockedUntil)) {
      // Suppressed by the optimistic lock — remember it so the real state
      // shows through once the lock expires (a dropped command produces no
      // further events to correct the optimistic value).
      _suppressedLightsOn[roomId] = lightsOn;
      _armLockExpiryTimer(roomId, lockedUntil);
      return;
    }
    _suppressedLightsOn.remove(roomId);
    _state = room_state.setRoomLightsOn(
        state: _state, roomId: roomId, lightsOn: lightsOn);
    await _save();
    notifyListeners();
  }

  /// Set lights_on from a local UI toggle with a 3s lock.
  ///
  /// Locks the room so that incoming hub state events don't
  /// immediately overwrite the optimistic value before the bridge
  /// has processed the command.
  ///
  /// Local power toggles do not rewrite semantic room mode.
  Future<void> setRoomLightsOnLocal(String roomId, bool lightsOn) async {
    _lightsOnLockedUntil[roomId] =
        DateTime.now().add(const Duration(seconds: 3));
    // A fresh optimistic toggle supersedes whatever the previous lock
    // suppressed — re-applying it now would fight this toggle.
    _suppressedLightsOn.remove(roomId);
    _acknowledgedLightsOn.remove(roomId);
    _state = room_state.setRoomLightsOn(
        state: _state, roomId: roomId, lightsOn: lightsOn);
    await _save();
    notifyListeners();
  }

  /// Set time offset for a room (from dragging the blue dot).
  Future<void> setRoomTimeOffset(String roomId, double offsetMinutes) async {
    _state = room_state.setRoomTimeOffset(
        state: _state, roomId: roomId, timeOffsetMinutes: offsetMinutes);
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
    required double latitude,
    required double longitude,
    required int year,
    required int month,
    required int day,
    required String timezone,
    required double currentHour,
  }) {
    final result = room_state.calculateRoomActionResult(
      state: _state,
      config: config,
      latitude: latitude,
      longitude: longitude,
      year: year,
      month: month,
      day: day,
      timezone: timezone,
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
    required double latitude,
    required double longitude,
    required int year,
    required int month,
    required int day,
    required String timezone,
    required double currentHour,
    required RhythmActionDto action,
  }) async {
    final room = currentRoom;
    if (room == null) return null;

    final result = room_state.calculateRoomActionResult(
      state: _state,
      config: config,
      latitude: latitude,
      longitude: longitude,
      year: year,
      month: month,
      day: day,
      timezone: timezone,
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
    if (_snapshotNodes != null) return _snapshotNodes![roomId];
    if (!identical(_indexedState, _state)) {
      _nodeIndex = {for (final node in _state.rooms) node.id: node};
      _indexedState = _state;
    }
    return _nodeIndex[roomId];
  }

  RoomDto? getNode(String nodeId) => getRoom(nodeId);

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
    _roomStateLockedUntil.clear();
    _cancelLockExpiryTimers();
    _serverReportedLightsOn.clear();
    _roomStates.clear();
    _roomModes.clear();
    _cancelRoomTransitionTimers();
    _roomTransitioning.clear();
    _cancelNodeDispatchPendingTimers();
    _nodeDispatchPending.clear();
    _roomBrightness.clear();
    _roomKelvin.clear();
    _roomColor.clear();
    _roomMoodColor.clear();
    _roomMoodBrightness.clear();
    _roomMoodEnabled.clear();
    _roomMoodActive.clear();
    _lastTickTime.clear();
    notifyListeners();
  }

  @override
  void dispose() {
    _cancelRoomTransitionTimers();
    _cancelNodeDispatchPendingTimers();
    _cancelLockExpiryTimers();
    _sourceChangedController.close();
    super.dispose();
  }

  /// Clear all rooms and associated transient state.
  Future<void> clearAllRooms() async {
    _state = room_state.emptyRunnerState();
    _currentIndex = 0;
    _motionTimers.clear();
    _roomsWithSensors.clear();
    _lightsOnLockedUntil.clear();
    _roomStateLockedUntil.clear();
    _cancelLockExpiryTimers();
    _serverReportedLightsOn.clear();
    _roomStates.clear();
    _roomModes.clear();
    _cancelRoomTransitionTimers();
    _roomTransitioning.clear();
    _cancelNodeDispatchPendingTimers();
    _nodeDispatchPending.clear();
    _roomBrightness.clear();
    _roomKelvin.clear();
    _roomColor.clear();
    _roomMoodColor.clear();
    _roomMoodBrightness.clear();
    _roomMoodEnabled.clear();
    _roomMoodActive.clear();
    _lastTickTime.clear();
    await _save();
    // Update room count analytics property
    AnalyticsService().setRoomCount(0);
    notifyListeners();
  }
}
