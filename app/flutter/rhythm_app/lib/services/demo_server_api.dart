/// Shared local demo server state for TestFlight / App Store Connect builds.
///
/// This lets demo mode exercise the same topology, device-detail, and triage
/// surfaces as the real app without requiring a live rhythm server.
library;

import 'dart:async';

import 'package:dio/dio.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class DemoServerSnapshot {
  final List<RhythmRoom> helloNodes;
  final List<RhythmTopologyNode> topologyNodes;
  final List<Map<String, dynamic>> hubInfos;
  final int triagePendingCount;
  final int triagePendingDevices;
  final int triagePendingRooms;

  const DemoServerSnapshot({
    required this.helloNodes,
    required this.topologyNodes,
    required this.hubInfos,
    required this.triagePendingCount,
    required this.triagePendingDevices,
    required this.triagePendingRooms,
  });
}

class DemoServerApi extends RhythmServerApi {
  static final DemoServerApi instance = DemoServerApi._();

  DemoServerApi._() : super(Dio());

  static const firmwareVersion = '2026.4-demo';
  static const serverPlatformType = 'server';
  static const serverPlatformContext = 'app_store_demo';
  static const _demoHubAddress = 'demo-hue.local';
  static const _demoHubName = 'Demo Hue Bridge';

  final StreamController<void> _changes = StreamController<void>.broadcast();

  final Map<String, Map<String, dynamic>> _nodeStates = {};
  final Map<String, Map<String, dynamic>> _topologyNodes = {};
  final Map<String, Map<String, dynamic>> _canonicalDevices = {};
  final Map<String, Map<String, dynamic>> _triageEntries = {};

  bool _seeded = false;
  int _nextRoomOrdinal = 1;
  bool _powerSave = false;
  RhythmMode _activeMode = RhythmMode.day;

  Stream<void> get changes => _changes.stream;

  void ensureSeeded() {
    if (_seeded) return;
    reset();
  }

  void reset() {
    _seeded = true;
    _powerSave = false;
    _activeMode = RhythmMode.day;
    _nextRoomOrdinal = 5;
    _nodeStates.clear();
    _topologyNodes.clear();
    _canonicalDevices.clear();
    _triageEntries.clear();

    _addRoom(
      roomId: 'hue_demo_1',
      name: 'Living Room',
      brightness: 74,
      kelvin: 3100,
    );
    _addDevice(
      id: 'demo_living_ceiling',
      name: 'Ceiling Lights',
      kind: RhythmNodeKind.lightDevice,
      parentId: 'hue_demo_1',
      manufacturer: 'Signify',
      model: 'White Ambiance',
      nativeId: 'hue-living-ceiling',
    );
    _addDevice(
      id: 'demo_living_strip',
      name: 'Media Strip',
      kind: RhythmNodeKind.lightDevice,
      parentId: 'hue_demo_1',
      manufacturer: 'Signify',
      model: 'Play Gradient',
      nativeId: 'hue-living-strip',
    );
    _addDevice(
      id: 'demo_living_dimmer',
      name: 'Dimmer Switch',
      kind: RhythmNodeKind.button,
      parentId: 'hue_demo_1',
      manufacturer: 'Philips Hue',
      model: 'Dimmer v2',
      nativeId: 'hue-living-dimmer',
    );
    _addDevice(
      id: 'demo_living_motion',
      name: 'Living Room Motion',
      kind: RhythmNodeKind.motionSensor,
      parentId: 'hue_demo_1',
      manufacturer: 'Philips Hue',
      model: 'Motion Sensor',
      nativeId: 'hue-living-motion',
      controls: const [
        {
          'kind': 'motion',
          'target_id': 'hue_demo_1',
          'inherited': false,
        },
      ],
    );

    _addRoom(
      roomId: 'hue_demo_2',
      name: 'Bedroom',
      brightness: 52,
      kelvin: 2700,
    );
    _addDevice(
      id: 'demo_bedroom_left',
      name: 'Bedside Left',
      kind: RhythmNodeKind.lightDevice,
      parentId: 'hue_demo_2',
      manufacturer: 'Signify',
      model: 'Candle E12',
      nativeId: 'hue-bedroom-left',
    );
    _addDevice(
      id: 'demo_bedroom_right',
      name: 'Bedside Right',
      kind: RhythmNodeKind.lightDevice,
      parentId: 'hue_demo_2',
      manufacturer: 'Signify',
      model: 'Candle E12',
      nativeId: 'hue-bedroom-right',
    );

    _addRoom(
      roomId: 'hue_demo_3',
      name: 'Kitchen',
      brightness: 82,
      kelvin: 3900,
    );
    _addDevice(
      id: 'demo_kitchen_main',
      name: 'Main Lights',
      kind: RhythmNodeKind.lightDevice,
      parentId: 'hue_demo_3',
      manufacturer: 'Signify',
      model: 'White and Color',
      nativeId: 'hue-kitchen-main',
    );
    _addDevice(
      id: 'demo_kitchen_island',
      name: 'Island Pendant',
      kind: RhythmNodeKind.lightDevice,
      parentId: 'hue_demo_3',
      manufacturer: 'Signify',
      model: 'Filament',
      nativeId: 'hue-kitchen-island',
    );
    _addDevice(
      id: 'demo_kitchen_switch',
      name: 'Kitchen Switch',
      kind: RhythmNodeKind.button,
      parentId: 'hue_demo_3',
      manufacturer: 'Philips Hue',
      model: 'Tap Dial',
      nativeId: 'hue-kitchen-switch',
    );

    _addRoom(
      roomId: 'hue_demo_4',
      name: 'Office',
      brightness: 68,
      kelvin: 3350,
    );
    _addDevice(
      id: 'demo_office_desk',
      name: 'Desk Lamp',
      kind: RhythmNodeKind.lightDevice,
      parentId: 'hue_demo_4',
      manufacturer: 'Signify',
      model: 'Go Portable',
      nativeId: 'hue-office-desk',
    );
    _addDevice(
      id: 'demo_office_button',
      name: 'Scene Button',
      kind: RhythmNodeKind.button,
      parentId: 'hue_demo_4',
      manufacturer: 'Philips Hue',
      model: 'Smart Button',
      nativeId: 'hue-office-button',
    );

    _addDevice(
      id: 'demo_floor_lamp',
      name: 'Floor Lamp',
      kind: RhythmNodeKind.lightDevice,
      parentId: null,
      manufacturer: 'Signify',
      model: 'Signe',
      nativeId: 'hue-floor-lamp',
    );

    _triageEntries['demo_unassigned_floor_lamp'] = {
      'id': 'demo_unassigned_floor_lamp',
      'kind': 'unassigned_device',
      'unassigned_device': {
        'id': 'demo_floor_lamp',
        'name': 'Floor Lamp',
        'device_type': 'light',
        'manufacturer': 'Signify',
        'model': 'Signe',
        'hub_name': _demoHubName,
      },
    };
    _triageEntries['demo_hub_configured'] = {
      'id': 'demo_hub_configured',
      'kind': 'hub_configured',
      'hub_configured': {
        'hub_type': 'hue',
        'address': _demoHubAddress,
      },
      'hub_key': {
        'hub_type': 'hue',
        'address': _demoHubAddress,
      },
    };
  }

  DemoServerSnapshot snapshot() {
    ensureSeeded();
    final triageEntries = _triageEntries.values.toList(growable: false);
    final deviceCount = triageEntries
        .where(
          (entry) => switch (entry['kind']) {
            'device_merge' || 'unassigned_device' => true,
            _ => false,
          },
        )
        .length;
    final roomCount = triageEntries
        .where(
          (entry) => switch (entry['kind']) {
            'room_binding' || 'hub_configured' => true,
            _ => false,
          },
        )
        .length;

    return DemoServerSnapshot(
      helloNodes: lightAddressableNodes,
      topologyNodes: topologyNodes,
      hubInfos: const [
        {
          'type': 'hue',
          'address': _demoHubAddress,
          'connected': true,
        },
      ],
      triagePendingCount: triageEntries.length,
      triagePendingDevices: deviceCount,
      triagePendingRooms: roomCount,
    );
  }

  List<RhythmRoom> get lightAddressableNodes {
    ensureSeeded();
    final nodes = _nodeStates.values
        .map((json) => RhythmRoom.fromJson(json))
        .toList(growable: false)
      ..sort((left, right) => left.name.compareTo(right.name));
    return nodes;
  }

  List<RhythmTopologyNode> get topologyNodes {
    ensureSeeded();
    final nodes = _topologyNodes.values
        .map((json) => RhythmTopologyNode.fromJson(json))
        .toList(growable: false)
      ..sort((left, right) {
        if (left.kind.isRoom != right.kind.isRoom) {
          return left.kind.isRoom ? -1 : 1;
        }
        return left.name.compareTo(right.name);
      });
    return nodes;
  }

  List<RoomDto> buildRoomDtos() {
    ensureSeeded();
    return lightAddressableNodes
        .map(
          (node) => RoomDto(
            id: node.id,
            name: node.name,
            source: _sourceForHubTypes(node.hubTypes),
            kind: _roomNodeKind(node.kind),
            parentId: node.parentId,
            placement: _roomPlacement(node.placement),
            deviceIds: _lightDeviceIdsForRoom(node.id),
            rhythmEnabled: node.rhythmEnabled,
            disabled: node.disabled,
            lightsOn: node.lightsOn ?? false,
            timeOffsetMinutes: node.timeOffset,
            brightnessOffset: node.brightnessOffset,
            curveConfig: null,
          ),
        )
        .toList(growable: false);
  }

  void updateRoomLightState(
    String roomId, {
    required bool on,
    int? brightness,
    int? kelvin,
    RoomModeState? state,
  }) {
    ensureSeeded();
    final room = _nodeStates[roomId];
    if (room == null) return;
    room['lights_on'] = on;
    room['state'] =
        (state ?? (on ? RoomModeState.active : RoomModeState.hardOff))
            .wireValue;
    if (brightness != null) {
      room['brightness'] = brightness;
    }
    if (kelvin != null) {
      room['kelvin'] = kelvin;
    }
  }

  @override
  Future<RhythmSettings?> getSettings() async {
    ensureSeeded();
    return RhythmSettings.fromJson({
      'power_save': _powerSave,
    });
  }

  @override
  Future<bool> settingsSet({bool? powerSave}) async {
    ensureSeeded();
    if (powerSave != null) {
      _powerSave = powerSave;
      _changes.add(null);
    }
    return true;
  }

  @override
  Future<RhythmModeResource?> getMode() async {
    ensureSeeded();
    return RhythmModeResource.fromJson({
      'active': _activeMode.wireValue,
      'configs': const [],
    });
  }

  @override
  Future<void> setActiveMode(RhythmMode mode) async {
    ensureSeeded();
    _activeMode = mode;
    _changes.add(null);
  }

  @override
  Future<Map<String, dynamic>?> getCanonicalDevice(String id) async {
    ensureSeeded();
    final device = _canonicalDevices[id];
    if (device == null) return null;
    return Map<String, dynamic>.from(device);
  }

  @override
  Future<List<Map<String, dynamic>>?> getCanonicalDevices() async {
    ensureSeeded();
    return _canonicalDevices.values
        .map((device) => Map<String, dynamic>.from(device))
        .toList(growable: false);
  }

  @override
  Future<List<Map<String, dynamic>>?> getTriageEntries() async {
    ensureSeeded();
    return _triageEntries.values
        .map((entry) => Map<String, dynamic>.from(entry))
        .toList(growable: false);
  }

  @override
  Future<Map<String, dynamic>?> getTriageCount() async {
    final data = snapshot();
    return {
      'pending_count': data.triagePendingCount,
      'pending_devices': data.triagePendingDevices,
      'pending_rooms': data.triagePendingRooms,
      'pending_hub_configured': _triageEntries.values
          .where((entry) => entry['kind'] == 'hub_configured')
          .length,
      'pending_unassigned': _triageEntries.values
          .where((entry) => entry['kind'] == 'unassigned_device')
          .length,
    };
  }

  @override
  Future<List<RhythmTopologyNode>> getTopologyNodes() async => topologyNodes;

  @override
  Future<bool> topologyRenameRoom(String roomId, String name) async {
    ensureSeeded();
    final trimmed = name.trim();
    if (trimmed.isEmpty) return false;
    final roomNode = _topologyNodes[roomId];
    final roomState = _nodeStates[roomId];
    if (roomNode == null || roomState == null) return false;
    roomNode['name'] = trimmed;
    roomState['name'] = trimmed;
    _changes.add(null);
    return true;
  }

  @override
  Future<bool> topologyDeleteRoom(String roomId) async {
    ensureSeeded();
    if (!_topologyNodes.containsKey(roomId) ||
        !_nodeStates.containsKey(roomId)) {
      return false;
    }

    _topologyNodes.remove(roomId);
    _nodeStates.remove(roomId);
    for (final node in _topologyNodes.values) {
      if (node['parent_id'] == roomId) {
        node['parent_id'] = null;
      }
    }
    _changes.add(null);
    return true;
  }

  @override
  Future<bool> assignDeviceParent(String deviceId, String? parentId) async {
    ensureSeeded();
    final node = _topologyNodes[deviceId];
    if (node == null) return false;
    if (parentId != null && !_topologyNodes.containsKey(parentId)) return false;
    node['parent_id'] = parentId;
    _triageEntries.removeWhere((_, entry) {
      final device = entry['unassigned_device'] as Map<String, dynamic>?;
      return device?['id'] == deviceId;
    });
    _changes.add(null);
    return true;
  }

  @override
  Future<Map<String, dynamic>?> createTopologyRoom(String name) async {
    ensureSeeded();
    final trimmed = name.trim();
    if (trimmed.isEmpty) return null;
    final roomId = 'demo_room_${_nextRoomOrdinal++}';
    _addRoom(
      roomId: roomId,
      name: trimmed,
      brightness: 68,
      kelvin: 3200,
    );
    _changes.add(null);
    return {
      'id': roomId,
      'name': trimmed,
    };
  }

  @override
  Future<Map<String, dynamic>?> triggerSync() async {
    ensureSeeded();
    _changes.add(null);
    return const {'status': 'ok'};
  }

  @override
  Future<bool> resolveTriageDismiss(String entryId) async {
    ensureSeeded();
    final removed = _triageEntries.remove(entryId);
    if (removed == null) return false;
    _changes.add(null);
    return true;
  }

  @override
  Future<bool> resolveTriageRoom(String entryId, String roomId) async {
    ensureSeeded();
    final entry = _triageEntries[entryId];
    final device =
        entry?['unassigned_device'] as Map<String, dynamic>? ?? const {};
    final deviceId = device['id'] as String?;
    if (deviceId == null || deviceId.isEmpty) return false;
    final assigned = await assignDeviceParent(deviceId, roomId);
    if (!assigned) return false;
    _triageEntries.remove(entryId);
    _changes.add(null);
    return true;
  }

  void _addRoom({
    required String roomId,
    required String name,
    required int brightness,
    required int kelvin,
  }) {
    _topologyNodes[roomId] = {
      'id': roomId,
      'name': name,
      'kind': 'room',
    };
    _nodeStates[roomId] = {
      'id': roomId,
      'name': name,
      'kind': 'room',
      'hub_types': const ['hue'],
      'state': RoomModeState.active.wireValue,
      'rhythm_enabled': true,
      'disabled': false,
      'time_offset': 0.0,
      'brightness_offset': 0.0,
      'lights_on': true,
      'brightness': brightness,
      'kelvin': kelvin,
    };
  }

  void _addDevice({
    required String id,
    required String name,
    required RhythmNodeKind kind,
    required String? parentId,
    required String manufacturer,
    required String model,
    required String nativeId,
    List<Map<String, Object?>> controls = const [],
  }) {
    _topologyNodes[id] = {
      'id': id,
      'name': name,
      'kind': _kindWireValue(kind),
      'parent_id': parentId,
      'manufacturer': manufacturer,
      'model': model,
      if (controls.isNotEmpty) 'controls': controls,
    };
    _canonicalDevices[id] = {
      'id': id,
      'name': name,
      'manufacturer': manufacturer,
      'model': model,
      'endpoints': [
        {
          'hub_key': const {
            'hub_type': 'hue',
            'address': _demoHubAddress,
          },
          'native_id': nativeId,
          'preferred': true,
        },
      ],
    };
  }

  List<String> _lightDeviceIdsForRoom(String roomId) {
    return topologyNodes
        .where(
          (node) =>
              node.parentId == roomId &&
              node.kind == RhythmNodeKind.lightDevice,
        )
        .map((node) => node.id)
        .toList(growable: false);
  }

  String _kindWireValue(RhythmNodeKind kind) => switch (kind) {
        RhythmNodeKind.room => 'room',
        RhythmNodeKind.lightDevice => 'light_device',
        RhythmNodeKind.switchDevice => 'switch_device',
        RhythmNodeKind.motionSensor => 'motion_sensor',
        RhythmNodeKind.sensor => 'sensor',
        RhythmNodeKind.button => 'button',
        RhythmNodeKind.otherDevice => 'other_device',
      };

  RoomSourceDto _sourceForHubTypes(List<String> hubTypes) {
    if (hubTypes.contains('matter')) return RoomSourceDto.matter;
    if (hubTypes.contains('hue')) return RoomSourceDto.hue;
    if (hubTypes.contains('homeassistant') ||
        hubTypes.contains('home_assistant')) {
      return RoomSourceDto.homeAssistant;
    }
    if (hubTypes.contains('esp32')) return RoomSourceDto.esp32;
    return RoomSourceDto.unknown;
  }

  RoomNodeKind _roomNodeKind(RhythmNodeKind kind) => switch (kind) {
        RhythmNodeKind.room => RoomNodeKind.room,
        RhythmNodeKind.lightDevice => RoomNodeKind.lightDevice,
        RhythmNodeKind.switchDevice => RoomNodeKind.switchDevice,
        RhythmNodeKind.motionSensor => RoomNodeKind.motionSensor,
        RhythmNodeKind.sensor => RoomNodeKind.sensor,
        RhythmNodeKind.button => RoomNodeKind.button,
        RhythmNodeKind.otherDevice => RoomNodeKind.otherDevice,
      };

  RoomNodePlacement? _roomPlacement(RhythmNodePlacement? placement) =>
      switch (placement) {
        RhythmNodePlacement.hubDefault => RoomNodePlacement.hubDefault,
        RhythmNodePlacement.userOverride => RoomNodePlacement.userOverride,
        RhythmNodePlacement.standalone => RoomNodePlacement.standalone,
        null => null,
      };
}
