/// Rhythm Runner - Standalone adaptive lighting controller.
///
/// The Rhythm Runner is a thin orchestration layer that:
/// - Periodically calls Rust tick() for lighting updates
/// - Handles button actions by calling Rust handle_action()
/// - Persists state via SharedPreferences (Rust owns the state logic)
/// - Executes light commands via providers
/// - Receives input events from registered event sources
library;

import 'dart:async';

import 'package:shared_preferences/shared_preferences.dart';

import '../events/event_source.dart';
import '../src/rust/api/curve.dart' show getSunTimes;
import '../src/rust/api/dto/curve.dart' show CurveConfigDto;
import '../src/rust/api/dto/runner.dart';
import '../src/rust/api/runner.dart';
import 'provider_manager.dart';
import 'runner_state_json.dart' as json_util;

/// Configuration for the Rhythm Runner.
class RhythmRunnerConfig {
  /// Latitude for solar calculations.
  final double latitude;

  /// Longitude for solar calculations.
  final double longitude;

  /// IANA timezone name (e.g., "America/New_York").
  final String timezone;

  /// The curve configuration to use.
  final CurveConfigDto curveConfig;

  /// Interval for periodic light updates.
  final Duration updateInterval;

  const RhythmRunnerConfig({
    required this.latitude,
    required this.longitude,
    required this.timezone,
    required this.curveConfig,
    this.updateInterval = const Duration(minutes: 1),
  });
}

/// Callback for runner state changes.
typedef RunnerStateCallback = void Function(RhythmRunnerStatus status);

/// Callback for room state changes.
typedef RoomChangeCallback = void Function(String roomId, RoomDto room);

/// Callback for errors.
typedef RunnerErrorCallback = void Function(String roomId, Object error);

/// Status of the Rhythm Runner.
enum RhythmRunnerStatus {
  /// Runner is stopped.
  stopped,

  /// Runner is running.
  running,

  /// Runner is paused (e.g., app in background).
  paused,
}

/// Rhythm Runner - Thin orchestration layer over Rust state management.
///
/// All state logic lives in Rust. Dart is responsible for:
/// - Timer management
/// - Persistence (SharedPreferences)
/// - Executing commands via providers
///
/// Usage:
/// ```dart
/// final runner = RhythmRunner(
///   config: RhythmRunnerConfig(...),
///   providerManager: providerManager,
/// );
///
/// await runner.load(); // Load persisted state
/// await runner.start();
/// await runner.handleAction('living_room', RhythmAction.onPress);
/// ```
class RhythmRunner {
  final RhythmRunnerConfig config;
  final ProviderManager providerManager;

  Timer? _periodicTimer;
  RhythmRunnerStatus _status = RhythmRunnerStatus.stopped;

  /// Current runner state (owned by Rust, stored here for FFI calls).
  RunnerStateDto _state = createRunnerState();

  /// Registered event sources.
  final Map<String, EventSource> _eventSources = {};

  /// Subscriptions to event source streams.
  final Map<String, StreamSubscription<InputEvent>> _eventSubscriptions = {};

  static const String _stateKey = 'rhythm_runner_state';

  /// Callbacks
  RunnerStateCallback? onStatusChanged;
  RoomChangeCallback? onRoomChanged;
  RunnerErrorCallback? onError;

  /// Callback for event source state changes.
  void Function(String sourceId, EventSourceState state)? onEventSourceStateChanged;

  RhythmRunner({
    required this.config,
    required this.providerManager,
  });

  /// Current status of the runner.
  RhythmRunnerStatus get status => _status;

  /// Whether the runner is currently running.
  bool get isRunning => _status == RhythmRunnerStatus.running;

  /// Get the current state (read-only view).
  RunnerStateDto get state => _state;

  /// Get all room IDs.
  List<String> get roomIds => runnerGetRoomIds(state: _state);

  /// Get a room by ID.
  RoomDto? getRoom(String roomId) => runnerGetRoom(state: _state, roomId: roomId);

  /// Load persisted state from SharedPreferences.
  Future<void> load() async {
    final prefs = await SharedPreferences.getInstance();
    final json = prefs.getString(_stateKey);

    if (json != null) {
      final loaded = json_util.runnerStateFromJson(json);
      if (loaded != null) {
        _state = loaded;
      }
    }
  }

  /// Save current state to SharedPreferences.
  Future<void> save() async {
    final prefs = await SharedPreferences.getInstance();
    final json = json_util.runnerStateToJson(_state);
    await prefs.setString(_stateKey, json);
  }

  /// Add a room to the runner.
  Future<void> addRoom(RoomDto room) async {
    _state = runnerAddRoom(state: _state, room: room);
    await save();
  }

  /// Remove a room from the runner.
  Future<void> removeRoom(String roomId) async {
    _state = runnerRemoveRoom(state: _state, roomId: roomId);
    await save();
  }

  /// Set devices for a room.
  Future<void> setRoomDevices(String roomId, List<String> deviceIds) async {
    _state = runnerSetRoomDevices(state: _state, roomId: roomId, deviceIds: deviceIds);
    await save();
  }

  // ========================================================================
  // Event Source Management
  // ========================================================================

  /// Get all registered event sources.
  Iterable<EventSource> get eventSources => _eventSources.values;

  /// Get an event source by ID.
  EventSource? getEventSource(String sourceId) => _eventSources[sourceId];

  /// Register an event source.
  ///
  /// The source will automatically connect when the runner starts
  /// and disconnect when it stops.
  void registerEventSource(EventSource source) {
    if (_eventSources.containsKey(source.id)) {
      throw StateError('Event source ${source.id} is already registered');
    }

    _eventSources[source.id] = source;

    // If runner is already running, connect the source immediately
    if (_status == RhythmRunnerStatus.running) {
      _connectEventSource(source);
    }
  }

  /// Unregister an event source.
  ///
  /// This disconnects and disposes the source.
  Future<void> unregisterEventSource(String sourceId) async {
    final source = _eventSources.remove(sourceId);
    if (source == null) return;

    await _disconnectEventSource(source);
    await source.dispose();
  }

  /// Connect all event sources.
  Future<void> _connectAllEventSources() async {
    await Future.wait(
      _eventSources.values.map(_connectEventSource),
    );
  }

  /// Disconnect all event sources.
  Future<void> _disconnectAllEventSources() async {
    await Future.wait(
      _eventSources.values.map(_disconnectEventSource),
    );
  }

  /// Connect a single event source.
  Future<void> _connectEventSource(EventSource source) async {
    // Subscribe to events
    _eventSubscriptions[source.id] = source.events.listen((event) {
      _handleInputEvent(event);
    });

    // Subscribe to state changes
    source.stateChanges.listen((state) {
      onEventSourceStateChanged?.call(source.id, state);
    });

    // Connect
    try {
      await source.connect();
    } catch (e) {
      onError?.call('event_source_${source.id}', e);
    }
  }

  /// Disconnect a single event source.
  Future<void> _disconnectEventSource(EventSource source) async {
    await _eventSubscriptions.remove(source.id)?.cancel();

    try {
      await source.disconnect();
    } catch (e) {
      // Ignore disconnect errors
    }
  }

  /// Handle an input event from an event source.
  void _handleInputEvent(InputEvent event) {
    handleAction(event.roomId, event.action);
  }

  /// Start periodic updates and connect event sources.
  Future<void> start() async {
    if (_status == RhythmRunnerStatus.running) return;

    _status = RhythmRunnerStatus.running;
    onStatusChanged?.call(_status);

    // Connect all event sources
    await _connectAllEventSources();

    // Start periodic timer
    _periodicTimer = Timer.periodic(config.updateInterval, (_) {});
  }

  /// Stop periodic updates and disconnect event sources.
  Future<void> stop() async {
    _periodicTimer?.cancel();
    _periodicTimer = null;

    // Disconnect all event sources
    await _disconnectAllEventSources();

    _status = RhythmRunnerStatus.stopped;
    onStatusChanged?.call(_status);
  }

  /// Pause periodic updates (e.g., when app goes to background).
  ///
  /// Event sources remain connected but the runner won't send periodic updates.
  void pause() {
    _periodicTimer?.cancel();
    _periodicTimer = null;
    _status = RhythmRunnerStatus.paused;
    onStatusChanged?.call(_status);
  }

  /// Resume periodic updates after pause.
  Future<void> resume() async {
    if (_status != RhythmRunnerStatus.paused) return;

    _status = RhythmRunnerStatus.running;
    onStatusChanged?.call(_status);

    // Restart periodic timer
    _periodicTimer = Timer.periodic(config.updateInterval, (_) {});
  }

  /// Handle a button/remote action for a room.
  Future<void> handleAction(String roomId, RhythmActionDto action) async {
    // Get current solar info
    final now = DateTime.now();
    final sunTimes = getSunTimes(
      latitude: config.latitude,
      longitude: config.longitude,
      year: now.year,
      month: now.month,
      day: now.day,
      timezone: config.timezone,
    );

    // Process action in Rust
    final result = runnerHandleAction(
      state: _state,
      config: config.curveConfig,
      solarNoonHour: sunTimes.solarNoon,
      latitude: config.latitude,
      dayOfYear: _dayOfYear(now),
      currentHour: _currentHour(now),
      roomId: roomId,
      action: action,
    );

    // Update state
    _state = result.state;

    // Execute commands
    await _executeCommands(result.commands);

    // Save if state changed
    if (result.stateChanged) {
      await save();
      final room = getRoom(roomId);
      if (room != null) {
        onRoomChanged?.call(roomId, room);
      }
    }
  }

  /// Execute light commands via providers.
  Future<void> _executeCommands(List<LightCommandDto> commands) async {
    for (final cmd in commands) {
      final provider = providerManager.getProviderForRoom(cmd.roomId);
      if (provider == null) continue;

      try {
        switch (cmd.commandType) {
          case LightCommandType.turnOn:
            await provider.turnOn(
              cmd.deviceId,
              brightness: cmd.brightness ?? 100,
              kelvin: cmd.kelvin ?? 4000,
            );
          case LightCommandType.turnOff:
            await provider.turnOff(cmd.deviceId);
        }
      } catch (e) {
        onError?.call(cmd.roomId, e);
      }
    }
  }

  /// Get the current time as decimal hours (0-24).
  double _currentHour(DateTime now) {
    return now.hour + now.minute / 60.0 + now.second / 3600.0;
  }

  /// Get the day of year (1-365/366).
  int _dayOfYear(DateTime date) {
    return date.difference(DateTime(date.year, 1, 1)).inDays + 1;
  }

  /// Dispose the runner and release resources.
  Future<void> dispose() async {
    await stop();

    // Dispose all event sources
    for (final source in _eventSources.values) {
      await source.dispose();
    }
    _eventSources.clear();
  }
}

/// Factory function to create a fully configured RhythmRunner.
///
/// Loads persisted state and sets up the runner with providers.
Future<RhythmRunner> createRhythmRunner({
  required RhythmRunnerConfig config,
  required ProviderManager providerManager,
}) async {
  final runner = RhythmRunner(
    config: config,
    providerManager: providerManager,
  );

  // Load persisted state
  await runner.load();

  return runner;
}
