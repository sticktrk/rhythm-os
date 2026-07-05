/// Stateful connection manager for Rhythm server communication.
///
/// Manages polling, SSE, reconnect, and cache diffing. Exposes typed
/// streams for real-time node state updates.
///
/// Uses [StreamController]s instead of ChangeNotifier for pure Dart
/// compatibility (no Flutter dependency).
library;

import 'dart:async';
import 'dart:convert';

import 'package:dio/dio.dart';
import 'package:logging/logging.dart';

import '../api_auth.dart';
import '../api/rhythm_runtime_api.dart';
import '../api/rhythm_server_api.dart';
import '../json_parsing.dart';
import '../models/rhythm_connection_state.dart';
import '../models/rhythm_dispatch_failure.dart';
import '../models/rhythm_environment.dart';
import '../models/rhythm_firmware.dart';
import '../models/rhythm_hello.dart';
import '../models/rhythm_input_event.dart';
import '../models/rhythm_pairing.dart';
import '../models/rhythm_room.dart';
import '../models/rhythm_runtime.dart';
import '../models/rhythm_settings.dart';
import '../rhythm_log_interceptor.dart';

/// Cached node state for diff detection.
class _CachedNodeState {
  final bool rhythmEnabled;
  final double timeOffset;
  final double brightnessOffset;
  final RoomModeState state;
  final bool transitioning;
  final bool pendingDispatch;
  final RhythmMode? mode;
  final bool? powerFresh;
  final String? powerSource;
  final bool? lightsOn;
  final int? brightness;
  final int? kelvin;
  final bool? motionActive;
  final bool? motionOwned;
  final int? motionRemaining;
  final int? motionTimeout;
  final bool? warningActive;
  final bool hasMotionSensor;

  const _CachedNodeState({
    required this.rhythmEnabled,
    required this.timeOffset,
    required this.brightnessOffset,
    required this.state,
    this.transitioning = false,
    this.pendingDispatch = false,
    this.mode,
    this.powerFresh,
    this.powerSource,
    this.lightsOn,
    this.brightness,
    this.kelvin,
    this.motionActive,
    this.motionOwned,
    this.motionRemaining,
    this.motionTimeout,
    this.warningActive,
    this.hasMotionSensor = false,
  });
}

/// Stateful connection manager for a Rhythm server.
///
/// Call [connect] to establish a connection. The manager automatically
/// polls for state changes and attempts SSE for real-time updates.
class RhythmConnection {
  static final _log = Logger('rhythm_sdk.connection');

  // Stream controllers (broadcast so multiple listeners work).
  final _helloController = StreamController<RhythmHello>.broadcast();
  final _rhythmStateController = StreamController<RhythmRoomState>.broadcast();
  final _hubEventController = StreamController<
      ({String event, String? hubType, String? address})>.broadcast();
  final _motionTimerController =
      StreamController<RhythmMotionTimer>.broadcast();
  final _inputEventController = StreamController<RhythmInputEvent>.broadcast();
  final _modeChangedController =
      StreamController<RhythmModeResource>.broadcast();
  final _settingsChangedController =
      StreamController<RhythmSettings>.broadcast();
  final _lightBreakerChangedController =
      StreamController<RhythmLightBreaker>.broadcast();
  final _outdoorChangedController =
      StreamController<RhythmEnvironmentSnapshot>.broadcast();
  final _scopeNodeChangedController = StreamController<String>.broadcast();
  final _pipelineTraceAvailableController =
      StreamController<RhythmPipelineTraceEvent>.broadcast();
  final _powerSchedulesChangedController =
      StreamController<RhythmPowerSchedules>.broadcast();
  final _syncRequiredController =
      StreamController<RhythmSyncRequired>.broadcast();
  final _newNodesController = StreamController<void>.broadcast();
  final _triageChangedController =
      StreamController<Map<String, dynamic>>.broadcast();
  final _connectionStateController =
      StreamController<RhythmConnectionState>.broadcast();
  final _pairingProgressController =
      StreamController<RhythmPairingProgress>.broadcast();
  final _otaUpdateProgressController =
      StreamController<RhythmOtaUpdateProgress>.broadcast();
  final _dispatchFailureController =
      StreamController<RhythmDispatchFailure>.broadcast();

  // Connection state.
  RhythmConnectionState _connectionState = RhythmConnectionState.disconnected;
  String? _host;
  int _port = 80;
  Dio? _dio;

  // Polling.
  Timer? _pollTimer;
  int _consecutivePollFailures = 0;
  static const int _maxPollFailures = 3;
  static const Duration _pollInterval = Duration(seconds: 15);

  // Reconnect.
  Timer? _reconnectTimer;
  int _reconnectAttempts = 0;

  // SSE state.
  StreamSubscription? _sseSubscription;
  CancelToken? _sseCancelToken;
  bool _sseConnected = false;
  bool _sseSupported = true;
  Timer? _sseReconnectTimer;
  int _sseReconnectAttempts = 0;
  bool _sseConnecting = false;
  bool _suppressNextSettingsChangedAfterMode = false;
  Timer? _settingsChangedSuppressTimer;

  // Direct SSE state (for HA addon web).
  String? _serverPlatformContext;
  int? _listenPort;
  bool _sseDirectAttempted = false;

  // SSE keepalive watchdog.
  DateTime _lastSseActivity = DateTime.fromMillisecondsSinceEpoch(0);
  Timer? _sseWatchdogTimer;
  static const Duration _sseWatchdogInterval = Duration(seconds: 15);
  static const Duration _sseStaleThreshold = Duration(seconds: 45);
  static const Duration _fastPollDebounce = Duration(seconds: 5);

  // SSE event history for debug display.
  final Map<String, ({String type, String summary, DateTime time})>
      _lastSseEvents = {};

  // Cached node states for diff detection.
  final Map<String, _CachedNodeState> _cachedNodeStates = {};
  final Map<String, bool> _cachedHubConnected = {};
  final Set<String> _reHelloSuppressedNodeIds = {};
  DateTime _lastFreshStateAt = DateTime.fromMillisecondsSinceEpoch(0);

  // Ordering guards. State arrives from three unordered sources (SSE, polls,
  // action responses) with no server-side sequence numbers, so an older
  // response applied after a newer one silently reverts state.
  /// Bumped whenever SSE applies node state; an in-flight poll captured
  /// before the bump must discard its response.
  int _sseApplyGeneration = 0;

  /// Bumped by [_stopPolling]; an in-flight poll from a cancelled cycle must
  /// discard its response.
  int _pollEpoch = 0;

  // Optional web base URL (consumer passes Uri.base.toString() on web).
  String? _webBaseUrl;
  String? _authToken;
  bool _useSsl = false;

  // The server API (uses the shared Dio instance).
  RhythmServerApi? _api;
  RhythmRuntimeApi? _runtimeApi;

  // --------------------------------------------------------------------------
  // Public getters
  // --------------------------------------------------------------------------

  Stream<RhythmHello> get helloEvents => _helloController.stream;
  Stream<RhythmRoomState> get rhythmStateEvents =>
      _rhythmStateController.stream;
  Stream<({String event, String? hubType, String? address})> get hubEvents =>
      _hubEventController.stream;
  Stream<RhythmMotionTimer> get motionTimerEvents =>
      _motionTimerController.stream;
  Stream<RhythmInputEvent> get inputEvents => _inputEventController.stream;
  Stream<RhythmModeResource> get modeChangedEvents =>
      _modeChangedController.stream;
  Stream<RhythmSettings> get settingsChangedEvents =>
      _settingsChangedController.stream;
  Stream<RhythmLightBreaker> get lightBreakerChangedEvents =>
      _lightBreakerChangedController.stream;
  Stream<RhythmEnvironmentSnapshot> get outdoorChangedEvents =>
      _outdoorChangedController.stream;
  Stream<String> get scopeNodeChangedEvents =>
      _scopeNodeChangedController.stream;
  Stream<RhythmPipelineTraceEvent> get pipelineTraceAvailableEvents =>
      _pipelineTraceAvailableController.stream;
  Stream<RhythmPowerSchedules> get powerSchedulesChangedEvents =>
      _powerSchedulesChangedController.stream;
  Stream<RhythmSyncRequired> get syncRequiredEvents =>
      _syncRequiredController.stream;
  Stream<void> get newNodesDetected => _newNodesController.stream;
  Stream<void> get newRoomsDetected => newNodesDetected;
  Stream<Map<String, dynamic>> get triageChangedEvents =>
      _triageChangedController.stream;
  Stream<RhythmConnectionState> get connectionStateStream =>
      _connectionStateController.stream;

  /// Real-time pairing progress events from the server (matter, zigbee, etc.).
  Stream<RhythmPairingProgress> get pairingProgressEvents =>
      _pairingProgressController.stream;

  /// Real-time OTA update progress events for the server's own self-update
  /// flow (`/api/ota/check` and `/api/ota/update`).
  Stream<RhythmOtaUpdateProgress> get otaUpdateProgressEvents =>
      _otaUpdateProgressController.stream;

  /// Hub light commands that failed, timed out, or were dropped.
  ///
  /// Dispatch is fire-and-forget server-side, so this stream is the only
  /// signal that a light command did not physically reach its target.
  Stream<RhythmDispatchFailure> get dispatchFailureEvents =>
      _dispatchFailureController.stream;

  RhythmConnectionState get connectionState => _connectionState;
  bool get connected => _connectionState == RhythmConnectionState.connected;
  String? get host => _host;
  bool get sseConnected => _sseConnected;
  bool get sseSupported => _sseSupported;
  DateTime get lastSseActivity => _lastSseActivity;
  int get sseReconnectAttempts => _sseReconnectAttempts;
  Map<String, ({String type, String summary, DateTime time})>
      get lastSseEvents => Map.unmodifiable(_lastSseEvents);

  /// The server API client. Available after [connect].
  RhythmServerApi get api {
    if (_api == null) {
      throw StateError('Not connected. Call connect() first.');
    }
    return _api!;
  }

  /// Rust-first runtime API client. Available after [connect].
  RhythmRuntimeApi get runtimeApi {
    if (_runtimeApi == null) {
      throw StateError('Not connected. Call connect() first.');
    }
    return _runtimeApi!;
  }

  // --------------------------------------------------------------------------
  // Connect / Disconnect
  // --------------------------------------------------------------------------

  /// Connect to a server device.
  ///
  /// [webBaseUrl] is required on web platforms (pass `Uri.base.toString()`).
  Future<void> connect(
    String host, {
    int port = 80,
    bool useSsl = false,
    String? webBaseUrl,
    String? authToken,
  }) async {
    if (_connectionState == RhythmConnectionState.connected &&
        _host == host &&
        _port == port &&
        _useSsl == useSsl &&
        _authToken == authToken) {
      return;
    }

    // Tear down transport state for any prior connection attempt — including
    // same-params re-entry while reconnecting, which would otherwise leave a
    // stale reconnect timer racing this call with a duplicate hello and leak
    // the old Dio client.
    _stopPolling();
    _disconnectSse();
    _sseReconnectTimer?.cancel();
    _sseReconnectTimer = null;
    _sseReconnectAttempts = 0;
    _reconnectTimer?.cancel();
    _reconnectTimer = null;
    _reconnectAttempts = 0;

    if (_host != null &&
        (_host != host ||
            _port != port ||
            _useSsl != useSsl ||
            _authToken != authToken)) {
      _cachedNodeStates.clear();
      _cachedHubConnected.clear();
      _reHelloSuppressedNodeIds.clear();
      _sseSupported = true;
      _sseDirectAttempted = false;
    }

    _host = host;
    _port = port;
    _useSsl = useSsl;
    _webBaseUrl = webBaseUrl;
    _authToken = authToken;
    final baseUrl = _buildBaseUrl(host, port, useSsl, webBaseUrl);
    _dio?.close();
    _dio = Dio(BaseOptions(
      baseUrl: baseUrl,
      connectTimeout: const Duration(seconds: 5),
      receiveTimeout: const Duration(seconds: 10),
      headers: bearerAuthHeaders(authToken),
    ));
    _dio!.interceptors.add(RhythmLogInterceptor(_log));
    _api = RhythmServerApi(_dio!, onStatesReceived: _updateCacheFromStates);
    _runtimeApi =
        RhythmRuntimeApi(_dio!, onStatesReceived: _updateCacheFromStates);

    _log.config('Connecting to $host:$port');
    await _connectInternal();
  }

  /// Force a full reconnect.
  ///
  /// When [authoritative] is true, the initial hello fetch uses
  /// `GET /api/state?authoritative=true`.
  Future<void> reconnect({bool authoritative = false}) async {
    if (_host == null) return;
    _stopPolling();
    _disconnectSse();
    _sseReconnectTimer?.cancel();
    _sseReconnectTimer = null;
    _sseReconnectAttempts = 0;
    _reconnectAttempts = 0;
    _consecutivePollFailures = 0;
    _cachedNodeStates.clear();
    // Keep _cachedHubConnected across reconnects — the hello will update it
    // authoritatively, but preserving it prevents the re-established SSE
    // stream's initial hub_status event from looking like a new change
    // (which would trigger another reconnect → infinite loop).
    _sseDirectAttempted = false;
    await _connectInternal(authoritative: authoritative);
  }

  /// Disconnect from the server.
  void disconnect() {
    _stopPolling();
    _disconnectSse();
    _sseReconnectTimer?.cancel();
    _sseReconnectTimer = null;
    _sseReconnectAttempts = 0;
    _reconnectTimer?.cancel();
    _reconnectTimer = null;
    _reconnectAttempts = 0;

    _host = null;
    _authToken = null;
    _dio?.close();
    _dio = null;
    _api = null;
    _runtimeApi = null;

    _cachedNodeStates.clear();
    _cachedHubConnected.clear();
    _reHelloSuppressedNodeIds.clear();
    _sseSupported = true;
    _sseDirectAttempted = false;

    _setConnectionState(RhythmConnectionState.disconnected);
  }

  /// Trigger an immediate poll cycle.
  Future<void> pollNow() async {
    if (connected) {
      _cachedNodeStates.clear();
      await _poll();
    }
  }

  /// Ping if connected, or attempt immediate reconnect if not.
  Future<void> pingOrReconnect() async {
    if (_host == null || _dio == null) return;

    if (connected) {
      final ok = await api.ping();
      if (!ok) {
        _stopPolling();
        _disconnectSse();
        _sseReconnectTimer?.cancel();
        _sseReconnectTimer = null;
        _sseReconnectAttempts = 0;
        _setConnectionState(RhythmConnectionState.reconnecting);
        _reconnectTimer?.cancel();
        _reconnectAttempts = 0;
        _connectInternal();
      } else if (_sseConnected) {
        final elapsed = DateTime.now().difference(_lastSseActivity);
        if (elapsed > _sseStaleThreshold) {
          _sseReconnectAttempts = 0;
          _handleSseDisconnect();
        }
      } else if (!_sseConnected && _sseSupported && !_sseConnecting) {
        _sseReconnectAttempts = 0;
        _connectSse();
      }
    } else {
      _reconnectTimer?.cancel();
      _reconnectAttempts = 0;
      _connectInternal();
    }
  }

  /// Release all resources.
  void dispose() {
    disconnect();
    _helloController.close();
    _rhythmStateController.close();
    _hubEventController.close();
    _motionTimerController.close();
    _inputEventController.close();
    _modeChangedController.close();
    _settingsChangedController.close();
    _lightBreakerChangedController.close();
    _outdoorChangedController.close();
    _scopeNodeChangedController.close();
    _pipelineTraceAvailableController.close();
    _powerSchedulesChangedController.close();
    _syncRequiredController.close();
    _newNodesController.close();
    _triageChangedController.close();
    _connectionStateController.close();
    _pairingProgressController.close();
    _otaUpdateProgressController.close();
    _dispatchFailureController.close();
    _settingsChangedSuppressTimer?.cancel();
  }

  // --------------------------------------------------------------------------
  // Internal: connect
  // --------------------------------------------------------------------------

  Future<void> _connectInternal({bool authoritative = false}) async {
    if (_dio == null || _host == null) return;

    _sseReconnectTimer?.cancel();
    _sseReconnectTimer = null;
    _sseReconnectAttempts = 0;

    _setConnectionState(_reconnectAttempts > 0
        ? RhythmConnectionState.reconnecting
        : RhythmConnectionState.connecting);

    try {
      final data = await _getHelloPayload(authoritative: authoritative);
      final hello = RhythmHello.fromJson(data);
      _log.fine('hello last_tick_epoch_ms=${hello.lastTickEpochMs}');
      _serverPlatformContext = hello.platformContext;
      _listenPort = hello.listenPort;
      _sseDirectAttempted = false;

      _cachedNodeStates.clear();
      for (final node in hello.nodes) {
        _cachedNodeStates[node.id] = _CachedNodeState(
          rhythmEnabled: node.rhythmEnabled,
          timeOffset: node.timeOffset,
          brightnessOffset: node.brightnessOffset,
          state: node.state,
          transitioning: node.transitioning,
          pendingDispatch: node.pendingDispatch,
          powerFresh: node.powerFresh,
          powerSource: node.powerSource,
          lightsOn: node.lightsOn,
          brightness: node.brightness,
          kelvin: node.kelvin,
          motionActive: node.motionActive,
          motionOwned: node.motionOwned,
          motionRemaining: node.remainingSecs,
          motionTimeout: node.timeoutSecs,
          warningActive: node.warningActive,
          hasMotionSensor: node.hasMotionSensor,
        );
      }

      _reHelloSuppressedNodeIds.removeAll(_cachedNodeStates.keys);
      // Cache per-hub connected state from hello for diff detection.
      _cachedHubConnected.clear();
      for (final hub in hello.hubInfos) {
        if (hub.configured) {
          _cachedHubConnected[_hubCacheKey(hub.type, hub.address)] =
              hub.connected;
        }
      }

      _setConnectionState(RhythmConnectionState.connected);
      _reconnectAttempts = 0;
      _consecutivePollFailures = 0;
      _lastFreshStateAt = DateTime.now();

      _log.config('Connected, platform=${hello.platformType}, '
          '${hello.nodes.length} nodes');

      _helloController.add(hello);
      _startPolling();

      if (hello.platformType == 'embedded') {
        _sseSupported = false;
      } else {
        _connectSse();
      }
    } catch (e) {
      _log.severe('Connection failed', e);
      _scheduleReconnect();
    }
  }

  Future<Map<String, dynamic>> _getHelloPayload({
    bool authoritative = false,
  }) async {
    final response = await _dio!.get(
      'api/state',
      queryParameters: authoritative ? const {'authoritative': 'true'} : null,
    );
    return Map<String, dynamic>.from(response.data as Map<String, dynamic>);
  }

  // --------------------------------------------------------------------------
  // Internal: polling
  // --------------------------------------------------------------------------

  void _startPolling() {
    if (_pollTimer != null) return;
    if (DateTime.now().difference(_lastFreshStateAt) >= _fastPollDebounce) {
      _poll();
    }
    _pollTimer = Timer.periodic(_pollInterval, (_) => _poll());
  }

  void _stopPolling() {
    _pollTimer?.cancel();
    _pollTimer = null;
    _consecutivePollFailures = 0;
    // Invalidate any in-flight poll: its response must not apply after the
    // poller was stopped (e.g. SSE just became authoritative).
    _pollEpoch++;
  }

  Future<void> _poll() async {
    if (_dio == null || !connected) return;

    final pollEpoch = _pollEpoch;
    final sseGeneration = _sseApplyGeneration;
    try {
      final response = await _dio!.get('api/nodes/state');
      // Discard stale responses: if SSE applied fresher state or the poller
      // was stopped while this request was in flight, applying the response
      // would revert newer state (the poll diffs against the *current* cache,
      // so an older snapshot registers as a "change").
      if (pollEpoch != _pollEpoch || sseGeneration != _sseApplyGeneration) {
        return;
      }
      final data = response.data as Map<String, dynamic>;
      _consecutivePollFailures = 0;
      _lastFreshStateAt = DateTime.now();

      // Poll doesn't carry per-hub status — skip hub event emission.
      // The hello (which runs on reconnect) provides authoritative per-hub state.

      final previousNodeIds = _cachedNodeStates.keys.toSet();
      final nodes = data['nodes'] as List<dynamic>? ??
          data['rooms'] as List<dynamic>? ??
          [];
      final seenNodeIds = <String>{};

      for (final raw in nodes) {
        final nodeJson = raw as Map<String, dynamic>;
        final nodeState = RhythmRoomState.fromJson(nodeJson);
        final nodeId = nodeState.nodeId;
        if (nodeId.isEmpty) continue;
        seenNodeIds.add(nodeId);

        final motionActive = nodeJson['motion_active'] as bool?;
        final motionOwned = nodeJson['motion_owned'] as bool?;
        final motionRemaining = jsonInt(nodeJson['remaining_secs'],
                preferredKeys: const ['remaining_secs']) ??
            jsonInt(
              nodeJson['motion_remaining'],
              preferredKeys: const ['motion_remaining', 'remaining_secs'],
            );
        final motionTimeout = jsonInt(nodeJson['timeout_secs'],
                preferredKeys: const ['timeout_secs']) ??
            jsonInt(
              nodeJson['motion_timeout'],
              preferredKeys: const ['motion_timeout', 'timeout_secs'],
            );
        final warningActive = nodeJson['warning_active'] as bool?;

        final cached = _cachedNodeStates[nodeId];
        final nextPowerFresh = nodeState.powerFresh ?? cached?.powerFresh;
        final nextPowerSource = nodeState.powerSource ?? cached?.powerSource;
        final hasMotionSensor = motionActive != null ||
            motionOwned != null ||
            motionRemaining != null ||
            motionTimeout != null ||
            warningActive != null;

        final rhythmChanged = cached == null ||
            cached.rhythmEnabled != nodeState.rhythmEnabled ||
            cached.timeOffset != nodeState.timeOffset ||
            cached.brightnessOffset != nodeState.brightnessOffset ||
            cached.state != nodeState.state ||
            cached.transitioning != nodeState.transitioning ||
            cached.pendingDispatch != nodeState.pendingDispatch ||
            cached.lightsOn != nodeState.lightsOn ||
            cached.powerFresh != nextPowerFresh ||
            cached.powerSource != nextPowerSource ||
            cached.brightness != nodeState.brightness ||
            cached.kelvin != nodeState.kelvin;

        final motionChanged = cached == null ||
            cached.motionActive != motionActive ||
            cached.motionOwned != motionOwned ||
            cached.motionRemaining != motionRemaining ||
            cached.motionTimeout != motionTimeout ||
            cached.warningActive != warningActive;

        if (rhythmChanged || motionChanged) {
          _cachedNodeStates[nodeId] = _CachedNodeState(
            rhythmEnabled: nodeState.rhythmEnabled,
            timeOffset: nodeState.timeOffset,
            brightnessOffset: nodeState.brightnessOffset,
            state: nodeState.state,
            transitioning: nodeState.transitioning,
            pendingDispatch: nodeState.pendingDispatch,
            mode: nodeState.mode,
            powerFresh: nextPowerFresh,
            powerSource: nextPowerSource,
            lightsOn: nodeState.lightsOn,
            brightness: nodeState.brightness,
            kelvin: nodeState.kelvin,
            motionActive: motionActive,
            motionOwned: motionOwned,
            motionRemaining: motionRemaining,
            motionTimeout: motionTimeout,
            warningActive: warningActive,
            hasMotionSensor: hasMotionSensor,
          );

          if (rhythmChanged) {
            _rhythmStateController.add(nodeState);
          }

          if (motionChanged) {
            if (motionActive != null && motionTimeout != null) {
              _motionTimerController.add(RhythmMotionTimer.node(
                nodeId: nodeId,
                motionActive: motionActive,
                motionOwned: motionOwned ?? false,
                remainingSecs: motionRemaining,
                timeoutSecs: motionTimeout,
                warningActive: warningActive ?? false,
              ));
            } else if (cached?.motionActive != null) {
              _motionTimerController.add(RhythmMotionTimer.cleared(nodeId));
            }
          }
        }
      }

      // Detect new nodes.
      final newNodeIds = seenNodeIds.difference(previousNodeIds);
      final unsuppressedIds = newNodeIds.difference(_reHelloSuppressedNodeIds);
      if (unsuppressedIds.isNotEmpty && previousNodeIds.isNotEmpty) {
        _reHelloSuppressedNodeIds.addAll(unsuppressedIds);
        _newNodesController.add(null);
      }

      // Clear motion for removed nodes.
      for (final entry in _cachedNodeStates.entries.toList()) {
        if (!seenNodeIds.contains(entry.key) &&
            entry.value.motionActive != null) {
          _motionTimerController.add(RhythmMotionTimer.cleared(entry.key));
          _cachedNodeStates[entry.key] = _CachedNodeState(
            rhythmEnabled: entry.value.rhythmEnabled,
            timeOffset: entry.value.timeOffset,
            brightnessOffset: entry.value.brightnessOffset,
            state: entry.value.state,
            transitioning: entry.value.transitioning,
            pendingDispatch: entry.value.pendingDispatch,
            mode: entry.value.mode,
            powerFresh: entry.value.powerFresh,
            powerSource: entry.value.powerSource,
            lightsOn: entry.value.lightsOn,
            brightness: entry.value.brightness,
            kelvin: entry.value.kelvin,
            warningActive: false,
            hasMotionSensor: entry.value.hasMotionSensor,
          );
        }
      }
    } catch (e) {
      _consecutivePollFailures++;
      _log.warning(
          'Poll failed ($_consecutivePollFailures/$_maxPollFailures)', e);
      if (_consecutivePollFailures >= _maxPollFailures) {
        _stopPolling();
        _setConnectionState(RhythmConnectionState.reconnecting);
        _scheduleReconnect();
      }
    }
  }

  // --------------------------------------------------------------------------
  // Internal: SSE
  // --------------------------------------------------------------------------

  void _connectSse() async {
    if (_dio == null || !connected || !_sseSupported || _sseConnecting) return;

    // Order matters: _disconnectSse resets _sseConnecting, so the guard must
    // be raised after it or concurrent callers (reconnect timer, watchdog,
    // pingOrReconnect) race each other mid-handshake.
    _disconnectSse();
    _sseConnecting = true;

    final cancelToken = CancelToken();
    _sseCancelToken = cancelToken;

    String sseBaseUrl = _dio!.options.baseUrl;
    _log.config('SSE connecting to $sseBaseUrl');
    final useDirectSse = _webBaseUrl != null &&
        _serverPlatformContext == 'ha_addon' &&
        _listenPort != null &&
        !_sseDirectAttempted;
    if (useDirectSse) {
      final host = Uri.parse(_webBaseUrl!).host;
      sseBaseUrl = 'http://$host:$_listenPort/';
      _sseDirectAttempted = true;
    }

    try {
      final sseDio = Dio(BaseOptions(
        baseUrl: sseBaseUrl,
        connectTimeout: const Duration(seconds: 30),
        receiveTimeout: Duration.zero,
      ));
      final response = await sseDio.get(
        'api/events',
        options: Options(
          responseType: ResponseType.stream,
          headers: bearerAuthHeaders(
            _authToken,
            extra: {'Accept': 'text/event-stream'},
          ),
        ),
        cancelToken: cancelToken,
      );

      // A newer connect/disconnect superseded this attempt during the
      // handshake — its state must not be touched by this stale one.
      if (!identical(cancelToken, _sseCancelToken) || cancelToken.isCancelled) {
        return;
      }

      final stream = (response.data as ResponseBody?)?.stream;
      if (stream == null) {
        _sseConnecting = false;
        // Treat like any other transport failure so SSE retries instead of
        // staying dead until the next app-resume ping.
        _handleSseDisconnect();
        return;
      }

      _sseConnected = true;
      _sseConnecting = false;
      _sseReconnectAttempts = 0;
      _log.config('SSE connected');
      _lastSseActivity = DateTime.now();
      // SSE is now the authoritative realtime transport. Stop the HTTP poller
      // so we don't keep hitting /api/nodes/state while the stream is healthy.
      _stopPolling();
      _startSseWatchdog();
      // Resync once if state is stale: anything broadcast while SSE was down
      // was silently lost — there is no Last-Event-ID resume, and the
      // periodic poller is now stopped. The freshness debounce keeps rapid
      // stream flaps from hammering the server with a poll per reconnect.
      if (DateTime.now().difference(_lastFreshStateAt) >= _fastPollDebounce) {
        unawaited(_poll());
      }

      String? eventType;
      final dataBuffer = StringBuffer();

      _sseSubscription = stream
          .cast<List<int>>()
          .transform(utf8.decoder)
          .transform(const LineSplitter())
          .listen(
        (line) {
          _lastSseActivity = DateTime.now();

          if (line.startsWith('event:')) {
            eventType = line.substring(6).trim();
          } else if (line.startsWith('data:')) {
            if (dataBuffer.isNotEmpty) dataBuffer.write('\n');
            // Per the SSE spec, strip at most one leading space — trimming
            // corrupts whitespace-significant payloads.
            final value = line.substring(5);
            dataBuffer.write(value.startsWith(' ') ? value.substring(1) : value);
          } else if (line.isEmpty && eventType != null) {
            _handleSseEvent(eventType!, dataBuffer.toString());
            eventType = null;
            dataBuffer.clear();
          } else if (line.isEmpty) {
            dataBuffer.clear();
          }
        },
        onError: (error) {
          if (!cancelToken.isCancelled) {
            _handleSseDisconnect();
          }
        },
        onDone: () {
          if (!cancelToken.isCancelled) {
            _handleSseDisconnect();
          }
        },
      );
    } on DioException catch (e) {
      // A superseding attempt owns the state now — don't reset its guard or
      // schedule retries on its behalf.
      if (!identical(cancelToken, _sseCancelToken)) return;
      _sseConnecting = false;
      if (e.type == DioExceptionType.cancel) return;
      if (e.response?.statusCode == 404) {
        _log.config('SSE not supported (404)');
        _sseSupported = false;
      } else if (useDirectSse) {
        _log.warning('SSE direct connect failed, retrying via proxy', e);
        _connectSse();
      } else {
        _log.warning('SSE connection failed', e);
        _handleSseDisconnect();
      }
    } catch (e) {
      if (!identical(cancelToken, _sseCancelToken)) return;
      _sseConnecting = false;
      if (useDirectSse) {
        _log.warning('SSE direct connect error, retrying via proxy', e);
        _connectSse();
      } else {
        _log.warning('SSE connection error', e);
        _handleSseDisconnect();
      }
    }
  }

  void _handleSseEvent(String eventType, String data) {
    _log.fine('SSE event: $eventType');
    _lastFreshStateAt = DateTime.now();
    _lastSseEvents[eventType] = (
      type: eventType,
      summary: data.length > 100 ? '${data.substring(0, 100)}...' : data,
      time: DateTime.now(),
    );
    try {
      switch (eventType) {
        case 'node_state':
        case 'room_state':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final payload = json['data'] as Map<String, dynamic>? ?? json;
          final nodes = payload['nodes'] as List<dynamic>? ??
              payload['rooms'] as List<dynamic>? ??
              [];
          _sseApplyGeneration++;
          for (final raw in nodes) {
            // One malformed node must not drop the rest of the event.
            final RhythmRoomState nodeState;
            try {
              nodeState = RhythmRoomState.fromJson(raw as Map<String, dynamic>);
            } catch (e) {
              _log.warning('Skipping malformed node in $eventType event', e);
              continue;
            }
            final nodeId = nodeState.nodeId;
            if (nodeId.isEmpty) continue;

            final existing = _cachedNodeStates[nodeId];
            final hasMotionSensor = existing?.hasMotionSensor == true ||
                nodeState.motionActive != null ||
                nodeState.motionOwned != null ||
                nodeState.remainingSecs != null ||
                nodeState.timeoutSecs != null ||
                nodeState.warningActive != null;
            _cachedNodeStates[nodeId] = _CachedNodeState(
              rhythmEnabled: nodeState.rhythmEnabled,
              timeOffset: nodeState.timeOffset,
              brightnessOffset: nodeState.brightnessOffset,
              state: nodeState.state,
              transitioning: nodeState.transitioning,
              pendingDispatch: nodeState.pendingDispatch,
              mode: nodeState.mode,
              powerFresh: nodeState.powerFresh ?? existing?.powerFresh,
              powerSource: nodeState.powerSource ?? existing?.powerSource,
              lightsOn: nodeState.lightsOn ?? existing?.lightsOn,
              brightness: nodeState.brightness ?? existing?.brightness,
              kelvin: nodeState.kelvin ?? existing?.kelvin,
              motionActive: nodeState.motionActive ?? existing?.motionActive,
              motionOwned: nodeState.motionOwned ?? existing?.motionOwned,
              motionRemaining:
                  nodeState.remainingSecs ?? existing?.motionRemaining,
              motionTimeout: nodeState.timeoutSecs ?? existing?.motionTimeout,
              warningActive: nodeState.warningActive ?? existing?.warningActive,
              hasMotionSensor: hasMotionSensor,
            );

            _rhythmStateController.add(nodeState);
          }
          break;

        case 'motion_timer':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final payload = json['data'] as Map<String, dynamic>? ?? json;
          final timers = payload['timers'] as List<dynamic>? ?? [];

          final seenNodeIds = <String>{};
          for (final raw in timers) {
            final timer = raw as Map<String, dynamic>;
            final nodeId = timer['node_id'] as String? ??
                timer['room_id'] as String? ??
                '';
            if (nodeId.isEmpty) continue;
            seenNodeIds.add(nodeId);

            final motionActive = timer['motion_active'] as bool? ?? false;
            final motionOwned = timer['motion_owned'] as bool? ?? false;
            final remainingSecs = jsonInt(
              timer['remaining_secs'],
              preferredKeys: const ['remaining_secs'],
            );
            final timeoutSecs = jsonInt(timer['timeout_secs'],
                    preferredKeys: const ['timeout_secs']) ??
                0;
            final warningActive = timer['warning_active'] as bool? ?? false;

            final cached = _cachedNodeStates[nodeId];
            if (cached != null) {
              _cachedNodeStates[nodeId] = _CachedNodeState(
                rhythmEnabled: cached.rhythmEnabled,
                timeOffset: cached.timeOffset,
                brightnessOffset: cached.brightnessOffset,
                state: cached.state,
                transitioning: cached.transitioning,
                pendingDispatch: cached.pendingDispatch,
                mode: cached.mode,
                powerFresh: cached.powerFresh,
                powerSource: cached.powerSource,
                lightsOn: cached.lightsOn,
                brightness: cached.brightness,
                kelvin: cached.kelvin,
                motionActive: motionActive,
                motionOwned: motionOwned,
                motionRemaining: remainingSecs,
                motionTimeout: timeoutSecs,
                warningActive: warningActive,
                hasMotionSensor: true,
              );
            }

            _motionTimerController.add(RhythmMotionTimer.node(
              nodeId: nodeId,
              motionActive: motionActive,
              motionOwned: motionOwned,
              remainingSecs: remainingSecs,
              timeoutSecs: timeoutSecs,
              warningActive: warningActive,
            ));
          }

          for (final entry in _cachedNodeStates.entries.toList()) {
            if (entry.value.motionActive != null &&
                !seenNodeIds.contains(entry.key)) {
              _motionTimerController.add(RhythmMotionTimer.cleared(entry.key));
              _cachedNodeStates[entry.key] = _CachedNodeState(
                rhythmEnabled: entry.value.rhythmEnabled,
                timeOffset: entry.value.timeOffset,
                brightnessOffset: entry.value.brightnessOffset,
                state: entry.value.state,
                transitioning: entry.value.transitioning,
                pendingDispatch: entry.value.pendingDispatch,
                mode: entry.value.mode,
                lightsOn: entry.value.lightsOn,
                brightness: entry.value.brightness,
                kelvin: entry.value.kelvin,
                warningActive: false,
                hasMotionSensor: entry.value.hasMotionSensor,
              );
            }
          }
          break;

        case 'input_event':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final payload = json['data'] as Map<String, dynamic>? ?? json;
          final eventPayload =
              payload['event'] as Map<String, dynamic>? ?? payload;
          _inputEventController.add(RhythmInputEvent.fromJson(eventPayload));
          break;

        case 'hub_status':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final payload = json['data'] as Map<String, dynamic>? ?? json;
          final hubConnected = payload['connected'] as bool? ?? false;
          final hubType = payload['hub_type'] as String?;
          final address = payload['address'] as String?;
          if (hubType == null) break; // No hub type → ignore
          final hubKey = _hubCacheKey(hubType, address);
          if (_cachedHubConnected[hubKey] != hubConnected) {
            _cachedHubConnected[hubKey] = hubConnected;
            _hubEventController.add((
              event: hubConnected ? 'connected' : 'disconnected',
              hubType: hubType,
              address: address,
            ));
          }
          break;

        case 'mode_changed':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final payload = json['data'] as Map<String, dynamic>? ?? json;
          _modeChangedController.add(RhythmModeResource.fromJson(payload));
          _suppressNextSettingsChangedAfterMode = true;
          _settingsChangedSuppressTimer?.cancel();
          _settingsChangedSuppressTimer = Timer(const Duration(seconds: 2), () {
            _suppressNextSettingsChangedAfterMode = false;
            _settingsChangedSuppressTimer = null;
          });
          break;

        case 'settings_changed':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final payload = json['data'] as Map<String, dynamic>? ?? json;
          final settingsJson =
              payload['settings'] as Map<String, dynamic>? ?? payload;
          if (settingsJson.containsKey('power_save') ||
              settingsJson.containsKey('auto_update')) {
            _settingsChangedController.add(RhythmSettings.fromJson(
              Map<String, dynamic>.from(settingsJson),
            ));
          }
          if (_suppressNextSettingsChangedAfterMode) {
            _suppressNextSettingsChangedAfterMode = false;
            _settingsChangedSuppressTimer?.cancel();
            _settingsChangedSuppressTimer = null;
            break;
          }
          _reHelloSuppressedNodeIds.clear();
          _newNodesController.add(null);
          break;

        case 'light_breaker_changed':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final payload = json['data'] as Map<String, dynamic>? ?? json;
          final lightBreakerJson =
              payload['light_breaker'] as Map<String, dynamic>? ?? payload;
          _lightBreakerChangedController.add(RhythmLightBreaker.fromJson(
            Map<String, dynamic>.from(lightBreakerJson),
          ));
          break;

        case 'outdoor_changed':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final payload = json['data'] as Map<String, dynamic>? ?? json;
          final outdoorJson =
              payload['outdoor'] as Map<String, dynamic>? ?? payload;
          _outdoorChangedController.add(
            RhythmEnvironmentSnapshot.fromJson(
              Map<String, dynamic>.from(outdoorJson),
            ),
          );
          break;

        case 'scope_node_changed':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final payload = json['data'] as Map<String, dynamic>? ?? json;
          final nodeId = payload['node_id']?.toString() ?? '';
          if (nodeId.isNotEmpty) {
            _scopeNodeChangedController.add(nodeId);
          }
          break;

        case 'pipeline_trace_available':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final payload = json['data'] as Map<String, dynamic>? ?? json;
          _pipelineTraceAvailableController.add(
            RhythmPipelineTraceEvent.fromJson(payload),
          );
          break;

        case 'power_schedules_changed':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final payload = json['data'] as Map<String, dynamic>? ?? json;
          final schedulesJson =
              payload['schedules'] as Map<String, dynamic>? ?? payload;
          _powerSchedulesChangedController.add(
            RhythmPowerSchedules.fromJson(
              Map<String, dynamic>.from(schedulesJson),
            ),
          );
          break;

        case 'config_changed':
        case 'nodes_changed':
        case 'rooms_changed':
          _reHelloSuppressedNodeIds.clear();
          _newNodesController.add(null);
          break;

        case 'sync_required':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final payload = json['data'] as Map<String, dynamic>? ?? json;
          _syncRequiredController.add(RhythmSyncRequired.fromJson(payload));
          _reHelloSuppressedNodeIds.clear();
          _newNodesController.add(null);
          break;

        case 'triage_changed':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final payload = json['data'] as Map<String, dynamic>? ?? json;
          _triageChangedController.add(payload);
          break;

        case 'pairing_progress':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final payload = json['data'] as Map<String, dynamic>? ?? json;
          _pairingProgressController
              .add(RhythmPairingProgress.fromJson(payload));
          break;

        case 'ota_update_progress':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final payload = json['data'] as Map<String, dynamic>? ?? json;
          _otaUpdateProgressController
              .add(RhythmOtaUpdateProgress.fromJson(payload));
          break;

        case 'dispatch_failure':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final payload = json['data'] as Map<String, dynamic>? ?? json;
          _dispatchFailureController
              .add(RhythmDispatchFailure.fromJson(payload));
          break;

        case 'lagged':
          reconnect();
          break;
      }
    } catch (e) {
      _log.warning('SSE event parse error for $eventType', e);
    }
  }

  void _disconnectSse() {
    _sseCancelToken?.cancel();
    _sseCancelToken = null;
    _sseSubscription?.cancel();
    _sseSubscription = null;
    _sseConnected = false;
    _sseConnecting = false;
    _stopSseWatchdog();
  }

  void _handleSseDisconnect() {
    _disconnectSse();
    if (connected && _pollTimer == null) {
      _startPolling();
    }

    final delaySecs = switch (_sseReconnectAttempts) {
      0 => 2,
      1 => 5,
      2 => 10,
      3 => 20,
      _ => 30,
    };
    _sseReconnectAttempts++;
    _log.config('SSE disconnected, reconnecting in ${delaySecs}s '
        '(attempt $_sseReconnectAttempts)');

    _sseReconnectTimer?.cancel();
    _sseReconnectTimer = Timer(Duration(seconds: delaySecs), () {
      if (connected && _sseSupported) {
        _connectSse();
      }
    });
  }

  void _startSseWatchdog() {
    _stopSseWatchdog();
    _sseWatchdogTimer = Timer.periodic(_sseWatchdogInterval, (_) {
      if (!_sseConnected) {
        _stopSseWatchdog();
        return;
      }
      final elapsed = DateTime.now().difference(_lastSseActivity);
      if (elapsed > _sseStaleThreshold) {
        _handleSseDisconnect();
      }
    });
  }

  void _stopSseWatchdog() {
    _sseWatchdogTimer?.cancel();
    _sseWatchdogTimer = null;
  }

  // --------------------------------------------------------------------------
  // Internal: reconnect
  // --------------------------------------------------------------------------

  void _scheduleReconnect() {
    _reconnectTimer?.cancel();
    final delaySecs = (1 << _reconnectAttempts).clamp(1, 30);
    _reconnectAttempts++;
    _log.config('Reconnecting in ${delaySecs}s (attempt $_reconnectAttempts)');

    _reconnectTimer = Timer(Duration(seconds: delaySecs), () {
      if (_host != null) {
        _connectInternal();
      }
    });
  }

  // --------------------------------------------------------------------------
  // Internal: helpers
  // --------------------------------------------------------------------------

  void _setConnectionState(RhythmConnectionState state) {
    if (_connectionState != state) {
      _log.config('State: ${state.name}');
      _connectionState = state;
      _connectionStateController.add(state);
    }
  }

  /// Update internal cache from action responses to prevent duplicate events.
  void _updateCacheFromStates(List<RhythmRoomState> states) {
    for (final state in states) {
      final existing = _cachedNodeStates[state.nodeId];
      _cachedNodeStates[state.nodeId] = _CachedNodeState(
        rhythmEnabled: state.rhythmEnabled,
        timeOffset: state.timeOffset,
        brightnessOffset: state.brightnessOffset,
        state: state.state,
        transitioning: state.transitioning,
        // pending_dispatch is SSE-authoritative: action responses are
        // snapshotted while the command is still queued (pending=true), and
        // for fast hubs the SSE clear can beat the HTTP response — letting
        // the response raise the flag would re-assert a stale spinner. A
        // response may only keep or lower it.
        pendingDispatch:
            (existing?.pendingDispatch ?? false) && state.pendingDispatch,
        mode: state.mode ?? existing?.mode,
        powerFresh: state.powerFresh ?? existing?.powerFresh,
        powerSource: state.powerSource ?? existing?.powerSource,
        lightsOn: state.lightsOn ?? existing?.lightsOn,
        brightness: state.brightness ?? existing?.brightness,
        kelvin: state.kelvin ?? existing?.kelvin,
        motionActive: state.motionActive ?? existing?.motionActive,
        motionOwned: state.motionOwned ?? existing?.motionOwned,
        motionRemaining: state.remainingSecs ?? existing?.motionRemaining,
        motionTimeout: state.timeoutSecs ?? existing?.motionTimeout,
        warningActive: state.warningActive ?? existing?.warningActive,
        hasMotionSensor: existing?.hasMotionSensor == true ||
            state.motionActive != null ||
            state.motionOwned != null ||
            state.remainingSecs != null ||
            state.timeoutSecs != null ||
            state.warningActive != null,
      );
    }
  }

  static String _hubCacheKey(String hubType, String? address) {
    if (address == null || address.isEmpty) return hubType;
    return '$hubType@$address';
  }

  static String _buildBaseUrl(
    String host,
    int port,
    bool useSsl,
    String? webBaseUrl,
  ) {
    if (webBaseUrl != null) {
      return webBaseUrl.endsWith('/') ? webBaseUrl : '$webBaseUrl/';
    }
    final scheme = useSsl ? 'https' : 'http';
    return '$scheme://$host:$port/';
  }
}
