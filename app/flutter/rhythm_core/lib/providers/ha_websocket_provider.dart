/// Home Assistant WebSocket provider for real-time event subscriptions.
///
/// This provider connects to Home Assistant's WebSocket API for:
/// - Real-time event subscriptions (zha_event, rhythm_service_event)
/// - Device and area registry queries
/// - Service calls for light control
library;

import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:web_socket_channel/web_socket_channel.dart';

import 'light_provider.dart';

/// A Home Assistant event from WebSocket subscription.
class HaEvent {
  final String eventType;
  final Map<String, dynamic> data;
  final DateTime timestamp;

  const HaEvent({
    required this.eventType,
    required this.data,
    required this.timestamp,
  });

  factory HaEvent.fromJson(Map<String, dynamic> json) {
    return HaEvent(
      eventType: json['event_type'] as String? ?? '',
      data: json['data'] as Map<String, dynamic>? ?? {},
      timestamp: DateTime.now(),
    );
  }

  @override
  String toString() => 'HaEvent($eventType: $data)';
}

/// A discovered Hue switch device with area mapping.
class DiscoveredSwitch {
  /// IEEE address of the device.
  final String ieee;

  /// Home Assistant device ID.
  final String deviceId;

  /// Area ID the device is assigned to.
  final String areaId;

  /// Human-readable area name.
  final String areaName;

  /// Device name.
  final String name;

  const DiscoveredSwitch({
    required this.ieee,
    required this.deviceId,
    required this.areaId,
    required this.areaName,
    required this.name,
  });

  @override
  String toString() => 'DiscoveredSwitch($name in $areaName)';
}

/// Home Assistant device from device registry.
class HaDevice {
  final String id;
  final String? name;
  final String? areaId;
  final List<List<String>> identifiers;

  const HaDevice({
    required this.id,
    this.name,
    this.areaId,
    required this.identifiers,
  });

  factory HaDevice.fromJson(Map<String, dynamic> json) {
    final identifiers = (json['identifiers'] as List<dynamic>?)
            ?.map((e) => (e as List<dynamic>).map((i) => i.toString()).toList())
            .toList() ??
        [];
    return HaDevice(
      id: json['id'] as String,
      name: json['name'] as String?,
      areaId: json['area_id'] as String?,
      identifiers: identifiers,
    );
  }

  /// Check if this device is a ZHA device.
  bool get isZha => identifiers.any((id) => id.contains('zha'));

  /// Get the ZHA IEEE address if this is a ZHA device.
  String? get zhaIeee {
    for (final id in identifiers) {
      if (id.length >= 2 && id[0] == 'zha') {
        return id[1];
      }
    }
    return null;
  }

  /// Check if this is a Hue/Philips device based on IEEE prefix.
  ///
  /// Uses the shared logic from rhythm_core::device::ieee::is_hue().
  bool get isHueDevice {
    final ieee = zhaIeee;
    if (ieee == null) return false;
    return rust_api.isHueIeee(ieee: ieee);
  }
}

/// Home Assistant area from area registry.
class HaArea {
  final String id;
  final String name;

  const HaArea({
    required this.id,
    required this.name,
  });

  factory HaArea.fromJson(Map<String, dynamic> json) {
    return HaArea(
      id: json['area_id'] as String,
      name: json['name'] as String,
    );
  }
}

/// WebSocket connection state.
enum WsConnectionState {
  disconnected,
  connecting,
  authenticating,
  connected,
  error,
}

/// Home Assistant WebSocket provider.
///
/// Provides real-time event subscriptions and service calls via WebSocket.
class HaWebSocketProvider {
  final HomeAssistantConfig config;

  WebSocketChannel? _channel;
  StreamSubscription? _subscription;
  int _messageId = 1;
  final Map<int, Completer<dynamic>> _pendingRequests = {};
  final _eventController = StreamController<HaEvent>.broadcast();
  WsConnectionState _connectionState = WsConnectionState.disconnected;
  String? _lastError;

  // Cached registries
  List<HaDevice>? _deviceRegistry;
  List<HaArea>? _areaRegistry;
  Map<String, dynamic>? _haConfig;

  HaWebSocketProvider(this.config);

  /// Current connection state.
  WsConnectionState get connectionState => _connectionState;

  /// Last error message if connection failed.
  String? get lastError => _lastError;

  /// Stream of events from subscriptions.
  Stream<HaEvent> get eventStream => _eventController.stream;

  /// Whether currently connected and authenticated.
  bool get isConnected => _connectionState == WsConnectionState.connected;

  /// Connect to Home Assistant WebSocket API.
  Future<bool> connect() async {
    if (_connectionState == WsConnectionState.connected) {
      return true;
    }

    _connectionState = WsConnectionState.connecting;
    _lastError = null;

    try {
      final protocol = config.useSsl ? 'wss' : 'ws';
      final uri = Uri.parse('$protocol://${config.host}:${config.port}/api/websocket');

      debugPrint('HaWebSocket: Connecting to $uri');
      _channel = WebSocketChannel.connect(uri);

      // Set up message handler
      final completer = Completer<bool>();
      _subscription = _channel!.stream.listen(
        (message) => _handleMessage(message, completer),
        onError: (error) {
          debugPrint('HaWebSocket: Connection error: $error');
          _connectionState = WsConnectionState.error;
          _lastError = error.toString();
          if (!completer.isCompleted) {
            completer.complete(false);
          }
        },
        onDone: () {
          debugPrint('HaWebSocket: Connection closed');
          _connectionState = WsConnectionState.disconnected;
          if (!completer.isCompleted) {
            completer.complete(false);
          }
        },
      );

      // Wait for auth result
      final result = await completer.future.timeout(
        const Duration(seconds: 10),
        onTimeout: () {
          _lastError = 'Connection timeout';
          return false;
        },
      );

      if (result) {
        _connectionState = WsConnectionState.connected;
        debugPrint('HaWebSocket: Connected and authenticated');
      }

      return result;
    } catch (e) {
      debugPrint('HaWebSocket: Failed to connect: $e');
      _connectionState = WsConnectionState.error;
      _lastError = e.toString();
      return false;
    }
  }

  /// Handle incoming WebSocket messages.
  void _handleMessage(dynamic message, Completer<bool>? authCompleter) {
    try {
      final data = jsonDecode(message as String) as Map<String, dynamic>;
      final type = data['type'] as String?;

      switch (type) {
        case 'auth_required':
          // Send authentication
          _connectionState = WsConnectionState.authenticating;
          _send({
            'type': 'auth',
            'access_token': config.token,
          });
          break;

        case 'auth_ok':
          debugPrint('HaWebSocket: Authentication successful');
          authCompleter?.complete(true);
          break;

        case 'auth_invalid':
          debugPrint('HaWebSocket: Authentication failed: ${data['message']}');
          _lastError = data['message'] as String? ?? 'Authentication failed';
          _connectionState = WsConnectionState.error;
          authCompleter?.complete(false);
          break;

        case 'result':
          // Handle response to a request
          final id = data['id'] as int?;
          if (id != null && _pendingRequests.containsKey(id)) {
            final completer = _pendingRequests.remove(id)!;
            if (data['success'] == true) {
              completer.complete(data['result']);
            } else {
              completer.completeError(
                Exception(data['error']?['message'] ?? 'Unknown error'),
              );
            }
          }
          break;

        case 'event':
          // Handle subscribed event
          final event = data['event'] as Map<String, dynamic>?;
          if (event != null) {
            _eventController.add(HaEvent.fromJson(event));
          }
          break;

        default:
          debugPrint('HaWebSocket: Unknown message type: $type');
      }
    } catch (e) {
      debugPrint('HaWebSocket: Error handling message: $e');
    }
  }

  /// Send a message to the WebSocket.
  void _send(Map<String, dynamic> message) {
    if (_channel == null) return;
    _channel!.sink.add(jsonEncode(message));
  }

  /// Send a request and wait for response.
  Future<dynamic> _request(String type, [Map<String, dynamic>? data]) async {
    if (!isConnected) {
      throw StateError('Not connected to Home Assistant');
    }

    final id = _messageId++;
    final completer = Completer<dynamic>();
    _pendingRequests[id] = completer;

    final message = {
      'id': id,
      'type': type,
      ...?data,
    };
    _send(message);

    return completer.future.timeout(
      const Duration(seconds: 30),
      onTimeout: () {
        _pendingRequests.remove(id);
        throw TimeoutException('Request timed out');
      },
    );
  }

  /// Subscribe to specific event types.
  Future<void> subscribeEvents(List<String> eventTypes) async {
    for (final eventType in eventTypes) {
      await _request('subscribe_events', {'event_type': eventType});
      debugPrint('HaWebSocket: Subscribed to $eventType');
    }
  }

  /// Get Home Assistant configuration.
  Future<Map<String, dynamic>> getConfig() async {
    if (_haConfig != null) return _haConfig!;
    _haConfig = await _request('get_config') as Map<String, dynamic>;
    return _haConfig!;
  }

  /// Get device registry.
  Future<List<HaDevice>> getDeviceRegistry() async {
    if (_deviceRegistry != null) return _deviceRegistry!;

    final result = await _request('config/device_registry/list');
    final devices = (result as List<dynamic>)
        .map((d) => HaDevice.fromJson(d as Map<String, dynamic>))
        .toList();

    _deviceRegistry = devices;
    return devices;
  }

  /// Get area registry.
  Future<List<HaArea>> getAreaRegistry() async {
    if (_areaRegistry != null) return _areaRegistry!;

    final result = await _request('config/area_registry/list');
    final areas = (result as List<dynamic>)
        .map((a) => HaArea.fromJson(a as Map<String, dynamic>))
        .toList();

    _areaRegistry = areas;
    return areas;
  }

  /// Discover Hue switches from device registry.
  ///
  /// Filters for:
  /// - ZHA devices with Philips/Hue IEEE prefix (00:17:88:01:09)
  /// - Devices assigned to an area
  Future<List<DiscoveredSwitch>> discoverHueSwitches() async {
    final devices = await getDeviceRegistry();
    final areas = await getAreaRegistry();

    final areaNames = {for (final a in areas) a.id: a.name};

    final switches = <DiscoveredSwitch>[];
    for (final device in devices) {
      // Must be a Hue ZHA device with area
      if (!device.isHueDevice || device.areaId == null) continue;

      final ieee = device.zhaIeee;
      if (ieee == null) continue;

      switches.add(DiscoveredSwitch(
        ieee: ieee,
        deviceId: device.id,
        areaId: device.areaId!,
        areaName: areaNames[device.areaId] ?? device.areaId!,
        name: device.name ?? ieee,
      ));
    }

    debugPrint('HaWebSocket: Discovered ${switches.length} Hue switches');
    return switches;
  }

  /// Call a Home Assistant service.
  Future<void> callService(
    String domain,
    String service,
    Map<String, dynamic> data, {
    Map<String, dynamic>? target,
  }) async {
    await _request('call_service', {
      'domain': domain,
      'service': service,
      'service_data': data,
      if (target != null) 'target': target,
    });
  }

  /// Turn on lights in an area with brightness and color temperature.
  Future<void> turnOnArea(
    String areaId, {
    required int brightness,
    required int kelvin,
  }) async {
    await callService(
      'light',
      'turn_on',
      {
        'brightness_pct': brightness.clamp(0, 100),
        'kelvin': kelvin.clamp(1000, 10000),
      },
      target: {'area_id': areaId},
    );
  }

  /// Turn off lights in an area.
  Future<void> turnOffArea(String areaId) async {
    await callService(
      'light',
      'turn_off',
      {},
      target: {'area_id': areaId},
    );
  }

  /// Clear cached registries to force refresh.
  void clearCache() {
    _deviceRegistry = null;
    _areaRegistry = null;
    _haConfig = null;
  }

  /// Disconnect from WebSocket.
  Future<void> disconnect() async {
    _subscription?.cancel();
    _subscription = null;
    await _channel?.sink.close();
    _channel = null;
    _connectionState = WsConnectionState.disconnected;
    _pendingRequests.clear();
    clearCache();
    debugPrint('HaWebSocket: Disconnected');
  }

  /// Dispose of resources.
  Future<void> dispose() async {
    await disconnect();
    await _eventController.close();
  }
}
