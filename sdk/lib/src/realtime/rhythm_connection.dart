/// Stateful connection manager for Rhythm server communication.
///
/// Manages polling, SSE, reconnect, and cache diffing. Exposes typed
/// streams for real-time room state updates.
///
/// Uses [StreamController]s instead of ChangeNotifier for pure Dart
/// compatibility (no Flutter dependency).
library;

import 'dart:async';
import 'dart:convert';

import 'package:dio/dio.dart';

import '../api/rhythm_server_api.dart';
import '../models/rhythm_connection_state.dart';
import '../models/rhythm_hello.dart';
import '../models/rhythm_room.dart';

/// Cached room state for diff detection.
class _CachedRoomState {
  final bool rhythmEnabled;
  final double timeOffset;
  final double brightnessOffset;
  final bool softOff;
  final bool? lightsOn;
  final int? brightness;
  final int? kelvin;
  final bool? motionActive;
  final bool? motionOwned;
  final int? motionRemaining;
  final int? motionTimeout;
  final bool hasMotionSensor;

  const _CachedRoomState({
    required this.rhythmEnabled,
    required this.timeOffset,
    required this.brightnessOffset,
    required this.softOff,
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
  // Stream controllers (broadcast so multiple listeners work).
  final _helloController = StreamController<RhythmHello>.broadcast();
  final _rhythmStateController =
      StreamController<RhythmRoomState>.broadcast();
  final _hubEventController = StreamController<String>.broadcast();
  final _motionTimerController =
      StreamController<RhythmMotionTimer>.broadcast();
  final _newRoomsController = StreamController<void>.broadcast();
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

  // Cached room states for diff detection.
  final Map<String, _CachedRoomState> _cachedRoomStates = {};
  bool? _cachedHubConnected;
  final Set<String> _reHelloSuppressedRoomIds = {};

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
  Stream<String> get hubEvents => _hubEventController.stream;
  Stream<RhythmMotionTimer> get motionTimerEvents =>
      _motionTimerController.stream;
  Stream<void> get newRoomsDetected => _newRoomsController.stream;
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
      _cachedRoomStates.clear();
      _cachedHubConnected = null;
      _reHelloSuppressedRoomIds.clear();
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
    _api = RhythmServerApi(_dio!, onStatesReceived: _updateCacheFromStates);

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
    _cachedRoomStates.clear();
    _cachedHubConnected = null;
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

    _cachedRoomStates.clear();
    _cachedHubConnected = null;
    _reHelloSuppressedRoomIds.clear();
    _sseSupported = true;
    _sseDirectAttempted = false;

    _setConnectionState(RhythmConnectionState.disconnected);
  }

  /// Trigger an immediate poll cycle.
  Future<void> pollNow() async {
    if (connected) {
      _cachedRoomStates.clear();
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
    _newRoomsController.close();
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
      final response = await _dio!.get('api/state');
      final data = response.data as Map<String, dynamic>;
      final hello = RhythmHello.fromJson(data);
      _serverPlatformContext = hello.platformContext;
      _listenPort = hello.listenPort;
      _sseDirectAttempted = false;

      _cachedRoomStates.clear();
      for (final room in hello.rooms) {
        _cachedRoomStates[room.id] = _CachedRoomState(
          rhythmEnabled: room.rhythmEnabled,
          timeOffset: room.timeOffset,
          brightnessOffset: room.brightnessOffset,
          softOff: room.softOff,
          lightsOn: room.lightsOn,
          brightness: room.brightness,
          kelvin: room.kelvin,
          hasMotionSensor: room.hasMotionSensor,
        );
      }

      _reHelloSuppressedRoomIds.removeAll(_cachedRoomStates.keys);
      _cachedHubConnected = hello.hub['connected'] as bool?;

      _setConnectionState(RhythmConnectionState.connected);
      _reconnectAttempts = 0;
      _consecutivePollFailures = 0;

      _helloController.add(hello);
      _startPolling();

      if (hello.platformType == 'embedded') {
        _sseSupported = false;
      } else {
        _connectSse();
      }
    } catch (e) {
      _scheduleReconnect();
    }
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
      final response = await _dio!.get('api/rooms/state');
      final data = response.data as Map<String, dynamic>;
      _consecutivePollFailures = 0;

      // Hub connected changes.
      final hubConnected = data['hub_connected'] as bool?;
      if (hubConnected != null && hubConnected != _cachedHubConnected) {
        _cachedHubConnected = hubConnected;
        _hubEventController.add(hubConnected ? 'connected' : 'disconnected');
      }

      final previousRoomIds = _cachedRoomStates.keys.toSet();
      final rooms = data['rooms'] as List<dynamic>? ?? [];
      final seenRoomIds = <String>{};

      for (final raw in rooms) {
        final roomJson = raw as Map<String, dynamic>;
        final roomId = roomJson['id'] as String? ?? '';
        if (roomId.isEmpty) continue;
        seenRoomIds.add(roomId);

        final rhythmEnabled = roomJson['rhythm_enabled'] as bool? ?? false;
        final timeOffset =
            (roomJson['time_offset'] as num?)?.toDouble() ?? 0.0;
        final brightnessOffset =
            (roomJson['brightness_offset'] as num?)?.toDouble() ?? 0.0;
        final softOff = roomJson['soft_off'] as bool? ?? false;
        final lightsOn = roomJson['lights_on'] as bool?;
        final brightness = (roomJson['brightness'] as num?)?.toInt();
        final kelvin = (roomJson['kelvin'] as num?)?.toInt();

        final motionActive = roomJson['motion_active'] as bool?;
        final motionOwned = roomJson['motion_owned'] as bool?;
        final motionRemaining =
            (roomJson['remaining_secs'] as num?)?.toInt() ??
                (roomJson['motion_remaining'] as num?)?.toInt();
        final motionTimeout =
            (roomJson['timeout_secs'] as num?)?.toInt() ??
                (roomJson['motion_timeout'] as num?)?.toInt();

        final cached = _cachedRoomStates[roomId];
        final hasMotionSensor = motionActive != null;

        final rhythmChanged = cached == null ||
            cached.rhythmEnabled != rhythmEnabled ||
            cached.timeOffset != timeOffset ||
            cached.brightnessOffset != brightnessOffset ||
            cached.softOff != softOff ||
            cached.lightsOn != lightsOn ||
            cached.brightness != brightness ||
            cached.kelvin != kelvin;

        final motionChanged = cached == null ||
            cached.motionActive != motionActive ||
            cached.motionOwned != motionOwned ||
            cached.motionRemaining != motionRemaining ||
            cached.motionTimeout != motionTimeout;

        if (rhythmChanged || motionChanged) {
          _cachedRoomStates[roomId] = _CachedRoomState(
            rhythmEnabled: rhythmEnabled,
            timeOffset: timeOffset,
            brightnessOffset: brightnessOffset,
            softOff: softOff,
            lightsOn: lightsOn,
            brightness: brightness,
            kelvin: kelvin,
            motionActive: motionActive,
            motionOwned: motionOwned,
            motionRemaining: motionRemaining,
            motionTimeout: motionTimeout,
            hasMotionSensor: hasMotionSensor,
          );

          if (rhythmChanged) {
            _rhythmStateController.add(RhythmRoomState(
              roomId: roomId,
              rhythmEnabled: rhythmEnabled,
              timeOffset: timeOffset,
              brightnessOffset: brightnessOffset,
              softOff: softOff,
              lightsOn: lightsOn,
              brightness: brightness,
              kelvin: kelvin,
            ));
          }

          if (motionChanged) {
            if (motionActive != null && motionTimeout != null) {
              _motionTimerController.add(RhythmMotionTimer(
                roomId: roomId,
                motionActive: motionActive,
                motionOwned: motionOwned ?? false,
                remainingSecs: motionRemaining,
                timeoutSecs: motionTimeout,
              ));
            } else if (cached?.motionActive != null) {
              _motionTimerController.add(RhythmMotionTimer.cleared(roomId));
            }
          }
        }
      }

      // Detect new rooms.
      final newRoomIds = seenRoomIds.difference(previousRoomIds);
      final unsuppressedIds = newRoomIds.difference(_reHelloSuppressedRoomIds);
      if (unsuppressedIds.isNotEmpty && previousRoomIds.isNotEmpty) {
        _reHelloSuppressedRoomIds.addAll(unsuppressedIds);
        _newRoomsController.add(null);
      }

      // Clear motion for removed rooms.
      for (final entry in _cachedRoomStates.entries.toList()) {
        if (!seenRoomIds.contains(entry.key) &&
            entry.value.motionActive != null) {
          _motionTimerController.add(RhythmMotionTimer.cleared(entry.key));
          _cachedRoomStates[entry.key] = _CachedRoomState(
            rhythmEnabled: entry.value.rhythmEnabled,
            timeOffset: entry.value.timeOffset,
            brightnessOffset: entry.value.brightnessOffset,
            softOff: entry.value.softOff,
            hasMotionSensor: entry.value.hasMotionSensor,
          );
        }
      }
    } catch (e) {
      _consecutivePollFailures++;
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
      _lastSseActivity = DateTime.now();
      _stopPolling();
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
        _sseSupported = false;
      } else if (useDirectSse) {
        _connectSse();
      } else {
        _handleSseDisconnect();
      }
    } catch (e) {
      _sseConnecting = false;
      if (useDirectSse) {
        _connectSse();
      } else {
        _handleSseDisconnect();
      }
    }
  }

  void _handleSseEvent(String eventType, String data) {
    _lastSseEvents[eventType] = (
      type: eventType,
      summary: data.length > 100 ? '${data.substring(0, 100)}...' : data,
      time: DateTime.now(),
    );
    try {
      switch (eventType) {
        case 'room_state':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final rooms =
              (json['data'] as Map<String, dynamic>?)?['rooms']
                  as List<dynamic>? ??
              [];
          for (final raw in rooms) {
            final room = raw as Map<String, dynamic>;
            final roomId = room['id'] as String? ?? '';
            if (roomId.isEmpty) continue;

            final rhythmEnabled = room['rhythm_enabled'] as bool? ?? false;
            final timeOffset =
                (room['time_offset'] as num?)?.toDouble() ?? 0.0;
            final brightnessOffset =
                (room['brightness_offset'] as num?)?.toDouble() ?? 0.0;
            final softOff = room['soft_off'] as bool? ?? false;
            final lightsOn = room['lights_on'] as bool?;
            final brightness = (room['brightness'] as num?)?.toInt();
            final kelvin = (room['kelvin'] as num?)?.toInt();

            final existing = _cachedRoomStates[roomId];
            _cachedRoomStates[roomId] = _CachedRoomState(
              rhythmEnabled: rhythmEnabled,
              timeOffset: timeOffset,
              brightnessOffset: brightnessOffset,
              softOff: softOff,
              lightsOn: lightsOn,
              brightness: brightness,
              kelvin: kelvin,
              motionActive: existing?.motionActive,
              motionOwned: existing?.motionOwned,
              motionRemaining: existing?.motionRemaining,
              motionTimeout: existing?.motionTimeout,
              hasMotionSensor: existing?.hasMotionSensor ?? false,
            );

            _rhythmStateController.add(RhythmRoomState(
              roomId: roomId,
              rhythmEnabled: rhythmEnabled,
              timeOffset: timeOffset,
              brightnessOffset: brightnessOffset,
              softOff: softOff,
              lightsOn: lightsOn,
              brightness: brightness,
              kelvin: kelvin,
            ));
          }

        case 'motion_timer':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final timers =
              (json['data'] as Map<String, dynamic>?)?['timers']
                  as List<dynamic>? ??
              [];

          final seenRoomIds = <String>{};
          for (final raw in timers) {
            final timer = raw as Map<String, dynamic>;
            final roomId = timer['room_id'] as String? ?? '';
            if (roomId.isEmpty) continue;
            seenRoomIds.add(roomId);

            final motionActive = timer['motion_active'] as bool? ?? false;
            final motionOwned = timer['motion_owned'] as bool? ?? false;
            final remainingSecs =
                (timer['remaining_secs'] as num?)?.toInt();
            final timeoutSecs =
                (timer['timeout_secs'] as num?)?.toInt() ?? 0;

            final cached = _cachedRoomStates[roomId];
            if (cached != null) {
              _cachedRoomStates[roomId] = _CachedRoomState(
                rhythmEnabled: cached.rhythmEnabled,
                timeOffset: cached.timeOffset,
                brightnessOffset: cached.brightnessOffset,
                softOff: cached.softOff,
                motionActive: motionActive,
                motionOwned: motionOwned,
                motionRemaining: remainingSecs,
                motionTimeout: timeoutSecs,
                hasMotionSensor: true,
              );
            }

            _motionTimerController.add(RhythmMotionTimer(
              roomId: roomId,
              motionActive: motionActive,
              motionOwned: motionOwned,
              remainingSecs: remainingSecs,
              timeoutSecs: timeoutSecs,
            ));
          }

          for (final entry in _cachedRoomStates.entries.toList()) {
            if (entry.value.motionActive != null &&
                !seenRoomIds.contains(entry.key)) {
              _motionTimerController
                  .add(RhythmMotionTimer.cleared(entry.key));
              _cachedRoomStates[entry.key] = _CachedRoomState(
                rhythmEnabled: entry.value.rhythmEnabled,
                timeOffset: entry.value.timeOffset,
                brightnessOffset: entry.value.brightnessOffset,
                softOff: entry.value.softOff,
                hasMotionSensor: entry.value.hasMotionSensor,
              );
            }
          }

        case 'hub_status':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final hubConnected =
              (json['data'] as Map<String, dynamic>?)?['connected']
                  as bool? ??
              false;
          if (hubConnected != _cachedHubConnected) {
            _cachedHubConnected = hubConnected;
            _hubEventController
                .add(hubConnected ? 'connected' : 'disconnected');
          }

        case 'settings_changed':
        case 'config_changed':
        case 'rooms_changed':
          _reHelloSuppressedRoomIds.clear();
          _newRoomsController.add(null);

        case 'triage_changed':
          final json = jsonDecode(data) as Map<String, dynamic>;
          final payload = json['data'] as Map<String, dynamic>? ?? json;
          _triageChangedController.add(payload);

        case 'lagged':
          reconnect();
      }
    } catch (_) {}
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
      _connectionState = state;
      _connectionStateController.add(state);
    }
  }

  /// Update internal cache from action responses to prevent duplicate events.
  void _updateCacheFromStates(List<RhythmRoomState> states) {
    for (final state in states) {
      final existing = _cachedRoomStates[state.roomId];
      _cachedRoomStates[state.roomId] = _CachedRoomState(
        rhythmEnabled: state.rhythmEnabled,
        timeOffset: state.timeOffset,
        brightnessOffset: state.brightnessOffset,
        softOff: state.softOff,
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
