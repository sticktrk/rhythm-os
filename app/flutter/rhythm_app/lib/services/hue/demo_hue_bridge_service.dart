/// Demo Hue bridge service implementation.
///
/// Provides mock data for App Store review without requiring a real
/// Philips Hue bridge. All operations simulate network delays.
library;

import 'package:rhythm_core/rhythm_core.dart';

import '../demo_server_api.dart';
import 'hue_bridge_service.dart';

/// Demo implementation of [HueBridgeService].
///
/// Used for App Store review when user signs in with demo credentials.
/// All operations use simulated delays and mock data.
class DemoHueBridgeService implements HueBridgeService {
  static final DemoHueBridgeService instance = DemoHueBridgeService._();

  DemoHueBridgeService._();

  /// Track room light states for demo mode.
  final Map<String, bool> _roomStates = {};

  /// Demo bridge IP address.
  static const _demoBridgeIp = '192.168.1.100';

  /// Demo app key returned during pairing.
  static const _demoAppKey = 'demo-hue-app-key';

  /// Mock Hue config for demo mode.
  static const _demoConfig = HueConfig(
    bridgeIp: _demoBridgeIp,
    username: _demoAppKey,
  );

  @override
  HueConfig? get config => _demoConfig;

  @override
  void configure(HueConfig config) {
    // No-op in demo mode - config is fixed
  }

  @override
  void reset() {
    _roomStates.clear();
  }

  void seedRooms(Iterable<RoomDto> rooms) {
    final validIds = rooms.map((room) => room.id).toSet();
    _roomStates.removeWhere((roomId, _) => !validIds.contains(roomId));
    for (final room in rooms) {
      _roomStates.putIfAbsent(room.id, () => room.lightsOn);
    }
  }

  // ============================================================================
  // Bridge Discovery and Pairing
  // ============================================================================

  @override
  Future<List<String>> discoverBridges({Duration? timeout}) async {
    // Simulate network delay
    await Future.delayed(const Duration(seconds: 1));
    return [_demoBridgeIp];
  }

  @override
  Future<String?> pair(String bridgeIp) async {
    // Simulate pairing delay (instant success, no button press needed)
    await Future.delayed(const Duration(milliseconds: 500));
    return _demoAppKey;
  }

  // ============================================================================
  // Room Operations
  // ============================================================================

  @override
  Future<List<RoomDto>> fetchRooms() async {
    await Future.delayed(const Duration(milliseconds: 500));
    final rooms = DemoServerApi.instance.buildRoomDtos();
    seedRooms(rooms);
    return rooms;
  }

  @override
  Future<bool> isRoomOn(String roomId) async {
    if (_roomStates.containsKey(roomId)) {
      return _roomStates[roomId] ?? false;
    }
    final rooms = DemoServerApi.instance.buildRoomDtos();
    seedRooms(rooms);
    return _roomStates[roomId] ?? false;
  }

  @override
  Future<bool> toggleRoom(String roomId, {int? brightness, int? mireds}) async {
    await Future.delayed(const Duration(milliseconds: 200));
    final newState = !(await isRoomOn(roomId));
    _roomStates[roomId] = newState;
    DemoServerApi.instance.updateRoomLightState(
      roomId,
      on: newState,
      brightness: brightness,
      kelvin: mireds != null && mireds > 0 ? (1000000 ~/ mireds) : null,
    );
    return newState;
  }

  @override
  Future<void> setRoomState(
    String roomId, {
    required bool on,
    int? brightness,
    int? mireds,
  }) async {
    await Future.delayed(const Duration(milliseconds: 200));
    _roomStates[roomId] = on;
    DemoServerApi.instance.updateRoomLightState(
      roomId,
      on: on,
      brightness: brightness,
      kelvin: mireds != null && mireds > 0 ? (1000000 ~/ mireds) : null,
    );
  }

  @override
  Future<Map<String, bool>> fetchAllRoomStates() async {
    final rooms = DemoServerApi.instance.buildRoomDtos();
    seedRooms(rooms);
    final states = <String, bool>{};
    for (final room in rooms) {
      states[room.id] = _roomStates[room.id] ?? false;
    }
    return states;
  }

  // ============================================================================
  // SSE for Button Events (No-op in Demo Mode)
  // ============================================================================

  @override
  bool get supportsSse => false;

  @override
  EventSourceState get sseState => EventSourceState.disconnected;

  @override
  Stream<EventSourceState> get sseStateChanges => const Stream.empty();

  @override
  Future<void> initializeSse({
    required List<RoomDto> rooms,
    required void Function(String roomId, RhythmActionDto action) onButtonEvent,
  }) async {
    // No-op - demo mode has no real SSE connection
  }

  @override
  Future<void> disposeSse() async {
    // No-op
  }

  @override
  Future<void> resyncDevices() async {
    // No-op
  }

  @override
  Future<void> startLightMonitoring({
    required void Function(String roomId, bool isOn) onLightStateChanged,
  }) async {
    // No-op - demo mode has no real SSE connection
  }

  @override
  Future<void> stopLightMonitoring() async {
    // No-op
  }

  @override
  Future<(double, double)?> fetchGeolocation() async => null;
}
