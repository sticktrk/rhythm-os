/// Hue V2 API SSE event source.
///
/// Connects to a Hue Bridge's Server-Sent Events endpoint to receive
/// real-time button events from Hue switches and dimmers.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math' show min;

import '../src/rust/api/hue.dart' show mapHueButtonEvent, parseHueButtonEventType;
import 'event_source.dart';

/// Configuration for Hue SSE connection.
class HueSseConfig {
  /// Hue bridge IP address.
  final String bridgeIp;

  /// Hue application key (from bridge pairing).
  final String applicationKey;

  /// Whether to accept self-signed certificates.
  /// Hue bridges use self-signed certs by default.
  final bool acceptSelfSignedCerts;

  /// Initial reconnection delay (default: 1 second).
  final Duration initialReconnectDelay;

  /// Maximum reconnection delay (default: 60 seconds).
  final Duration maxReconnectDelay;

  /// Backoff multiplier for exponential backoff (default: 2.0).
  final double backoffMultiplier;

  /// Maximum reconnection attempts before giving up.
  /// Set to -1 for unlimited attempts.
  final int maxReconnectAttempts;

  /// Heartbeat timeout - if no data (including keep-alive comments)
  /// is received within this duration, the connection is considered stale.
  /// Hue bridges typically send `: hi` every ~10 seconds.
  /// Default: 45 seconds (allows for some network jitter).
  final Duration heartbeatTimeout;

  const HueSseConfig({
    required this.bridgeIp,
    required this.applicationKey,
    this.acceptSelfSignedCerts = true,
    this.initialReconnectDelay = const Duration(seconds: 1),
    this.maxReconnectDelay = const Duration(seconds: 60),
    this.backoffMultiplier = 2.0,
    this.maxReconnectAttempts = -1,
    this.heartbeatTimeout = const Duration(seconds: 45),
  });
}

/// Mapping from Hue resource ID to Rhythm room ID.
typedef HueResourceToRoomMapper = String? Function(String hueResourceId);

/// Lookup function for button control_id from button resource ID.
typedef HueButtonLookup = int? Function(String buttonResourceId);

/// Check if a device is configured in Hue app.
typedef HueDeviceConfiguredCheck = bool Function(String deviceId);

/// Callback for behavior_instance events.
/// [eventType] is "add", "update", or "delete".
/// [data] is the behavior_instance resource data.
typedef HueBehaviorInstanceCallback = void Function(String eventType, Map<String, dynamic> data);

/// Callback for button events (before processing).
/// [deviceId] is the owner device resource ID.
/// [buttonIndex] is the button control_id (1-4 for dimmer).
/// [eventType] is the Hue button event type string (e.g., "short_release").
/// [ignored] is true if the event was ignored (device is Hue-configured).
typedef HueButtonEventCallback = void Function(String deviceId, int buttonIndex, String eventType, bool ignored);

/// Callback for grouped_light state changes.
/// [groupedLightId] is the Hue grouped_light resource ID.
/// [isOn] is true if any light in the group is on.
typedef HueGroupedLightCallback = void Function(String groupedLightId, bool isOn);

/// Hue V2 API SSE event source.
///
/// Connects to the Hue bridge's SSE endpoint and emits [InputEvent]s
/// when button presses are detected.
///
/// Usage:
/// ```dart
/// final source = HueSseSource(
///   config: HueSseConfig(
///     bridgeIp: '192.168.1.100',
///     applicationKey: 'your-app-key',
///   ),
///   resourceToRoomMapper: (resourceId) => deviceRegistry.getRoomForDevice(resourceId),
/// );
///
/// await source.connect();
/// source.events.listen((event) {
///   runner.handleAction(event.roomId, event.action);
/// });
/// ```
class HueSseSource extends EventSource {
  final HueSseConfig config;
  final HueResourceToRoomMapper resourceToRoomMapper;
  final HueButtonLookup buttonControlIdLookup;
  final HueDeviceConfiguredCheck? isDeviceConfigured;
  final HueBehaviorInstanceCallback? onBehaviorInstanceEvent;
  final HueButtonEventCallback? onButtonEvent;
  final HueGroupedLightCallback? onGroupedLightEvent;

  final StreamController<InputEvent> _eventController =
      StreamController<InputEvent>.broadcast();
  final StreamController<EventSourceState> _stateController =
      StreamController<EventSourceState>.broadcast();

  EventSourceState _state = EventSourceState.disconnected;
  HttpClient? _httpClient;
  StreamSubscription<String>? _sseSubscription;
  int _reconnectAttempts = 0;
  Timer? _reconnectTimer;
  Timer? _heartbeatTimer;
  DateTime? _lastDataReceived;
  bool _disposed = false;
  late Duration _currentReconnectDelay;

  HueSseSource({
    required this.config,
    required this.resourceToRoomMapper,
    required this.buttonControlIdLookup,
    this.isDeviceConfigured,
    this.onBehaviorInstanceEvent,
    this.onButtonEvent,
    this.onGroupedLightEvent,
  }) : _currentReconnectDelay = config.initialReconnectDelay;

  @override
  String get id => 'hue_sse_${config.bridgeIp}';

  @override
  String get name => 'Hue Bridge (${config.bridgeIp})';

  @override
  EventSourceState get state => _state;

  @override
  Stream<InputEvent> get events => _eventController.stream;

  @override
  Stream<EventSourceState> get stateChanges => _stateController.stream;

  @override
  Future<void> connect() async {
    if (_disposed) {
      throw StateError('Cannot connect: HueSseSource has been disposed');
    }

    if (_state == EventSourceState.connected ||
        _state == EventSourceState.connecting) {
      return;
    }

    _setState(EventSourceState.connecting);
    _reconnectAttempts = 0;
    _currentReconnectDelay = config.initialReconnectDelay;

    await _connect();
  }

  @override
  Future<void> disconnect() async {
    _reconnectTimer?.cancel();
    _reconnectTimer = null;

    _stopHeartbeatTimer();

    await _sseSubscription?.cancel();
    _sseSubscription = null;

    _httpClient?.close(force: true);
    _httpClient = null;

    _setState(EventSourceState.disconnected);
  }

  /// Start the heartbeat timer to detect stale connections.
  void _startHeartbeatTimer() {
    _stopHeartbeatTimer();
    _lastDataReceived = DateTime.now();

    // Check heartbeat at half the timeout interval for faster detection
    final checkInterval = Duration(
      milliseconds: config.heartbeatTimeout.inMilliseconds ~/ 2,
    );

    _heartbeatTimer = Timer.periodic(checkInterval, (_) {
      _checkHeartbeat();
    });
  }

  /// Stop the heartbeat timer.
  void _stopHeartbeatTimer() {
    _heartbeatTimer?.cancel();
    _heartbeatTimer = null;
    _lastDataReceived = null;
  }

  /// Check if we've received data recently.
  void _checkHeartbeat() {
    if (_disposed || _state != EventSourceState.connected) return;
    if (_lastDataReceived == null) return;

    final timeSinceLastData = DateTime.now().difference(_lastDataReceived!);
    if (timeSinceLastData > config.heartbeatTimeout) {
      // Connection has gone stale - trigger reconnect
      _handleError(Exception(
        'SSE heartbeat timeout: no data received for ${timeSinceLastData.inSeconds}s',
      ));
    }
  }

  /// Called when any data (including keep-alive comments) is received.
  void _onDataReceived() {
    _lastDataReceived = DateTime.now();
  }

  @override
  Future<void> dispose() async {
    _disposed = true;
    await disconnect();
    await _eventController.close();
    await _stateController.close();
  }

  Future<void> _connect() async {
    try {
      _httpClient = HttpClient();

      if (config.acceptSelfSignedCerts) {
        _httpClient!.badCertificateCallback = (cert, host, port) => true;
      }

      final uri = Uri.parse(
        'https://${config.bridgeIp}/eventstream/clip/v2',
      );

      final request = await _httpClient!.getUrl(uri);
      request.headers.set('hue-application-key', config.applicationKey);
      request.headers.set('Accept', 'text/event-stream');

      final response = await request.close();

      if (response.statusCode != 200) {
        throw HttpException(
          'SSE connection failed: ${response.statusCode}',
          uri: uri,
        );
      }

      _setState(EventSourceState.connected);
      _reconnectAttempts = 0;
      _currentReconnectDelay = config.initialReconnectDelay;

      // Start heartbeat monitoring
      _startHeartbeatTimer();

      // Process SSE stream
      _sseSubscription = response
          .transform(utf8.decoder)
          .transform(const LineSplitter())
          .transform(_SseEventTransformer(onAnyData: _onDataReceived))
          .listen(
            _handleSseEvent,
            onError: _handleError,
            onDone: _handleDone,
          );
    } catch (e) {
      _handleError(e);
    }
  }

  void _handleSseEvent(String data) {
    if (data.isEmpty) return;

    try {
      // SSE data is JSON array of event containers
      // Each container has: { "type": "update", "data": [ { "type": "button", ... } ] }
      final containers = jsonDecode(data) as List<dynamic>;

      for (final container in containers) {
        if (container is! Map<String, dynamic>) continue;

        // Get event type (add, update, delete)
        final eventType = container['type'] as String?;

        // The actual resources are in the "data" array
        final resourceList = container['data'] as List<dynamic>?;
        if (resourceList == null) continue;

        for (final resource in resourceList) {
          if (resource is! Map<String, dynamic>) continue;

          final resourceType = resource['type'] as String?;
          if (resourceType == 'button') {
            _processButtonEvent(resource);
          } else if (resourceType == 'motion') {
            // Motion sensor events — ESP32 handles timer logic.
            // No action needed on the Flutter side.
          } else if (resourceType == 'grouped_light') {
            _processGroupedLightEvent(resource);
          } else if (resourceType == 'behavior_instance' && eventType != null) {
            // Forward behavior_instance events to callback
            onBehaviorInstanceEvent?.call(eventType, resource);
          }
        }
      }
    } catch (e) {
      // Ignore JSON parse errors - might be partial data
    }
  }

  void _processButtonEvent(Map<String, dynamic> data) {
    // Get button resource ID
    final buttonResourceId = data['id'] as String?;
    if (buttonResourceId == null) return;

    // Extract button data
    final buttonData = data['button'] as Map<String, dynamic>?;
    if (buttonData == null) return;

    final lastEvent = buttonData['last_event'] as String?;
    if (lastEvent == null) return;

    // Look up control_id from button resource ID (discovered during init)
    final controlId = buttonControlIdLookup(buttonResourceId);
    if (controlId == null) return;

    // Get owner device ID to map to room
    final owner = data['owner'] as Map<String, dynamic>?;
    final ownerRid = owner?['rid'] as String?;
    if (ownerRid == null) return;

    // Check if device is configured in Hue app
    // If so, Hue handles the button events, so we skip it
    if (isDeviceConfigured?.call(ownerRid) ?? false) {
      onButtonEvent?.call(ownerRid, controlId, lastEvent, true);
      return;
    }

    // Parse event type using Rust FFI
    final eventType = parseHueButtonEventType(apiValue: lastEvent);
    if (eventType == null) return;

    // Map to rhythm action using Rust FFI
    final action = mapHueButtonEvent(
      buttonIndex: controlId,
      eventType: eventType,
    );
    if (action == null) return;

    // Look up room for this device
    final roomId = resourceToRoomMapper(ownerRid);
    if (roomId == null) return;

    // Log processed event
    onButtonEvent?.call(ownerRid, controlId, lastEvent, false);

    // Emit event
    _eventController.add(InputEvent(
      roomId: roomId,
      action: action,
      sourceId: id,
      deviceId: ownerRid,
    ));
  }

  void _processGroupedLightEvent(Map<String, dynamic> data) {
    final groupedLightId = data['id'] as String?;
    if (groupedLightId == null) return;

    final on = data['on'] as Map<String, dynamic>?;
    if (on == null) return;

    final isOn = on['on'] as bool?;
    if (isOn == null) return;

    onGroupedLightEvent?.call(groupedLightId, isOn);
  }

  void _handleError(Object error) {
    if (_disposed) return;

    _stopHeartbeatTimer();

    _sseSubscription?.cancel();
    _sseSubscription = null;
    _httpClient?.close(force: true);
    _httpClient = null;

    if (config.maxReconnectAttempts >= 0 &&
        _reconnectAttempts >= config.maxReconnectAttempts) {
      _setState(EventSourceState.failed);
      return;
    }

    _setState(EventSourceState.reconnecting);
    _scheduleReconnect();
  }

  void _handleDone() {
    if (_disposed) return;

    // Connection closed, attempt reconnect
    _handleError(Exception('SSE connection closed'));
  }

  void _scheduleReconnect() {
    _reconnectTimer?.cancel();

    // Use current delay for this attempt
    final delay = _currentReconnectDelay;

    // Calculate next delay with exponential backoff (capped at max)
    _currentReconnectDelay = Duration(
      milliseconds: min(
        (_currentReconnectDelay.inMilliseconds * config.backoffMultiplier).round(),
        config.maxReconnectDelay.inMilliseconds,
      ),
    );

    _reconnectTimer = Timer(delay, () {
      if (_disposed || _state != EventSourceState.reconnecting) return;
      _reconnectAttempts++;
      _connect();
    });
  }

  void _setState(EventSourceState newState) {
    if (_state != newState) {
      _state = newState;
      _stateController.add(newState);
    }
  }
}

/// Transforms raw SSE lines into event data strings.
///
/// SSE format:
/// ```
/// : hi
///
/// id: 1234
/// data: [{"type":"button",...}]
///
/// id: 1235
/// data: [{"type":"light",...}]
/// ```
class _SseEventTransformer extends StreamTransformerBase<String, String> {
  /// Callback invoked whenever any data is received (including keep-alive comments).
  /// Used for heartbeat detection.
  final void Function()? onAnyData;

  const _SseEventTransformer({this.onAnyData});

  @override
  Stream<String> bind(Stream<String> stream) {
    return Stream.eventTransformed(
      stream,
      (sink) => _SseEventSink(sink, onAnyData: onAnyData),
    );
  }
}

class _SseEventSink implements EventSink<String> {
  final EventSink<String> _outputSink;
  final StringBuffer _dataBuffer = StringBuffer();
  final void Function()? onAnyData;

  _SseEventSink(this._outputSink, {this.onAnyData});

  @override
  void add(String line) {
    // Notify that we received any data (for heartbeat tracking)
    // This includes keep-alive comments like ": hi"
    onAnyData?.call();

    if (line.startsWith('data: ')) {
      _dataBuffer.write(line.substring(6));
    } else if (line.isEmpty && _dataBuffer.isNotEmpty) {
      // Empty line signals end of event
      _outputSink.add(_dataBuffer.toString());
      _dataBuffer.clear();
    }
    // Note: comment lines (starting with ':') and id: lines are now tracked
    // for heartbeat purposes even though we don't process their content
  }

  @override
  void addError(Object error, [StackTrace? stackTrace]) {
    _outputSink.addError(error, stackTrace);
  }

  @override
  void close() {
    if (_dataBuffer.isNotEmpty) {
      _outputSink.add(_dataBuffer.toString());
    }
    _outputSink.close();
  }
}
