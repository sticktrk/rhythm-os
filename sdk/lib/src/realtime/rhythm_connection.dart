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

import '../api/rhythm_server_api.dart';
import '../json_parsing.dart';
import '../models/rhythm_connection_state.dart';
import '../models/rhythm_hello.dart';
import '../models/rhythm_room.dart';
import '../rhythm_log_interceptor.dart';

/// Cached node state for diff detection.
class _CachedNodeState {
  final bool rhythmEnabled;
  final double timeOffset;
  final double brightnessOffset;
  final RoomModeState state;
  final bool transitioning;
  final RhythmMode? mode;
  final bool? lightsOn;
  final int? brightness;
  final int? kelvin;
  final bool? motionActive;
  final bool? motionOwned;
  final int? motionRemaining;
  final int? motionTimeout;
  final bool hasMotionSensor;

  const _CachedNodeState({
    required this.rhythmEnabled,
    required this.timeOffset,
    required this.brightnessOffset,
    required this.state,
    this.transitioning = false,
    this.mode,
    this.lightsOn,
    this.brightness,
    this.kelvin,
    this.motionActive,
    this.motionOwned,
    this.motionRemaining,
    this.motionTimeout,
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
  final _hubEventController =
      StreamController<({String event, String? hubType})>.broadcast();
  final _motionTimerController =
      StreamController<RhythmMotionTimer>.broadcast();
  final _newNodesController = StreamController<void>.broadcast();
  final _triageChangedController =
      StreamController<Map<String, dynamic>>.broadcast();
  final _connectionStateController =
      StreamController<RhythmConnectionState>.broadcast();

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

  // Direct SSE state (for HA addon web).
  String? _serverPlatformContext;
  int? _listenPort;
  bool _sseDirectAttempted = false;

  // SSE keepalive watchdog.
  DateTime _lastSseActivity = DateTime.fromMillisecondsSinceEpoch(0);
  Timer? _sseWatchdogTimer;
  static const Duration _sseWatchdogInterval = Duration(seconds: 15);
  static const Duration _sseStaleThreshold = Duration(seconds: 45);

  // SSE event history for debug display.
  final Map<String, ({String type, String summary, DateTime time})>
      _lastSseEvents = {};

  // Cached node states for diff detection.
  final Map<String, _CachedNodeState> _cachedNodeStates = {};
  final Map<String, bool> _cachedHubConnected = {};
  final Set<String> _reHelloSuppressedNodeIds = {};

  // Optional web base URL (consumer passes Uri.base.toString() on web).
  String? _webBaseUrl;

  // The server API (uses the shared Dio instance).
  RhythmServerApi? _api;

  // --------------------------------------------------------------------------
  // Public getters
  // --------------------------------------------------------------------------

  Stream<RhythmHello> get helloEvents => _helloController.stream;
  Stream<RhythmRoomState> get rhythmStateEvents =>
      _rhythmStateController.stream;
  Stream<({String event, String? hubType})> get hubEvents =>
      _hubEventController.stream;
  Stream<RhythmMotionTimer> get motionTimerEvents =>
      _motionTimerController.stream;
  Stream<void> get newNodesDetected => _newNodesController.stream;
  Stream<void> get newRoomsDetected => newNodesDetected;
  Stream<Map<String, dynamic>> get triageChangedEvents =>
      _triageChangedController.stream;
  Stream<RhythmConnectionState> get connectionStateStream =>
      _connectionStateController.stream;

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

  // --------------------------------------------------------------------------
  // Connect / Disconnect
  // --------------------------------------------------------------------------

  /// Connect to a server device.
  ///
  /// [webBaseUrl] is required on web platforms (pass `Uri.base.toString()`).
  Future<void> connect(String host, {int port = 80, String? webBaseUrl}) async {
    if (_connectionState == RhythmConnectionState.connected &&
        _host == host &&
        _port == port) {
      return;
    }

    if (_host != null && (_host != host || _port != port)) {
      _stopPolling();
      _disconnectSse();
      _sseReconnectTimer?.cancel();
      _sseReconnectTimer = null;
      _sseReconnectAttempts = 0;
      _reconnectTimer?.cancel();
      _reconnectTimer = null;
      _reconnectAttempts = 0;
      _cachedNodeStates.clear();
      _cachedHubConnected.clear();
      _reHelloSuppressedNodeIds.clear();
      _sseSupported = true;
      _sseDirectAttempted = false;
    }

    _host = host;
    _port = port;
    _webBaseUrl = webBaseUrl;
    final baseUrl = _buildBaseUrl(host, port, webBaseUrl);
    _dio = Dio(BaseOptions(
      baseUrl: baseUrl,
      connectTimeout: const Duration(seconds: 5),
      receiveTimeout: const Duration(seconds: 10),
    ));
    _dio!.interceptors.add(RhythmLogInterceptor(_log));
    _api = RhythmServerApi(_dio!, onStatesReceived: _updateCacheFromStates);

    _log.config('Connecting to $host:$port');
    await _connectInternal();
  }

  /// Force a full reconnect.
  Future<void> reconnect() async {
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
    await _connectInternal();
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
    _dio?.close();
    _dio = null;
    _api = null;

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
    _newNodesController.close();
    _triageChangedController.close();
    _connectionStateController.close();
  }

  // --------------------------------------------------------------------------
  // Internal: connect
  // --------------------------------------------------------------------------

  Future<void> _connectInternal() async {
    if (_dio == null || _host == null) return;

    _sseReconnectTimer?.cancel();
    _sseReconnectTimer = null;
    _sseReconnectAttempts = 0;

    _setConnectionState(_reconnectAttempts > 0
        ? RhythmConnectionState.reconnecting
        : RhythmConnectionState.connecting);

    try {
      final data = await _getHelloPayload();
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
          lightsOn: node.lightsOn,
          brightness: node.brightness,
          kelvin: node.kelvin,
          motionActive: node.motionActive,
          motionOwned: node.motionOwned,
          motionRemaining: node.remainingSecs,
          motionTimeout: node.timeoutSecs,
          hasMotionSensor: node.hasMotionSensor,
        );
      }

      _reHelloSuppressedNodeIds.removeAll(_cachedNodeStates.keys);
      // Cache per-hub connected state from hello for diff detection.
      _cachedHubConnected.clear();
      for (final hub in hello.hubInfos) {
        if (hub.configured) {
          _cachedHubConnected[hub.type] = hub.connected;
        }
      }

      _setConnectionState(RhythmConnectionState.connected);
      _reconnectAttempts = 0;
      _consecutivePollFailures = 0;

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

  Future<Map<String, dynamic>> _getHelloPayload() async {
    final response = await _dio!.get('api/state');
    return Map<String, dynamic>.from(response.data as Map<String, dynamic>);
  }

  // --------------------------------------------------------------------------
  // Internal: polling
  // --------------------------------------------------------------------------

  void _startPolling() {
    _stopPolling();
    _poll();
    _pollTimer = Timer.periodic(_pollInterval, (_) => _poll());
  }

  void _stopPolling() {
    _pollTimer?.cancel();
    _pollTimer = null;
    _consecutivePollFailures = 0;
  }

  Future<void> _poll() async {
    if (_dio == null || !connected) return;

    try {
      final response = await _dio!.get('api/nodes/state');
      final data = response.data as Map<String, dynamic>;
      _consecutivePollFailures = 0;

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

        final cached = _cachedNodeStates[nodeId];
        final hasMotionSensor = motionActive != null;

        final rhythmChanged = cached == null ||
            cached.rhythmEnabled != nodeState.rhythmEnabled ||
            cached.timeOffset != nodeState.timeOffset ||
            cached.brightnessOffset != nodeState.brightnessOffset ||
            cached.state != nodeState.state ||
            cached.transitioning != nodeState.transitioning ||
            cached.lightsOn != nodeState.lightsOn ||
            cached.brightness != nodeState.brightness ||
            cached.kelvin != nodeState.kelvin;

        final motionChanged = cached == null ||
            cached.motionActive != motionActive ||
            cached.motionOwned != motionOwned ||
            cached.motionRemaining != motionRemaining ||
            cached.motionTimeout != motionTimeout;

        if (rhythmChanged || motionChanged) {
          _cachedNodeStates[nodeId] = _CachedNodeState(
            rhythmEnabled: nodeState.rhythmEnabled,
            timeOffset: nodeState.timeOffset,
            brightnessOffset: nodeState.brightnessOffset,
            state: nodeState.state,
            transitioning: nodeState.transitioning,
            mode: nodeState.mode,
            lightsOn: nodeState.lightsOn,
            brightness: nodeState.brightness,
            kelvin: nodeState.kelvin,
            motionActive: motionActive,
            motionOwned: motionOwned,
            motionRemaining: motionRemaining,
            motionTimeout: motionTimeout,
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
            mode: entry.value.mode,
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

    _sseConnecting = true;
    _disconnectSse();

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
          headers: {'Accept': 'text/event-stream'},
        ),
        cancelToken: cancelToken,
      );

      final stream = (response.data as ResponseBody?)?.stream;
      if (stream == null) {
        _sseConnecting = false;
        return;
      }

      _sseConnected = true;
      _sseConnecting = false;
      _sseReconnectAttempts = 0;
      _log.config('SSE connected');
      _lastSseActivity = DateTime.now();
      // Keep polling active even when SSE is connected as a safety net for
      // dropped streams and runtime recovery.
      _startSseWatchdog();

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
            dataBuffer.write(line.substring(5).trim());
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
          for (final raw in nodes) {
            final node = raw as Map<String, dynamic>;
            final nodeState = RhythmRoomState.fromJson(node);
            final nodeId = nodeState.nodeId;
            if (nodeId.isEmpty) continue;

            final existing = _cachedNodeStates[nodeId];
            _cachedNodeStates[nodeId] = _CachedNodeState(
              rhythmEnabled: nodeState.rhythmEnabled,
              timeOffset: nodeState.timeOffset,
              brightnessOffset: nodeState.brightnessOffset,
              state: nodeState.state,
              transitioning: nodeState.transitioning,
              mode: nodeState.mode,
              lightsOn: nodeState.lightsOn,
              brightness: nodeState.brightness,
              kelvin: nodeState.kelvin,
              motionActive: existing?.motionActive,
              motionOwned: existing?.motionOwned,
              motionRemaining: existing?.motionRemaining,
              motionTimeout: existing?.motionTimeout,
              hasMotionSensor: existing?.hasMotionSensor ?? false,
            );

            _rhythmStateController.add(nodeState);
          }

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

            final cached = _cachedNodeStates[nodeId];
            if (cached != null) {
              _cachedNodeStates[nodeId] = _CachedNodeState(
                rhythmEnabled: cached.rhythmEnabled,
                timeOffset: cached.timeOffset,
                brightnessOffset: cached.brightnessOffset,
                state: cached.state,
                transitioning: cached.transitioning,
                mode: cached.mode,
                motionActive: motionActive,
                motionOwned: motionOwned,
                motionRemaining: remainingSecs,
                motionTimeout: timeoutSecs,
                hasMotionSensor: true,
              );
            }

            _motionTimerController.add(RhythmMotionTimer.node(
              nodeId: nodeId,
              motionActive: motionActive,
              motionOwned: motionOwned,
              remainingSecs: remainingSecs,
              timeoutSecs: timeoutSecs,
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
                mode: entry.value.mode,
                hasMotionSensor: entry.value.hasMotionSensor,
              );
            }
          }

        case 'hub_status':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final payload = json['data'] as Map<String, dynamic>? ?? json;
          final hubConnected = payload['connected'] as bool? ?? false;
          final hubType = payload['hub_type'] as String?;
          if (hubType == null) break; // No hub type → ignore
          if (_cachedHubConnected[hubType] != hubConnected) {
            _cachedHubConnected[hubType] = hubConnected;
            _hubEventController.add((
              event: hubConnected ? 'connected' : 'disconnected',
              hubType: hubType,
            ));
          }

        case 'settings_changed':
        case 'config_changed':
        case 'nodes_changed':
        case 'rooms_changed':
          _reHelloSuppressedNodeIds.clear();
          _newNodesController.add(null);

        case 'triage_changed':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final payload = json['data'] as Map<String, dynamic>? ?? json;
          _triageChangedController.add(payload);

        case 'lagged':
          reconnect();
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
    if (connected) {
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
        mode: state.mode ?? existing?.mode,
        lightsOn: state.lightsOn ?? existing?.lightsOn,
        brightness: state.brightness ?? existing?.brightness,
        kelvin: state.kelvin ?? existing?.kelvin,
        motionActive: existing?.motionActive,
        motionOwned: existing?.motionOwned,
        motionRemaining: existing?.motionRemaining,
        motionTimeout: existing?.motionTimeout,
        hasMotionSensor: existing?.hasMotionSensor ?? false,
      );
    }
  }

  static String _buildBaseUrl(String host, int port, String? webBaseUrl) {
    if (webBaseUrl != null) {
      return webBaseUrl.endsWith('/') ? webBaseUrl : '$webBaseUrl/';
    }
    return 'http://$host:$port/';
  }
}
