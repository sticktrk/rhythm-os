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
  final RhythmReviewSummary review;
  final int triagePendingCount;
  final int triagePendingDevices;
  final int triagePendingRooms;

  const DemoServerSnapshot({
    required this.helloNodes,
    required this.topologyNodes,
    required this.hubInfos,
    required this.review,
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
  final Map<String, RhythmInputBinding> _inputBindings = {};
  List<RhythmModeTransitionConfig> _modeTransitions = const [];

  bool _seeded = false;
  int _nextRoomOrdinal = 1;
  bool _powerSave = true;
  bool _autoUpdate = true;
  bool _lightBreakerEnabled = true;
  RhythmMode _activeMode = RhythmMode.day;
  RhythmLightRuntime _lightRuntime = RhythmLightRuntime.rhythmAdaptive;

  Stream<void> get changes => _changes.stream;

  List<RhythmModeTransitionConfig> _defaultModeTransitions() {
    return const [
      RhythmModeTransitionConfig(
        id: 'sleep_to_day',
        label: 'Sleep to Day',
        fromMode: RhythmMode.sleep,
        toMode: RhythmMode.day,
        trigger: RhythmTransitionTrigger.solar('astronomical_twilight'),
        duration: TransitionDuration.auto(),
        preserveHardOff: true,
      ),
      RhythmModeTransitionConfig(
        id: 'day_to_sleep',
        label: 'Day to Sleep',
        fromMode: RhythmMode.day,
        toMode: RhythmMode.sleep,
        trigger: RhythmTransitionTrigger.solar('nautical_twilight'),
        duration: TransitionDuration.auto(),
        preserveHardOff: true,
      ),
    ];
  }

  void ensureSeeded() {
    if (_seeded) return;
    reset();
  }

  void reset() {
    _seeded = true;
    _powerSave = true;
    _autoUpdate = true;
    _lightBreakerEnabled = true;
    _activeMode = RhythmMode.day;
    _lightRuntime = RhythmLightRuntime.rhythmAdaptive;
    _nextRoomOrdinal = 5;
    _nodeStates.clear();
    _topologyNodes.clear();
    _canonicalDevices.clear();
    _triageEntries.clear();
    _inputBindings.clear();
    _modeTransitions = _defaultModeTransitions();

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

  @override
  Future<List<RhythmSceneDefinition>> getScenes({String? targetId}) async =>
      _demoScenes;

  static RhythmLightSceneEntry _rgbEntry(
    String nodeId,
    int r,
    int g,
    int b, {
    int brightness = 80,
  }) =>
      RhythmLightSceneEntry(
        target: RhythmLightTarget.node(nodeId),
        output: RhythmLightSceneOutput.on(
          brightness: brightness,
          color: RhythmLightColor.rgb(RhythmSceneRgbColor(r: r, g: g, b: b)),
          transitionMs: 800,
        ),
      );

  static RhythmLightSceneEntry _kelvinEntry(
    String nodeId,
    int kelvin, {
    int brightness = 70,
  }) =>
      RhythmLightSceneEntry(
        target: RhythmLightTarget.node(nodeId),
        output: RhythmLightSceneOutput.on(
          brightness: brightness,
          color: RhythmLightColor.kelvin(kelvin),
          transitionMs: 800,
        ),
      );

  /// A handful of vivid demo scenes so Mood mode has presets to show without a
  /// live server. Swatches in the picker are sampled from these colors.
  static final List<RhythmSceneDefinition> _demoScenes = [
    RhythmSceneDefinition(
      id: 'demo_scene_sunset',
      name: 'Sunset',
      description: 'Warm amber fading into dusk pink',
      light: RhythmLightScene(
        defaultTransitionMs: 800,
        entries: [
          _rgbEntry('demo_living_ceiling', 255, 122, 48, brightness: 78),
          _rgbEntry('demo_living_strip', 255, 56, 124, brightness: 70),
        ],
      ),
    ),
    RhythmSceneDefinition(
      id: 'demo_scene_lagoon',
      name: 'Lagoon',
      description: 'Cool ocean blues and teal',
      light: RhythmLightScene(
        defaultTransitionMs: 800,
        entries: [
          _rgbEntry('demo_living_ceiling', 0, 150, 255, brightness: 72),
          _rgbEntry('demo_living_strip', 0, 214, 180, brightness: 66),
          _rgbEntry('demo_floor_lamp', 90, 90, 255, brightness: 60),
        ],
      ),
    ),
    RhythmSceneDefinition(
      id: 'demo_scene_focus',
      name: 'Focus',
      description: 'Crisp, energizing daylight',
      light: RhythmLightScene(
        defaultTransitionMs: 600,
        entries: [
          _kelvinEntry('demo_office_desk', 5200, brightness: 100),
          _rgbEntry('demo_kitchen_main', 188, 214, 255, brightness: 90),
        ],
      ),
    ),
    RhythmSceneDefinition(
      id: 'demo_scene_candlelight',
      name: 'Candlelight',
      description: 'Soft, low, golden warmth',
      light: RhythmLightScene(
        defaultTransitionMs: 1200,
        entries: [
          _kelvinEntry('demo_bedroom_left', 2000, brightness: 22),
          _kelvinEntry('demo_bedroom_right', 2200, brightness: 18),
        ],
      ),
    ),
    RhythmSceneDefinition(
      id: 'demo_scene_forest',
      name: 'Forest',
      description: 'Fresh greens and moss',
      light: RhythmLightScene(
        defaultTransitionMs: 900,
        entries: [
          _rgbEntry('demo_kitchen_main', 40, 184, 92, brightness: 74),
          _rgbEntry('demo_kitchen_island', 154, 204, 40, brightness: 64),
        ],
      ),
    ),
    RhythmSceneDefinition(
      id: 'demo_scene_nebula',
      name: 'Nebula',
      description: 'Electric violet and magenta',
      light: RhythmLightScene(
        defaultTransitionMs: 1000,
        entries: [
          _rgbEntry('demo_living_ceiling', 142, 58, 255, brightness: 68),
          _rgbEntry('demo_living_strip', 255, 56, 184, brightness: 72),
          _rgbEntry('demo_floor_lamp', 44, 92, 255, brightness: 58),
        ],
      ),
    ),
  ];

  DemoServerSnapshot snapshot() {
    ensureSeeded();
    final now = DateTime.now().millisecondsSinceEpoch ~/ 1000;
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
    final reviewEntries = [
      for (final entry in triageEntries) _reviewEntryFromPending(entry, now),
      RhythmReviewEntry(
        id: 'demo_history_kept_separate',
        kind: 'device_merge',
        status: 'new_device',
        hubType: 'hue',
        hubAddress: _demoHubAddress,
        nativeId: 'demo-history-device',
        name: 'Desk Accent',
        createdAt: now - 86400 * 5,
        resolvedAt: now - 86400 * 4,
        resolvedBy: 'api',
        summary: 'Kept as a separate device',
        guidance: null,
      ),
      RhythmReviewEntry(
        id: 'demo_history_dismissed',
        kind: 'room_binding',
        status: 'dismissed',
        hubType: 'hue',
        hubAddress: _demoHubAddress,
        nativeId: 'demo-history-room',
        name: 'Guest Room',
        createdAt: now - 86400 * 3,
        resolvedAt: now - 86400 * 2,
        resolvedBy: 'api',
        summary: 'Room binding proposal dismissed',
        guidance: null,
      ),
    ];
    final hubConfiguredConflicts = reviewEntries
        .where(
          (entry) => entry.kind == 'hub_configured' && entry.isPending,
        )
        .toList(growable: false);

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
      review: RhythmReviewSummary(
        pending: RhythmReviewCounts(
          devices: deviceCount,
          rooms: roomCount,
          unassigned: _triageEntries.values
              .where((entry) => entry['kind'] == 'unassigned_device')
              .length,
          hubConfigured: _triageEntries.values
              .where((entry) => entry['kind'] == 'hub_configured')
              .length,
          total: triageEntries.length,
        ),
        triageEntries: reviewEntries,
        hubConfiguredConflicts: hubConfiguredConflicts,
      ),
      triagePendingCount: triageEntries.length,
      triagePendingDevices: deviceCount,
      triagePendingRooms: roomCount,
    );
  }

  RhythmReviewEntry _reviewEntryFromPending(
    Map<String, dynamic> entry,
    int now,
  ) {
    final kind = entry['kind'] as String? ?? 'device_merge';
    final hubKey = Map<String, dynamic>.from(
      entry['hub_key'] as Map? ?? const <String, dynamic>{},
    );
    final device = Map<String, dynamic>.from(
      switch (kind) {
        'unassigned_device' =>
          entry['unassigned_device'] as Map? ?? const <String, dynamic>{},
        'room_binding' =>
          entry['room_binding'] as Map? ?? const <String, dynamic>{},
        'hub_configured' =>
          entry['hub_configured'] as Map? ?? const <String, dynamic>{},
        _ => entry['discovered'] as Map? ?? const <String, dynamic>{},
      },
    );

    final summary = switch (kind) {
      'room_binding' =>
        'Review whether this hub room should merge into an existing Rhythm room',
      'unassigned_device' => 'Assign this device to a Rhythm room',
      'hub_configured' =>
        'Native hub automation is still configured for this device',
      _ => 'Review whether these endpoints represent the same physical device',
    };
    final guidance = switch (kind) {
      'room_binding' =>
        'Merge only when both rooms should act as one Rhythm room.',
      'unassigned_device' =>
        'Assign the device to keep routing and automations stable.',
      'hub_configured' =>
        'Remove the native automation in the hub app, then recheck.',
      _ => 'Keep separate only if they are different physical devices.',
    };

    return RhythmReviewEntry(
      id: entry['id'] as String? ?? '',
      kind: kind,
      status: 'pending',
      hubType:
          hubKey['hub_type'] as String? ?? device['hub_type'] as String? ?? '',
      hubAddress:
          hubKey['address'] as String? ?? device['address'] as String? ?? '',
      nativeId: device['id'] as String? ??
          device['native_id'] as String? ??
          device['hub_room_id'] as String? ??
          '',
      name: device['name'] as String? ??
          device['hub_room_name'] as String? ??
          'Review item',
      createdAt: now - 3600,
      summary: summary,
      guidance: guidance,
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
    (int, int, int)? color,
    RoomModeState? state,
  }) {
    ensureSeeded();
    final room = _nodeStates[roomId];
    if (room == null) return;
    room['lights_on'] = on;
    room['state'] =
        (state ?? (on ? RoomModeState.active : RoomModeState.hardOff))
            .wireValue;
    final effectiveState =
        state ?? (on ? RoomModeState.active : RoomModeState.hardOff);
    room['mood_enabled'] = (room['mood_enabled'] as bool? ?? false) ||
        effectiveState == RoomModeState.mood;
    room['mood_active'] = effectiveState == RoomModeState.mood;
    final standbyActive = effectiveState == RoomModeState.standby ||
        effectiveState == RoomModeState.idle;
    room['standby_active'] = standbyActive;
    if (standbyActive) {
      room['standby_enabled'] = true;
    }
    final profileSettings = Map<String, dynamic>.from(
        (room['profile_settings'] as Map?) ?? const {});
    profileSettings['mood_enabled'] = room['mood_enabled'];
    room['profile_settings'] = profileSettings;
    if (brightness != null) {
      room['brightness'] = brightness;
    }
    if (kelvin != null) {
      room['kelvin'] = kelvin;
    }
    if (color != null) {
      room['color'] = {'r': color.$1, 'g': color.$2, 'b': color.$3};
    }
  }

  @override
  Future<RhythmSettings?> getSettings() async {
    ensureSeeded();
    return RhythmSettings.fromJson({
      'power_save': _powerSave,
      'auto_update': _autoUpdate,
      'light_runtime': _lightRuntime.id,
    });
  }

  @override
  Future<bool> settingsSet({bool? powerSave, bool? autoUpdate}) async {
    ensureSeeded();
    var mutated = false;
    if (powerSave != null) {
      _powerSave = powerSave;
      mutated = true;
    }
    if (autoUpdate != null) {
      _autoUpdate = autoUpdate;
      mutated = true;
    }
    if (mutated) {
      _changes.add(null);
    }
    return true;
  }

  @override
  Future<RhythmLightBreaker?> getLightBreaker() async {
    ensureSeeded();
    return RhythmLightBreaker(enabled: _lightBreakerEnabled);
  }

  @override
  Future<bool> setLightBreaker(bool enabled) async {
    ensureSeeded();
    if (_lightBreakerEnabled != enabled) {
      _lightBreakerEnabled = enabled;
      _changes.add(null);
    }
    return true;
  }

  @override
  Future<RhythmModeResource?> getMode() async {
    ensureSeeded();
    return RhythmModeResource.fromJson({
      'active': _activeMode.wireValue,
      'light_runtime': _lightRuntime.id,
      'configs': [
        {
          'mode': 'day',
          'active_profile_id': 'rhythm',
        },
        {
          'mode': 'sleep',
          'active_profile_id': 'sleep',
        },
      ],
    });
  }

  @override
  Future<RhythmLightRuntimeState?> getLightRuntime() async {
    ensureSeeded();
    return RhythmLightRuntimeState(
      runtime: _lightRuntime,
      availableRuntimes: RhythmLightRuntime.values,
    );
  }

  @override
  Future<RhythmLightRuntimeState?> setLightRuntime(
    RhythmLightRuntime runtime, {
    int? transitionMs,
  }) async {
    ensureSeeded();
    _lightRuntime = runtime;
    _activeMode = RhythmMode.day;
    _changes.add(null);
    return getLightRuntime();
  }

  @override
  Future<void> setActiveMode(RhythmMode mode) async {
    ensureSeeded();
    _activeMode = mode;
    _changes.add(null);
  }

  @override
  Future<List<RhythmModeTransitionConfig>> getTransitions() async {
    ensureSeeded();
    return List<RhythmModeTransitionConfig>.unmodifiable(_modeTransitions);
  }

  @override
  Future<bool> setTransitions(
    List<RhythmModeTransitionConfig> transitions,
  ) async {
    ensureSeeded();
    _modeTransitions = List<RhythmModeTransitionConfig>.from(transitions);
    _changes.add(null);
    return true;
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
  Future<bool> renameCanonicalDevice(String id, String name) async {
    ensureSeeded();
    final trimmed = name.trim();
    if (trimmed.isEmpty) return false;
    final device = _canonicalDevices[id];
    final node = _topologyNodes[id];
    final nodeState = _nodeStates[id];
    if (device == null || node == null || nodeState == null) return false;
    device['name'] = trimmed;
    node['name'] = trimmed;
    nodeState['name'] = trimmed;
    _changes.add(null);
    return true;
  }

  @override
  Future<bool> flashCanonicalDevice(String id) async {
    ensureSeeded();
    return _canonicalDevices.containsKey(id);
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
  Future<bool> setTopologyNodeControlTargets({
    required String nodeId,
    required String controlKind,
    required List<String> targetIds,
  }) async {
    ensureSeeded();
    final node = _topologyNodes[nodeId];
    if (node == null ||
        targetIds.any((id) => !_topologyNodes.containsKey(id))) {
      return false;
    }

    final normalizedTargetIds = targetIds.toSet().toList()..sort();
    final existingControls = (node['controls'] as List<dynamic>? ?? const [])
        .whereType<Map>()
        .map((control) => control.cast<String, dynamic>())
        .where((control) => control['kind'] != controlKind)
        .toList();
    final parentId = node['parent_id'] as String?;
    node['controls'] = [
      ...existingControls,
      for (final targetId in normalizedTargetIds)
        {
          'kind': controlKind,
          'target_id': targetId,
          'inherited': false,
        },
      if (normalizedTargetIds.isEmpty && parentId != null)
        {
          'kind': controlKind,
          'target_id': parentId,
          'inherited': true,
        },
    ];
    _changes.add(null);
    return true;
  }

  @override
  Future<List<RhythmInputBinding>> getInputBindings() async {
    ensureSeeded();
    return _inputBindings.values.toList(growable: false);
  }

  @override
  Future<List<RhythmInputBinding>> createDaySleepToggleInputBinding({
    required String sourceNodeId,
    RhythmButtonAction? buttonAction = RhythmButtonAction.onPress,
    bool enabled = true,
  }) {
    return createPresetInputBinding(
      preset: RhythmInputBindingPreset.daySleepToggle,
      sourceNodeId: sourceNodeId,
      buttonAction: buttonAction,
      enabled: enabled,
    );
  }

  @override
  Future<List<RhythmInputBinding>> createPresetInputBinding({
    required RhythmInputBindingPreset preset,
    required String sourceNodeId,
    RhythmButtonAction? buttonAction,
    bool enabled = true,
  }) async {
    ensureSeeded();
    final binding = _presetBinding(
      id: _presetBindingId(preset, sourceNodeId, buttonAction),
      preset: preset,
      sourceNodeId: sourceNodeId,
      buttonAction: buttonAction,
      enabled: enabled,
    );
    _inputBindings[binding.id] = binding;
    _changes.add(null);
    return getInputBindings();
  }

  @override
  Future<List<RhythmInputBinding>> setInputBinding(
    RhythmInputBinding binding,
  ) async {
    ensureSeeded();
    _inputBindings[binding.id] = binding;
    _changes.add(null);
    return getInputBindings();
  }

  @override
  Future<List<RhythmInputBinding>> deleteInputBinding(String id) async {
    ensureSeeded();
    _inputBindings.remove(id);
    _changes.add(null);
    return getInputBindings();
  }

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

  /// Inject a fake Matter device into demo state.
  ///
  /// Used by [MatterDeviceAddScreen] when demo mode is on, so the demo user
  /// experiences a successful pair without a real backend. Notifies
  /// [changes] so [ServerSyncProvider] refreshes its topology.
  ///
  /// Returns the canonical device id assigned to the new node.
  String addDemoMatterDevice({
    required String nativeId,
    String? name,
    String manufacturer = 'Demo Lighting',
    String model = 'Matter Bulb',
  }) {
    ensureSeeded();
    final ordinal = _canonicalDevices.keys
            .where((id) => id.startsWith('demo_matter_'))
            .length +
        1;
    final id = 'demo_matter_$ordinal';
    final deviceName = name ?? 'Matter Bulb $ordinal';

    _topologyNodes[id] = {
      'id': id,
      'name': deviceName,
      'kind': 'light_device',
      'parent_id': null,
      'manufacturer': manufacturer,
      'model': model,
    };
    _canonicalDevices[id] = {
      'id': id,
      'name': deviceName,
      'device_type': 'light',
      'manufacturer': manufacturer,
      'model': model,
      'endpoints': [
        {
          'hub_key': const {
            'hub_type': 'matter',
            'address': 'demo-matter.local',
          },
          'native_id': nativeId,
          'preferred': true,
        },
      ],
    };
    _changes.add(null);
    return id;
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
      'mood_enabled': false,
      'mood_active': false,
      'profile_settings': const {'mood_enabled': false},
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
    if (hubTypes.contains('hue') || hubTypes.contains('hue_ble')) {
      return RoomSourceDto.hue;
    }
    if (hubTypes.contains('homeassistant') ||
        hubTypes.contains('home_assistant')) {
      return RoomSourceDto.homeAssistant;
    }
    if (hubTypes.contains('bridge')) return RoomSourceDto.bridge;
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

  RhythmInputBinding _presetBinding({
    required String id,
    required RhythmInputBindingPreset preset,
    required String sourceNodeId,
    required RhythmButtonAction? buttonAction,
    required bool enabled,
  }) {
    return RhythmInputBinding(
      id: id,
      preset: preset,
      sourceNodeId: sourceNodeId,
      trigger: RhythmInputBindingTrigger.button(buttonAction: buttonAction),
      action: const RhythmModeCycleAction(
        modes: [RhythmMode.day, RhythmMode.sleep],
        transition: RhythmModeTransitionSelection.auto(),
      ),
      enabled: enabled,
    );
  }

  String _presetBindingId(
    RhythmInputBindingPreset preset,
    String sourceNodeId,
    RhythmButtonAction? buttonAction,
  ) {
    final source = sourceNodeId
        .split('')
        .map((char) => RegExp(r'[A-Za-z0-9_-]').hasMatch(char) ? char : '_')
        .join();
    return '${preset.wireValue}:$source:${buttonAction?.wireValue ?? 'any'}';
  }
}
