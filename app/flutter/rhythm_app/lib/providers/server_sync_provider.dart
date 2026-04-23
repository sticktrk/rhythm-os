/// Server sync provider — bridges SDK connection with app providers.
///
/// Handles:
/// - Auto-connect to server when hub exists
/// - Hello reconciliation (accept server rooms + config, push location)
/// - Server-driven room sync (server discovers rooms from hub)
/// - rhythm_state events → update RoomProvider
/// - Location pushes from app → server
library;

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter_timezone/flutter_timezone.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../services/cloud_backed_server_api.dart';
import '../services/demo_server_api.dart';
import '../services/hue/demo_hue_bridge_service.dart';
import '../services/hue/hue_service_locator.dart';
import 'home_provider.dart';
import 'room_provider.dart';

/// Whether a timezone string looks like a proper IANA name (contains '/').
/// Abbreviations like "EST", "PST" don't handle DST transitions.
bool _isIanaTimezone(String? tz) => tz != null && tz.contains('/');

/// Syncs app state with a server (ESP32, rhythm-server, addon) via
/// [RhythmConnection] from the SDK.
///
/// This provider is the glue between [RhythmConnection], [RoomProvider],
/// and [HomeProvider]. It manages:
///
/// 1. **Auto-connect**: When a server hub exists, connects the HTTP client.
/// 2. **Hello reconciliation**: On connect, accepts server's rooms and config
///    as authoritative and pushes any location differences.
/// 3. **Server-driven room sync**: When hub pairing changes, triggers
///    server-side re-discovery via `POST /api/sync`.
/// 4. **Rhythm state sync**: Listens for server rhythm_state events and updates
///    RoomProvider (server is authoritative for rhythm state).
class ServerSyncProvider extends ChangeNotifier {
  final RhythmConnection _connection;
  final RoomProvider _roomProvider;
  final HomeProvider _homeProvider;

  StreamSubscription<RhythmHello>? _helloSub;
  StreamSubscription<RhythmRoomState>? _rhythmStateSub;
  StreamSubscription<({String event, String? hubType})>? _hubEventSub;
  StreamSubscription<RoomSourceDto>? _sourceChangedSub;
  StreamSubscription<RhythmMotionTimer>? _motionTimerSub;
  StreamSubscription<void>? _newNodesSub;
  StreamSubscription<Map<String, dynamic>>? _triageChangedSub;
  StreamSubscription<RhythmConnectionState>? _connectionStateSub;
  StreamSubscription<void>? _demoChangeSub;

  /// Suppresses push-back when receiving rhythm_state from server.
  bool _receivingFromServer = false;

  /// Prevents redundant sync triggers during hello processing.
  bool _isProcessingHello = false;

  /// Suppresses the next source-change sync after hello populates rooms.
  ///
  /// The `_isProcessingHello` guard doesn't catch source-change events because
  /// the broadcast stream delivers them asynchronously (after the flag resets).
  /// This flag bridges that gap — set in `_onHello`, cleared in
  /// `_onSourceRoomsChanged`.
  bool _suppressNextSourceSync = false;

  /// Last known server hub for auto-connect.
  Hub? _serverHub;

  /// All hub infos from the last server hello: [{type, address, connected}, ...].
  List<Map<String, dynamic>> _lastHubInfos = [];

  /// Host capability metadata from the last server hello.
  RhythmCapabilities? _capabilities;

  /// Cooldown: last time a hub-connected event triggered a re-hello.
  DateTime? _lastHubReconnectTime;

  /// Cooldown: last time a hub disconnect triggered a state refresh.
  DateTime? _lastHubDisconnectRefreshTime;

  /// Full node-state snapshot from the last server hello.
  List<RhythmRoom> _helloNodes = [];

  /// Cached room summaries derived from hello/topology for room-centric UI.
  List<RhythmRoom> _helloRooms = [];

  /// Raw topology graph from `/api/topology/nodes`.
  List<RhythmTopologyNode> _topologyNodes = [];

  /// Previous connection state for detecting transitions.
  RhythmConnectionState _previousConnectionState =
      RhythmConnectionState.disconnected;

  /// Tracks the last poll time for debouncing [fullRefresh] and [pollNow].
  DateTime _lastPollTime = DateTime.fromMillisecondsSinceEpoch(0);

  /// Firmware version reported by server in the hello message.
  String _firmwareVersion = '0.0.0';

  /// Platform type reported by server ("desktop" or "embedded").
  String _serverPlatformType = 'desktop';

  /// Deployment context reported by server ("ha_addon", "server", "embedded", etc.).
  String _serverPlatformContext = 'server';

  /// Whether power-save mode is active on the server.
  bool _powerSave = false;

  /// Active global mode from the server (`day` / `sleep`).
  RhythmMode? _activeMode;

  /// Saved mode transitions from the server.
  List<RhythmModeTransitionConfig> _modeTransitions = const [];

  /// Mode configs from the server (profile routing per mode).
  List<RhythmModeConfig> _modeConfigs = const [];

  /// Profile configs from the server hello.
  List<RhythmCurveConfig> _profiles = const [];

  /// Active resolved profile ID from `/api/state.active_profile`.
  String? _activeProfileId;

  /// Rhythm update interval in seconds from server settings.
  int _rhythmIntervalSecs = 60;

  /// Effective fade duration from server (auto-computed, always present).
  int? _effectiveFadeMs;

  /// Effective motion timeout from server (auto-computed, always present).
  int? _effectiveMotionTimeoutSecs;

  /// Review summary from `/api/state.review`.
  RhythmReviewSummary _review = const RhythmReviewSummary();

  /// Pending triage counts from SSE triage_changed events.
  int _triagePendingCount = 0;
  int _triagePendingDevices = 0;
  int _triagePendingRooms = 0;

  /// Whether we're currently connected and synced.
  bool get synced => _connection.connected || HueServiceLocator.isDemoMode;

  /// Whether the server can dispatch room actions.
  bool get canDispatchActions =>
      _connection.connected || HueServiceLocator.isDemoMode;

  /// Connection state of the underlying connection.
  RhythmConnectionState get connectionState => HueServiceLocator.isDemoMode
      ? RhythmConnectionState.connected
      : _connection.connectionState;

  /// The server entry currently selected for the active connection.
  Hub? get connectedServerHub => _serverHub;

  /// Firmware version reported by server.
  String get firmwareVersion => _firmwareVersion;

  /// Platform type reported by server ("desktop" or "embedded").
  String get serverPlatformType => _serverPlatformType;

  /// Deployment context reported by server ("ha_addon", "server", "embedded", etc.).
  String get serverPlatformContext => _serverPlatformContext;

  /// Whether the connected server is embedded (constrained sockets/memory).
  bool get isEmbeddedServer => _serverPlatformType == 'embedded';

  /// Whether power-save mode is active on the server.
  bool get powerSave => _powerSave;

  /// Whether sleep mode is active on the server.
  bool get sleepMode => _activeMode == RhythmMode.sleep;

  /// Active global mode.
  RhythmMode? get activeMode => _activeMode;

  /// Saved mode transitions.
  List<RhythmModeTransitionConfig> get modeTransitions => _modeTransitions;

  /// Mode configs (profile routing per mode).
  List<RhythmModeConfig> get modeConfigs => _modeConfigs;

  /// Profile configs from the server.
  List<RhythmCurveConfig> get profiles => _profiles;

  /// Active resolved profile ID for display/edit sync.
  String? get activeProfileId => _activeProfileId;

  /// Rhythm update interval in seconds.
  int get rhythmIntervalSecs => _rhythmIntervalSecs;

  /// Effective fade duration from server (auto-computed).
  int? get effectiveFadeMs => _effectiveFadeMs;

  /// Effective motion timeout from server (auto-computed).
  int? get effectiveMotionTimeoutSecs => _effectiveMotionTimeoutSecs;

  /// Restore/review metadata from the latest server state snapshot.
  RhythmReviewSummary get review => _review;

  /// Hubs that still need to reconnect after restore or startup.
  List<RhythmReviewHub> get disconnectedReviewHubs => review.disconnectedHubs;

  /// Devices that preserved an explicit preferred endpoint.
  List<RhythmPreferredEndpoint> get preferredReviewEndpoints =>
      review.preferredEndpoints;

  /// Pending native-automation conflicts.
  List<RhythmReviewEntry> get hubConfiguredConflicts =>
      review.hubConfiguredConflicts;

  /// Recent resolved triage history carried in the state snapshot.
  List<RhythmReviewEntry> get reviewHistory => review.resolvedEntries;

  /// Whether review follow-up is still needed after restore/reconnect.
  bool get hasReviewAttention => review.hasAttention;

  /// Total pending triage entries (devices + rooms).
  int get triagePendingCount => _triagePendingCount;

  /// Pending device merge entries.
  int get triagePendingDevices => _triagePendingDevices;

  /// Pending room binding entries.
  int get triagePendingRooms => _triagePendingRooms;

  /// All hub infos from last server hello.
  List<Map<String, dynamic>> get serverHubInfos => _lastHubInfos;

  /// Parsed hub infos from last server hello.
  List<RhythmHubInfo> get serverHubs => _lastHubInfos
      .map(RhythmHubInfo.fromJson)
      .where((hub) => hub.configured)
      .toList(growable: false);

  /// Host capabilities from the last server hello, if the server advertises them.
  RhythmCapabilities? get serverCapabilities => _capabilities;

  /// Whether the host explicitly advertised supported hub types.
  bool get hasExplicitHubCapabilities => _capabilities != null;

  /// Explicit per-hub capabilities, keyed by hub type.
  RhythmHubCapabilities? hubCapabilities(String hubType) {
    final normalized = switch (hubType) {
      'home_assistant' => 'homeassistant',
      _ => hubType,
    };
    return _capabilities?.hub(normalized) ?? _capabilities?.hub(hubType);
  }

  /// Whether the current host supports configuring the given hub type.
  ///
  /// Legacy servers omit hub capability metadata entirely, so we default to
  /// the historical UI behavior when that block is absent.
  bool canConfigureHub(String hubType) {
    if (!hasExplicitHubCapabilities) return true;
    return hubCapabilities(hubType)?.configurable ?? false;
  }

  RhythmHubCapabilities? get matterCapabilities => hubCapabilities('matter');

  /// Whether the server advertises explicit Matter add methods.
  bool get hasExplicitMatterCapabilities => matterCapabilities != null;

  /// Whether the UI should offer any Matter add-device entry point.
  bool get canAddMatterDevice =>
      matterCapabilities?.canAddDevice ?? !hasExplicitHubCapabilities;

  /// Add an already-on-network Matter device via setup code / QR.
  bool get canAddMatterOnNetworkDevice =>
      matterCapabilities?.supportsDeviceOnboardingMethod(
        RhythmDeviceOnboardingMethod.matterOnNetworkSetupCode,
      ) ??
      false;

  /// Commission a new Matter device over BLE using stored Wi-Fi credentials.
  bool get canCommissionMatterBleWifi =>
      matterCapabilities?.supportsDeviceOnboardingMethod(
        RhythmDeviceOnboardingMethod.matterBleWifiCommissioning,
      ) ??
      false;

  /// Whether the UI should allow Matter decommissioning.
  bool get canUnpairMatterDevices =>
      matterCapabilities?.supportsUnpairing ?? !hasExplicitHubCapabilities;

  /// Whether a Matter device may exist before room assignment.
  bool get supportsMatterRoomlessDevices =>
      matterCapabilities?.supportsRoomlessDevices ??
      !hasExplicitHubCapabilities;

  /// Whether no hubs are configured on the server.
  bool get hasNoHubConfigured =>
      _lastHubInfos.isEmpty ||
      _lastHubInfos.every((h) => h['type'] == 'none' || h['type'] == null);

  /// Hub types currently connected on the server.
  Set<String> get connectedHubTypes => _lastHubInfos
      .where((h) => h['connected'] == true && h['type'] != 'none')
      .map((h) => h['type'] as String)
      .toSet();

  /// Hub types configured on the server (connected or not).
  Set<String> get configuredHubTypes => _lastHubInfos
      .where((h) => h['type'] != null && h['type'] != 'none')
      .map((h) => h['type'] as String)
      .toSet();

  /// Whether a room's hub is currently connected on the server.
  bool isRoomHubConnected(RoomSourceDto source) {
    final hubType = switch (source) {
      RoomSourceDto.matter => 'matter',
      RoomSourceDto.hue => 'hue',
      RoomSourceDto.homeAssistant => 'homeassistant',
      RoomSourceDto.esp32 => 'esp32',
      _ => null,
    };
    // If we don't know the hub type, or have no hub info yet, assume connected.
    if (hubType == null || _lastHubInfos.isEmpty) return true;
    // If this hub type isn't even configured, assume connected (local-only).
    if (!configuredHubTypes.contains(hubType)) return true;
    return connectedHubTypes.contains(hubType);
  }

  /// Whether the server needs a location (has a Hue hub and runs as HA addon).
  bool get serverNeedsLocation {
    if (!configuredHubTypes.contains('hue')) return false;
    return _serverPlatformContext == 'ha_addon';
  }

  /// Light-addressable nodes from the last server hello.
  List<RhythmRoom> get helloNodes => _helloNodes;

  /// Room summaries derived from the last hello/topology refresh.
  List<RhythmRoom> get helloRooms => _helloRooms;

  /// Raw node topology graph for topology/editor plumbing.
  List<RhythmTopologyNode> get topologyNodes => _topologyNodes;

  RhythmRoom? nodeById(String nodeId) =>
      _helloNodes.where((node) => node.id == nodeId).firstOrNull;

  RhythmTopologyNode? topologyNodeById(String nodeId) =>
      _topologyNodes.where((node) => node.id == nodeId).firstOrNull;

  Iterable<RhythmTopologyControlLink> controlsForSourceNode(String nodeId) =>
      topologyNodeById(nodeId)?.controls ?? const [];

  RhythmTopologyControlLink? controlForSourceNode(
    String nodeId,
    String controlKind,
  ) {
    for (final control in controlsForSourceNode(nodeId)) {
      if (control.kind == controlKind) return control;
    }
    return null;
  }

  String? controlTargetNodeId({
    required String sourceNodeId,
    required String controlKind,
  }) {
    return controlForSourceNode(sourceNodeId, controlKind)?.targetId;
  }

  List<RhythmTopologyNode> controlSourceNodesForTarget({
    required String targetNodeId,
    String? controlKind,
  }) {
    return _topologyNodes.where((node) {
      for (final control in node.controls) {
        if (control.targetId != targetNodeId) continue;
        if (controlKind == null || control.kind == controlKind) return true;
      }
      return false;
    }).toList();
  }

  bool nodeHasIncomingControl({
    required String targetNodeId,
    String? controlKind,
  }) {
    return controlSourceNodesForTarget(
      targetNodeId: targetNodeId,
      controlKind: controlKind,
    ).isNotEmpty;
  }

  bool nodeHasMotionControlTarget(String targetNodeId) =>
      nodeHasIncomingControl(
        targetNodeId: targetNodeId,
        controlKind: 'motion',
      );

  bool isNodeLightDevice(String nodeId) =>
      nodeById(nodeId)?.kind == RhythmNodeKind.lightDevice;

  bool isNodeRoom(String nodeId) => nodeById(nodeId)?.kind.isRoom ?? true;

  RhythmDevice? deviceForNode(String nodeId) {
    final topologyNode = topologyNodeById(nodeId);
    if (topologyNode == null || !topologyNode.isDevice) return null;
    return RhythmDevice.fromTopologyNode(topologyNode);
  }

  /// Number of typed lights for a room (0 until lights are typed in the backend).
  int lightCountForRoom(String roomId) {
    return _helloRooms.where((r) => r.id == roomId).firstOrNull?.lightCount ??
        0;
  }

  /// All typed devices for a room (lights, buttons, motion sensors).
  List<RhythmDevice> devicesForRoom(String roomId) {
    return _helloRooms.where((r) => r.id == roomId).firstOrNull?.devices ?? [];
  }

  /// Human-readable device summary for a room (e.g. "4 lights, 2 buttons").
  String deviceSummaryForRoom(String roomId) {
    final roomSummary = _helloRooms.where((r) => r.id == roomId).firstOrNull;
    if (roomSummary != null) return roomSummary.deviceSummary;
    final node = nodeById(roomId);
    if (node?.kind == RhythmNodeKind.lightDevice) return '1 light';
    return _helloRooms
            .where((r) => r.id == roomId)
            .firstOrNull
            ?.deviceSummary ??
        '';
  }

  /// All rooms grouped by hub type (for hub debug sections).
  /// Multi-hub rooms appear under each of their hub types.
  Map<String, List<RhythmRoom>> get roomsByHubType {
    final result = <String, List<RhythmRoom>>{};
    for (final room in _helloRooms) {
      if (room.hubTypes.isNotEmpty) {
        for (final key in room.hubTypes) {
          (result[key] ??= []).add(room);
        }
      } else {
        (result['unknown'] ??= []).add(room);
      }
    }
    return result;
  }

  /// All unique devices for a hub type, de-duplicated and sorted lights->buttons->motion.
  List<RhythmDevice> devicesForHub(String hubType) {
    final seen = <String>{};
    final devices = <RhythmDevice>[];
    for (final room in _helloRooms.where((r) => r.hubTypes.contains(hubType))) {
      for (final device in room.devices) {
        if (seen.add(device.id)) {
          devices.add(device);
        }
      }
    }
    devices.sort((a, b) {
      const order = {
        RhythmDeviceType.light: 0,
        RhythmDeviceType.button: 1,
        RhythmDeviceType.motion: 2
      };
      return (order[a.type] ?? 3).compareTo(order[b.type] ?? 3);
    });
    return devices;
  }

  /// Summary string for a hub type (e.g. "12 lights, 4 buttons across 5 rooms").
  String deviceSummaryForHub(String hubType) {
    final devices = devicesForHub(hubType);
    final rooms = roomsByHubType[hubType] ?? [];
    final l = devices.where((d) => d.type == RhythmDeviceType.light).length;
    final b = devices.where((d) => d.type == RhythmDeviceType.button).length;
    final m = devices.where((d) => d.type == RhythmDeviceType.motion).length;
    final parts = <String>[];
    if (l > 0) parts.add('$l light${l > 1 ? 's' : ''}');
    if (b > 0) parts.add('$b button${b > 1 ? 's' : ''}');
    if (m > 0) parts.add('$m sensor${m > 1 ? 's' : ''}');
    if (parts.isEmpty) return 'No devices';
    final roomCount = rooms.length;
    return '${parts.join(', ')} across $roomCount room${roomCount > 1 ? 's' : ''}';
  }

  /// The underlying SDK connection (for SSE debug props, ping, etc.).
  RhythmConnection get connection => _connection;

  /// The server API client (available after connect).
  CloudBackedServerApi get api => CloudBackedServerApi(
        delegate: HueServiceLocator.isDemoMode
            ? DemoServerApi.instance
            : _connection.api,
      );

  ServerSyncProvider({
    required RhythmConnection connection,
    required RoomProvider roomProvider,
    required HomeProvider homeProvider,
  })  : _connection = connection,
        _roomProvider = roomProvider,
        _homeProvider = homeProvider {
    // Listen for connection events
    _helloSub = _connection.helloEvents.listen(_onHello);
    _rhythmStateSub = _connection.rhythmStateEvents.listen(_onRhythmState);
    _hubEventSub = _connection.hubEvents.listen(_onHubEvent);
    _motionTimerSub = _connection.motionTimerEvents.listen(_onMotionTimer);
    _newNodesSub = _connection.newNodesDetected.listen(_onNewNodesDetected);
    _triageChangedSub =
        _connection.triageChangedEvents.listen(_onTriageChanged);

    // Listen for room source changes (Hue pairing, re-sync, disconnect)
    _sourceChangedSub =
        _roomProvider.onSourceRoomsChanged.listen(_onSourceRoomsChanged);

    // Listen for connection state changes
    _connectionStateSub =
        _connection.connectionStateStream.listen(_onConnectionStateChanged);

    _demoChangeSub = DemoServerApi.instance.changes.listen((_) {
      if (!HueServiceLocator.isDemoMode) return;
      unawaited(_refreshDemoState());
    });
  }

  /// Connect to server if a hub is available.
  ///
  /// Called from the ProxyProvider update. Since ProxyProvider fires on
  /// every dependency change (including frequent RoomProvider notifies),
  /// this method is guarded to only act when the hub config actually changes.
  void connectIfAvailable() {
    // Demo mode: set the server hub reference so UI sees a hub,
    // but don't actually connect to the fake 127.0.0.1 host.
    if (HueServiceLocator.isDemoMode) {
      final hubs = _homeProvider.currentHomeHubs;
      final serverHub = hubs.where((h) => h.type == HubType.server).firstOrNull;
      if (serverHub != null) {
        final hubChanged = _serverHub?.id != serverHub.id;
        _serverHub = serverHub;
        if (hubChanged) {
          notifyListeners();
        }
        unawaited(_refreshDemoState());
      }
      return;
    }

    final hubs = _homeProvider.currentHomeHubs;
    final serverHub = hubs.where((h) => h.type == HubType.server).firstOrNull;

    if (serverHub != null) {
      if (serverHub.id == _serverHub?.id &&
          serverHub.endpoint.host == _serverHub?.endpoint.host &&
          serverHub.endpoint.port == _serverHub?.endpoint.port) {
        return; // Same hub, no change
      }
      debugPrint(
          'ServerSync: connectIfAvailable — connecting to ${serverHub.endpoint.host}:${serverHub.endpoint.port}');
      _serverHub = serverHub;
      // Defer all side-effects to avoid notifyListeners during ProxyProvider build phase
      final host = serverHub.endpoint.host;
      final port = serverHub.endpoint.port;
      Future.microtask(() async {
        _roomProvider.clearTransientState();
        _connection.connect(host, port: port);
      });
    } else if (_serverHub != null) {
      debugPrint('ServerSync: connectIfAvailable — hub removed, disconnecting');
      _serverHub = null;
      Future.microtask(() async {
        await _roomProvider.clearAllRooms();
        _connection.disconnect();
      });
    }
  }

  /// Trigger a full reconnect (for pull-to-refresh).
  ///
  /// Re-fetches `GET /api/state` and emits a fresh hello so the UI gets the
  /// complete picture (devices, config, rooms, sensors, settings).
  ///
  /// Skips if called within 2 seconds of the last refresh to protect the server
  /// from rapid pull-refreshes.
  Future<void> fullRefresh() async {
    if (HueServiceLocator.isDemoMode) {
      await _refreshDemoState();
      return;
    }
    final now = DateTime.now();
    if (now.difference(_lastPollTime).inSeconds < 2) return;
    _lastPollTime = now;
    await _connection.reconnect();
  }

  /// Trigger an immediate lightweight poll (rooms/state only).
  Future<void> pollNow() async {
    if (HueServiceLocator.isDemoMode) {
      await _refreshDemoState();
      return;
    }
    final now = DateTime.now();
    if (now.difference(_lastPollTime).inSeconds < 2) return;
    _lastPollTime = now;
    await _connection.pollNow();
  }

  // ============================================================================
  // Event handlers (Server → App)
  // ============================================================================

  /// Handle hello from server — accept rooms and reconcile config.
  void _onHello(RhythmHello hello) {
    debugPrint(
        'ServerSync: Hello received with ${hello.nodes.length} nodes, version=${hello.version}');
    debugPrint('ServerSync: Server active profile: ${hello.activeProfile}');
    debugPrint('ServerSync: Server location: ${hello.location}');
    for (final r in hello.nodes) {
      debugPrint(
          'ServerSync: Server node "${r.name}" kind=${r.kind.name} rhythm=${r.rhythmEnabled} offset=${r.timeOffset} state=${r.state.wireValue} transitioning=${r.transitioning}');
    }
    _firmwareVersion = hello.version;
    _serverPlatformType = hello.platformType;
    _serverPlatformContext = hello.platformContext;
    RhythmCurveConfig? activeProfileConfig;
    if (hello.activeProfile.isNotEmpty) {
      try {
        activeProfileConfig = RhythmCurveConfig.fromJson(hello.activeProfile);
      } catch (e) {
        debugPrint('ServerSync: Failed to parse active profile config: $e');
      }
    }
    _powerSave = hello.settings?.powerSave ?? false;
    _activeMode = hello.mode?.active;
    _modeTransitions = [...hello.transitions];
    _modeConfigs = [...?hello.mode?.configs];
    _profiles = [...hello.profiles];
    _activeProfileId = hello.activeProfile['id'] as String? ??
        hello.mode?.activeConfig?.activeProfileId;
    _rhythmIntervalSecs = activeProfileConfig?.rhythmIntervalSecs ?? 60;
    _effectiveFadeMs = activeProfileConfig?.fadeMs ?? hello.effectiveFadeMs;
    _effectiveMotionTimeoutSecs = activeProfileConfig?.motionTimeoutSecs ??
        hello.effectiveMotionTimeoutSecs;
    _review = hello.review;
    _helloNodes = hello.nodes;
    _helloRooms = _buildRoomSummaries();
    _lastHubInfos = hello.hubs;
    _capabilities = hello.capabilities;

    // Bootstrap countdown timer from server's last tick timestamp
    if (hello.lastTickEpochMs != null) {
      final lastTick =
          DateTime.fromMillisecondsSinceEpoch(hello.lastTickEpochMs!);
      for (final room in hello.nodes) {
        if (room.id.isNotEmpty && room.rhythmEnabled) {
          _roomProvider.setLastTickTime(room.id, lastTick);
        }
      }
    }

    _isProcessingHello = true;
    // Only suppress the next source-change event if we're actually going to
    // add rooms (which triggers addRoomsFromSource → onSourceRoomsChanged).
    // If the server has 0 rooms, no event fires, and a stale suppress flag
    // would eat the next real event (e.g. Hue pairing).
    _suppressNextSourceSync = hello.nodes.isNotEmpty;
    try {
      // 1. Accept server nodes as authoritative.
      _acceptServerNodes(hello.nodes);

      // 2. Reconcile motion sensors — mark rooms that have sensors,
      //    unmark rooms that lost their sensors since last hello
      _roomProvider.setMotionSensorNodes(_sensorTargetNodeIds());

      // 3. Accept server config as authoritative, push location if different
      _acceptServerConfig(hello.activeProfile);
      _pushLocationIfUnset(hello.location);
    } finally {
      _isProcessingHello = false;
    }

    // Fetch initial triage count (non-blocking). The hello stream can deliver
    // a buffered event after the connection has been torn down (e.g. right
    // after a factory reset), so skip if the underlying api is gone.
    if (_connection.connected) {
      api.getTriageCount().then((data) {
        if (data != null) _onTriageChanged(data);
      });
    }
    _refreshTopologyNodes();

    notifyListeners();
  }

  /// Accept light-addressable nodes from the server as the authoritative source.
  ///
  /// The backend now exposes both rooms and individual bulbs as nodes. The UI
  /// only materializes light-addressable nodes into cards, so buttons and
  /// sensors remain in topology metadata but do not become room cards.
  void _acceptServerNodes(List<RhythmRoom> serverNodes) {
    final validNodes = serverNodes
        .where((node) => node.id.isNotEmpty && node.kind.isLightAddressable)
        .toList();
    if (validNodes.isEmpty) {
      debugPrint(
          'ServerSync: Server has no light-addressable nodes — clearing local rooms');
      _roomProvider.clearAllRooms();
      return;
    }

    // Group nodes by a canonical source derived from their own hub types.
    final grouped = <RoomSourceDto, List<RhythmRoom>>{};
    for (final sr in validNodes) {
      final source = _canonicalSourceForNode(sr);
      (grouped[source] ??= []).add(sr);
    }
    debugPrint(
        'ServerSync: Accepting ${validNodes.length} light nodes across ${grouped.length} source(s): ${grouped.entries.map((e) => '${e.key}=${e.value.length}').join(', ')}');

    final serverRoomIds = validNodes.map((r) => r.id).toSet();
    for (final otherSource in RoomSourceDto.values) {
      if (grouped.containsKey(otherSource)) continue;
      final stale = _roomProvider
          .getRoomsBySource(otherSource)
          .where((r) => !serverRoomIds.contains(r.id))
          .toList();
      if (stale.isNotEmpty) {
        debugPrint(
            'ServerSync: Removing ${stale.length} stale room(s) from source=$otherSource');
        for (final room in stale) {
          _roomProvider.removeRoom(room.id);
        }
      }
    }

    // Add each source group atomically while preserving user-owned runtime state.
    for (final entry in grouped.entries) {
      final source = entry.key;
      final rooms = <RoomDto>[];
      for (final sr in entry.value) {
        rooms.add(RoomDto(
          id: sr.id,
          name: sr.name,
          source: source,
          kind: _roomNodeKindFromSdk(sr.kind),
          parentId: sr.parentId,
          placement: _roomNodePlacementFromSdk(sr.placement),
          deviceIds: sr.kind == RhythmNodeKind.lightDevice
              ? [sr.id]
              : sr.devices
                  .where((d) => d.type == RhythmDeviceType.light)
                  .map((d) => d.id)
                  .toList(),
          rhythmEnabled: sr.rhythmEnabled,
          disabled: sr.disabled,
          lightsOn: sr.lightsOn ?? false,
          timeOffsetMinutes: sr.timeOffset,
          brightnessOffset: sr.brightnessOffset,
        ));
      }
      _roomProvider.addRoomsFromSource(source, rooms);
    }

    // Apply runtime state from server atomically — single save + notify per node.
    _receivingFromServer = true;
    try {
      for (final sr in validNodes) {
        if (_roomProvider.getNode(sr.id) != null) {
          _roomProvider.applyServerNodeState(
            sr.id,
            rhythmEnabled: sr.rhythmEnabled,
            timeOffset: sr.timeOffset,
            brightnessOffset: sr.brightnessOffset,
            state: sr.state,
            transitioning: sr.transitioning,
            lightsOn: sr.lightsOn,
            brightness: sr.brightness,
            kelvin: sr.kelvin,
          );
        }
      }
    } finally {
      _receivingFromServer = false;
    }
  }

  /// Handle source rooms changed (Hue pairing, re-sync, disconnect).
  ///
  /// Hub credentials are only pushed via explicit user action (hub picker,
  /// configurator screens, server settings) — not automatically here.
  void _onSourceRoomsChanged(RoomSourceDto source) {
    debugPrint(
        'ServerSync: onSourceRoomsChanged($source), connected=${_connection.connected}, processingHello=$_isProcessingHello, suppressSync=$_suppressNextSourceSync');
    if (!_connection.connected || _isProcessingHello) return;
    if (_suppressNextSourceSync) {
      _suppressNextSourceSync = false;
      debugPrint(
          'ServerSync: Suppressing source sync (hello just populated rooms)');
      return;
    }
  }

  /// Handle rhythm_state from server (button event, tick, poll diff).
  void _onRhythmState(RhythmRoomState state) {
    _receivingFromServer = true;
    try {
      if (_roomProvider.getNode(state.nodeId) == null) return;
      _roomProvider.applyServerNodeState(
        state.nodeId,
        rhythmEnabled: state.rhythmEnabled,
        timeOffset: state.timeOffset,
        brightnessOffset: state.brightnessOffset,
        state: state.state,
        transitioning: state.transitioning,
        mode: state.mode,
        lightsOn: state.lightsOn,
        brightness: state.brightness,
        kelvin: state.kelvin,
        color: state.color != null
            ? (state.color!.r, state.color!.g, state.color!.b)
            : null,
        tick: state.tick,
      );
    } finally {
      _receivingFromServer = false;
    }
  }

  /// Handle motion timer updates from server.
  void _onMotionTimer(RhythmMotionTimer event) {
    // Any motion event (even clearing) means this node has a sensor.
    _roomProvider.markNodeHasSensor(event.nodeId);

    // Idle sensor (no active motion, no countdown) — just mark presence.
    final isIdle = !event.motionActive && event.remainingSecs == null;
    if (event.isCleared || isIdle) {
      _roomProvider.clearNodeMotionTimer(event.nodeId);
    } else {
      _roomProvider.updateNodeMotionTimer(
        event.nodeId,
        MotionTimerInfo(
          motionActive: event.motionActive,
          motionOwned: event.motionOwned,
          remainingSecs: event.remainingSecs,
          timeoutSecs: event.timeoutSecs,
          receivedAt: DateTime.now(),
        ),
      );
    }
  }

  /// Handle new nodes detected in poll — trigger a full re-hello.
  void _onNewNodesDetected(void _) {
    debugPrint('ServerSync: New nodes detected in poll — triggering re-hello');
    _connection.reconnect();
  }

  /// Handle hub lifecycle events from server.
  ///
  /// When a hub connects, trigger a re-hello to pick up newly discovered
  /// rooms. When a hub disconnects, just update the UI.
  void _onHubEvent(({String event, String? hubType}) hubEvent) {
    final (:event, :hubType) = hubEvent;
    debugPrint('ServerSync: hub_status=$event hub=$hubType');
    // Update the specific hub's connected state in _lastHubInfos.
    // If hubType is null (legacy/aggregate event), ignore it — the hello
    // provides authoritative per-hub status and we can't safely guess
    // which hub this event refers to.
    if (_lastHubInfos.isNotEmpty && hubType != null) {
      _lastHubInfos = [
        for (final h in _lastHubInfos)
          if (h['type'] == hubType)
            {...h, 'connected': event == 'connected'}
          else
            h,
      ];
      notifyListeners();
    }

    // A hub just connected — re-fetch full state to pick up new rooms.
    // Guard against rapid re-entry: if we already triggered a re-hello
    // within the last 5 seconds, skip — the previous hello will have
    // picked up the new state.
    if (event == 'connected') {
      final now = DateTime.now();
      if (_lastHubReconnectTime != null &&
          now.difference(_lastHubReconnectTime!).inSeconds < 5) {
        debugPrint('ServerSync: Hub connected — skipping re-hello (cooldown)');
        return;
      }
      _lastHubReconnectTime = now;
      debugPrint(
          'ServerSync: Hub connected — triggering re-hello for room sync');
      unawaited(_connection.reconnect());
      return;
    }

    // Retry metadata now lives in /api/state, not hub_status, so refetch
    // when a hub disconnects if the UI is surfacing startup backoff state.
    if (event == 'disconnected') {
      final now = DateTime.now();
      if (_lastHubDisconnectRefreshTime != null &&
          now.difference(_lastHubDisconnectRefreshTime!).inSeconds < 2) {
        debugPrint(
            'ServerSync: Hub disconnected — skipping state refresh (cooldown)');
        return;
      }
      _lastHubDisconnectRefreshTime = now;
      debugPrint(
          'ServerSync: Hub disconnected — refreshing state for retry metadata');
      unawaited(_connection.reconnect());
    }
  }

  /// Handle connection state transitions — reset metadata but keep rooms.
  ///
  /// Rooms are preserved so the UI doesn't flicker during reconnect.
  /// They'll be re-synced via hello on reconnect. Widgets can check
  /// [connectionState] to show a reconnecting indicator.
  void _onTriageChanged(Map<String, dynamic> data) {
    final pendingDevices = (data['pending_devices'] as num?)?.toInt() ??
        (data['devices'] as num?)?.toInt() ??
        0;
    final pendingUnassigned =
        (data['pending_unassigned'] as num?)?.toInt() ?? 0;
    final pendingRooms = (data['pending_rooms'] as num?)?.toInt() ??
        (data['rooms'] as num?)?.toInt() ??
        0;
    final pendingHubConfigured =
        (data['pending_hub_configured'] as num?)?.toInt() ?? 0;
    final devices = pendingDevices + pendingUnassigned;
    final rooms = pendingRooms + pendingHubConfigured;
    final count = (data['total'] as num?)?.toInt() ??
        (data['pending_count'] as num?)?.toInt() ??
        (devices + rooms);
    if (_triagePendingCount != count ||
        _triagePendingDevices != devices ||
        _triagePendingRooms != rooms) {
      _triagePendingCount = count;
      _triagePendingDevices = devices;
      _triagePendingRooms = rooms;
      notifyListeners();
    }
  }

  void _onConnectionStateChanged(RhythmConnectionState current) {
    if (HueServiceLocator.isDemoMode) {
      notifyListeners();
      return;
    }
    final previous = _previousConnectionState;
    _previousConnectionState = current;

    if (current != previous &&
        (current == RhythmConnectionState.disconnected ||
            current == RhythmConnectionState.reconnecting)) {
      debugPrint(
          'ServerSync: Connection lost ($previous → $current) — resetting metadata, keeping rooms');
      _firmwareVersion = '0.0.0';
      _serverPlatformType = 'desktop';
      _serverPlatformContext = 'server';
      _powerSave = false;
      _activeMode = null;
      _activeProfileId = null;
      _helloNodes = [];
      _helloRooms = [];
      _topologyNodes = [];
      _lastHubInfos = [];
      _capabilities = null;
      _rhythmIntervalSecs = 60;
      _effectiveFadeMs = null;
      _effectiveMotionTimeoutSecs = null;
      _review = const RhythmReviewSummary();
      _triagePendingCount = 0;
      _triagePendingDevices = 0;
      _triagePendingRooms = 0;
    }

    notifyListeners();
  }

  // ============================================================================
  // Push triggers (App → Server)
  // ============================================================================

  /// Dispatch a node action through the server.
  ///
  /// Returns true if dispatched to server, false if not connected.
  /// On success, applies the server's response immediately for fast convergence.
  bool dispatchNodeAction(String nodeId, String action) {
    if (HueServiceLocator.isDemoMode) {
      return true; // optimistic UI already applied
    }
    if (!_connection.connected) return false;
    _connection.api
        .nodeAction(nodeId: nodeId, action: action)
        .then((serverState) {
      if (serverState != null) _onRhythmState(serverState);
    });
    return true;
  }

  bool dispatchAction(String roomId, String action) =>
      dispatchNodeAction(roomId, action);

  /// Dispatch multiple node actions in a single batch request.
  ///
  /// Returns true if dispatched to server, false if not connected.
  /// On success, applies each returned state for fast convergence.
  Future<bool> dispatchBatchNodeActions(
      List<({String nodeId, String action})> actions) async {
    if (!_connection.connected || actions.isEmpty) return false;
    final states = await _connection.api.nodeActionBatch(actions);
    for (final state in states) {
      _onRhythmState(state);
    }
    return true;
  }

  Future<bool> dispatchBatchActions(
      List<({String roomId, String action})> actions) async {
    return dispatchBatchNodeActions([
      for (final action in actions)
        (nodeId: action.roomId, action: action.action),
    ]);
  }

  /// Set node brightness through the server.
  ///
  /// Returns true if dispatched to server, false if not connected.
  bool dispatchNodeBrightness(String nodeId, int brightness) {
    if (HueServiceLocator.isDemoMode) {
      DemoServerApi.instance.updateRoomLightState(
        nodeId,
        on: true,
        brightness: brightness,
        kelvin: _roomProvider.getKelvin(nodeId),
      );
      _roomProvider.applyServerNodeState(
        nodeId,
        rhythmEnabled: _roomProvider.getNode(nodeId)?.rhythmEnabled ?? true,
        timeOffset: 0,
        brightnessOffset: 0,
        state: RoomModeState.active,
        lightsOn: true,
        brightness: brightness,
        kelvin: _roomProvider.getKelvin(nodeId),
      );
      return true;
    }
    if (!_connection.connected) return false;
    _connection.api.nodeBrightness(nodeId: nodeId, brightness: brightness);
    return true;
  }

  bool dispatchBrightness(String roomId, int brightness) =>
      dispatchNodeBrightness(roomId, brightness);

  /// Push node preferences to the server (user-state only, no topology).
  void pushNodePreferences(String nodeId,
      {bool? rhythmEnabled,
      bool? disabled,
      RoomModeState? state,
      Map<String, dynamic>? profileSettings}) {
    if (HueServiceLocator.isDemoMode) return; // optimistic UI already applied
    if (!_connection.connected || _receivingFromServer) return;
    debugPrint(
        'ServerSync: pushNodePreferences $nodeId rhythmEnabled=$rhythmEnabled disabled=$disabled state=${state?.wireValue}');
    api.nodePreferencesSet(
      nodeId: nodeId,
      rhythmEnabled: rhythmEnabled,
      disabled: disabled,
      state: state,
      profileSettings: profileSettings,
    );
  }

  void pushRoomPreferences(String roomId,
      {bool? rhythmEnabled,
      bool? disabled,
      RoomModeState? state,
      Map<String, dynamic>? profileSettings}) {
    pushNodePreferences(
      roomId,
      rhythmEnabled: rhythmEnabled,
      disabled: disabled,
      state: state,
      profileSettings: profileSettings,
    );
  }

  /// Push node preferences for multiple nodes in a single batch request.
  ///
  /// Avoids per-node socket overhead on constrained servers (ESP32).
  void pushBatchNodePreferences(List<Map<String, dynamic>> items) {
    if (!_connection.connected || _receivingFromServer || items.isEmpty) return;
    debugPrint('ServerSync: pushBatchNodePreferences (${items.length} nodes)');
    api.nodePreferencesBatchSet(items);
  }

  void pushBatchRoomPreferences(List<Map<String, dynamic>> items) {
    pushBatchNodePreferences(items);
  }

  /// Reset a single node to its current adaptive curve position.
  ///
  /// Per-node equivalent of [dispatchFixMyLights].
  void dispatchResetNode(String nodeId) {
    if (HueServiceLocator.isDemoMode) {
      DemoServerApi.instance.updateRoomLightState(
        nodeId,
        on: true,
        brightness: 75,
        kelvin: _roomProvider.getKelvin(nodeId) ?? 3200,
      );
      _roomProvider.applyServerNodeState(
        nodeId,
        rhythmEnabled: _roomProvider.getNode(nodeId)?.rhythmEnabled ?? true,
        timeOffset: 0,
        brightnessOffset: 0,
        state: RoomModeState.active,
        lightsOn: true,
        brightness: 75,
        kelvin: _roomProvider.getKelvin(nodeId) ?? 3200,
      );
      _roomProvider.bumpResetGeneration();
      return;
    }
    if (!_connection.connected) return;
    _connection.api
        .nodeAction(nodeId: nodeId, action: 'reset')
        .then((serverState) {
      if (serverState != null) _onRhythmState(serverState);
      _roomProvider.bumpResetGeneration();
    });
  }

  void dispatchResetRoom(String roomId) => dispatchResetNode(roomId);

  /// Reset all currently-on light-addressable nodes to their curve position.
  ///
  /// Returns node states for immediate UI convergence.
  Future<List<RhythmRoomState>> dispatchFixMyLights() async {
    if (HueServiceLocator.isDemoMode) {
      // Reset all on-rooms locally
      for (final room in _roomProvider.rooms) {
        if (room.lightsOn) {
          DemoServerApi.instance.updateRoomLightState(
            room.id,
            on: true,
            brightness: 75,
            kelvin: 3200,
          );
          await _roomProvider.applyServerNodeState(
            room.id,
            rhythmEnabled: true,
            timeOffset: 0,
            brightnessOffset: 0,
            state: RoomModeState.active,
            lightsOn: true,
            brightness: 75,
            kelvin: 3200,
          );
        }
      }
      _roomProvider.bumpResetGeneration();
      return [];
    }
    if (!_connection.connected) return [];
    final nodeIds = _helloNodes
        .where((node) => node.isLightAddressable && (node.lightsOn ?? false))
        .map((node) => node.id)
        .toSet();
    if (nodeIds.isEmpty) {
      nodeIds.addAll(
        _roomProvider.rooms
            .where((node) => node.lightsOn)
            .map((node) => node.id)
            .where((id) => id.isNotEmpty),
      );
    }
    final states = await _connection.api.fixMyLights(nodeIds: nodeIds);
    for (final state in states) {
      _onRhythmState(state);
    }
    if (states.isNotEmpty) {
      _roomProvider.bumpResetGeneration();
    }
    return states;
  }

  /// Set the active global mode on the server.
  Future<void> dispatchSetActiveMode(RhythmMode mode) async {
    if (HueServiceLocator.isDemoMode) {
      _activeMode = mode;
      await DemoServerApi.instance.setActiveMode(mode);
      notifyListeners();
      return;
    }
    if (!_connection.connected) return;
    _activeMode = mode;
    notifyListeners();
    await api.setActiveMode(mode);
    await fullRefresh();
  }

  /// Update a single mode transition on the server.
  Future<bool> dispatchUpdateTransition(
      RhythmModeTransitionConfig updated) async {
    if (!_connection.connected) return false;
    final newList = _modeTransitions.map((t) {
      if (t.id == updated.id ||
          (t.id.isEmpty &&
              updated.id.isEmpty &&
              t.fromMode == updated.fromMode &&
              t.toMode == updated.toMode)) {
        return updated;
      }
      return t;
    }).toList();
    _modeTransitions = newList;
    notifyListeners();
    final success = await api.setTransitions(newList);
    if (success) await fullRefresh();
    return success;
  }

  /// Run a saved transition by ID.
  Future<bool> dispatchRunTransition(String transitionId) async {
    if (!_connection.connected) return false;
    final success = await api.triggerTransition(transitionId);
    if (success) {
      await fullRefresh();
    }
    return success;
  }

  /// Activate sleep mode on the server.
  Future<void> dispatchSleep() async {
    await dispatchSetActiveMode(RhythmMode.sleep);
  }

  /// Deactivate sleep mode (wake) on the server.
  Future<void> dispatchWake() async {
    await dispatchSetActiveMode(RhythmMode.day);
  }

  /// Trigger server-side room discovery from the connected hub.
  ///
  /// After sync completes, does a full refresh to pick up new rooms/devices.
  Future<void> triggerServerSync() async {
    if (HueServiceLocator.isDemoMode) {
      await _refreshDemoState();
      return;
    }
    if (!_connection.connected) return;
    debugPrint('ServerSync: Triggering server-side sync');
    await api.triggerSync();
    await _connection.reconnect();
  }

  /// Push location to the server.
  void pushLocation({
    required double lat,
    required double lon,
    double? utcOffset,
    String? timezoneName,
  }) {
    if (HueServiceLocator.isDemoMode) return;
    if (!_connection.connected) return;
    api.locationSet(
      lat: lat,
      lon: lon,
      utcOffset: utcOffset,
      timezoneName: timezoneName,
    );
  }

  /// Push per-node motion timeout to the server.
  void pushNodeMotionTimeout(String nodeId, int? timeoutSecs) {
    if (!_connection.connected) return;
    api.motionTimeoutSet(nodeId: nodeId, timeoutSecs: timeoutSecs);
  }

  void pushMotionTimeout(String roomId, int? timeoutSecs) {
    pushNodeMotionTimeout(roomId, timeoutSecs);
  }

  // ============================================================================
  // Config/location reconciliation
  // ============================================================================

  /// Push hub credentials for a specific room source.
  ///
  /// Called after Hue pairing or other hub configuration changes so the
  /// server gets the credentials it needs to connect to the hub.
  Future<void> pushHubCredentials(RoomSourceDto source) async {
    if (!_connection.connected) return;
    await _pushHubCredentialsForSource(source);
  }

  /// Tell the addon to auto-configure HA using its SUPERVISOR_TOKEN.
  /// Sends empty credentials — server fills them from its environment.
  Future<bool> configureAddonHaHub() async {
    if (!_connection.connected) return false;
    await api.hubCredentials(
      hubType: 'homeassistant',
      address: '',
      credentials: {},
    );
    _lastHubReconnectTime = DateTime.now();
    await _connection.reconnect(); // Re-fetch state with new rooms
    return true;
  }

  /// Tell the server to disconnect ALL hubs — clears all credentials, runtimes,
  /// and rooms.
  Future<void> disconnectHub() async {
    if (!_connection.connected) return;
    debugPrint('ServerSync: Sending hub disconnect to server');
    await api.hubDisconnect();
  }

  /// Disconnect a single hub by type + address.
  Future<void> disconnectOneHub(String hubType, String address) async {
    if (!_connection.connected) return;
    debugPrint('ServerSync: Disconnecting hub $hubType @ $address');
    await api.hubDisconnectOne(hubType: hubType, address: address);
  }

  /// Manually re-arm startup retry for a single stored hub credential set.
  Future<bool> retryHub(
    String hubType,
    String address, {
    bool refreshState = true,
  }) async {
    if (!_connection.connected) return false;
    if (hubType.isEmpty || address.isEmpty) return false;
    debugPrint('ServerSync: Retrying hub $hubType @ $address');
    final accepted = await api.hubRetry(hubType: hubType, address: address);
    if (accepted && refreshState) {
      await _connection.reconnect();
    }
    return accepted;
  }

  /// Re-arm startup retry for multiple hubs, then refresh state once.
  Future<int> retryHubs(Iterable<Map<String, dynamic>> hubInfos) async {
    if (!_connection.connected) return 0;
    var accepted = 0;
    for (final hubInfo in hubInfos) {
      final hubType = hubInfo['type'] as String? ?? '';
      final address = hubInfo['address'] as String? ?? '';
      if (hubType.isEmpty || address.isEmpty) continue;
      final ok = await retryHub(
        hubType,
        address,
        refreshState: false,
      );
      if (ok) accepted++;
    }
    if (accepted > 0) {
      await _connection.reconnect();
    }
    return accepted;
  }

  Future<void> _pushHubCredentialsForSource(RoomSourceDto source) async {
    final hubType = _hubTypeForSource(source);
    if (hubType == null) return;

    final hub = _homeProvider.getFirstHubOfType(hubType);
    if (hub == null || !hub.hasCredentials) return;

    // HA expects {"token": "..."}, Hue expects {"username": "..."}
    final credentials = hubType == HubType.homeAssistant
        ? {'token': hub.token}
        : {'username': hub.token};

    debugPrint(
        'ServerSync: Pushing ${hub.typeName} credentials after source change');
    await api.hubCredentials(
      hubType: _hubTypeWireName(hubType),
      address: '${hub.endpoint.host}:${hub.endpoint.port}',
      credentials: credentials,
    );
    _lastHubReconnectTime = DateTime.now();
    await _connection.reconnect();
  }

  /// Accept server config as authoritative — update app's Home if different.
  ///
  /// The server (addon) owns the curve config. On hello, if the server's
  /// config differs from the app's cached copy, we update the app to match.
  void _acceptServerConfig(Map<String, dynamic> serverConfig) {
    if (serverConfig.isEmpty) {
      debugPrint('ServerSync: CONFIG ACCEPT skipped — server config missing');
      return;
    }

    RhythmCurveConfig profileConfig;
    try {
      profileConfig = RhythmCurveConfig.fromJson(serverConfig);
    } catch (e) {
      debugPrint('ServerSync: CONFIG ACCEPT skipped — parse failed: $e');
      return;
    }

    final superGaussian = profileConfig.superGaussianCurve;
    if (superGaussian == null) {
      debugPrint('ServerSync: CONFIG ACCEPT skipped — unsupported curve type '
          '${profileConfig.curve.type}');
      return;
    }

    final srvMinBri = profileConfig.minBrightness;
    final srvMaxBri = profileConfig.maxBrightness;
    final srvMinCct = profileConfig.minColorTemp;
    final srvMaxCct = profileConfig.maxColorTemp;
    final srvWlBri = superGaussian.widthLeftBri;
    final srvWrBri = superGaussian.widthRightBri;
    final srvWlCct = superGaussian.widthLeftCct;
    final srvWrCct = superGaussian.widthRightCct;
    final srvShapeP = superGaussian.shapeP;
    final srvMaxDim = profileConfig.maxDimSteps;
    final srvFadeMs = profileConfig.fadeMs;
    final srvMotionTimeout = profileConfig.motionTimeoutSecs;

    final serverCurve = CurveConfigDto(
      minBrightness: srvMinBri,
      maxBrightness: srvMaxBri,
      minColorTemp: srvMinCct,
      maxColorTemp: srvMaxCct,
      widthLeftBri: srvWlBri,
      widthRightBri: srvWrBri,
      widthLeftCct: srvWlCct,
      widthRightCct: srvWrCct,
      shapeP: srvShapeP,
      maxDimSteps: srvMaxDim,
      fadeMs: srvFadeMs ?? defaultCurveConfig.fadeMs,
      motionTimeoutSecs:
          srvMotionTimeout ?? defaultCurveConfig.motionTimeoutSecs,
    );

    final appConfig = _homeProvider.currentHome?.curveConfig;

    debugPrint('ServerSync: CONFIG COMPARE — '
        'Server: profile=${profileConfig.id} curve=${profileConfig.curve.type} '
        'bri=$srvMinBri-$srvMaxBri cct=$srvMinCct-$srvMaxCct '
        'wBri=$srvWlBri/$srvWrBri wCct=$srvWlCct/$srvWrCct shapeP=$srvShapeP | '
        'App: bri=${appConfig?.minBrightness}-${appConfig?.maxBrightness} '
        'cct=${appConfig?.minColorTemp}-${appConfig?.maxColorTemp} '
        'wBri=${appConfig?.widthLeftBri}/${appConfig?.widthRightBri} '
        'wCct=${appConfig?.widthLeftCct}/${appConfig?.widthRightCct} '
        'shapeP=${appConfig?.shapeP}');

    if (appConfig != serverCurve) {
      debugPrint(
          'ServerSync: Config mismatch — accepting server config into app');
      _homeProvider.updateCurrentHomeCurveConfig(serverCurve);
    }
  }

  void _pushLocationIfUnset(Map<String, dynamic> serverLocation) {
    final home = _homeProvider.currentHome;
    final loc = home?.location;
    if (loc == null) return;

    final srvLat = (serverLocation['latitude'] as num?)?.toDouble();
    final srvLon = (serverLocation['longitude'] as num?)?.toDouble();

    // Only push when the server has no location at all. Location is the
    // *house* location (fixed, tied to the server/hub) — once set, it should
    // only change via an explicit user action in location settings.
    if (srvLat == null || srvLon == null) {
      debugPrint('ServerSync: Server has no location — pushing app location');
      _pushLocationWithIanaTimezone(loc);
    }
  }

  /// Push location to server, ensuring a proper IANA timezone name.
  ///
  /// If the Home's timezone is a non-IANA abbreviation (e.g. "EST"),
  /// falls back to the device's local timezone via [FlutterTimezone].
  void _pushLocationWithIanaTimezone(HomeLocation loc) {
    final homeTz = _homeProvider.currentHome?.timezone;
    if (_isIanaTimezone(homeTz)) {
      api.locationSet(
        lat: loc.latitude,
        lon: loc.longitude,
        timezoneName: homeTz,
      );
    } else {
      // Resolve proper IANA name asynchronously.
      FlutterTimezone.getLocalTimezone().then((tz) {
        final ianaTz = tz.identifier;
        debugPrint(
            'ServerSync: Resolved IANA timezone: $ianaTz (was: $homeTz)');
        api.locationSet(
          lat: loc.latitude,
          lon: loc.longitude,
          timezoneName: ianaTz,
        );

        // Also fix the Home model so future syncs don't need this fallback.
        final home = _homeProvider.currentHome;
        if (home != null && home.timezone != ianaTz) {
          _homeProvider.updateCurrentHome(
            home.copyWith(
                timezone: ianaTz, updatedAt: DateTime.now(), pendingSync: true),
          );
        }
      }).catchError((e) {
        debugPrint(
            'ServerSync: FlutterTimezone failed: $e, using home timezone');
        api.locationSet(
          lat: loc.latitude,
          lon: loc.longitude,
          timezoneName: homeTz,
        );
      });
    }
  }

  void _refreshTopologyNodes() {
    if (HueServiceLocator.isDemoMode) {
      unawaited(_refreshDemoState());
      return;
    }
    if (!_connection.connected) return;
    unawaited(() async {
      final topologyNodes = await api.getTopologyNodes();
      if (topologyNodes.isEmpty && _topologyNodes.isNotEmpty) return;
      _topologyNodes = topologyNodes;
      _helloRooms = _buildRoomSummaries();
      _roomProvider.setMotionSensorNodes(_sensorTargetNodeIds());
      if (hasListeners) notifyListeners();
    }());
  }

  Future<bool> setNodeControlTarget({
    required String sourceNodeId,
    required String controlKind,
    required String? targetNodeId,
  }) async {
    if (!HueServiceLocator.isDemoMode && !_connection.connected) return false;
    final success = await api.setTopologyNodeControlTarget(
      nodeId: sourceNodeId,
      controlKind: controlKind,
      targetId: targetNodeId,
    );
    if (success) {
      _refreshTopologyNodes();
    }
    return success;
  }

  Future<void> _refreshDemoState() async {
    DemoServerApi.instance.ensureSeeded();
    final snapshot = DemoServerApi.instance.snapshot();
    final roomDtos = DemoServerApi.instance.buildRoomDtos();
    final settings = await DemoServerApi.instance.getSettings();
    final mode = await DemoServerApi.instance.getMode();

    _firmwareVersion = DemoServerApi.firmwareVersion;
    _serverPlatformType = DemoServerApi.serverPlatformType;
    _serverPlatformContext = DemoServerApi.serverPlatformContext;
    _powerSave = settings?.powerSave ?? false;
    _activeMode = mode?.active;
    _activeProfileId = null;
    _modeTransitions = const [];
    _modeConfigs = const [];
    _profiles = const [];
    _rhythmIntervalSecs = 60;
    _effectiveFadeMs = 1800;
    _effectiveMotionTimeoutSecs = 120;
    _review = snapshot.review;
    _lastHubInfos = snapshot.hubInfos;
    _capabilities = null;
    _helloNodes = snapshot.helloNodes;
    _topologyNodes = snapshot.topologyNodes;
    _triagePendingCount = snapshot.triagePendingCount;
    _triagePendingDevices = snapshot.triagePendingDevices;
    _triagePendingRooms = snapshot.triagePendingRooms;

    DemoHueBridgeService.instance.seedRooms(roomDtos);
    _acceptServerNodes(_helloNodes);
    _helloRooms = _buildRoomSummaries();
    _roomProvider.setMotionSensorNodes(_sensorTargetNodeIds());

    if (hasListeners) {
      notifyListeners();
    }
  }

  List<RhythmRoom> _buildRoomSummaries() {
    if (_helloNodes.isEmpty) return const [];

    final nodeStateById = <String, RhythmRoom>{
      for (final node in _helloNodes) node.id: node,
    };

    if (_topologyNodes.isEmpty) {
      return _helloNodes.where((node) => node.kind.isRoom).toList();
    }

    final roomChildren = <String, List<RhythmTopologyNode>>{};
    for (final node in _topologyNodes.where((node) => node.isDevice)) {
      final parentId = node.parentId;
      if (parentId == null || parentId.isEmpty) continue;
      (roomChildren[parentId] ??= []).add(node);
    }

    final rooms = <RhythmRoom>[];
    for (final topologyRoom in _topologyNodes.where((node) => node.isRoom)) {
      final state = nodeStateById[topologyRoom.id];
      final devices = (roomChildren[topologyRoom.id] ?? const [])
          .map(RhythmDevice.fromTopologyNode)
          .toList()
        ..sort((left, right) {
          const order = {
            RhythmDeviceType.light: 0,
            RhythmDeviceType.button: 1,
            RhythmDeviceType.motion: 2,
          };
          return (order[left.type] ?? 3).compareTo(order[right.type] ?? 3);
        });
      rooms.add(RhythmRoom(
        id: topologyRoom.id,
        name: topologyRoom.name,
        kind: topologyRoom.kind,
        parentId: topologyRoom.parentId,
        placement: topologyRoom.placement,
        groupedLightId: state?.groupedLightId ?? '',
        state: state?.state ?? RoomModeState.active,
        transitioning: state?.transitioning ?? false,
        rhythmEnabled: state?.rhythmEnabled ?? false,
        disabled: state?.disabled ?? false,
        timeOffset: state?.timeOffset ?? 0,
        brightnessOffset: state?.brightnessOffset ?? 0,
        hubTypes: state?.hubTypes ??
            topologyRoom.hubRoomBindings
                .map((binding) => binding.hubKey?['hub_type']?.toString())
                .whereType<String>()
                .toSet()
                .toList(),
        manufacturer: state?.manufacturer,
        model: state?.model,
        deviceIds: devices
            .where((device) => device.type == RhythmDeviceType.light)
            .map((device) => device.id)
            .toList(),
        devices: devices,
        profileSettings: state?.profileSettings,
        lightsOn: state?.lightsOn,
        brightness: state?.brightness,
        kelvin: state?.kelvin,
        motionActive: state?.motionActive,
        motionOwned: state?.motionOwned,
        remainingSecs: state?.remainingSecs,
        timeoutSecs: state?.timeoutSecs,
        warningActive: state?.warningActive,
      ));
    }

    rooms.sort((left, right) => left.name.compareTo(right.name));
    return rooms;
  }

  Set<String> _sensorTargetNodeIds() {
    final sensorTargetIds = <String>{
      for (final node in _helloNodes)
        if (node.id.isNotEmpty && node.hasMotionSensor) node.id,
    };

    for (final node in _topologyNodes) {
      for (final control in node.controls) {
        final targetId = control.targetId;
        if (!control.isMotion || targetId == null || targetId.isEmpty) {
          continue;
        }
        sensorTargetIds.add(targetId);
      }
    }

    return sensorTargetIds;
  }

  // ============================================================================
  // Hub-agnostic helpers
  // ============================================================================

  HubType? _hubTypeForSource(RoomSourceDto source) {
    switch (source) {
      case RoomSourceDto.matter:
        return null;
      case RoomSourceDto.hue:
        return HubType.hue;
      case RoomSourceDto.homeAssistant:
        return HubType.homeAssistant;
      default:
        return null;
    }
  }

  RoomSourceDto _canonicalSourceForNode(RhythmRoom node) {
    final sources =
        node.hubTypes.map(_sourceForHubType).whereType<RoomSourceDto>().toSet();

    if (sources.isEmpty) {
      return _roomProvider.getNode(node.id)?.source ?? RoomSourceDto.unknown;
    }

    final existingSource = _roomProvider.getNode(node.id)?.source;
    if (existingSource != null && sources.contains(existingSource)) {
      return existingSource;
    }

    if (sources.length == 1) {
      return sources.first;
    }

    for (final candidate in const [
      RoomSourceDto.matter,
      RoomSourceDto.hue,
      RoomSourceDto.homeAssistant,
      RoomSourceDto.esp32,
    ]) {
      if (sources.contains(candidate)) return candidate;
    }

    return RoomSourceDto.unknown;
  }

  RoomSourceDto? _sourceForHubType(String hubType) {
    return switch (hubType) {
      'matter' => RoomSourceDto.matter,
      'hue' => RoomSourceDto.hue,
      'homeassistant' || 'home_assistant' => RoomSourceDto.homeAssistant,
      'esp32' => RoomSourceDto.esp32,
      _ => null,
    };
  }

  RoomNodeKind _roomNodeKindFromSdk(RhythmNodeKind kind) => switch (kind) {
        RhythmNodeKind.lightDevice => RoomNodeKind.lightDevice,
        RhythmNodeKind.switchDevice => RoomNodeKind.switchDevice,
        RhythmNodeKind.motionSensor => RoomNodeKind.motionSensor,
        RhythmNodeKind.sensor => RoomNodeKind.sensor,
        RhythmNodeKind.button => RoomNodeKind.button,
        RhythmNodeKind.otherDevice => RoomNodeKind.otherDevice,
        RhythmNodeKind.room => RoomNodeKind.room,
      };

  RoomNodePlacement? _roomNodePlacementFromSdk(
          RhythmNodePlacement? placement) =>
      switch (placement) {
        RhythmNodePlacement.hubDefault => RoomNodePlacement.hubDefault,
        RhythmNodePlacement.userOverride => RoomNodePlacement.userOverride,
        RhythmNodePlacement.standalone => RoomNodePlacement.standalone,
        null => null,
      };

  /// Map Dart HubType to the Rust wire string.
  ///
  /// Dart enum names are camelCase (`homeAssistant`) but the Rust server
  /// expects lowercase (`homeassistant`).
  String _hubTypeWireName(HubType type) {
    switch (type) {
      case HubType.homeAssistant:
        return 'homeassistant';
      case HubType.hue:
        return 'hue';
      case HubType.server:
        return 'server';
    }
  }

  @override
  void dispose() {
    _helloSub?.cancel();
    _rhythmStateSub?.cancel();
    _hubEventSub?.cancel();
    _sourceChangedSub?.cancel();
    _motionTimerSub?.cancel();
    _newNodesSub?.cancel();
    _triageChangedSub?.cancel();
    _connectionStateSub?.cancel();
    _demoChangeSub?.cancel();
    super.dispose();
  }
}
