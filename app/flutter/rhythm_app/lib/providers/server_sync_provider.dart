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

import 'package:connectivity_plus/connectivity_plus.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_timezone/flutter_timezone.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:uuid/uuid.dart';

import '../backend/backend.dart' show AuthUser;
import '../config/feature_flags.dart';
import '../services/analytics_service.dart';
import '../services/auth_service.dart';
import '../services/cloud_backed_server_api.dart';
import '../services/demo_server_api.dart';
import '../services/device_pairing_code.dart'
    show locallyRegisteredLocalBleProfileIds;
import '../services/hue/demo_hue_bridge_service.dart';
import '../services/hue/hue_service_locator.dart';
import '../services/local_rhythm_server_service.dart';
import '../services/remote_access_service.dart';
import '../services/server_identity.dart';
import '../services/server_activity_cloud_provisioning_service.dart';
import 'home_provider.dart';
import 'room_provider.dart';

/// Whether a timezone string looks like a proper IANA name (contains '/').
/// Abbreviations like "EST", "PST" don't handle DST transitions.
bool _isIanaTimezone(String? tz) => tz != null && tz.contains('/');

bool _isGeneratedMoodSceneId(String? sceneId) =>
    sceneId != null &&
    (sceneId.startsWith('node-mood-scene-') ||
        sceneId.startsWith('node_mood_scene_'));

String _sceneAnalyticsSource(RhythmSceneDefinition? scene) {
  if (scene == null || scene.source.kind == RhythmSceneSourceKind.user) {
    return 'rhythm';
  }
  return scene.source.provider == 'hue' ? 'native_hue' : 'imported';
}

void _logRoomSceneCatalog(
  List<RhythmSceneDefinition> scenes, {
  required String outcome,
}) {
  AnalyticsService().logMoodSceneCatalogLoaded(
    sceneCount: scenes.length,
    nativeSceneCount:
        scenes.where((scene) => scene.source.provider == 'hue').length,
    outcome: outcome,
  );
}

bool _sameEndpoint(HubEndpoint? left, HubEndpoint? right) {
  if (left == null || right == null) return left == right;
  return left == right;
}

bool _sameServerHubIdentity(Hub? left, Hub? right) {
  if (left == null || right == null) return left == right;
  return left.id == right.id && left.homeId == right.homeId;
}

typedef ServerEndpointReachability = Future<bool> Function(
  HubEndpoint endpoint,
  String? authToken,
);

typedef ServerAuthApiFactory = RhythmAuthApi Function({
  required String baseUrl,
});

typedef RemoteAccessAutoEnableScheduler = void Function({
  required Home home,
  required Hub serverHub,
  required RemoteAccessHubSaver saveHub,
  RemoteAccessLatestHubResolver? resolveLatestHub,
  RemoteAccessEnabledCallback? onEnabled,
});

List<RhythmSceneDefinition> _userVisibleScenes(
  Iterable<RhythmSceneDefinition> scenes,
) =>
    scenes
        .where((scene) =>
            !_isGeneratedMoodSceneId(scene.id) &&
            _firstLitSceneOutput(scene) != null)
        .toList();

bool _isLitSceneOutput(RhythmLightSceneOutput? output) =>
    output != null && !output.isOff && output.color?.isComplete == true;

RhythmLightSceneOutput? _firstLitSceneOutput(RhythmSceneDefinition scene) {
  final defaultOutput = scene.light.defaultOutput;
  if (_isLitSceneOutput(defaultOutput)) return defaultOutput;
  for (final output in scene.light.palette) {
    if (_isLitSceneOutput(output)) return output;
  }
  for (final entry in scene.light.entries) {
    if (_isLitSceneOutput(entry.output)) return entry.output;
  }
  return null;
}

int? _sceneRepresentativeBrightness(RhythmSceneDefinition scene) {
  final brightness = _firstLitSceneOutput(scene)?.brightness;
  return brightness?.clamp(1, 100).toInt();
}

/// Syncs app state with a server (bridge, rhythm-server, addon) via
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
  final ServerEndpointReachability? _endpointReachability;
  final Future<List<ConnectivityResult>> Function()? _connectivityCheck;
  final ServerAuthApiFactory _authApiFactory;
  final RemoteAccessAutoEnableScheduler _remoteAccessAutoEnableScheduler;

  StreamSubscription<RhythmHello>? _helloSub;
  StreamSubscription<RhythmRoomState>? _rhythmStateSub;
  StreamSubscription<RhythmDispatchFailure>? _dispatchFailureSub;
  StreamSubscription<({String event, String? hubType, String? address})>?
      _hubEventSub;
  StreamSubscription<RoomSourceDto>? _sourceChangedSub;
  StreamSubscription<RhythmMotionTimer>? _motionTimerSub;
  StreamSubscription<RhythmModeResource>? _modeChangedSub;
  StreamSubscription<RhythmSettings>? _settingsChangedSub;
  StreamSubscription<RhythmLightBreaker>? _lightBreakerChangedSub;
  StreamSubscription<void>? _newNodesSub;
  StreamSubscription<Map<String, dynamic>>? _triageChangedSub;
  StreamSubscription<RhythmConnectionState>? _connectionStateSub;
  StreamSubscription<void>? _demoChangeSub;
  StreamSubscription<AuthUser?>? _authStateSub;
  Timer? _activityCloudProvisioningTimer;

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

  /// Endpoint currently selected for the SDK connection.
  HubEndpoint? _activeConnectionEndpoint;
  bool _remoteFailoverInProgress = false;
  DateTime _lastRemoteFailoverAt = DateTime.fromMillisecondsSinceEpoch(0);
  static const Duration _remoteFailoverCooldown = Duration(seconds: 10);

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

  /// Per-node optimistic locks for the Low glow (Standby) preference.
  ///
  /// The server can emit one stale hello immediately after the preference write,
  /// which otherwise makes the room detail switch jump back before the confirmed
  /// value arrives.
  final Map<String, DateTime> _standbyEnabledLockedUntil = {};
  final Map<String, bool> _optimisticStandbyEnabled = {};
  final Map<String, bool> _suppressedStandbyEnabled = {};
  final Map<String, Timer> _standbyEnabledLockTimers = {};
  static const Duration _standbyEnabledLockDuration = Duration(seconds: 3);

  /// Motion admission uses a synchronous authoritative server mutation. Keep
  /// one request in flight per node so rapid taps cannot reorder the final
  /// persisted value.
  final Set<String> _motionActivationPending = {};
  final Set<String> _lightProfileOverridePending = {};
  static const Uuid _uuid = Uuid();

  /// Raw topology graph from `/api/topology/nodes`.
  List<RhythmTopologyNode> _topologyNodes = [];

  /// Previous connection state for detecting transitions.
  RhythmConnectionState _previousConnectionState =
      RhythmConnectionState.disconnected;

  /// Tracks the last poll time for debouncing [fullRefresh] and [pollNow].
  DateTime _lastPollTime = DateTime.fromMillisecondsSinceEpoch(0);

  /// Blocks the Rooms screen while a reconnect/full refresh is waiting for the
  /// next authoritative hello. This is intentionally separate from
  /// [_hasBeenSynced], which stays sticky so non-room tabs do not flicker out
  /// during ordinary reconnects.
  bool _roomReadinessRefreshPending = false;
  bool _automaticHubStartupGraceActive = false;
  String? _automaticHubStartupGraceSignature;
  Timer? _roomReadinessGraceTimer;
  static const Duration _automaticHubStartupGrace = Duration(seconds: 8);

  /// Explicit gate used when the user enters a Home/server hub from the
  /// chooser. Unlike background reconnects, this must block cached rooms until
  /// a fresh authoritative hello has been accepted.
  bool _homeEntryRefreshPending = false;
  bool _homeEntryRefreshAwaitingHello = false;
  String? _homeEntryRefreshHomeName;
  String? _homeEntryRefreshHubId;
  String? _homeEntryRefreshHomeId;
  String? _homeEntryRefreshError;
  Completer<void>? _homeEntryRefreshHelloCompleter;
  int _homeEntryRefreshGeneration = 0;
  static const Duration _homeEntryWifiFastPathTimeout =
      Duration(milliseconds: 1200);
  static const Duration _homeEntryConnectivityTimeout =
      Duration(milliseconds: 300);

  /// Per-node cooldown for preview refreshes triggered by room detail views.
  final Map<String, DateTime> _lastPreviewRefreshTimeByNode = {};

  /// Firmware version reported by server in the hello message.
  String _firmwareVersion = '0.0.0';
  String? _lastServerInstanceId;
  static const Duration _activityCloudProvisioningInterval =
      Duration(minutes: 5);

  /// Platform type reported by server ("desktop" or "embedded"/"bridge").
  String _serverPlatformType = 'desktop';

  /// Deployment context reported by server ("ha_addon", "server", "rpiz", "bridge", etc.).
  String _serverPlatformContext = 'server';

  /// Legacy power-save cache for older servers that still report it.
  bool _powerSave = true;

  /// Whether automatic firmware updates are enabled on the server.
  /// Default true to match server contract for new installs / missing field.
  bool _autoUpdate = true;

  /// Whether autonomous global light control is enabled on the server.
  bool _lightBreakerEnabled = true;

  /// Active light runtime from the server.
  RhythmLightRuntime _lightRuntime = RhythmLightRuntime.rhythmAdaptive;

  /// Active global mode from the server (`day` / `sleep`).
  RhythmMode? _activeMode;

  /// Bumps whenever the server confirms a global mode event.
  int _modeChangeGeneration = 0;

  /// Saved mode transitions from the server.
  List<RhythmModeTransitionConfig> _modeTransitions = const [];

  /// Physical input bindings from the server.
  List<RhythmInputBinding> _inputBindings = const [];

  /// Saved scenes (presets) from the server, used for scene-backed Mood.
  List<RhythmSceneDefinition> _scenes = const [];

  /// Room-scoped scene catalogs, including integration-owned native scenes.
  final Map<String, List<RhythmSceneDefinition>> _roomScenes = {};

  /// Local scene selection while the server catches up to Mood edits.
  final Map<String, String?> _optimisticMoodSceneIds = {};

  /// Rejects stale scene outcomes after a newer selection for the same room.
  final Map<String, int> _moodSceneApplyGenerations = {};

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

  /// Sticky flag: true once we've completed at least one hello with the
  /// currently paired server hub. Stays true across transient reconnects
  /// (e.g. pull-to-refresh), and resets when the selected Home/server identity
  /// changes or the hub is unpaired. Used by UI surfaces that should not
  /// flicker out during reconnect — like the Transitions bottom-nav tab.
  bool _hasBeenSynced = false;
  bool get hasBeenSynced => _hasBeenSynced || HueServiceLocator.isDemoMode;

  /// Whether the server can dispatch room actions.
  bool get canDispatchActions =>
      _connection.connected || HueServiceLocator.isDemoMode;

  /// Connection state of the underlying connection.
  RhythmConnectionState get connectionState => HueServiceLocator.isDemoMode
      ? RhythmConnectionState.connected
      : _connection.connectionState;

  /// Whether an explicit Home entry/login flow is waiting on fresh server data.
  bool get isHomeEntryRefreshPending => _homeEntryRefreshPending;

  /// Whether the Home/Rooms surface is waiting for a fresh room snapshot.
  bool get isRoomReadinessRefreshPending =>
      !HueServiceLocator.isDemoMode && _roomReadinessRefreshPending;

  /// Whether the Rooms screen can show room controls without exposing stale
  /// post-boot/post-update state.
  bool get roomsReadyForDisplay =>
      HueServiceLocator.isDemoMode ||
      (_hasBeenSynced &&
          !_roomReadinessRefreshPending &&
          !_homeEntryRefreshPending &&
          !_homeEntryRefreshAwaitingHello &&
          !hasPendingAutomaticHubStartup);

  /// Whether the Home tab should be gated by Home entry loading/error UI.
  bool get hasHomeEntryRefreshGate =>
      _homeEntryRefreshPending || _homeEntryRefreshError != null;

  /// Display name for the Home currently being entered.
  String? get homeEntryRefreshHomeName => _homeEntryRefreshHomeName;

  /// Error shown when the explicit Home entry refresh fails.
  String? get homeEntryRefreshError => _homeEntryRefreshError;

  /// The server entry currently selected for the active connection.
  Hub? get connectedServerHub => _serverHub;

  /// Durable appliance identity from the most recent hello on the active
  /// connection. Null means the current endpoint has not been authenticated by
  /// a durable hello identity yet.
  String? get connectedServerInstanceId {
    final identity = normalizeServerIdentity(_lastServerInstanceId);
    return serverIdentityKind(identity) == ServerIdentityKind.durable
        ? identity
        : null;
  }

  /// Endpoint currently selected for the active SDK connection.
  HubEndpoint? get activeConnectionEndpoint => _activeConnectionEndpoint;

  /// Firmware version reported by server.
  String get firmwareVersion => _firmwareVersion;

  /// Platform type reported by server ("desktop" or "embedded"/"bridge").
  String get serverPlatformType => _serverPlatformType;

  /// Deployment context reported by server ("ha_addon", "server", "rpiz", "bridge", etc.).
  String get serverPlatformContext => _serverPlatformContext;

  /// Whether the connected server is a Rhythm bridge (constrained sockets/memory).
  bool get isBridgeServer =>
      _serverPlatformType == 'bridge' || _serverPlatformType == 'embedded';

  /// Legacy power-save cache for older servers that still report it.
  bool get powerSave => _powerSave;

  /// Whether automatic firmware updates are enabled on the server.
  bool get autoUpdate => _autoUpdate;

  /// Whether autonomous global light control is enabled on the server.
  bool get lightBreakerEnabled => _lightBreakerEnabled;

  /// Active light runtime.
  RhythmLightRuntime get lightRuntime => _lightRuntime;

  /// Whether sleep mode is active on the server.
  bool get sleepMode => _activeMode == RhythmMode.sleep;

  /// Active global mode.
  RhythmMode? get activeMode => _activeMode;

  /// Saved mode transitions.
  List<RhythmModeTransitionConfig> get modeTransitions => _modeTransitions;

  /// Physical input bindings.
  List<RhythmInputBinding> get inputBindings => _inputBindings;

  /// Saved scenes (presets) available to use as moods.
  List<RhythmSceneDefinition> get scenes => _userVisibleScenes(_scenes);

  /// Saved and native scenes applicable to [roomId].
  List<RhythmSceneDefinition> scenesForRoom(String roomId) =>
      _userVisibleScenes(_roomScenes[roomId] ?? _scenes);

  /// Whether the authoritative room topology includes a Philips Hue binding.
  bool roomHasHueBinding(String roomId) {
    for (final node in _helloNodes) {
      if (node.id == roomId) {
        return node.hubTypes.any(
          (hubType) => hubType.trim().toLowerCase() == 'hue',
        );
      }
    }
    return _roomProvider.getNode(roomId)?.source == RoomSourceDto.hue;
  }

  /// The scene id currently bound as the given room's mood, if any.
  String? moodSceneIdForRoom(String roomId) {
    if (_optimisticMoodSceneIds.containsKey(roomId)) {
      final sceneId = _optimisticMoodSceneIds[roomId];
      return _isGeneratedMoodSceneId(sceneId) ? null : sceneId;
    }
    for (final node in _helloNodes) {
      if (node.id == roomId) {
        final sceneId = node.profileSettings?.moodSceneId;
        return _isGeneratedMoodSceneId(sceneId) ? null : sceneId;
      }
    }
    return null;
  }

  /// Look up a cached scene by id (including non-user-visible ones).
  RhythmSceneDefinition? sceneById(String id) {
    for (final scene in _scenes) {
      if (scene.id == id) return scene;
    }
    for (final scenes in _roomScenes.values) {
      for (final scene in scenes) {
        if (scene.id == id) return scene;
      }
    }
    return null;
  }

  /// Fetch the latest scenes from the server, optionally including native
  /// integration scenes for [roomId]. Returns the matching cache when offline.
  Future<List<RhythmSceneDefinition>> fetchScenes({String? roomId}) async {
    if (HueServiceLocator.isDemoMode) {
      _scenes = await DemoServerApi.instance.getScenes();
      notifyListeners();
      final result = roomId == null ? scenes : scenesForRoom(roomId);
      if (roomId != null) {
        _logRoomSceneCatalog(result, outcome: 'demo');
      }
      return result;
    }
    if (!_connection.connected) {
      final result = roomId == null ? scenes : scenesForRoom(roomId);
      if (roomId != null) {
        _logRoomSceneCatalog(result, outcome: 'offline_cache');
      }
      return result;
    }
    final catalog = await _connection.api.getSceneCatalog(targetId: roomId);
    if (catalog == null) {
      final result = roomId == null ? scenes : scenesForRoom(roomId);
      if (roomId != null) {
        _logRoomSceneCatalog(result, outcome: 'request_failed');
      }
      return result;
    }
    final fetched = catalog.scenes;
    if (roomId == null) {
      _scenes = fetched;
      notifyListeners();
      return scenes;
    }
    final cached = _roomScenes[roomId];
    if (catalog.nativeDiscoveryFailed && cached != null) {
      final mergedById = <String, RhythmSceneDefinition>{
        for (final scene in fetched) scene.id: scene,
      };
      for (final scene in cached) {
        if (scene.id.startsWith('native-')) {
          mergedById.putIfAbsent(scene.id, () => scene);
        }
      }
      _roomScenes[roomId] = mergedById.values.toList();
    } else {
      _roomScenes[roomId] = fetched;
    }
    notifyListeners();
    final result = scenesForRoom(roomId);
    _logRoomSceneCatalog(
      result,
      outcome: catalog.nativeDiscoveryFailed ? 'partial' : 'succeeded',
    );
    return result;
  }

  /// Apply [sceneId] to [roomId] and bind it as that room's Mood scene.
  ///
  /// [color] is the scene's representative RGB, used to optimistically tint the
  /// room card while the server confirms. Returns whether the server accepted
  /// and applied the scene.
  Future<bool> applyMoodScene(
    String roomId,
    String sceneId, {
    (int, int, int)? color,
    int? transitionMs,
  }) async {
    final scene = sceneById(sceneId);
    final sceneSource = _sceneAnalyticsSource(scene);
    final journeyId = 'mood-scene-${_uuid.v4()}';
    if (!HueServiceLocator.isDemoMode && !_connection.connected) {
      AnalyticsService().logMoodSceneApplyCompleted(
        journeyId: journeyId,
        sceneSource: sceneSource,
        outcome: 'failed',
        failureStage: 'offline',
      );
      return false;
    }
    final generation = (_moodSceneApplyGenerations[roomId] ?? 0) + 1;
    _moodSceneApplyGenerations[roomId] = generation;
    final hadPreviousOverride = _optimisticMoodSceneIds.containsKey(roomId);
    final previousOverride = _optimisticMoodSceneIds[roomId];
    final previousRoomColor = _roomProvider.getRoomColor(roomId);
    final previousMoodColor = _roomProvider.getMoodColor(roomId);
    final previousMoodBrightness = _roomProvider.getMoodBrightness(roomId);
    final previousMoodEnabled = _roomProvider.isMoodEnabled(roomId);
    if (color != null) {
      _roomProvider.setRoomColorLocal(
        roomId,
        color.$1,
        color.$2,
        color.$3,
        rememberAsMood: true,
      );
    }
    if (scene != null) {
      final brightness = _sceneRepresentativeBrightness(scene);
      if (brightness != null) {
        _roomProvider.setMoodBrightnessLocal(roomId, brightness);
      }
    }
    _roomProvider.setMoodEnabledLocal(roomId, true);
    final sceneOverrideChanged = !_optimisticMoodSceneIds.containsKey(roomId) ||
        _optimisticMoodSceneIds[roomId] != sceneId;
    _optimisticMoodSceneIds[roomId] = sceneId;
    if (sceneOverrideChanged) notifyListeners();

    if (HueServiceLocator.isDemoMode) {
      DemoServerApi.instance.updateRoomLightState(
        roomId,
        on: true,
        color: color,
        state: RoomModeState.mood,
      );
      AnalyticsService().logMoodSceneApplyCompleted(
        journeyId: journeyId,
        sceneSource: sceneSource,
        outcome: 'succeeded',
      );
      return true;
    }
    RhythmSceneActionResult? result;
    var failureStage = 'server_rejected';
    try {
      result = await _connection.api.applyScene(
        sceneId: sceneId,
        targetId: roomId,
        transitionMs: transitionMs,
        correlationId: journeyId,
      );
    } catch (_) {
      result = null;
      failureStage = 'request_exception';
    }
    if (result != null) {
      AnalyticsService().logMoodSceneApplyCompleted(
        journeyId: journeyId,
        sceneSource: sceneSource,
        outcome: 'succeeded',
      );
      return true;
    }
    AnalyticsService().logMoodSceneApplyCompleted(
      journeyId: journeyId,
      sceneSource: sceneSource,
      outcome: 'failed',
      failureStage: failureStage,
    );
    if (_moodSceneApplyGenerations[roomId] != generation) {
      return false;
    }
    if (hadPreviousOverride) {
      _optimisticMoodSceneIds[roomId] = previousOverride;
    } else {
      _optimisticMoodSceneIds.remove(roomId);
    }
    _roomProvider.restoreMoodPresentationLocal(
      roomId,
      roomColor: previousRoomColor,
      moodColor: previousMoodColor,
      moodBrightness: previousMoodBrightness,
      moodEnabled: previousMoodEnabled,
    );
    notifyListeners();
    return false;
  }

  /// Current day/sleep toggle binding, if configured.
  RhythmInputBinding? get daySleepToggleInputBinding {
    for (final binding in _inputBindings) {
      if (binding.preset == RhythmInputBindingPreset.daySleepToggle) {
        return binding;
      }
    }
    return null;
  }

  /// Stable summary for page widgets that need to detect binding changes.
  String get daySleepToggleBindingSignature {
    final binding = daySleepToggleInputBinding;
    if (binding == null) return 'none';
    return [
      binding.id,
      binding.sourceNodeId,
      binding.trigger.buttonAction?.wireValue ?? '',
      binding.enabled.toString(),
    ].join('|');
  }

  /// Mode configs (profile routing per mode).
  List<RhythmModeConfig> get modeConfigs => _modeConfigs;

  /// Profile configs from the server.
  List<RhythmCurveConfig> get profiles => _profiles;

  /// Replace (or insert) a single cached profile config so curve-editing UI
  /// reflects a server-side change immediately, without waiting for the next
  /// `hello`.
  ///
  /// The Time Simulator's "Absorb" action edits a profile's curve on the
  /// server and receives the updated config back in the HTTP response. Its
  /// curve graph is rebuilt from [profiles], which is otherwise only refreshed
  /// by an asynchronous `hello` after a reconnect — so absorb appeared to do
  /// nothing until that landed. Keep the cache in sync the moment the edit
  /// returns.
  void applyProfileConfig(RhythmCurveConfig config) {
    final index = _profiles.indexWhere((profile) => profile.id == config.id);
    if (index == -1) {
      _profiles = [..._profiles, config];
    } else {
      final updated = List<RhythmCurveConfig>.of(_profiles);
      updated[index] = config;
      _profiles = updated;
    }
    notifyListeners();
  }

  /// Persisted Mood color resolved from server profile settings, if present.
  (int r, int g, int b)? moodColorForNode(String nodeId) {
    final node = nodeById(nodeId);
    if (node == null) return null;
    return _moodColorForNode(node);
  }

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

  /// True while the server is still automatically trying to reconnect a
  /// configured hub after boot/update. Manual failures are allowed through so
  /// the recovery banner can offer a retry action.
  bool get hasPendingAutomaticHubStartup {
    if (!_automaticHubStartupGraceActive) return false;
    for (final hub in serverHubs) {
      if (hub.connected) continue;
      if (hubCapabilities(hub.type)?.blocksRoomReadiness == false) continue;
      if (hub.startupRetry?.isManualRetryRequired == true) continue;
      if (hub.startupRetry?.isScheduled == true) return true;
      if (hub.startupRetry == null) return true;
    }
    return false;
  }

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

  RhythmHubCapabilities? get hueBleCapabilities => hubCapabilities('hue_ble');
  RhythmHubCapabilities? get localBleCapabilities =>
      hubCapabilities('local_ble');
  RhythmHubCapabilities? get hueBridgeCapabilities => hubCapabilities('hue');

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

  /// Whether this appliance can pair a Philips Hue bulb directly over BLE.
  ///
  /// Unlike legacy Matter behavior, this is only offered when a server
  /// explicitly advertises the vendor-specific onboarding method.
  bool get canAddHueBleDevice =>
      hueBleCapabilities?.supportsDeviceOnboardingMethod(
        RhythmDeviceOnboardingMethod.hueBleNearbyScan,
      ) ??
      false;

  Map<String, String> get _localBleProfileRoutes {
    final capabilities = localBleCapabilities;
    if (capabilities?.supportsDeviceOnboardingMethod(
          RhythmDeviceOnboardingMethod.localBleQr,
        ) !=
        true) {
      return const {};
    }
    final routes = <String, String>{};
    final ambiguousLocalIds = <String>{};
    for (final profile in capabilities!.deviceProfiles) {
      if (!profile.supportsOnboardingMethod(
        RhythmDeviceOnboardingMethod.localBleQr,
      )) {
        continue;
      }
      for (final localId in locallyRegisteredLocalBleProfileIds) {
        if (!profile.acceptsProfileId(localId) ||
            ambiguousLocalIds.contains(localId)) {
          continue;
        }
        final previous = routes[localId];
        if (previous != null && previous != profile.id) {
          routes.remove(localId);
          ambiguousLocalIds.add(localId);
        } else {
          routes[localId] = profile.id;
        }
      }
    }
    return routes;
  }

  /// Parser IDs understood by this app build and accepted by the appliance.
  /// Intake keeps using these IDs so older app parsers remain deterministic.
  Set<String> get supportedLocalBleProfileIds =>
      _localBleProfileRoutes.keys.toSet();

  /// Resolve a parser-emitted compatible ID to the appliance's one current
  /// profile ID. Only this canonical value should cross the pairing API.
  String? canonicalLocalBleProfileId(String localProfileId) =>
      _localBleProfileRoutes[localProfileId];

  RhythmDeviceType? localBleDeviceTypeForProfile(String localProfileId) {
    final canonicalProfileId = canonicalLocalBleProfileId(localProfileId);
    if (canonicalProfileId == null) return null;
    final profile = localBleCapabilities?.deviceProfiles
        .where((candidate) => candidate.id == canonicalProfileId)
        .firstOrNull;
    final deviceType = profile?.deviceType.trim();
    if (deviceType == null || deviceType.isEmpty) return null;
    return switch (deviceType) {
      'light' => RhythmDeviceType.light,
      'button' => RhythmDeviceType.button,
      'motion' => RhythmDeviceType.motion,
      'contact' => RhythmDeviceType.contact,
      _ => null,
    };
  }

  bool canAddLocalBleProfile(String profileId) =>
      supportedLocalBleProfileIds.contains(profileId);

  bool get canAddLocalBleDevice => supportedLocalBleProfileIds.isNotEmpty;

  /// Whether a connected Hue Bridge can search for a Zigbee bulb by the
  /// six-character serial printed on its label.
  bool get canAddHueBridgeDeviceBySerial =>
      connectedHubTypes.contains('hue') &&
      (hueBridgeCapabilities?.supportsDeviceOnboardingMethod(
            RhythmDeviceOnboardingMethod.hueBridgeSerialSearch,
          ) ??
          false);

  bool get canUnpairHueBleDevices =>
      hueBleCapabilities?.supportsUnpairing ?? false;

  bool get canUnpairLocalBleDevices =>
      localBleCapabilities?.supportsUnpairing ?? false;

  bool get canUnpairHueBridgeDevices =>
      hueBridgeCapabilities?.supportsUnpairing ?? false;

  bool get supportsHueBleRoomlessDevices =>
      hueBleCapabilities?.supportsRoomlessDevices ?? false;

  bool get supportsLocalBleRoomlessDevices =>
      localBleCapabilities?.supportsRoomlessDevices ?? false;

  bool get canScanToAddDevice =>
      canAddMatterDevice ||
      canAddHueBridgeDeviceBySerial ||
      canAddHueBleDevice ||
      canAddLocalBleDevice;

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
    final hubTypes = switch (source) {
      RoomSourceDto.matter => const {'matter'},
      RoomSourceDto.hue => const {'hue', 'hue_ble'},
      RoomSourceDto.homeAssistant => const {'homeassistant', 'home_assistant'},
      RoomSourceDto.bridge => const {'bridge'},
      _ => const <String>{},
    };
    // If we don't know the hub type, or have no hub info yet, assume connected.
    if (hubTypes.isEmpty || _lastHubInfos.isEmpty) return true;
    // If none of these hub types is configured, assume connected (local-only).
    if (!configuredHubTypes.any(hubTypes.contains)) return true;
    return connectedHubTypes.any(hubTypes.contains);
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

  /// Hardware-safe color-temperature envelope advertised for this node.
  ///
  /// Room nodes carry the server-computed intersection across their member
  /// lights. A null result means the server did not prove a compatible range,
  /// so callers must retain their legacy conservative bounds.
  RhythmColorTemperatureCapabilities? colorTemperatureCapabilitiesForNode(
    String nodeId,
  ) =>
      nodeById(nodeId)?.lightCapabilities?.colorTemperature;

  bool standbyEnabledForNode(String nodeId) =>
      nodeById(nodeId)?.standbyEnabled ?? false;

  bool motionActivationEnabledForNode(String nodeId) =>
      nodeById(nodeId)?.profileSettings?.isMotionActivationEnabled ?? true;

  bool motionActivationSupportedForNode(String nodeId) {
    if (HueServiceLocator.isDemoMode) return true;
    final node = nodeById(nodeId);
    return _capabilities?.supportsFeature(
              RhythmFeature.motionActivationToggle,
            ) ==
            true ||
        node?.profileSettings?.motionActivationEnabled != null;
  }

  bool lightProfileOverridesSupportedForNode(String nodeId) {
    if (HueServiceLocator.isDemoMode) return true;
    final featureSupported = _capabilities?.supportsFeature(
          RhythmFeature.roomLightProfileOverrides,
        ) ==
        true;
    if (!featureSupported) return false;
    final node = nodeById(nodeId);
    if (node?.kind == RhythmNodeKind.lightDevice) {
      // Older servers cannot prove safe per-device routing, so fail closed.
      return node?.lightCapabilities?.individualProfileOverrides == true;
    }
    return true;
  }

  bool hasNodeLightProfileOverrides(String nodeId) {
    final overrides = nodeById(nodeId)?.profileSettings?.profileOverrides;
    return overrides != null &&
        overrides.values.any((profileOverride) => !profileOverride.isEmpty);
  }

  /// Visual light-profile customization shown by compact room/node cues.
  ///
  /// Motion timeout and fade settings intentionally do not light this cue:
  /// those behaviors have their own room settings, while this badge explains
  /// why the visible brightness/color curve differs from the home profile.
  ({
    bool brightnessRange,
    bool colorTemperatureRange,
    bool otherVisual,
  }) lightProfileOverrideSummaryForNode(String nodeId) {
    final overrides = nodeById(nodeId)?.profileSettings?.profileOverrides;
    if (overrides == null || overrides.isEmpty) {
      return (
        brightnessRange: false,
        colorTemperatureRange: false,
        otherVisual: false,
      );
    }

    var brightnessRange = false;
    var colorTemperatureRange = false;
    var otherVisual = false;
    for (final profileOverride in overrides.values) {
      brightnessRange = brightnessRange ||
          profileOverride.minBrightness != null ||
          profileOverride.maxBrightness != null;
      colorTemperatureRange = colorTemperatureRange ||
          profileOverride.minColorTemp != null ||
          profileOverride.maxColorTemp != null;
      otherVisual = otherVisual ||
          profileOverride.curve != null ||
          profileOverride.maxDimSteps != null ||
          profileOverride.rhythmIntervalSetting != null;
    }
    return (
      brightnessRange: brightnessRange,
      colorTemperatureRange: colorTemperatureRange,
      otherVisual: otherVisual,
    );
  }

  bool motionActivationPendingForNode(String nodeId) =>
      _motionActivationPending.contains(nodeId);

  Future<bool> setNodeMotionActivationEnabled(
    String nodeId,
    bool enabled,
  ) async {
    final index = _helloNodes.indexWhere((node) => node.id == nodeId);
    if (index == -1 ||
        !motionActivationSupportedForNode(nodeId) ||
        _motionActivationPending.contains(nodeId) ||
        (!HueServiceLocator.isDemoMode &&
            (!_connection.connected || _receivingFromServer))) {
      return false;
    }
    final previous = _helloNodes[index];
    final previousTimer = _roomProvider.getMotionTimer(nodeId);
    final requestId = _uuid.v4();
    _motionActivationPending.add(nodeId);
    _helloNodes[index] = _copyNodeWithMotionActivationEnabled(
      previous,
      enabled,
    );
    _helloRooms = _buildRoomSummaries();
    if (!enabled) {
      _roomProvider.clearNodeMotionTimer(nodeId);
    }
    notifyListeners();

    if (HueServiceLocator.isDemoMode) {
      _motionActivationPending.remove(nodeId);
      notifyListeners();
      return true;
    }

    debugPrint(
      'ServerSync: setNodeMotionActivationEnabled $nodeId enabled=$enabled requestId=$requestId',
    );
    RhythmRoomState? authoritative;
    try {
      authoritative = await api.nodeMotionActivationSet(
        nodeId: nodeId,
        enabled: enabled,
        requestId: requestId,
      );
    } catch (error) {
      debugPrint(
        'ServerSync: motion activation request failed requestId=$requestId error=$error',
      );
    }
    final applied =
        authoritative?.profileSettings?.motionActivationEnabled == enabled;
    final currentIndex = _helloNodes.indexWhere((node) => node.id == nodeId);
    if (currentIndex != -1) {
      _helloNodes[currentIndex] = applied
          ? _copyNodeWithMotionActivationEnabled(
              _helloNodes[currentIndex],
              enabled,
            )
          : previous;
      _helloRooms = _buildRoomSummaries();
    }
    if (!applied && previousTimer != null) {
      _roomProvider.updateNodeMotionTimer(nodeId, previousTimer);
    }
    _motionActivationPending.remove(nodeId);
    notifyListeners();
    return applied;
  }

  RhythmRoom _copyNodeWithMotionActivationEnabled(
    RhythmRoom previous,
    bool enabled,
  ) {
    final previousSettings =
        previous.profileSettings ?? const RhythmNodeProfileSettings();
    return _copyNodeWithProfileSettings(
      previous,
      RhythmNodeProfileSettings(
        profileId: previousSettings.profileId,
        moodEnabled: previousSettings.moodEnabled,
        moodProfileId: previousSettings.moodProfileId,
        moodSceneId: previousSettings.moodSceneId,
        fadeSetting: previousSettings.fadeSetting,
        motionTimeoutSetting: previousSettings.motionTimeoutSetting,
        motionActivationEnabled: enabled,
        profileOverrides: previousSettings.profileOverrides,
        raw: previousSettings.raw,
      ),
    );
  }

  RhythmRoom _copyNodeWithProfileSettings(
    RhythmRoom previous,
    RhythmNodeProfileSettings profileSettings,
  ) {
    return RhythmRoom(
      id: previous.id,
      name: previous.name,
      kind: previous.kind,
      parentId: previous.parentId,
      placement: previous.placement,
      groupedLightId: previous.groupedLightId,
      state: previous.state,
      transitioning: previous.transitioning,
      pendingDispatch: previous.pendingDispatch,
      rhythmEnabled: previous.rhythmEnabled,
      disabled: previous.disabled,
      timeOffset: previous.timeOffset,
      brightnessOffset: previous.brightnessOffset,
      hubTypes: previous.hubTypes,
      manufacturer: previous.manufacturer,
      model: previous.model,
      lightCapabilities: previous.lightCapabilities,
      deviceIds: previous.deviceIds,
      devices: previous.devices,
      profileSettings: profileSettings,
      observedPower: previous.observedPower,
      moodEnabled: previous.moodEnabled,
      moodActive: previous.moodActive,
      standbyEnabled: previous.standbyEnabled,
      standbyActive: previous.standbyActive,
      lightsOn: previous.lightsOn,
      brightness: previous.brightness,
      kelvin: previous.kelvin,
      motionActive: previous.motionActive,
      motionOwned: previous.motionOwned,
      remainingSecs: previous.remainingSecs,
      timeoutSecs: previous.timeoutSecs,
      warningActive: previous.warningActive,
    );
  }

  void setNodeProfileMotionTimeoutLocal(
    String nodeId, {
    required RhythmMode mode,
    required String profileId,
    required RhythmTimerSetting? setting,
  }) {
    final index = _helloNodes.indexWhere((node) => node.id == nodeId);
    if (index == -1) return;

    final previous = _helloNodes[index];
    final previousSettings =
        previous.profileSettings ?? const RhythmNodeProfileSettings();
    final updatedSettings = RhythmNodeProfileSettings(
      profileId: previousSettings.profileId,
      moodEnabled: previousSettings.moodEnabled,
      moodProfileId: previousSettings.moodProfileId,
      moodSceneId: previousSettings.moodSceneId,
      fadeSetting: previousSettings.fadeSetting,
      motionTimeoutSetting: previousSettings.motionTimeoutSetting,
      motionActivationEnabled: previousSettings.motionActivationEnabled,
      profileOverrides: _withMotionTimeoutProfileOverride(
        previousSettings.profileOverrides,
        profileId: profileId,
        setting: setting,
      ),
      raw: previousSettings.raw,
    );

    _helloNodes[index] = RhythmRoom(
      id: previous.id,
      name: previous.name,
      kind: previous.kind,
      parentId: previous.parentId,
      placement: previous.placement,
      groupedLightId: previous.groupedLightId,
      state: previous.state,
      transitioning: previous.transitioning,
      pendingDispatch: previous.pendingDispatch,
      rhythmEnabled: previous.rhythmEnabled,
      disabled: previous.disabled,
      timeOffset: previous.timeOffset,
      brightnessOffset: previous.brightnessOffset,
      hubTypes: previous.hubTypes,
      manufacturer: previous.manufacturer,
      model: previous.model,
      lightCapabilities: previous.lightCapabilities,
      deviceIds: previous.deviceIds,
      devices: previous.devices,
      profileSettings: updatedSettings,
      observedPower: previous.observedPower,
      moodEnabled: previous.moodEnabled,
      moodActive: previous.moodActive,
      standbyEnabled: previous.standbyEnabled,
      standbyActive: previous.standbyActive,
      lightsOn: previous.lightsOn,
      brightness: previous.brightness,
      kelvin: previous.kelvin,
      motionActive: previous.motionActive,
      motionOwned: previous.motionOwned,
      remainingSecs: previous.remainingSecs,
      timeoutSecs: previous.timeoutSecs,
      warningActive: previous.warningActive,
    );
    _updateActiveMotionTimerTimeoutLocal(
      nodeId,
      mode: mode,
      settings: updatedSettings,
    );
    _helloRooms = _buildRoomSummaries();
    notifyListeners();
  }

  void _updateActiveMotionTimerTimeoutLocal(
    String nodeId, {
    required RhythmMode mode,
    required RhythmNodeProfileSettings settings,
  }) {
    if (_activeMode != mode) return;
    final existing = _roomProvider.getMotionTimer(nodeId);
    if (existing == null) return;

    final timeoutSecs = _effectiveMotionTimeoutSecsForNodeMode(mode, settings);
    final remainingSecs = existing.remainingSecs?.clamp(0, timeoutSecs).toInt();
    _roomProvider.updateNodeMotionTimer(
      nodeId,
      MotionTimerInfo(
        motionActive: existing.motionActive,
        motionOwned: existing.motionOwned,
        remainingSecs: remainingSecs,
        timeoutSecs: timeoutSecs,
        warningActive: existing.warningActive,
        receivedAt: DateTime.now(),
      ),
    );
  }

  int _effectiveMotionTimeoutSecsForNodeMode(
    RhythmMode mode,
    RhythmNodeProfileSettings settings,
  ) {
    final profileId = _activeProfileIdForMode(mode);
    final modeSetting =
        settings.profileOverrides[profileId]?.motionTimeoutSetting;
    final fixedValue =
        modeSetting?.fixedValue ?? settings.motionTimeoutSetting?.fixedValue;
    if (fixedValue != null) return fixedValue;

    for (final profile in _profiles) {
      if (profile.id == profileId) {
        return profile.motionTimeoutSecs ??
            RhythmCurveConfig.defaultMotionTimeoutSecs;
      }
    }
    return _effectiveMotionTimeoutSecs ??
        RhythmCurveConfig.defaultMotionTimeoutSecs;
  }

  String _activeProfileIdForMode(RhythmMode mode) {
    for (final config in _modeConfigs) {
      if (config.mode == mode && config.activeProfileId.isNotEmpty) {
        return config.activeProfileId;
      }
    }
    return mode == RhythmMode.sleep ? 'sleep' : 'rhythm';
  }

  Map<String, RhythmLightProfileNodeOverride> _withMotionTimeoutProfileOverride(
    Map<String, RhythmLightProfileNodeOverride> current, {
    required String profileId,
    required RhythmTimerSetting? setting,
  }) {
    final next = Map<String, RhythmLightProfileNodeOverride>.from(current);
    final existing = next[profileId];
    if (setting == null) {
      if (existing == null) {
        next.remove(profileId);
      } else {
        final updated = RhythmLightProfileNodeOverride(
          curve: existing.curve,
          minColorTemp: existing.minColorTemp,
          maxColorTemp: existing.maxColorTemp,
          minBrightness: existing.minBrightness,
          maxBrightness: existing.maxBrightness,
          maxDimSteps: existing.maxDimSteps,
          fadeSetting: existing.fadeSetting,
          rhythmIntervalSetting: existing.rhythmIntervalSetting,
          raw: existing.raw,
        );
        if (updated.isEmpty) {
          next.remove(profileId);
        } else {
          next[profileId] = updated;
        }
      }
      return Map.unmodifiable(next);
    }

    next[profileId] = RhythmLightProfileNodeOverride(
      curve: existing?.curve,
      minColorTemp: existing?.minColorTemp,
      maxColorTemp: existing?.maxColorTemp,
      minBrightness: existing?.minBrightness,
      maxBrightness: existing?.maxBrightness,
      maxDimSteps: existing?.maxDimSteps,
      fadeSetting: existing?.fadeSetting,
      motionTimeoutSetting: setting,
      rhythmIntervalSetting: existing?.rhythmIntervalSetting,
      raw: existing?.raw ?? const <String, dynamic>{},
    );
    return Map.unmodifiable(next);
  }

  void setNodeStandbyEnabledLocal(String nodeId, bool enabled) {
    final lockedUntil = DateTime.now().add(_standbyEnabledLockDuration);
    _standbyEnabledLockedUntil[nodeId] = lockedUntil;
    _optimisticStandbyEnabled[nodeId] = enabled;
    _suppressedStandbyEnabled.remove(nodeId);

    final index = _helloNodes.indexWhere((node) => node.id == nodeId);
    if (index == -1 || _helloNodes[index].standbyEnabled == enabled) return;

    _helloNodes[index] =
        _copyNodeWithStandbyEnabled(_helloNodes[index], enabled);
    _helloRooms = _buildRoomSummaries();
    notifyListeners();
  }

  RhythmRoom _copyNodeWithStandbyEnabled(
    RhythmRoom previous,
    bool enabled,
  ) {
    return RhythmRoom(
      id: previous.id,
      name: previous.name,
      kind: previous.kind,
      parentId: previous.parentId,
      placement: previous.placement,
      groupedLightId: previous.groupedLightId,
      state: previous.state,
      transitioning: previous.transitioning,
      pendingDispatch: previous.pendingDispatch,
      rhythmEnabled: previous.rhythmEnabled,
      disabled: previous.disabled,
      timeOffset: previous.timeOffset,
      brightnessOffset: previous.brightnessOffset,
      hubTypes: previous.hubTypes,
      manufacturer: previous.manufacturer,
      model: previous.model,
      lightCapabilities: previous.lightCapabilities,
      deviceIds: previous.deviceIds,
      devices: previous.devices,
      profileSettings: previous.profileSettings,
      observedPower: previous.observedPower,
      moodEnabled: previous.moodEnabled,
      moodActive: previous.moodActive,
      standbyEnabled: enabled,
      standbyActive: previous.standbyActive,
      lightsOn: previous.lightsOn,
      brightness: previous.brightness,
      kelvin: previous.kelvin,
      motionActive: previous.motionActive,
      motionOwned: previous.motionOwned,
      remainingSecs: previous.remainingSecs,
      timeoutSecs: previous.timeoutSecs,
      warningActive: previous.warningActive,
    );
  }

  bool _standbyEnabledFromIncoming(String nodeId, bool incoming) {
    final optimistic = _optimisticStandbyEnabled[nodeId];
    final lockedUntil = _standbyEnabledLockedUntil[nodeId];
    if (optimistic == null || lockedUntil == null) {
      _suppressedStandbyEnabled.remove(nodeId);
      return incoming;
    }

    final now = DateTime.now();
    if (!now.isBefore(lockedUntil)) {
      _standbyEnabledLockedUntil.remove(nodeId);
      _optimisticStandbyEnabled.remove(nodeId);
      _suppressedStandbyEnabled.remove(nodeId);
      _standbyEnabledLockTimers.remove(nodeId)?.cancel();
      return incoming;
    }

    if (incoming == optimistic) {
      _clearStandbyEnabledOptimisticState(nodeId);
      return incoming;
    }

    _suppressedStandbyEnabled[nodeId] = incoming;
    _armStandbyEnabledLockExpiry(nodeId, lockedUntil);
    return optimistic;
  }

  List<RhythmRoom> _mergeOptimisticStandbyEnabled(
    List<RhythmRoom> incomingNodes,
  ) {
    var changed = false;
    final merged = <RhythmRoom>[];
    for (final node in incomingNodes) {
      final standbyEnabled =
          _standbyEnabledFromIncoming(node.id, node.standbyEnabled);
      if (standbyEnabled == node.standbyEnabled) {
        merged.add(node);
      } else {
        changed = true;
        merged.add(_copyNodeWithStandbyEnabled(node, standbyEnabled));
      }
    }
    return changed ? merged : incomingNodes;
  }

  void _armStandbyEnabledLockExpiry(String nodeId, DateTime lockedUntil) {
    final delay = lockedUntil.difference(DateTime.now()) +
        const Duration(milliseconds: 50);
    _standbyEnabledLockTimers[nodeId]?.cancel();
    _standbyEnabledLockTimers[nodeId] =
        Timer(delay.isNegative ? Duration.zero : delay, () {
      _standbyEnabledLockTimers.remove(nodeId);

      final currentLock = _standbyEnabledLockedUntil[nodeId];
      if (currentLock != null && DateTime.now().isBefore(currentLock)) {
        _armStandbyEnabledLockExpiry(nodeId, currentLock);
        return;
      }

      _standbyEnabledLockedUntil.remove(nodeId);
      _optimisticStandbyEnabled.remove(nodeId);
      final suppressed = _suppressedStandbyEnabled.remove(nodeId);
      if (suppressed == null) return;

      final index = _helloNodes.indexWhere((node) => node.id == nodeId);
      if (index == -1 || _helloNodes[index].standbyEnabled == suppressed) {
        return;
      }

      _helloNodes[index] =
          _copyNodeWithStandbyEnabled(_helloNodes[index], suppressed);
      _helloRooms = _buildRoomSummaries();
      notifyListeners();
    });
  }

  void _clearStandbyEnabledOptimisticState(String nodeId) {
    _standbyEnabledLockedUntil.remove(nodeId);
    _optimisticStandbyEnabled.remove(nodeId);
    _suppressedStandbyEnabled.remove(nodeId);
    _standbyEnabledLockTimers.remove(nodeId)?.cancel();
  }

  void _clearStandbyEnabledOptimisticStates() {
    for (final timer in _standbyEnabledLockTimers.values) {
      timer.cancel();
    }
    _standbyEnabledLockTimers.clear();
    _standbyEnabledLockedUntil.clear();
    _optimisticStandbyEnabled.clear();
    _suppressedStandbyEnabled.clear();
  }

  RhythmTopologyNode? topologyNodeById(String nodeId) =>
      _topologyNodes.where((node) => node.id == nodeId).firstOrNull;

  (int r, int g, int b)? _moodColorForNode(RhythmRoom node) {
    final moodProfileId = node.profileSettings?.moodProfileId;
    if (moodProfileId == null || moodProfileId.isEmpty) return null;
    final profile =
        _profiles.where((profile) => profile.id == moodProfileId).firstOrNull;
    final directColor =
        profile == null ? null : _directColorForProfile(profile);
    final rgb = directColor?.rgb;
    if (rgb == null) return null;
    return (rgb.r, rgb.g, rgb.b);
  }

  int? _moodBrightnessForNode(RhythmRoom node) {
    final moodProfileId = node.profileSettings?.moodProfileId;
    if (moodProfileId == null || moodProfileId.isEmpty) return null;
    final profile =
        _profiles.where((profile) => profile.id == moodProfileId).firstOrNull;
    final curve = profile?.curve;
    if (curve is RhythmConstantCurve) {
      return curve.brightness;
    }
    return null;
  }

  RhythmDirectColor? _directColorForProfile(RhythmCurveConfig profile) {
    return switch (profile.curve) {
      RhythmSuperGaussianCurve(:final directColor) => directColor,
      RhythmConstantCurve(:final directColor) => directColor,
      _ => profile.directColor,
    };
  }

  void _syncServerMoodProfiles(Iterable<RhythmRoom> nodes) {
    if (_profiles.isEmpty) return;
    for (final node in nodes) {
      // Only override the cached mood color when this node actually resolves a
      // mood *profile* color. Scene-based moods carry their color on the live
      // node state instead, so pushing a null here would clobber it and leave
      // the mood indicator stuck on the rhythm-curve white (issue #12).
      final moodColor = _moodColorForNode(node);
      if (moodColor != null) {
        _roomProvider.setMoodColorFromServer(node.id, moodColor);
      }
      _roomProvider.setMoodBrightnessFromServer(
        node.id,
        _moodBrightnessForNode(node),
      );
    }
  }

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

  List<String> controlTargetNodeIds({
    required String sourceNodeId,
    required String controlKind,
  }) {
    final targetIds = controlsForSourceNode(sourceNodeId)
        .where((control) => control.kind == controlKind)
        .map((control) => control.targetId)
        .whereType<String>()
        .where((targetId) => targetId.isNotEmpty)
        .toSet()
        .toList()
      ..sort();
    return targetIds;
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

  List<RhythmTopologyNode> get buttonTopologyNodes => _topologyNodes
      .where((node) =>
          node.kind == RhythmNodeKind.button ||
          node.kind == RhythmNodeKind.switchDevice)
      .toList(growable: false);

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
        RhythmDeviceType.motion: 2,
        RhythmDeviceType.contact: 3,
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
    final c = devices.where((d) => d.type == RhythmDeviceType.contact).length;
    final parts = <String>[];
    if (l > 0) parts.add('$l light${l > 1 ? 's' : ''}');
    if (b > 0) parts.add('$b button${b > 1 ? 's' : ''}');
    if (m > 0) parts.add('$m sensor${m > 1 ? 's' : ''}');
    if (c > 0) parts.add('$c contact sensor${c > 1 ? 's' : ''}');
    if (parts.isEmpty) return 'No devices';
    final roomCount = rooms.length;
    return '${parts.join(', ')} across $roomCount room${roomCount > 1 ? 's' : ''}';
  }

  /// The underlying SDK connection (for SSE debug props, ping, etc.).
  RhythmConnection get connection => _connection;

  /// Whether the provider is backed by the in-memory demo server.
  bool get isDemoMode => HueServiceLocator.isDemoMode;

  /// Raw physical input events forwarded by the server.
  Stream<RhythmInputEvent> get inputEvents => _connection.inputEvents;

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
    @visibleForTesting ServerEndpointReachability? endpointReachability,
    Future<List<ConnectivityResult>> Function()? connectivityCheck,
    @visibleForTesting ServerAuthApiFactory? authApiFactory,
    @visibleForTesting
    RemoteAccessAutoEnableScheduler? remoteAccessAutoEnableScheduler,
  })  : _connection = connection,
        _roomProvider = roomProvider,
        _homeProvider = homeProvider,
        _endpointReachability = endpointReachability,
        _connectivityCheck = connectivityCheck,
        _authApiFactory = authApiFactory ?? _defaultAuthApiFactory,
        _remoteAccessAutoEnableScheduler = remoteAccessAutoEnableScheduler ??
            RemoteAccessService.instance.scheduleAutoEnableForHub {
    // Listen for connection events
    _helloSub = _connection.helloEvents.listen(_onHello);
    _rhythmStateSub = _connection.rhythmStateEvents.listen(_onRhythmState);
    _dispatchFailureSub =
        _connection.dispatchFailureEvents.listen(_onDispatchFailure);
    _hubEventSub = _connection.hubEvents.listen(_onHubEvent);
    _motionTimerSub = _connection.motionTimerEvents.listen(_onMotionTimer);
    _modeChangedSub = _connection.modeChangedEvents.listen(_onModeChanged);
    _settingsChangedSub =
        _connection.settingsChangedEvents.listen(_onSettingsChanged);
    _lightBreakerChangedSub =
        _connection.lightBreakerChangedEvents.listen(_onLightBreakerChanged);
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

    _authStateSub = AuthService().authStateChanges.listen(_onAuthStateChanged);
  }

  void _beginRoomReadinessRefresh() {
    if (HueServiceLocator.isDemoMode || _serverHub == null) return;
    if (_roomReadinessRefreshPending) return;
    _roomReadinessRefreshPending = true;
    notifyListeners();
  }

  void _completeRoomReadinessRefresh({bool notify = true}) {
    if (!_roomReadinessRefreshPending) return;
    _roomReadinessRefreshPending = false;
    if (notify) notifyListeners();
  }

  String? get _blockingAutomaticHubStartupSignature {
    final keys = <String>[];
    for (final hub in serverHubs) {
      if (hub.connected) continue;
      if (hubCapabilities(hub.type)?.blocksRoomReadiness == false) continue;
      if (hub.startupRetry?.isManualRetryRequired == true) continue;
      if (hub.startupRetry?.isScheduled == true || hub.startupRetry == null) {
        keys.add('${hub.type}@${hub.address}');
      }
    }
    if (keys.isEmpty) return null;
    keys.sort();
    return keys.join('|');
  }

  void _scheduleRoomReadinessGraceExpiryIfNeeded() {
    final signature = _blockingAutomaticHubStartupSignature;
    // Once hello already contains usable rooms, integration recovery is no
    // longer a presentation gate and does not need an eight-second timer.
    if (_helloRooms.isNotEmpty ||
        signature == null ||
        HueServiceLocator.isDemoMode) {
      _roomReadinessGraceTimer?.cancel();
      _roomReadinessGraceTimer = null;
      _automaticHubStartupGraceActive = false;
      _automaticHubStartupGraceSignature = null;
      return;
    }

    // Repeated authoritative hellos for the same failed hub set must not
    // restart this gate forever. Recovery continues in the background and the
    // hub banner remains available after cached/healthy rooms are revealed.
    if (_automaticHubStartupGraceSignature == signature) return;

    _roomReadinessGraceTimer?.cancel();
    _automaticHubStartupGraceSignature = signature;
    _automaticHubStartupGraceActive = true;
    _roomReadinessGraceTimer = Timer(_automaticHubStartupGrace, () {
      _roomReadinessGraceTimer = null;
      _automaticHubStartupGraceActive = false;
      notifyListeners();
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
      final serverHub = _homeProvider.activeServerHub;
      if (serverHub != null) {
        final hubChanged = !_sameServerHubIdentity(_serverHub, serverHub);
        _serverHub = serverHub;
        if (hubChanged) {
          _hasBeenSynced = false;
          notifyListeners();
          // Refresh demo state once per hub change. Don't do this on every
          // call — ProxyProvider re-fires on every RoomProvider notify, and
          // `_refreshDemoState` itself notifies RoomProvider, which would
          // loop forever.
          unawaited(_refreshDemoState());
        }
      }
      return;
    }

    final serverHub = _homeProvider.activeServerHub;

    if (serverHub != null) {
      if (_sameServerHubIdentity(serverHub, _serverHub) &&
          serverHub.endpoint.host == _serverHub?.endpoint.host &&
          serverHub.endpoint.port == _serverHub?.endpoint.port &&
          serverHub.endpoint.useSsl == _serverHub?.endpoint.useSsl &&
          (!FeatureFlags.remoteAccessTunnel ||
              _sameEndpoint(
                  serverHub.remoteEndpoint, _serverHub?.remoteEndpoint)) &&
          serverHub.token == _serverHub?.token) {
        return; // Same hub, no change
      }
      debugPrint(
          'ServerSync: connectIfAvailable — connecting to ${serverHub.endpoint.host}:${serverHub.endpoint.port}');
      final hubChanged = !_sameServerHubIdentity(_serverHub, serverHub);
      _serverHub = serverHub;
      if (hubChanged) {
        _hasBeenSynced = false;
        _resetConnectionMetadata();
      } else {
        // The Hub record is the same, but its connection target or
        // credentials changed. Do not expose a hello identity learned from
        // the previous target while the replacement connection is starting.
        _lastServerInstanceId = null;
      }
      // Defer all side-effects to avoid notifyListeners during ProxyProvider build phase
      final pendingHub = serverHub;
      Future.microtask(() async {
        await _connectToServerHub(
          pendingHub,
          clearTransientState: true,
        );
      });
    } else if (_serverHub != null) {
      debugPrint('ServerSync: connectIfAvailable — hub removed, disconnecting');
      final removedHub = _serverHub;
      _serverHub = null;
      _activeConnectionEndpoint = null;
      _hasBeenSynced = false;
      _roomReadinessRefreshPending = false;
      _automaticHubStartupGraceActive = false;
      _automaticHubStartupGraceSignature = null;
      _roomReadinessGraceTimer?.cancel();
      _roomReadinessGraceTimer = null;
      _resetConnectionMetadata();
      Future.microtask(() async {
        final localServer = LocalRhythmServerService.instance;
        if (removedHub != null &&
            localServer.isLocalEndpoint(
              removedHub.endpoint.host,
              removedHub.endpoint.port,
            )) {
          await _stopLocalServer();
        }
        await _roomProvider.clearAllRooms();
        _connection.disconnect();
      });
    }
  }

  Future<void> _connectToServerHub(
    Hub hub, {
    required bool clearTransientState,
    bool assumeLanReachable = false,
    bool assumeSavedAuth = false,
  }) async {
    // Saved endpoints are the fastest usable candidates. Refreshing the same
    // hub through Supabase before every connection adds two cloud reads and
    // prevents an otherwise reachable LAN/tunnel endpoint from starting. The
    // existing disconnect/failover path refreshes endpoint metadata when a
    // saved candidate actually fails.
    final targetHub = hub;

    if (clearTransientState) {
      _roomProvider.clearTransientState();
    }
    await _syncLocalServerProcessForHub(targetHub);
    if (!_sameServerHubIdentity(_serverHub, targetHub)) return;

    final auth = await _prepareServerHubAuth(
      targetHub,
      assumeSavedAuth: assumeSavedAuth,
    );
    if (!_sameServerHubIdentity(_serverHub, targetHub)) return;

    _serverHub = auth.hub;
    // Owner claiming happens in _prepareServerHubAuth. Queue tunnel
    // provisioning as soon as that token is persisted instead of making it
    // wait for the full SDK connection, which can stall during first setup.
    _scheduleRemoteAccessAutoEnable(auth.hub);
    final endpoint = await _selectConnectionEndpoint(
      auth.hub,
      auth.authToken,
      assumeLanReachable: assumeLanReachable,
    );
    if (!_sameServerHubIdentity(_serverHub, targetHub)) return;
    if (endpoint == null) {
      _activeConnectionEndpoint = null;
      _connection.disconnect();
      notifyListeners();
      return;
    }

    if (!_sameEndpoint(_activeConnectionEndpoint, endpoint)) {
      _lastServerInstanceId = null;
    }
    _activeConnectionEndpoint = endpoint;
    await _connection.connect(
      endpoint.host,
      port: endpoint.port,
      useSsl: endpoint.useSsl,
      authToken: auth.authToken,
    );
    notifyListeners();
  }

  void _scheduleRemoteAccessAutoEnable(Hub hub) {
    if (HueServiceLocator.isDemoMode || !FeatureFlags.remoteAccessTunnel) {
      return;
    }
    if (hub.type != HubType.server) return;

    final home = _homeProvider.currentHome;
    if (home == null) return;

    _remoteAccessAutoEnableScheduler(
      home: home,
      serverHub: hub,
      saveHub: _homeProvider.updateHub,
      resolveLatestHub: _latestCurrentHomeHub,
      onEnabled: (_) => connectIfAvailable(),
    );
  }

  Hub? _latestCurrentHomeHub(String homeId, String hubId) {
    if (_homeProvider.currentHome?.id != homeId) return null;
    for (final hub in _homeProvider.currentHomeHubs) {
      if (hub.id == hubId) return hub;
    }
    return null;
  }

  /// Retry the active server by reselecting LAN vs tunnel before reconnecting.
  ///
  /// This intentionally does more than `RhythmConnection.pingOrReconnect()`;
  /// a stale LAN endpoint after app resume must be allowed to fall back to the
  /// saved tunnel endpoint.
  Future<void> retryActiveServerConnection({
    bool authoritative = false,
    bool assumeLanReachable = false,
    bool assumeSavedAuth = false,
  }) async {
    final hub = _homeProvider.activeServerHub ?? _serverHub;
    if (hub == null) return;

    final hubChanged = !_sameServerHubIdentity(_serverHub, hub);
    _serverHub = hub;
    if (hubChanged) {
      _hasBeenSynced = false;
      _resetConnectionMetadata();
      _roomProvider.clearTransientState();
    }
    if (authoritative) {
      _beginRoomReadinessRefresh();
    }
    await _connectToServerHub(
      hub,
      clearTransientState: false,
      assumeLanReachable: assumeLanReachable,
      assumeSavedAuth: assumeSavedAuth,
    );
    if (authoritative) {
      await _connection.reconnect(authoritative: true);
    }
  }

  Future<void> _syncLocalServerProcessForHub(Hub hub) async {
    final localServer = LocalRhythmServerService.instance;
    if (localServer.isLocalEndpoint(hub.endpoint.host, hub.endpoint.port)) {
      try {
        await localServer.start(port: hub.endpoint.port);
      } catch (error) {
        debugPrint('ServerSync: local Rhythm Server start failed: $error');
      }
      return;
    }

    await _stopLocalServer();
  }

  Future<void> _stopLocalServer() async {
    final localServer = LocalRhythmServerService.instance;
    if (!localServer.canManageLocalServer) return;

    try {
      await localServer.stop();
    } catch (error) {
      debugPrint('ServerSync: local Rhythm Server stop failed: $error');
    }
  }

  Future<({Hub hub, String? authToken})> _prepareServerHubAuth(
    Hub hub, {
    bool assumeSavedAuth = false,
  }) async {
    final existingToken = hub.token?.trim();
    if (existingToken != null && existingToken.isNotEmpty) {
      return (
        hub: hub,
        authToken: existingToken,
      );
    }
    if (assumeSavedAuth) {
      return (hub: hub, authToken: null);
    }

    try {
      final authApi = _authApiFactory(baseUrl: hub.endpoint.baseUrl);
      final status = await authApi.getStatus();

      final shouldClaimToken = status.claimAvailable &&
          (status.requiresAuth || FeatureFlags.remoteAccessTunnel);
      if (shouldClaimToken) {
        final claim = await authApi.claimOwnerToken();
        final claimedHub = hub.copyWith(token: claim.token);
        final saved = await _homeProvider.updateHub(claimedHub);
        if (!saved) {
          throw StateError('Could not persist the claimed server owner token.');
        }
        debugPrint(
          'ServerSync: claimed owner token for ${hub.endpoint.host}:${hub.endpoint.port}',
        );
        return (hub: claimedHub, authToken: claim.token);
      }

      if (!status.requiresAuth) {
        return (hub: hub, authToken: null);
      }

      if (!status.claimAvailable) {
        debugPrint(
          'ServerSync: ${hub.endpoint.host}:${hub.endpoint.port} requires '
          'API auth but no owner token is saved',
        );
        return (hub: hub, authToken: null);
      }

      return (hub: hub, authToken: null);
    } catch (error) {
      debugPrint(
        'ServerSync: unable to resolve API auth for ${hub.endpoint.host}:${hub.endpoint.port}: $error',
      );
      return (
        hub: hub,
        authToken: existingToken == null || existingToken.isEmpty
            ? null
            : existingToken,
      );
    }
  }

  static RhythmAuthApi _defaultAuthApiFactory({required String baseUrl}) {
    return RhythmAuthApi(baseUrl: baseUrl);
  }

  Future<HubEndpoint?> _selectConnectionEndpoint(
    Hub hub,
    String? authToken, {
    bool assumeLanReachable = false,
  }) async {
    final remote = hub.remoteEndpoint;
    if (!FeatureFlags.remoteAccessTunnel || remote == null) {
      return hub.endpoint;
    }

    if (assumeLanReachable) {
      return hub.endpoint;
    }

    if (await _isCellularOnly()) {
      if (authToken?.trim().isEmpty != false) {
        debugPrint(
          'ServerSync: Cellular connection requires a saved owner token for remote access',
        );
        return null;
      }
      debugPrint(
        'ServerSync: Cellular connection detected, skipping LAN probe and using ${remote.host}:${remote.port}',
      );
      return remote;
    }

    if (await _canReachEndpoint(hub.endpoint, authToken)) {
      return hub.endpoint;
    }

    if (authToken?.trim().isEmpty != false) {
      debugPrint(
        'ServerSync: LAN endpoint ${hub.endpoint.host}:${hub.endpoint.port} '
        'unreachable and remote access has no saved owner token',
      );
      return null;
    }

    debugPrint(
      'ServerSync: LAN endpoint ${hub.endpoint.host}:${hub.endpoint.port} '
      'unreachable, falling back to ${remote.host}:${remote.port}',
    );
    return remote;
  }

  Future<bool> _isCellularOnly() async {
    final connectivityCheck = _connectivityCheck;
    if (connectivityCheck == null) return false;
    try {
      final results =
          await connectivityCheck().timeout(_homeEntryConnectivityTimeout);
      final hasLan = results.contains(ConnectivityResult.wifi) ||
          results.contains(ConnectivityResult.ethernet);
      return !hasLan && results.contains(ConnectivityResult.mobile);
    } catch (_) {
      // Unknown connectivity still gets the bounded LAN reachability probe.
      return false;
    }
  }

  Future<bool> _canReachEndpoint(
    HubEndpoint endpoint,
    String? authToken,
  ) async {
    final reachability = _endpointReachability;
    if (reachability != null) {
      return reachability(endpoint, authToken);
    }

    try {
      await RhythmAuthApi(
        baseUrl: endpoint.baseUrl,
        authToken: authToken,
      ).getStatus().timeout(const Duration(seconds: 2));
      return true;
    } catch (_) {
      return false;
    }
  }

  /// Start gating the Home tab while a user is entering/logging into a Home.
  ///
  /// The paired server may still be resolving LAN vs tunnel at this point; the
  /// authoritative refresh is kicked off separately once the HomeProvider has
  /// switched the active hub.
  void beginHomeEntryRefresh({required String homeName}) {
    _homeEntryRefreshGeneration++;
    _completeHomeEntryRefreshWaiter();
    _homeEntryRefreshHelloCompleter = null;
    _homeEntryRefreshPending = true;
    _homeEntryRefreshAwaitingHello = false;
    _homeEntryRefreshHomeName = homeName;
    _homeEntryRefreshHubId =
        _homeProvider.activeServerHub?.id ?? _serverHub?.id;
    _homeEntryRefreshHomeId =
        _homeProvider.activeServerHub?.homeId ?? _serverHub?.homeId;
    _homeEntryRefreshError = null;
    notifyListeners();
  }

  /// Clear the Home-entry gate, optionally leaving an error for retry UI.
  void cancelHomeEntryRefresh({String? error}) {
    _homeEntryRefreshGeneration++;
    _completeHomeEntryRefreshWaiter();
    _homeEntryRefreshHelloCompleter = null;
    _homeEntryRefreshPending = false;
    _homeEntryRefreshAwaitingHello = false;
    _homeEntryRefreshHubId = null;
    _homeEntryRefreshHomeId = null;
    _homeEntryRefreshError = error;
    notifyListeners();
  }

  void _completeHomeEntryRefreshWaiter() {
    final completer = _homeEntryRefreshHelloCompleter;
    if (completer != null && !completer.isCompleted) {
      completer.complete();
    }
  }

  /// Force a fresh authoritative hello before allowing All Rooms to render.
  Future<bool> refreshForHomeEntry({
    required String homeName,
    Duration timeout = const Duration(seconds: 20),
    bool allowWifiFastPath = true,
    Duration wifiFastPathTimeout = _homeEntryWifiFastPathTimeout,
  }) async {
    if (HueServiceLocator.isDemoMode) {
      beginHomeEntryRefresh(homeName: homeName);
      try {
        await _refreshDemoState();
        cancelHomeEntryRefresh();
        return true;
      } catch (error) {
        debugPrint('ServerSync: demo Home entry refresh failed: $error');
        cancelHomeEntryRefresh(error: 'Could not refresh $homeName');
        return false;
      }
    }

    final hub = _homeProvider.activeServerHub ?? _serverHub;
    if (hub == null) {
      if (!_homeEntryRefreshPending) {
        beginHomeEntryRefresh(homeName: homeName);
      }
      cancelHomeEntryRefresh(error: 'No Box is saved for $homeName');
      return false;
    }

    if (allowWifiFastPath &&
        !_homeEntryRefreshPending &&
        _homeEntryRefreshError == null &&
        await _canUseWifiHomeEntryFastPath(hub)) {
      final fastPathSucceeded = await _refreshForHomeEntryFastPath(
        hub: hub,
        homeName: homeName,
        timeout: wifiFastPathTimeout,
      );
      if (fastPathSucceeded) {
        return true;
      }
    }

    if (!_homeEntryRefreshPending) {
      beginHomeEntryRefresh(homeName: homeName);
    } else {
      _homeEntryRefreshHomeName = homeName;
      _homeEntryRefreshError = null;
      notifyListeners();
    }

    final generation = _homeEntryRefreshGeneration;
    _homeEntryRefreshHubId = hub.id;
    _homeEntryRefreshHomeId = hub.homeId;
    notifyListeners();

    try {
      await retryActiveServerConnection();
      await _waitForHomeEntryConnection(
        hubId: hub.id,
        homeId: hub.homeId,
        generation: generation,
        timeout: timeout,
      );
      if (_homeEntryRefreshGeneration != generation) return false;

      final helloCompleter = Completer<void>();
      _homeEntryRefreshHelloCompleter = helloCompleter;
      _homeEntryRefreshAwaitingHello = true;
      notifyListeners();

      await _connection.reconnect(authoritative: true);
      await helloCompleter.future.timeout(timeout);

      if (_homeEntryRefreshGeneration != generation) return false;
      cancelHomeEntryRefresh();
      return true;
    } on TimeoutException catch (error) {
      debugPrint('ServerSync: Home entry refresh timed out: $error');
      if (_homeEntryRefreshGeneration == generation) {
        cancelHomeEntryRefresh(error: 'Could not refresh $homeName');
      }
      return false;
    } catch (error) {
      debugPrint('ServerSync: Home entry refresh failed: $error');
      if (_homeEntryRefreshGeneration == generation) {
        cancelHomeEntryRefresh(error: 'Could not refresh $homeName');
      }
      return false;
    }
  }

  Future<bool> _refreshForHomeEntryFastPath({
    required Hub hub,
    required String homeName,
    required Duration timeout,
  }) async {
    _homeEntryRefreshGeneration++;
    _completeHomeEntryRefreshWaiter();
    final generation = _homeEntryRefreshGeneration;
    final helloCompleter = Completer<void>();
    _homeEntryRefreshHelloCompleter = helloCompleter;
    _homeEntryRefreshPending = false;
    _homeEntryRefreshAwaitingHello = true;
    _homeEntryRefreshHomeName = homeName;
    _homeEntryRefreshHubId = hub.id;
    _homeEntryRefreshHomeId = hub.homeId;
    _homeEntryRefreshError = null;

    try {
      await retryActiveServerConnection(
        assumeLanReachable: true,
        assumeSavedAuth: true,
      );
      if (_homeEntryRefreshGeneration != generation) return false;

      await _connection.reconnect(authoritative: true);
      await helloCompleter.future.timeout(timeout);

      if (_homeEntryRefreshGeneration != generation) return false;
      cancelHomeEntryRefresh();
      return true;
    } catch (error) {
      debugPrint('ServerSync: Wi-Fi Home entry fast path missed: $error');
      if (_homeEntryRefreshGeneration == generation) {
        _completeHomeEntryRefreshWaiter();
        _homeEntryRefreshHelloCompleter = null;
        _homeEntryRefreshAwaitingHello = false;
        _homeEntryRefreshHubId = null;
        _homeEntryRefreshHomeId = null;
      }
      return false;
    }
  }

  Future<bool> _canUseWifiHomeEntryFastPath(Hub hub) async {
    if (!_hasBeenSynced || !_sameServerHubIdentity(_serverHub, hub)) {
      return false;
    }

    try {
      final results =
          await (_connectivityCheck ?? Connectivity().checkConnectivity)
              .call()
              .timeout(_homeEntryConnectivityTimeout);
      return results.contains(ConnectivityResult.wifi) ||
          results.contains(ConnectivityResult.ethernet);
    } catch (_) {
      return false;
    }
  }

  Future<void> _waitForHomeEntryConnection({
    required String hubId,
    required String homeId,
    required int generation,
    required Duration timeout,
  }) async {
    final deadline = DateTime.now().add(timeout);

    while (true) {
      if (_homeEntryRefreshGeneration != generation) return;

      final activeHub = _homeProvider.activeServerHub ?? _serverHub;
      if (activeHub != null &&
          (activeHub.id != hubId || activeHub.homeId != homeId)) {
        throw StateError('Active server hub changed while entering Home');
      }

      final connectedHub = _serverHub;
      if ((_connection.connected ||
              _connection.connectionState == RhythmConnectionState.connected) &&
          connectedHub != null &&
          connectedHub.id == hubId &&
          connectedHub.homeId == homeId &&
          _activeEndpointIsUnsetOrBelongsToHub(connectedHub)) {
        return;
      }

      if (DateTime.now().isAfter(deadline)) {
        throw TimeoutException('Timed out connecting to Home server', timeout);
      }

      await Future<void>.delayed(const Duration(milliseconds: 100));
    }
  }

  bool _activeEndpointIsUnsetOrBelongsToHub(Hub hub) {
    final activeEndpoint = _activeConnectionEndpoint;
    if (activeEndpoint == null) return true;
    return _sameEndpoint(activeEndpoint, hub.endpoint) ||
        _sameEndpoint(activeEndpoint, hub.remoteEndpoint);
  }

  /// Trigger a full reconnect (for pull-to-refresh).
  ///
  /// Re-fetches `GET /api/state?authoritative=true` and emits a fresh hello so
  /// the UI gets the complete picture (devices, config, rooms, sensors,
  /// settings).
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
    _beginRoomReadinessRefresh();
    await _connection.reconnect(authoritative: true);
  }

  /// Reconcile room membership after a successful topology mutation.
  ///
  /// Unlike pull-to-refresh, this must not be throttled: dismissing a move
  /// surface while the source/destination rooms still reflect the old hello
  /// leaves the main room UI observably stale. The zero-duration yield lets
  /// the async hello stream update this provider before topology is refreshed.
  Future<bool> refreshAfterTopologyMutation() async {
    if (HueServiceLocator.isDemoMode) {
      await _refreshDemoState();
      return true;
    }
    _beginRoomReadinessRefresh();
    await _connection.reconnect(authoritative: true);
    if (!_connection.connected) return false;
    await Future<void>.delayed(Duration.zero);
    return _refreshTopologyNodesWithResult();
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

  /// Ensure a room-detail live preview has a recent rhythm tick timestamp.
  ///
  /// The top countdown ring in [RoomSettingsSheet] is driven by
  /// `RoomProvider.getLastTickTime`. If the app missed tick events before the
  /// sheet opens, that timestamp can be absent or stale until the user performs
  /// a pull-to-refresh. In that case, request the same authoritative state
  /// refresh used by pull-to-refresh.
  Future<void> ensureRoomPreviewStateFresh(String nodeId) async {
    final node = _roomProvider.getNode(nodeId);
    if (node == null || !node.rhythmEnabled || !node.lightsOn) return;

    final now = DateTime.now();
    final lastPreviewRefresh = _lastPreviewRefreshTimeByNode[nodeId];
    if (lastPreviewRefresh != null &&
        now.difference(lastPreviewRefresh).inSeconds < 2) {
      return;
    }

    final intervalSecs = _rhythmIntervalSecs <= 0 ? 60 : _rhythmIntervalSecs;
    final staleAfterSecs = intervalSecs + 2 < 10 ? 10 : intervalSecs + 2;
    final lastTick = _roomProvider.getLastTickTime(nodeId);
    if (lastTick != null &&
        now.difference(lastTick) <= Duration(seconds: staleAfterSecs)) {
      return;
    }

    _lastPreviewRefreshTimeByNode[nodeId] = now;
    await fullRefresh();
  }

  // ============================================================================
  // Event handlers (Server → App)
  // ============================================================================

  /// Handle hello from server — accept rooms and reconcile config.
  void _onHello(RhythmHello hello) {
    final helloNodes = _mergeOptimisticStandbyEnabled(hello.nodes);
    debugPrint(
        'ServerSync: Hello received with ${helloNodes.length} nodes, version=${hello.version}');
    debugPrint('ServerSync: Server active profile: ${hello.activeProfile}');
    debugPrint('ServerSync: Server location: ${hello.location}');
    for (final r in helloNodes) {
      debugPrint(
          'ServerSync: Server node "${r.name}" kind=${r.kind.name} rhythm=${r.rhythmEnabled} offset=${r.timeOffset} state=${r.state.wireValue} transitioning=${r.transitioning}');
    }
    _lastServerInstanceId = hello.serverInstanceId;
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
    final settings = hello.settings;
    if (settings?.hasPowerSave == true) {
      _powerSave = settings!.powerSave;
    }
    _autoUpdate = settings?.autoUpdate ?? true;
    _lightBreakerEnabled = hello.lightBreaker?.enabled ?? true;
    _lightRuntime = _authoritativeLightRuntimeFromHello(hello) ?? _lightRuntime;
    _activeMode = hello.mode?.active;
    _modeTransitions = [...hello.transitions];
    _inputBindings = [...hello.inputBindings];
    // Only replace the cached scenes when this hello actually carries them —
    // incremental state pushes (e.g. after binding a mood scene) can arrive
    // with an empty list and would otherwise wipe the gallery.
    if (hello.scenes.isNotEmpty || _scenes.isEmpty) {
      _scenes = [...hello.scenes];
    }
    _optimisticMoodSceneIds.clear();
    _moodSceneApplyGenerations.clear();
    _modeConfigs = [...?hello.mode?.configs];
    _profiles = [...hello.profiles];
    _activeProfileId = hello.activeProfile['id'] as String? ??
        hello.mode?.activeConfig?.activeProfileId;
    _rhythmIntervalSecs = activeProfileConfig?.rhythmIntervalSecs ?? 60;
    _effectiveFadeMs = activeProfileConfig?.fadeMs ?? hello.effectiveFadeMs;
    _effectiveMotionTimeoutSecs = activeProfileConfig?.motionTimeoutSecs ??
        hello.effectiveMotionTimeoutSecs;
    _review = hello.review;
    _helloNodes = helloNodes;
    _helloRooms = _buildRoomSummaries();
    _lastHubInfos = hello.hubs;
    _capabilities = hello.capabilities;
    _hasBeenSynced = true;
    _completeRoomReadinessRefresh(notify: false);
    _scheduleRoomReadinessGraceExpiryIfNeeded();

    // Bootstrap countdown timer from server's last tick timestamp
    if (hello.lastTickEpochMs != null) {
      final lastTick =
          DateTime.fromMillisecondsSinceEpoch(hello.lastTickEpochMs!);
      for (final room in helloNodes) {
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
    _suppressNextSourceSync = helloNodes.isNotEmpty;
    try {
      // 1. Accept server nodes as authoritative.
      _acceptServerNodes(helloNodes);

      // 2. Reconcile motion sensors — mark rooms that have sensors,
      //    unmark rooms that lost their sensors since last hello
      _roomProvider.setMotionSensorNodes(_sensorTargetNodeIds());
      _syncHelloMotionState(helloNodes);

      // 3. Accept server config as authoritative, then reconcile location.
      _acceptServerConfig(hello.activeProfile);
      _reconcileLocation(hello.location);
    } finally {
      _isProcessingHello = false;
    }

    // Fetch initial triage count (non-blocking). The hello stream can deliver
    // a buffered event after the connection has been torn down (e.g. right
    // after a factory reset), so skip if the underlying api is gone.
    if (_connection.connected) {
      _ensureServerActivityCloudConfigured(
        serverInstanceId: hello.serverInstanceId,
      );
      _startActivityCloudProvisioningTimer();
      api.getTriageCount().then((data) {
        if (data != null) _onTriageChanged(data);
      });
    }
    unawaited(_refreshTopologyNodes());

    final homeEntryCompleter = _homeEntryRefreshHelloCompleter;
    if (_homeEntryRefreshAwaitingHello &&
        homeEntryCompleter != null &&
        !homeEntryCompleter.isCompleted &&
        (_homeEntryRefreshHubId == null ||
            _homeEntryRefreshHubId == _serverHub?.id) &&
        (_homeEntryRefreshHomeId == null ||
            _homeEntryRefreshHomeId == _serverHub?.homeId)) {
      homeEntryCompleter.complete();
    }

    notifyListeners();
  }

  void _onAuthStateChanged(AuthUser? user) {
    if (user == null || user.isAnonymous) return;
    _ensureServerActivityCloudConfigured(
      serverInstanceId: _lastServerInstanceId,
    );
  }

  void _startActivityCloudProvisioningTimer() {
    if (!_connection.connected ||
        HueServiceLocator.isDemoMode ||
        !ServerActivityCloudProvisioningService.instance.canProvision) {
      return;
    }
    _activityCloudProvisioningTimer ??= Timer.periodic(
      _activityCloudProvisioningInterval,
      (_) => _ensureServerActivityCloudConfigured(
        serverInstanceId: _lastServerInstanceId,
      ),
    );
  }

  void _stopActivityCloudProvisioningTimer() {
    _activityCloudProvisioningTimer?.cancel();
    _activityCloudProvisioningTimer = null;
  }

  void _ensureServerActivityCloudConfigured({String? serverInstanceId}) {
    if (!_connection.connected) return;
    final serverHub = _serverHub ?? _homeProvider.activeServerHub;
    if (serverHub == null ||
        HueServiceLocator.isDemoMode ||
        !ServerActivityCloudProvisioningService.instance.canProvision) {
      return;
    }
    unawaited(
      ServerActivityCloudProvisioningService.instance
          .ensureConfigured(
        serverHub: serverHub,
        runtimeApi: _connection.runtimeApi,
        home: _homeProvider.currentHome,
        serverInstanceId: serverInstanceId,
      )
          .catchError((Object error, StackTrace stackTrace) {
        debugPrint(
          'ServerSyncProvider: activity cloud provisioning skipped: $error',
        );
        debugPrint('$stackTrace');
      }),
    );
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
            pendingDispatch: sr.pendingDispatch,
            lightsOn: sr.lightsOn,
            brightness: sr.brightness,
            kelvin: sr.kelvin,
            moodEnabled: sr.moodEnabled,
            moodActive: sr.moodActive,
          );
        }
      }
    } finally {
      _receivingFromServer = false;
    }
    _syncServerMoodProfiles(validNodes);
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
  ///
  /// [fromActionResponse] marks states returned by dispatch HTTP calls. Those
  /// are snapshotted server-side while the command is still queued
  /// (`pending_dispatch=true`), and the SSE clear can beat the HTTP response —
  /// so a response may keep or lower the pending flag but never raise it, or
  /// a stale snapshot re-lights the spinner with nothing left to clear it.
  void _onRhythmState(RhythmRoomState state,
      {bool fromActionResponse = false}) {
    _receivingFromServer = true;
    var helloChanged = false;
    var moodSceneOverrideCleared = false;
    try {
      if (_roomProvider.getNode(state.nodeId) == null) return;
      final pendingDispatch = fromActionResponse
          ? state.pendingDispatch &&
              _roomProvider.isNodeDispatchPending(state.nodeId)
          : state.pendingDispatch;
      if (pendingDispatch) {
        // A new attempt is in flight — the spinner supersedes any stale
        // failure badge; a repeat failure arrives as a fresh event.
        _clearRecentDispatchFailure(state.nodeId);
      }
      _roomProvider.applyServerNodeState(
        state.nodeId,
        rhythmEnabled: state.rhythmEnabled,
        timeOffset: state.timeOffset,
        brightnessOffset: state.brightnessOffset,
        state: state.state,
        transitioning: state.transitioning,
        pendingDispatch: pendingDispatch,
        mode: state.mode,
        lightsOn: state.lightsOn,
        brightness: state.brightness,
        kelvin: state.kelvin,
        color: state.color != null
            ? (state.color!.r, state.color!.g, state.color!.b)
            : null,
        moodEnabled: state.moodEnabled,
        moodActive: state.moodActive,
        tick: state.tick,
      );
      helloChanged = _updateHelloNodeFromRhythmState(state);
      if (state.profileSettings != null &&
          _optimisticMoodSceneIds.containsKey(state.nodeId)) {
        _optimisticMoodSceneIds.remove(state.nodeId);
        _moodSceneApplyGenerations[state.nodeId] =
            (_moodSceneApplyGenerations[state.nodeId] ?? 0) + 1;
        moodSceneOverrideCleared = true;
      }
    } finally {
      _receivingFromServer = false;
    }
    if (helloChanged || moodSceneOverrideCleared) {
      notifyListeners();
    }
  }

  /// Handle a hub light command that failed, timed out, or was dropped.
  ///
  /// Dispatch is fire-and-forget server-side, so this event is the only
  /// signal that a command never physically reached its target — the node's
  /// pending flag still clears and the UI would otherwise read as success.
  void _onDispatchFailure(RhythmDispatchFailure failure) {
    debugPrint('ServerSync: dispatch failure node=${failure.nodeId} $failure');
    _recentDispatchFailures[failure.nodeId] = (
      failure: failure,
      receivedAt: DateTime.now(),
    );
    // Re-notify at window expiry so the failure badge disappears on its own.
    _dispatchFailureExpiryTimers[failure.nodeId]?.cancel();
    _dispatchFailureExpiryTimers[failure.nodeId] =
        Timer(_dispatchFailureWindow, () {
      _dispatchFailureExpiryTimers.remove(failure.nodeId);
      if (_recentDispatchFailures.remove(failure.nodeId) != null) {
        notifyListeners();
      }
    });
    notifyListeners();
  }

  static const _dispatchFailureWindow = Duration(seconds: 30);
  final Map<String, ({RhythmDispatchFailure failure, DateTime receivedAt})>
      _recentDispatchFailures = {};
  final Map<String, Timer> _dispatchFailureExpiryTimers = {};

  /// The most recent dispatch failure for [nodeId], if it happened within
  /// the last [_dispatchFailureWindow]. Cards use this to show delivery
  /// problems in place of the in-flight spinner.
  RhythmDispatchFailure? recentDispatchFailureForNode(String nodeId) {
    final entry = _recentDispatchFailures[nodeId];
    if (entry == null) return null;
    if (DateTime.now().difference(entry.receivedAt) > _dispatchFailureWindow) {
      return null;
    }
    return entry.failure;
  }

  /// Drop the stored failure for [nodeId] — a new command attempt supersedes
  /// it (the spinner takes over; a repeat failure arrives as a fresh event).
  void _clearRecentDispatchFailure(String nodeId) {
    _dispatchFailureExpiryTimers.remove(nodeId)?.cancel();
    if (_recentDispatchFailures.remove(nodeId) != null) {
      notifyListeners();
    }
  }

  /// Handle motion timer updates from server.
  void _onMotionTimer(RhythmMotionTimer event) {
    // Any motion event (even clearing) means this node has a sensor.
    _roomProvider.markNodeHasSensor(event.nodeId);
    _applyMotionTimerState(
      nodeId: event.nodeId,
      motionActive: event.motionActive,
      motionOwned: event.motionOwned,
      remainingSecs: event.remainingSecs,
      timeoutSecs: event.timeoutSecs,
      warningActive: event.warningActive,
      cleared: event.isCleared,
    );

    if (_updateHelloNodeMotionState(event)) {
      notifyListeners();
    }
  }

  /// Handle global mode changes from SSE so the main Day/Sleep pills update
  /// without waiting for a settings refresh or paced per-room node_state events.
  void _onModeChanged(RhythmModeResource mode) {
    final previous = _activeMode;
    final previousLightRuntime = _lightRuntime;
    _activeMode = mode.active;
    _lightRuntime = _authoritativeLightRuntimeFromMode(mode) ?? _lightRuntime;
    if (mode.configs.isNotEmpty) {
      _modeConfigs = [...mode.configs];
      _activeProfileId = mode.activeConfig?.activeProfileId;
    }
    _modeChangeGeneration++;
    debugPrint(
        'ServerSync: mode_changed active=${mode.active.wireValue} cause=${mode.lastChange?.cause ?? ''} transition=${mode.lastChange?.transitionId ?? ''}');
    if (previous != _activeMode || previousLightRuntime != _lightRuntime) {
      notifyListeners();
    }
  }

  /// Handle settings updates with payloads. Newer servers omit legacy
  /// power-save, so only treat it as authoritative when present.
  void _onSettingsChanged(RhythmSettings settings) {
    var changed = false;
    if (settings.hasPowerSave && _powerSave != settings.powerSave) {
      _powerSave = settings.powerSave;
      changed = true;
    }
    if (_autoUpdate != settings.autoUpdate) {
      _autoUpdate = settings.autoUpdate;
      changed = true;
    }
    if (!changed) return;
    notifyListeners();
  }

  /// Handle light-breaker updates so the global control switch stays live.
  void _onLightBreakerChanged(RhythmLightBreaker lightBreaker) {
    if (_lightBreakerEnabled == lightBreaker.enabled) return;
    _lightBreakerEnabled = lightBreaker.enabled;
    notifyListeners();
  }

  /// Handle new nodes detected in poll — trigger a full re-hello.
  void _onNewNodesDetected(void _) {
    debugPrint('ServerSync: New nodes detected in poll — triggering re-hello');
    _beginRoomReadinessRefresh();
    _connection.reconnect();
  }

  /// Handle hub lifecycle events from server.
  ///
  /// When a hub connects, trigger a re-hello to pick up newly discovered
  /// rooms. When a hub disconnects, just update the UI.
  void _onHubEvent(
      ({String event, String? hubType, String? address}) hubEvent) {
    final (:event, :hubType, :address) = hubEvent;
    debugPrint('ServerSync: hub_status=$event hub=$hubType address=$address');
    // Update the specific hub's connected state in _lastHubInfos.
    // If hubType is null (legacy/aggregate event), ignore it — the hello
    // provides authoritative per-hub status and we can't safely guess
    // which hub this event refers to.
    if (_lastHubInfos.isNotEmpty && hubType != null) {
      _lastHubInfos = [
        for (final h in _lastHubInfos)
          if (h['type'] == hubType &&
              (address == null || h['address'] == address))
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
      _beginRoomReadinessRefresh();
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
      _beginRoomReadinessRefresh();
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

  void _resetConnectionMetadata() {
    _firmwareVersion = '0.0.0';
    _lastServerInstanceId = null;
    _serverPlatformType = 'desktop';
    _serverPlatformContext = 'server';
    _powerSave = true;
    _autoUpdate = true;
    _lightBreakerEnabled = true;
    _lightRuntime = RhythmLightRuntime.rhythmAdaptive;
    _activeMode = null;
    _activeProfileId = null;
    _scenes = const [];
    _roomScenes.clear();
    _optimisticMoodSceneIds.clear();
    _moodSceneApplyGenerations.clear();
    _helloNodes = [];
    _helloRooms = [];
    _clearStandbyEnabledOptimisticStates();
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

  void _onConnectionStateChanged(RhythmConnectionState current) {
    if (HueServiceLocator.isDemoMode) {
      notifyListeners();
      return;
    }
    final previous = _previousConnectionState;
    _previousConnectionState = current;

    if (current != previous &&
        (current == RhythmConnectionState.connecting ||
            current == RhythmConnectionState.disconnected ||
            current == RhythmConnectionState.reconnecting)) {
      // A hello identity authenticates one connection generation. Never carry
      // it across a reconnect where the locator could now reach another Box.
      _lastServerInstanceId = null;
    }

    if (current != previous &&
        (current == RhythmConnectionState.disconnected ||
            current == RhythmConnectionState.reconnecting)) {
      _stopActivityCloudProvisioningTimer();
      final clearMetadata = !_hasBeenSynced || _homeEntryRefreshPending;
      debugPrint(
          'ServerSync: Connection lost ($previous → $current) — ${clearMetadata ? 'resetting metadata' : 'keeping synced metadata'} and keeping rooms');
      if (clearMetadata) {
        _resetConnectionMetadata();
      }
      _maybeFailOverToRemoteEndpoint(current);
    }

    notifyListeners();
  }

  void _maybeFailOverToRemoteEndpoint(RhythmConnectionState current) {
    if (!FeatureFlags.remoteAccessTunnel ||
        _remoteFailoverInProgress ||
        (current != RhythmConnectionState.reconnecting &&
            current != RhythmConnectionState.disconnected)) {
      return;
    }

    final hub = _serverHub;
    final activeEndpoint = _activeConnectionEndpoint;
    final activeIsLan = _sameEndpoint(activeEndpoint, hub?.endpoint);
    final activeIsRemote = _sameEndpoint(activeEndpoint, hub?.remoteEndpoint);
    if (hub == null || (!activeIsLan && !activeIsRemote)) {
      return;
    }

    final now = DateTime.now();
    if (now.difference(_lastRemoteFailoverAt) < _remoteFailoverCooldown) {
      return;
    }
    _lastRemoteFailoverAt = now;
    _remoteFailoverInProgress = true;

    Future.microtask(() async {
      try {
        final failoverHub = await _homeProvider.refreshServerHubEndpoints(hub);
        final currentHub = _serverHub;
        if (!_sameServerHubIdentity(currentHub, hub) ||
            !_sameEndpoint(_activeConnectionEndpoint, activeEndpoint)) {
          return;
        }

        final token = failoverHub.token?.trim();
        final savedToken = token == null || token.isEmpty ? null : token;
        final HubEndpoint? replacement;
        if (activeIsLan &&
            _sameEndpoint(failoverHub.endpoint, activeEndpoint)) {
          // The active LAN endpoint has just failed. Do not probe and select it
          // again merely because a reachability check still succeeds; move to
          // the refreshed tunnel candidate instead.
          replacement = savedToken == null ? null : failoverHub.remoteEndpoint;
        } else {
          // Cloud metadata can replace a stale LAN endpoint without providing
          // a tunnel. Reselect from the refreshed hub rather than assuming that
          // every recovery target is remote.
          replacement = await _selectConnectionEndpoint(
            failoverHub,
            savedToken,
          );
        }
        if (!_sameServerHubIdentity(_serverHub, hub) ||
            !_sameEndpoint(_activeConnectionEndpoint, activeEndpoint) ||
            replacement == null ||
            _sameEndpoint(replacement, activeEndpoint)) {
          return;
        }

        debugPrint(
          'ServerSync: Saved endpoint ${activeEndpoint?.host}:${activeEndpoint?.port} '
          'lost, reconnecting through refreshed endpoint '
          '${replacement.host}:${replacement.port}',
        );
        _serverHub = failoverHub;
        _activeConnectionEndpoint = replacement;
        await _connection.connect(
          replacement.host,
          port: replacement.port,
          useSsl: replacement.useSsl,
          authToken: savedToken,
        );
      } catch (error) {
        debugPrint('ServerSync: remote access failover failed: $error');
      } finally {
        _remoteFailoverInProgress = false;
      }
    });
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
    _clearRecentDispatchFailure(nodeId);
    _connection.api
        .nodeAction(nodeId: nodeId, action: action)
        .then((serverState) {
      if (serverState != null) {
        _onRhythmState(serverState, fromActionResponse: true);
      }
    });
    return true;
  }

  bool dispatchAction(String roomId, String action) =>
      dispatchNodeAction(roomId, action);

  /// Dispatch multiple node actions in a single batch request.
  ///
  /// Returns the server's dispatch metadata and states, or null when the
  /// request cannot be sent. On success, applies each returned state for fast
  /// convergence.
  Future<RhythmDispatchResult?> dispatchBatchNodeActionsResult(
    List<({String nodeId, String action})> actions, {
    String? correlationId,
  }) async {
    if (actions.isEmpty) return null;
    if (HueServiceLocator.isDemoMode) {
      for (final item in actions) {
        final currentBrightness =
            _roomProvider.getBrightness(item.nodeId) ?? 50;
        switch (item.action) {
          case 'step_down':
            dispatchNodeCurveBrightness(
              item.nodeId,
              (currentBrightness - 10).clamp(1, 100).toInt(),
            );
            break;
          case 'step_up':
            dispatchNodeCurveBrightness(
              item.nodeId,
              (currentBrightness + 10).clamp(1, 100).toInt(),
            );
            break;
          case 'reset':
            _roomProvider.setRoomStateLocal(
              item.nodeId,
              RoomModeState.active,
            );
            await _roomProvider.setRoomLightsOnLocal(item.nodeId, true);
            break;
        }
      }
      return RhythmDispatchResult(
        metadata: RhythmDispatchMetadata(dispatchCount: actions.length),
      );
    }
    if (!_connection.connected) return null;
    for (final action in actions) {
      _clearRecentDispatchFailure(action.nodeId);
    }
    final result = await _connection.api.nodeActionBatchResult(
      actions,
      correlationId: correlationId,
    );
    if (result.states.isEmpty && !result.metadata.hasLoadingMetadata) {
      return null;
    }
    for (final state in result.states) {
      _onRhythmState(state, fromActionResponse: true);
    }
    return result;
  }

  /// Dispatch multiple node actions in a single batch request.
  ///
  /// Returns true if dispatched to server, false if not connected.
  Future<bool> dispatchBatchNodeActions(
      List<({String nodeId, String action})> actions) async {
    return await dispatchBatchNodeActionsResult(actions) != null;
  }

  Future<bool> dispatchBatchActions(
      List<({String roomId, String action})> actions) async {
    return dispatchBatchNodeActions([
      for (final action in actions)
        (nodeId: action.roomId, action: action.action),
    ]);
  }

  /// Apply a brightness curve modifier through the server.
  ///
  /// Returns true if dispatched to server, false if not connected.
  bool dispatchNodeCurveBrightness(String nodeId, int brightness) {
    if (HueServiceLocator.isDemoMode) {
      final currentState = _roomProvider.getRoomState(nodeId);
      final moodActive = currentState == RoomModeState.mood;
      DemoServerApi.instance.updateRoomLightState(
        nodeId,
        on: true,
        brightness: brightness,
        kelvin: _roomProvider.getKelvin(nodeId),
        state: moodActive ? RoomModeState.mood : RoomModeState.active,
      );
      _roomProvider.applyServerNodeState(
        nodeId,
        rhythmEnabled: _roomProvider.getNode(nodeId)?.rhythmEnabled ?? true,
        timeOffset: 0,
        brightnessOffset: 0,
        state: moodActive ? RoomModeState.mood : RoomModeState.active,
        lightsOn: true,
        brightness: brightness,
        kelvin: _roomProvider.getKelvin(nodeId),
        moodEnabled: moodActive ? true : _roomProvider.isMoodEnabled(nodeId),
        moodActive: moodActive,
      );
      return true;
    }
    if (!_connection.connected) return false;
    _clearRecentDispatchFailure(nodeId);
    _connection.api
        .nodeCurveBrightness(nodeId: nodeId, brightness: brightness)
        .then((serverState) {
      if (serverState != null) {
        _onRhythmState(serverState, fromActionResponse: true);
      }
    });
    return true;
  }

  /// Set direct node brightness through the server runtime.
  ///
  /// Unlike [dispatchNodeCurveBrightness], this preserves scene-backed Mood
  /// and asks the server to update and reapply the bound scene brightness.
  bool dispatchNodeBrightness(String nodeId, int brightness) {
    if (HueServiceLocator.isDemoMode) {
      final currentState = _roomProvider.getRoomState(nodeId);
      final moodActive = currentState == RoomModeState.mood;
      DemoServerApi.instance.updateRoomLightState(
        nodeId,
        on: true,
        brightness: brightness,
        kelvin: _roomProvider.getKelvin(nodeId),
        state: moodActive ? RoomModeState.mood : RoomModeState.active,
      );
      _roomProvider.applyServerNodeState(
        nodeId,
        rhythmEnabled: _roomProvider.getNode(nodeId)?.rhythmEnabled ?? true,
        timeOffset: 0,
        brightnessOffset: 0,
        state: moodActive ? RoomModeState.mood : RoomModeState.active,
        lightsOn: true,
        brightness: brightness,
        kelvin: _roomProvider.getKelvin(nodeId),
        moodEnabled: moodActive ? true : _roomProvider.isMoodEnabled(nodeId),
        moodActive: moodActive,
      );
      return true;
    }
    if (!_connection.connected) return false;
    _clearRecentDispatchFailure(nodeId);
    _connection.api.nodeBrightness(
      nodeId: nodeId,
      brightness: brightness,
    );
    return true;
  }

  bool dispatchBrightness(String roomId, int brightness) =>
      dispatchNodeCurveBrightness(roomId, brightness);

  /// Apply curve brightness modifiers to several nodes in one batch.
  ///
  /// Returns the server's dispatch metadata and states, or null when the
  /// request cannot be sent. Demo mode preserves the single-node simulation
  /// behavior while reporting every locally-applied item as dispatched.
  Future<RhythmDispatchResult?> dispatchBatchNodeCurveBrightnessResult(
    List<({String nodeId, int brightness})> items, {
    String? correlationId,
  }) async {
    if (items.isEmpty) return null;
    if (HueServiceLocator.isDemoMode) {
      for (final item in items) {
        dispatchNodeCurveBrightness(item.nodeId, item.brightness);
      }
      return RhythmDispatchResult(
        metadata: RhythmDispatchMetadata(dispatchCount: items.length),
      );
    }
    if (!_connection.connected) return null;
    for (final item in items) {
      _clearRecentDispatchFailure(item.nodeId);
    }
    final result = await _connection.api.nodeCurveBrightnessBatchResult(
      items,
      correlationId: correlationId,
    );
    if (result.states.isEmpty && !result.metadata.hasLoadingMetadata) {
      return null;
    }
    for (final state in result.states) {
      _onRhythmState(state, fromActionResponse: true);
    }
    return result;
  }

  bool? _expectedLightsOnForState(RoomModeState? state) {
    return switch (state) {
      null => null,
      RoomModeState.hardOff => false,
      RoomModeState.active ||
      RoomModeState.mood ||
      RoomModeState.standby ||
      RoomModeState.idle ||
      RoomModeState.wake ||
      RoomModeState.warning =>
        true,
    };
  }

  /// Move a node along its active curve to a requested color temperature.
  ///
  /// This deliberately uses the curve-modifier endpoint rather than a one-shot
  /// color-temperature write.
  bool dispatchNodeCurveColorTemperature(
    String nodeId,
    int kelvin, {
    bool preserveBrightness = true,
  }) {
    if (HueServiceLocator.isDemoMode) {
      final brightness = _roomProvider.getBrightness(nodeId);
      DemoServerApi.instance.updateRoomLightState(
        nodeId,
        on: true,
        brightness: brightness,
        kelvin: kelvin,
        state: RoomModeState.active,
      );
      _roomProvider.applyServerNodeState(
        nodeId,
        rhythmEnabled: _roomProvider.getNode(nodeId)?.rhythmEnabled ?? true,
        timeOffset: _roomProvider.getNode(nodeId)?.timeOffsetMinutes ?? 0,
        brightnessOffset: _roomProvider.getNode(nodeId)?.brightnessOffset ?? 0,
        state: RoomModeState.active,
        lightsOn: true,
        brightness: brightness,
        kelvin: kelvin,
      );
      return true;
    }
    if (!_connection.connected) return false;
    _clearRecentDispatchFailure(nodeId);
    _connection.api
        .nodeCurveColorTemperature(
      nodeId: nodeId,
      kelvin: kelvin,
      preserveBrightness: preserveBrightness,
    )
        .then((serverState) {
      if (serverState != null) {
        _onRhythmState(serverState, fromActionResponse: true);
      }
    });
    return true;
  }

  /// Set a node's direct color (RGB) via the server runtime.
  ///
  /// Returns true if dispatched to server, false if not connected.
  bool dispatchNodeColor(
    String nodeId,
    int r,
    int g,
    int b, {
    String? scope,
    int? brightness,
    int? transitionMs,
  }) {
    final persistAsMood = scope == 'mood';
    final effectiveBrightness = brightness ??
        (persistAsMood ? _roomProvider.getMoodBrightness(nodeId) ?? 1 : null);
    _roomProvider.setRoomColorLocal(
      nodeId,
      r,
      g,
      b,
      rememberAsMood: persistAsMood,
    );
    if (persistAsMood) {
      _moodSceneApplyGenerations[nodeId] =
          (_moodSceneApplyGenerations[nodeId] ?? 0) + 1;
      final sceneOverrideChanged =
          !_optimisticMoodSceneIds.containsKey(nodeId) ||
              _optimisticMoodSceneIds[nodeId] != null;
      _optimisticMoodSceneIds[nodeId] = null;
      _roomProvider.setMoodEnabledLocal(nodeId, true);
      if (effectiveBrightness != null) {
        _roomProvider.setMoodBrightnessLocal(nodeId, effectiveBrightness);
      }
      if (sceneOverrideChanged) notifyListeners();
    }
    if (HueServiceLocator.isDemoMode) {
      DemoServerApi.instance.updateRoomLightState(
        nodeId,
        on: true,
        brightness: effectiveBrightness,
        kelvin: null,
        color: (r, g, b),
        state: persistAsMood ? RoomModeState.mood : null,
      );
      return true;
    }
    if (!_connection.connected) return false;
    _connection.api.nodeColor(
      nodeId: nodeId,
      r: r,
      g: g,
      b: b,
      brightness: effectiveBrightness,
      transitionMs: transitionMs,
      scope: scope,
    );
    return true;
  }

  bool dispatchRoomColor(String roomId, int r, int g, int b) =>
      dispatchNodeColor(roomId, r, g, b);

  /// Push node preferences to the server (user-state only, no topology).
  /// Returns a future completing when the server acknowledged the write, so
  /// callers that must order a follow-up request can await it.
  Future<void> pushNodePreferences(String nodeId,
      {bool? rhythmEnabled,
      bool? disabled,
      bool? standbyEnabled,
      RoomModeState? state,
      Map<String, dynamic>? profileSettings}) async {
    if (HueServiceLocator.isDemoMode) return; // optimistic UI already applied
    if (!_connection.connected || _receivingFromServer) return;
    debugPrint(
        'ServerSync: pushNodePreferences $nodeId rhythmEnabled=$rhythmEnabled disabled=$disabled standbyEnabled=$standbyEnabled state=${state?.wireValue}');
    await api.nodePreferencesSet(
      nodeId: nodeId,
      rhythmEnabled: rhythmEnabled,
      disabled: disabled,
      standbyEnabled: standbyEnabled,
      state: state,
      profileSettings: profileSettings,
    );
    _roomProvider.acknowledgeOptimisticNodeState(
      nodeId,
      state: state,
      lightsOn: _expectedLightsOnForState(state),
    );
  }

  /// Patch legacy timer-only per-profile overrides for one node.
  Future<bool> pushNodeProfileOverrides(
    String nodeId, {
    required Map<String, dynamic>? profileOverrides,
  }) async {
    if (HueServiceLocator.isDemoMode) return true;
    if (!_connection.connected || _receivingFromServer) return false;
    debugPrint('ServerSync: pushNodeProfileOverrides $nodeId');
    return api.nodeProfileOverridesSet(
      nodeId: nodeId,
      profileOverrides: profileOverrides,
    );
  }

  /// Replace one complete room/profile delta and keep unrelated profile
  /// overrides intact. Local state is optimistic and rolls back when the
  /// server rejects the request.
  Future<bool> setNodeLightProfileOverride(
    String nodeId, {
    required String profileId,
    required RhythmLightProfileNodeOverride? profileOverride,
    required String correlationId,
  }) async {
    final index = _helloNodes.indexWhere((node) => node.id == nodeId);
    if (index == -1 ||
        !lightProfileOverridesSupportedForNode(nodeId) ||
        _lightProfileOverridePending.contains(nodeId) ||
        (!HueServiceLocator.isDemoMode &&
            (!_connection.connected || _receivingFromServer))) {
      return false;
    }

    final previous = _helloNodes[index];
    final previousSettings =
        previous.profileSettings ?? const RhythmNodeProfileSettings();
    final nextOverrides = Map<String, RhythmLightProfileNodeOverride>.from(
      previousSettings.profileOverrides,
    );
    if (profileOverride == null || profileOverride.isEmpty) {
      nextOverrides.remove(profileId);
    } else {
      nextOverrides[profileId] = profileOverride;
    }
    final nextSettings = RhythmNodeProfileSettings(
      profileId: previousSettings.profileId,
      moodEnabled: previousSettings.moodEnabled,
      moodProfileId: previousSettings.moodProfileId,
      moodSceneId: previousSettings.moodSceneId,
      fadeSetting: previousSettings.fadeSetting,
      motionTimeoutSetting: previousSettings.motionTimeoutSetting,
      motionActivationEnabled: previousSettings.motionActivationEnabled,
      profileOverrides: Map.unmodifiable(nextOverrides),
      raw: previousSettings.raw,
    );
    _helloNodes[index] = _copyNodeWithProfileSettings(previous, nextSettings);
    _helloRooms = _buildRoomSummaries();
    _lightProfileOverridePending.add(nodeId);
    notifyListeners();

    if (HueServiceLocator.isDemoMode) {
      _lightProfileOverridePending.remove(nodeId);
      return true;
    }

    var accepted = false;
    try {
      accepted = await api.nodeProfileOverridesSet(
        nodeId: nodeId,
        profileOverrides: {
          profileId: profileOverride == null || profileOverride.isEmpty
              ? null
              : profileOverride.toJson(),
        },
        replace: true,
        correlationId: correlationId,
      );
    } catch (error) {
      debugPrint(
        'ServerSync: room light profile override failed node=$nodeId profile=$profileId error=$error',
      );
    } finally {
      _lightProfileOverridePending.remove(nodeId);
    }
    if (accepted) return true;

    final currentIndex = _helloNodes.indexWhere((node) => node.id == nodeId);
    if (currentIndex != -1 &&
        identical(_helloNodes[currentIndex].profileSettings, nextSettings)) {
      _helloNodes[currentIndex] = _copyNodeWithProfileSettings(
        _helloNodes[currentIndex],
        previousSettings,
      );
      _helloRooms = _buildRoomSummaries();
      notifyListeners();
    }
    return false;
  }

  /// Clear every Day/Sleep profile delta for a room while retaining unrelated
  /// room profile preferences.
  Future<bool> resetNodeLightProfileOverrides(
    String nodeId, {
    required String correlationId,
  }) async {
    final index = _helloNodes.indexWhere((node) => node.id == nodeId);
    if (index == -1 ||
        !lightProfileOverridesSupportedForNode(nodeId) ||
        _lightProfileOverridePending.contains(nodeId) ||
        (!HueServiceLocator.isDemoMode &&
            (!_connection.connected || _receivingFromServer))) {
      return false;
    }
    final previous = _helloNodes[index];
    final previousSettings =
        previous.profileSettings ?? const RhythmNodeProfileSettings();
    final nextSettings = RhythmNodeProfileSettings(
      profileId: previousSettings.profileId,
      moodEnabled: previousSettings.moodEnabled,
      moodProfileId: previousSettings.moodProfileId,
      moodSceneId: previousSettings.moodSceneId,
      fadeSetting: previousSettings.fadeSetting,
      motionTimeoutSetting: previousSettings.motionTimeoutSetting,
      motionActivationEnabled: previousSettings.motionActivationEnabled,
      raw: previousSettings.raw,
    );
    _helloNodes[index] = _copyNodeWithProfileSettings(previous, nextSettings);
    _helloRooms = _buildRoomSummaries();
    _lightProfileOverridePending.add(nodeId);
    notifyListeners();

    if (HueServiceLocator.isDemoMode) {
      _lightProfileOverridePending.remove(nodeId);
      return true;
    }

    var accepted = false;
    try {
      accepted = await api.nodeProfileOverridesSet(
        nodeId: nodeId,
        profileOverrides: null,
        replace: true,
        correlationId: correlationId,
      );
    } catch (error) {
      debugPrint(
        'ServerSync: room light profile reset failed node=$nodeId error=$error',
      );
    } finally {
      _lightProfileOverridePending.remove(nodeId);
    }
    if (accepted) return true;

    final currentIndex = _helloNodes.indexWhere((node) => node.id == nodeId);
    if (currentIndex != -1 &&
        identical(_helloNodes[currentIndex].profileSettings, nextSettings)) {
      _helloNodes[currentIndex] = _copyNodeWithProfileSettings(
        _helloNodes[currentIndex],
        previousSettings,
      );
      _helloRooms = _buildRoomSummaries();
      notifyListeners();
    }
    return false;
  }

  void pushRoomPreferences(String roomId,
      {bool? rhythmEnabled,
      bool? disabled,
      bool? standbyEnabled,
      RoomModeState? state,
      Map<String, dynamic>? profileSettings}) {
    pushNodePreferences(
      roomId,
      rhythmEnabled: rhythmEnabled,
      disabled: disabled,
      standbyEnabled: standbyEnabled,
      state: state,
      profileSettings: profileSettings,
    );
  }

  /// Push node preferences for multiple nodes in a single batch request.
  ///
  /// Avoids per-node socket overhead on a constrained Rhythm bridge.
  void pushBatchNodePreferences(List<Map<String, dynamic>> items) {
    if (!_connection.connected || _receivingFromServer || items.isEmpty) return;
    debugPrint('ServerSync: pushBatchNodePreferences (${items.length} nodes)');
    api.nodePreferencesBatchSet(items);
  }

  void pushBatchRoomPreferences(List<Map<String, dynamic>> items) {
    pushBatchNodePreferences(items);
  }

  /// Reset a single node to its current adaptive curve position.
  bool dispatchResetNode(String nodeId) {
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
      return true;
    }
    if (!_connection.connected) return false;
    _clearRecentDispatchFailure(nodeId);
    _connection.api
        .nodeAction(nodeId: nodeId, action: 'reset')
        .then((serverState) {
      if (serverState != null) {
        _onRhythmState(serverState, fromActionResponse: true);
      }
      _roomProvider.acknowledgeOptimisticNodeState(
        nodeId,
        state: RoomModeState.active,
        lightsOn: true,
      );
      _roomProvider.bumpResetGeneration();
    });
    return true;
  }

  bool dispatchResetRoom(String roomId) => dispatchResetNode(roomId);

  /// Set the active global mode on the server.
  Future<void> dispatchSetActiveMode(RhythmMode mode) async {
    if (HueServiceLocator.isDemoMode) {
      _activeMode = mode;
      await DemoServerApi.instance.setActiveMode(mode);
      notifyListeners();
      return;
    }
    if (!_connection.connected) return;
    final previous = _activeMode;
    final modeChangeGeneration = _modeChangeGeneration;
    _activeMode = mode;
    notifyListeners();
    final success = await api.modeSet(active: mode);
    if (!success &&
        _modeChangeGeneration == modeChangeGeneration &&
        _activeMode == mode) {
      _activeMode = previous;
      notifyListeners();
    }
  }

  /// Switch the selected light runtime.
  Future<bool> dispatchSetLightRuntime(
    RhythmLightRuntime runtime, {
    int transitionMs = 3000,
  }) async {
    if (!HueServiceLocator.isDemoMode && !_connection.connected) return false;

    final previousLightRuntime = _lightRuntime;
    final previousMode = _activeMode;
    final previousProfileId = _activeProfileId;
    final previousConfigs = _modeConfigs;
    final targetProfileId = _activeProfileIdForLightRuntime(runtime);

    _lightRuntime = runtime;
    _activeMode = RhythmMode.day;
    _activeProfileId = targetProfileId;
    _modeConfigs = _modeConfigsWithDayProfile(targetProfileId);
    notifyListeners();

    final response = HueServiceLocator.isDemoMode
        ? await DemoServerApi.instance.setLightRuntime(
            runtime,
            transitionMs: transitionMs,
          )
        : await api.setLightRuntime(
            runtime,
            transitionMs: transitionMs,
          );

    if (response == null) {
      if (_lightRuntime == runtime) {
        _lightRuntime = previousLightRuntime;
        _activeMode = previousMode;
        _activeProfileId = previousProfileId;
        _modeConfigs = previousConfigs;
        notifyListeners();
      }
      return false;
    }

    await _waitForLightRuntimeInitialApply(response.initialApply);

    _lightRuntime = response.runtime;
    _activeMode = RhythmMode.day;
    final activeProfileId = _activeProfileIdForLightRuntime(response.runtime);
    _activeProfileId = activeProfileId;
    _modeConfigs = _modeConfigsWithDayProfile(activeProfileId);
    notifyListeners();
    unawaited(_refreshAfterLightRuntimeSwitch());
    return true;
  }

  Future<void> _waitForLightRuntimeInitialApply(
    RhythmLightRuntimeInitialApply? initialApply,
  ) async {
    if (initialApply == null ||
        initialApply.dispatchCount <= 0 ||
        initialApply.error != null) {
      return;
    }
    final waitMs =
        (initialApply.estimatedDispatchMs + 1000).clamp(2500, 15000).toInt();
    await Future<void>.delayed(Duration(milliseconds: waitMs));
  }

  String _activeProfileIdForLightRuntime(RhythmLightRuntime _) => 'rhythm';

  RhythmLightRuntime? _authoritativeLightRuntimeFromHello(RhythmHello hello) {
    if (hello.hasLightRuntime) return hello.lightRuntime;
    if (hello.settings?.hasLightRuntime == true) {
      return hello.settings!.lightRuntime;
    }
    final modeRuntime = hello.mode == null
        ? null
        : _authoritativeLightRuntimeFromMode(hello.mode!);
    if (modeRuntime != null) return modeRuntime;
    return _lightRuntimeFromLegacyDayProfileId(
      hello.activeProfile['id'] as String?,
    );
  }

  RhythmLightRuntime? _authoritativeLightRuntimeFromMode(
    RhythmModeResource mode,
  ) {
    if (mode.hasLightRuntime) return mode.lightRuntime;

    // Older servers expressed the runtime choice through the Day profile, so
    // prefer the Day config over the currently active config.
    return _lightRuntimeFromLegacyDayProfileId(
          mode.configFor(RhythmMode.day)?.activeProfileId,
        ) ??
        _lightRuntimeFromLegacyDayProfileId(
          mode.activeConfig?.activeProfileId,
        );
  }

  RhythmLightRuntime? _lightRuntimeFromLegacyDayProfileId(String? profileId) {
    return switch (profileId) {
      'rhythm' => RhythmLightRuntime.rhythmAdaptive,
      _ => null,
    };
  }

  Future<void> _refreshAfterLightRuntimeSwitch() async {
    if (HueServiceLocator.isDemoMode) {
      await _refreshDemoState();
      return;
    }
    if (_connection.connected) {
      _beginRoomReadinessRefresh();
      await _connection.reconnect(authoritative: true);
    }
  }

  List<RhythmModeConfig> _modeConfigsWithDayProfile(String profileId) {
    var foundDay = false;
    final configs = <RhythmModeConfig>[
      for (final config in _modeConfigs)
        if (config.mode == RhythmMode.day)
          (() {
            foundDay = true;
            return config.copyWith(activeProfileId: profileId);
          })()
        else
          config,
    ];
    if (!foundDay) {
      configs.add(
        RhythmModeConfig(
          mode: RhythmMode.day,
          activeProfileId: profileId,
        ),
      );
    }
    return List<RhythmModeConfig>.unmodifiable(configs);
  }

  /// Set power-save mode on the server with an optimistic local cache update.
  Future<bool> setPowerSave(bool enabled) async {
    final previous = _powerSave;
    if (_powerSave != enabled) {
      _powerSave = enabled;
      notifyListeners();
    }

    final success = HueServiceLocator.isDemoMode
        ? await DemoServerApi.instance.settingsSet(powerSave: enabled)
        : _connection.connected
            ? await api.settingsSet(powerSave: enabled)
            : false;

    if (!success && _powerSave == enabled) {
      _powerSave = previous;
      notifyListeners();
    }
    return success;
  }

  /// Set automatic firmware update mode on the server with an optimistic
  /// local cache update.
  Future<bool> setAutoUpdate(bool enabled) async {
    final previous = _autoUpdate;
    if (_autoUpdate != enabled) {
      _autoUpdate = enabled;
      notifyListeners();
    }

    final success = HueServiceLocator.isDemoMode
        ? await DemoServerApi.instance.settingsSet(autoUpdate: enabled)
        : _connection.connected
            ? await api.settingsSet(autoUpdate: enabled)
            : false;

    if (!success && _autoUpdate == enabled) {
      _autoUpdate = previous;
      notifyListeners();
    }
    return success;
  }

  /// Set global autonomous light control on the server.
  Future<bool> setLightBreakerEnabled(bool enabled) async {
    final previous = _lightBreakerEnabled;
    if (_lightBreakerEnabled != enabled) {
      _lightBreakerEnabled = enabled;
      notifyListeners();
    }

    final success = HueServiceLocator.isDemoMode
        ? await DemoServerApi.instance.setLightBreaker(enabled)
        : _connection.connected
            ? await api.setLightBreaker(enabled)
            : false;

    if (!success && _lightBreakerEnabled == enabled) {
      _lightBreakerEnabled = previous;
      notifyListeners();
    }
    return success;
  }

  /// Replace mode transitions on the server with an optimistic local cache
  /// update. Callers should use this instead of `api.setTransitions` directly
  /// so local transition state cannot drift when a throttled refresh is skipped.
  Future<bool> dispatchSetTransitions(
    List<RhythmModeTransitionConfig> transitions,
  ) async {
    if (!HueServiceLocator.isDemoMode && !_connection.connected) return false;
    final previous = _modeTransitions;
    final next = List<RhythmModeTransitionConfig>.unmodifiable(transitions);
    _modeTransitions = next;
    notifyListeners();

    final serverApi = api;
    final success = await serverApi.setTransitions(next);
    if (!success) {
      if (identical(_modeTransitions, next)) {
        _modeTransitions = previous;
        notifyListeners();
      }
      return false;
    }

    final refreshed = await serverApi.getTransitions();
    if (refreshed.isNotEmpty || next.isEmpty) {
      _modeTransitions = List<RhythmModeTransitionConfig>.unmodifiable(
        refreshed,
      );
      notifyListeners();
    }

    return true;
  }

  /// Update a single mode transition on the server.
  Future<bool> dispatchUpdateTransition(
      RhythmModeTransitionConfig updated) async {
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
    return dispatchSetTransitions(newList);
  }

  /// Run a saved transition by ID.
  Future<bool> dispatchRunTransition(String transitionId) async {
    if (!_connection.connected) return false;
    final previous = _activeMode;
    final modeChangeGeneration = _modeChangeGeneration;
    final targetMode = _modeTransitions
        .where((transition) => transition.id == transitionId)
        .map((transition) => transition.toMode)
        .firstOrNull;
    if (targetMode != null && targetMode != _activeMode) {
      _activeMode = targetMode;
      notifyListeners();
    }
    final success = await api.triggerTransition(transitionId);
    if (!success &&
        _modeChangeGeneration == modeChangeGeneration &&
        targetMode != null &&
        _activeMode == targetMode) {
      _activeMode = previous;
      notifyListeners();
    }
    return success;
  }

  Future<bool> bindDaySleepToggleButton(
    String sourceNodeId, {
    RhythmButtonAction buttonAction = RhythmButtonAction.onPress,
  }) async {
    if (!HueServiceLocator.isDemoMode && !_connection.connected) return false;
    bool isRequestedBinding(RhythmInputBinding binding) {
      return binding.preset == RhythmInputBindingPreset.daySleepToggle &&
          binding.sourceNodeId == sourceNodeId &&
          binding.trigger.buttonAction == buttonAction &&
          binding.enabled;
    }

    final existing = [
      for (final binding in _inputBindings)
        if (binding.preset == RhythmInputBindingPreset.daySleepToggle) binding,
    ];

    final updated = await api.createDaySleepToggleInputBinding(
      sourceNodeId: sourceNodeId,
      buttonAction: buttonAction,
      enabled: true,
    );
    if (!updated.any(isRequestedBinding)) return false;

    var nextBindings = updated;
    for (final binding in existing) {
      if (binding.sourceNodeId == sourceNodeId &&
          binding.trigger.buttonAction == buttonAction) {
        continue;
      }
      if (!nextBindings.any((candidate) => candidate.id == binding.id)) {
        continue;
      }
      final afterDelete = await api.deleteInputBinding(binding.id);
      if (!afterDelete.any(isRequestedBinding)) return false;
      nextBindings = afterDelete;
    }
    _inputBindings = nextBindings;
    notifyListeners();
    return true;
  }

  Future<bool> setDaySleepToggleButtonEnabled(bool enabled) async {
    if (!HueServiceLocator.isDemoMode && !_connection.connected) return false;
    final binding = daySleepToggleInputBinding;
    if (binding == null) return true;

    final updated = await api.setInputBinding(
      binding.copyWith(enabled: enabled),
    );
    if (updated.isEmpty) return false;
    _inputBindings = updated;
    notifyListeners();
    return true;
  }

  Future<bool> unbindDaySleepToggleButton() async {
    if (!HueServiceLocator.isDemoMode && !_connection.connected) return false;
    final binding = daySleepToggleInputBinding;
    if (binding == null) return true;
    final updated = await api.deleteInputBinding(binding.id);
    _inputBindings = updated;
    notifyListeners();
    return true;
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
    _beginRoomReadinessRefresh();
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
  Future<bool> pushHubCredentials(RoomSourceDto source) async {
    if (!_connection.connected) return false;
    return _pushHubCredentialsForSource(source);
  }

  /// Tell the addon to auto-configure HA using its SUPERVISOR_TOKEN.
  /// Sends empty credentials — server fills them from its environment.
  Future<bool> configureAddonHaHub() async {
    if (!_connection.connected) return false;
    final hubConnected = await api.hubCredentials(
      hubType: 'homeassistant',
      address: '',
      credentials: {},
    );
    _lastHubReconnectTime = DateTime.now();
    _beginRoomReadinessRefresh();
    await _connection.reconnect(); // Re-fetch state with new rooms
    return hubConnected;
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
      _beginRoomReadinessRefresh();
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
      _beginRoomReadinessRefresh();
      await _connection.reconnect();
    }
    return accepted;
  }

  Future<bool> _pushHubCredentialsForSource(RoomSourceDto source) async {
    final hubType = _hubTypeForSource(source);
    if (hubType == null) return false;

    final hub = _homeProvider.getFirstHubOfType(hubType);
    if (hub == null || !hub.hasCredentials) return false;

    // HA expects {"token": "..."}, Hue expects {"username": "..."}
    final credentials = hubType == HubType.homeAssistant
        ? {'token': hub.token}
        : {'username': hub.token};

    debugPrint(
        'ServerSync: Pushing ${hub.typeName} credentials after source change');
    final hubConnected = await api.hubCredentials(
      hubType: _hubTypeWireName(hubType),
      address: '${hub.endpoint.host}:${hub.endpoint.port}',
      credentials: credentials,
    );
    _lastHubReconnectTime = DateTime.now();
    _beginRoomReadinessRefresh();
    await _connection.reconnect();
    return hubConnected;
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

  void _reconcileLocation(Map<String, dynamic> serverLocation) {
    final home = _homeProvider.currentHome;
    final loc = home?.location;

    final srvLat = _locationDouble(
      serverLocation,
      const ['latitude', 'lat'],
    );
    final srvLon = _locationDouble(
      serverLocation,
      const ['longitude', 'lon', 'lng'],
    );
    final srvTimezone = _locationString(
      serverLocation,
      const ['timezone', 'timezone_name'],
    );

    if (loc == null) {
      if (home != null && srvLat != null && srvLon != null) {
        debugPrint('ServerSync: Accepting server location into local home');
        unawaited(
          _homeProvider.updateCurrentHome(
            home.copyWith(
              location: HomeLocation(latitude: srvLat, longitude: srvLon),
              timezone:
                  _isIanaTimezone(srvTimezone) ? srvTimezone : home.timezone,
              updatedAt: DateTime.now(),
              pendingSync: home.pendingSync,
            ),
          ),
        );
      }
      return;
    }

    // Only push when the server has no location at all. Location is the
    // *house* location (fixed, tied to the server/hub) — once set, it should
    // only change via an explicit user action in location settings.
    if (srvLat == null || srvLon == null) {
      debugPrint('ServerSync: Server has no location — pushing app location');
      _pushLocationWithIanaTimezone(loc);
    }
  }

  double? _locationDouble(Map<String, dynamic> location, List<String> keys) {
    for (final key in keys) {
      final value = location[key];
      if (value is num) return value.toDouble();
      if (value is String) {
        final parsed = double.tryParse(value);
        if (parsed != null) return parsed;
      }
    }
    return null;
  }

  String? _locationString(Map<String, dynamic> location, List<String> keys) {
    for (final key in keys) {
      final value = location[key];
      if (value is String && value.isNotEmpty) return value;
    }
    return null;
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

  Future<void> _refreshTopologyNodes() async {
    await _refreshTopologyNodesWithResult();
  }

  Future<bool> _refreshTopologyNodesWithResult() async {
    if (HueServiceLocator.isDemoMode) {
      await _refreshDemoState();
      return true;
    }
    if (!_connection.connected) return false;
    final topologyNodes = await api.getTopologyNodes();
    if (topologyNodes.isEmpty && _topologyNodes.isNotEmpty) return false;
    _topologyNodes = topologyNodes;
    _helloRooms = _buildRoomSummaries();
    _roomProvider.setMotionSensorNodes(_sensorTargetNodeIds());
    if (hasListeners) notifyListeners();
    return true;
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
      await _refreshTopologyNodes();
    }
    return success;
  }

  Future<bool> setNodeControlTargets({
    required String sourceNodeId,
    required String controlKind,
    required Iterable<String> targetNodeIds,
  }) async {
    if (!HueServiceLocator.isDemoMode && !_connection.connected) return false;
    final normalizedTargetNodeIds = targetNodeIds
        .map((targetId) => targetId.trim())
        .where((targetId) => targetId.isNotEmpty)
        .toSet()
        .toList()
      ..sort();
    final success = await api.setTopologyNodeControlTargets(
      nodeId: sourceNodeId,
      controlKind: controlKind,
      targetIds: normalizedTargetNodeIds,
    );
    if (success) {
      await _refreshTopologyNodes();
    }
    return success;
  }

  Future<void> _refreshDemoState() async {
    DemoServerApi.instance.ensureSeeded();
    final snapshot = DemoServerApi.instance.snapshot();
    final roomDtos = DemoServerApi.instance.buildRoomDtos();
    final settings = await DemoServerApi.instance.getSettings();
    final lightBreaker = await DemoServerApi.instance.getLightBreaker();
    final mode = await DemoServerApi.instance.getMode();

    _firmwareVersion = DemoServerApi.firmwareVersion;
    _serverPlatformType = DemoServerApi.serverPlatformType;
    _serverPlatformContext = DemoServerApi.serverPlatformContext;
    if (settings?.hasPowerSave == true) {
      _powerSave = settings!.powerSave;
    }
    _autoUpdate = settings?.autoUpdate ?? true;
    _lightBreakerEnabled = lightBreaker?.enabled ?? true;
    _lightRuntime = mode == null
        ? _lightRuntime
        : _authoritativeLightRuntimeFromMode(mode) ?? _lightRuntime;
    _activeMode = mode?.active;
    _activeProfileId = mode?.activeConfig?.activeProfileId;
    _modeTransitions = await DemoServerApi.instance.getTransitions();
    _inputBindings = await DemoServerApi.instance.getInputBindings();
    _modeConfigs = [...?mode?.configs];
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
    _syncHelloMotionState(_helloNodes);

    if (hasListeners) {
      notifyListeners();
    }
  }

  void _syncHelloMotionState(Iterable<RhythmRoom> nodes) {
    for (final node in nodes) {
      if (node.id.isEmpty) continue;
      _applyMotionTimerState(
        nodeId: node.id,
        motionActive: node.motionActive ?? false,
        motionOwned: node.motionOwned ?? false,
        remainingSecs: node.remainingSecs,
        timeoutSecs: node.timeoutSecs ?? 0,
        warningActive: node.warningActive ?? false,
      );
    }
  }

  void _applyMotionTimerState({
    required String nodeId,
    required bool motionActive,
    required bool motionOwned,
    required int? remainingSecs,
    required int timeoutSecs,
    bool warningActive = false,
    bool cleared = false,
  }) {
    final isIdle = !motionActive && remainingSecs == null && !warningActive;
    if (cleared || isIdle) {
      _roomProvider.clearNodeMotionTimer(nodeId);
      return;
    }

    _roomProvider.updateNodeMotionTimer(
      nodeId,
      MotionTimerInfo(
        motionActive: motionActive,
        motionOwned: motionOwned,
        remainingSecs: remainingSecs,
        timeoutSecs: timeoutSecs,
        warningActive: warningActive,
        receivedAt: DateTime.now(),
      ),
    );
  }

  bool _updateHelloNodeFromRhythmState(RhythmRoomState state) {
    final index = _helloNodes.indexWhere((node) => node.id == state.nodeId);
    if (index == -1) return false;

    final previous = _helloNodes[index];
    final incomingStandbyEnabled =
        state.standbyEnabled ?? previous.standbyEnabled;
    final standbyEnabled =
        _standbyEnabledFromIncoming(state.nodeId, incomingStandbyEnabled);
    final updated = RhythmRoom(
      id: previous.id,
      name: state.name ?? previous.name,
      kind: state.kind ?? previous.kind,
      parentId: state.parentId ?? previous.parentId,
      placement: state.placement ?? previous.placement,
      groupedLightId: previous.groupedLightId,
      state: state.state,
      transitioning: state.transitioning,
      pendingDispatch: state.pendingDispatch,
      rhythmEnabled: state.rhythmEnabled,
      disabled: previous.disabled,
      timeOffset: state.timeOffset,
      brightnessOffset: state.brightnessOffset,
      hubTypes: state.hubTypes.isNotEmpty ? state.hubTypes : previous.hubTypes,
      manufacturer: state.manufacturer ?? previous.manufacturer,
      model: state.model ?? previous.model,
      lightCapabilities: state.lightCapabilities ?? previous.lightCapabilities,
      deviceIds: previous.deviceIds,
      devices: previous.devices,
      profileSettings: state.profileSettings ?? previous.profileSettings,
      moodEnabled: state.moodEnabled ?? previous.moodEnabled,
      moodActive: state.moodActive ?? previous.moodActive,
      standbyEnabled: standbyEnabled,
      standbyActive: state.standbyActive ?? previous.standbyActive,
      lightsOn: state.lightsOn ?? previous.lightsOn,
      brightness: state.brightness ?? previous.brightness,
      kelvin: state.kelvin ?? previous.kelvin,
      motionActive: state.motionActive ?? previous.motionActive,
      motionOwned: state.motionOwned ?? previous.motionOwned,
      remainingSecs: state.remainingSecs ?? previous.remainingSecs,
      timeoutSecs: state.timeoutSecs ?? previous.timeoutSecs,
      warningActive: state.warningActive ?? previous.warningActive,
    );

    if (!_helloNodeChanged(previous, updated)) {
      return false;
    }

    _helloNodes[index] = updated;
    _helloRooms = _buildRoomSummaries();
    return true;
  }

  bool _updateHelloNodeMotionState(RhythmMotionTimer event) {
    final index = _helloNodes.indexWhere((node) => node.id == event.nodeId);
    if (index == -1) return false;

    final previous = _helloNodes[index];
    final updated = RhythmRoom(
      id: previous.id,
      name: previous.name,
      kind: previous.kind,
      parentId: previous.parentId,
      placement: previous.placement,
      groupedLightId: previous.groupedLightId,
      state: previous.state,
      transitioning: previous.transitioning,
      pendingDispatch: previous.pendingDispatch,
      rhythmEnabled: previous.rhythmEnabled,
      disabled: previous.disabled,
      timeOffset: previous.timeOffset,
      brightnessOffset: previous.brightnessOffset,
      hubTypes: previous.hubTypes,
      manufacturer: previous.manufacturer,
      model: previous.model,
      lightCapabilities: previous.lightCapabilities,
      deviceIds: previous.deviceIds,
      devices: previous.devices,
      profileSettings: previous.profileSettings,
      moodEnabled: previous.moodEnabled,
      moodActive: previous.moodActive,
      standbyEnabled: previous.standbyEnabled,
      standbyActive: previous.standbyActive,
      lightsOn: previous.lightsOn,
      brightness: previous.brightness,
      kelvin: previous.kelvin,
      motionActive: event.isCleared ? false : event.motionActive,
      motionOwned: event.isCleared ? false : event.motionOwned,
      remainingSecs: event.isCleared ? null : event.remainingSecs,
      timeoutSecs: event.isCleared &&
              previous.timeoutSecs != null &&
              previous.timeoutSecs! > 0
          ? previous.timeoutSecs
          : event.timeoutSecs,
      warningActive: event.isCleared ? false : event.warningActive,
    );

    if (!_helloNodeChanged(previous, updated)) {
      return false;
    }

    _helloNodes[index] = updated;
    _helloRooms = _buildRoomSummaries();
    return true;
  }

  bool _helloNodeChanged(RhythmRoom left, RhythmRoom right) {
    return left.name != right.name ||
        left.kind != right.kind ||
        left.parentId != right.parentId ||
        left.placement != right.placement ||
        left.state != right.state ||
        left.transitioning != right.transitioning ||
        left.pendingDispatch != right.pendingDispatch ||
        left.rhythmEnabled != right.rhythmEnabled ||
        left.disabled != right.disabled ||
        left.timeOffset != right.timeOffset ||
        left.brightnessOffset != right.brightnessOffset ||
        !_stringListsEqual(left.hubTypes, right.hubTypes) ||
        left.manufacturer != right.manufacturer ||
        left.model != right.model ||
        (left.lightCapabilities == null) != (right.lightCapabilities == null) ||
        left.lightCapabilities?.colorTemperature?.minKelvin !=
            right.lightCapabilities?.colorTemperature?.minKelvin ||
        left.lightCapabilities?.colorTemperature?.maxKelvin !=
            right.lightCapabilities?.colorTemperature?.maxKelvin ||
        left.lightCapabilities?.individualProfileOverrides !=
            right.lightCapabilities?.individualProfileOverrides ||
        left.profileSettings?.toJson().toString() !=
            right.profileSettings?.toJson().toString() ||
        left.moodEnabled != right.moodEnabled ||
        left.moodActive != right.moodActive ||
        left.standbyEnabled != right.standbyEnabled ||
        left.standbyActive != right.standbyActive ||
        left.lightsOn != right.lightsOn ||
        left.brightness != right.brightness ||
        left.kelvin != right.kelvin ||
        left.motionActive != right.motionActive ||
        left.motionOwned != right.motionOwned ||
        left.remainingSecs != right.remainingSecs ||
        left.timeoutSecs != right.timeoutSecs ||
        left.warningActive != right.warningActive;
  }

  bool _stringListsEqual(List<String> left, List<String> right) {
    if (left.length != right.length) return false;
    for (var i = 0; i < left.length; i++) {
      if (left[i] != right[i]) return false;
    }
    return true;
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
            RhythmDeviceType.contact: 3,
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
        pendingDispatch: state?.pendingDispatch ?? false,
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
        lightCapabilities: state?.lightCapabilities,
        deviceIds: devices
            .where((device) => device.type == RhythmDeviceType.light)
            .map((device) => device.id)
            .toList(),
        devices: devices,
        profileSettings: state?.profileSettings,
        moodEnabled: state?.moodEnabled ?? false,
        moodActive: state?.moodActive ?? false,
        standbyEnabled: state?.standbyEnabled ?? false,
        standbyActive: state?.standbyActive ?? false,
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
      RoomSourceDto.bridge,
    ]) {
      if (sources.contains(candidate)) return candidate;
    }

    return RoomSourceDto.unknown;
  }

  RoomSourceDto? _sourceForHubType(String hubType) {
    return switch (hubType) {
      'matter' => RoomSourceDto.matter,
      'hue' || 'hue_ble' => RoomSourceDto.hue,
      'homeassistant' || 'home_assistant' => RoomSourceDto.homeAssistant,
      'bridge' => RoomSourceDto.bridge,
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
    _roomReadinessGraceTimer?.cancel();
    _helloSub?.cancel();
    _rhythmStateSub?.cancel();
    _dispatchFailureSub?.cancel();
    for (final timer in _dispatchFailureExpiryTimers.values) {
      timer.cancel();
    }
    _dispatchFailureExpiryTimers.clear();
    _clearStandbyEnabledOptimisticStates();
    _hubEventSub?.cancel();
    _sourceChangedSub?.cancel();
    _motionTimerSub?.cancel();
    _modeChangedSub?.cancel();
    _settingsChangedSub?.cancel();
    _lightBreakerChangedSub?.cancel();
    _newNodesSub?.cancel();
    _triageChangedSub?.cancel();
    _connectionStateSub?.cancel();
    _demoChangeSub?.cancel();
    _authStateSub?.cancel();
    _stopActivityCloudProvisioningTimer();
    _completeHomeEntryRefreshWaiter();
    super.dispose();
  }
}
