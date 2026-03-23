/// Event source abstraction for receiving input events from external sources.
///
/// This abstraction allows the RhythmRunner to receive events from various
/// sources like Hue SSE, Home Assistant events, or ZigBee directly.
library;

import 'dart:async';

import '../src/rust/api/dto/runner.dart' show RhythmActionDto;

/// An input event from an external source.
///
/// Represents a button press or other input that should trigger a rhythm action.
class InputEvent {
  /// The room ID this event targets.
  final String roomId;

  /// The action to perform.
  final RhythmActionDto action;

  /// ID of the event source that generated this event.
  final String sourceId;

  /// Optional device ID that triggered the event (e.g., switch MAC address).
  final String? deviceId;

  /// Timestamp when the event occurred.
  final DateTime timestamp;

  InputEvent({
    required this.roomId,
    required this.action,
    required this.sourceId,
    this.deviceId,
    DateTime? timestamp,
  }) : timestamp = timestamp ?? DateTime.now();

  @override
  String toString() =>
      'InputEvent(room: $roomId, action: $action, source: $sourceId, device: $deviceId)';
}

/// Connection state of an event source.
enum EventSourceState {
  /// Not connected.
  disconnected,

  /// Attempting to connect.
  connecting,

  /// Connected and receiving events.
  connected,

  /// Connection lost, will attempt to reconnect.
  reconnecting,

  /// Permanently failed (e.g., invalid credentials).
  failed,
}

/// Abstract interface for event sources.
///
/// Event sources provide a stream of input events that the RhythmRunner
/// can process. Each source manages its own connection lifecycle.
abstract class EventSource {
  /// Unique identifier for this source.
  String get id;

  /// Human-readable name for this source.
  String get name;

  /// Current connection state.
  EventSourceState get state;

  /// Whether the source is currently connected and receiving events.
  bool get isConnected => state == EventSourceState.connected;

  /// Stream of input events from this source.
  Stream<InputEvent> get events;

  /// Stream of state changes for this source.
  Stream<EventSourceState> get stateChanges;

  /// Connect to the event source.
  ///
  /// This should be called when the runner starts.
  /// The source should handle reconnection automatically.
  Future<void> connect();

  /// Disconnect from the event source.
  ///
  /// This should be called when the runner stops.
  Future<void> disconnect();

  /// Dispose of the source and release all resources.
  ///
  /// After calling this, the source cannot be used again.
  Future<void> dispose();
}
