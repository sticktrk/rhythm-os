/// Abstract interface for Hue bridge operations.
///
/// This Strategy Pattern interface allows swapping between real and demo
/// implementations. Use [HueServiceLocator] to get the appropriate instance.
library;

import 'package:rhythm_core/rhythm_core.dart';

/// Abstract interface defining all Hue bridge operations.
///
/// Implementations:
/// - [RealHueBridgeService]: Connects to real Philips Hue bridges
/// - [DemoHueBridgeService]: Mock implementation for App Store review
abstract class HueBridgeService {
  /// Configuration (null if not connected).
  HueConfig? get config;

  // ============================================================================
  // Bridge Discovery and Pairing
  // ============================================================================

  /// Discover Hue bridges on the local network.
  ///
  /// Uses mDNS and Philips discovery endpoint.
  Future<List<String>> discoverBridges({Duration? timeout});

  /// Start the link button pairing process with a Hue bridge.
  ///
  /// Call this after the user presses the link button on the bridge.
  /// Returns the username/application key if successful, null otherwise.
  Future<String?> pair(String bridgeIp);

  // ============================================================================
  // Room Operations
  // ============================================================================

  /// Fetch all rooms from a Hue bridge.
  ///
  /// Returns RoomDto objects ready for the RoomProvider.
  Future<List<RoomDto>> fetchRooms();

  /// Check if any light in a room is on.
  Future<bool> isRoomOn(String roomId);

  /// Toggle all lights in a room.
  ///
  /// Returns the new state (true = on, false = off).
  Future<bool> toggleRoom(String roomId, {int? brightness, int? mireds});

  /// Set light state for a room.
  Future<void> setRoomState(
    String roomId, {
    required bool on,
    int? brightness,
    int? mireds,
  });

  /// Fetch all room on/off states in a single API call.
  ///
  /// Returns a map of room IDs to their on/off state.
  Future<Map<String, bool>> fetchAllRoomStates();

  // ============================================================================
  // SSE for Button Events
  // ============================================================================

  /// Whether this implementation supports SSE for button events.
  bool get supportsSse;

  /// Current SSE connection state.
  ///
  /// Returns [EventSourceState.disconnected] if SSE is not initialized.
  EventSourceState get sseState;

  /// Stream of SSE connection state changes.
  ///
  /// Listen to this to update UI when SSE connects, disconnects, or reconnects.
  /// Returns an empty stream if SSE is not supported.
  Stream<EventSourceState> get sseStateChanges;

  /// Initialize SSE event source for direct Hue button events.
  ///
  /// Only available in real mode; demo mode is a no-op.
  /// [rooms] is used for auto-mapping Hue rooms to Rhythm rooms.
  /// [onButtonEvent] is called when a button event is received.
  Future<void> initializeSse({
    required List<RoomDto> rooms,
    required void Function(String roomId, RhythmActionDto action) onButtonEvent,
  });

  /// Dispose SSE resources.
  Future<void> disposeSse();

  /// Resync devices and behavior_instances.
  ///
  /// Call when new switches are added to pick them up without restart.
  Future<void> resyncDevices();

  // ============================================================================
  // SSE for Light State Monitoring
  // ============================================================================

  /// Start monitoring grouped_light SSE events for real-time on/off updates.
  ///
  /// [onLightStateChanged] is called with (roomId, isOn) when a room's
  /// aggregate light state changes. Requires rooms to have been fetched first
  /// (so grouped_light mappings are populated).
  Future<void> startLightMonitoring({
    required void Function(String roomId, bool isOn) onLightStateChanged,
  });

  /// Stop monitoring light state changes and release SSE resources.
  Future<void> stopLightMonitoring();

  // ============================================================================
  // Geolocation
  // ============================================================================

  /// Fetch geolocation from Hue bridge.
  ///
  /// Returns (latitude, longitude) tuple or null if not configured.
  Future<(double, double)?> fetchGeolocation();

  // ============================================================================
  // Lifecycle
  // ============================================================================

  /// Configure the service with Hue credentials.
  void configure(HueConfig config);

  /// Reset to unconfigured state.
  void reset();
}
