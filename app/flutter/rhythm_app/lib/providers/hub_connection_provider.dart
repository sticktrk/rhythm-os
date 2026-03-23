import 'dart:async';
import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_core/providers/hue_provider.dart';

/// Type of hub connection (legacy - for backward compatibility).
enum HubConnectionType {
  none,
  homeAssistant,
  hue,
}

/// Connection status for a hub.
enum ConnectionStatus {
  disconnected,
  connecting,
  connected,
  error,
}

/// Central hub connection state management.
///
/// Tracks active hub connections and provides real-time verification.
/// Displays connection status in UI.
///
/// Note: Hub credentials are now stored in Hub model (via HomeProvider).
/// This provider manages the active connection state.
class HubConnectionProvider extends ChangeNotifier {
  // Active hub configuration
  HubConnectionType _activeHubType = HubConnectionType.none;
  ConnectionStatus _connectionStatus = ConnectionStatus.disconnected;
  String? _lastError;
  DateTime? _lastVerified;

  // Separate Hue connection status (can be checked independently)
  ConnectionStatus _hueConnectionStatus = ConnectionStatus.disconnected;
  String? _hueLastError;

  // WebSocket connection for Home Assistant
  HaWebSocketProvider? _haWebSocket;
  StreamSubscription<HaEvent>? _eventSubscription;

  // Auto-refresh timer
  Timer? _refreshTimer;
  static const _refreshInterval = Duration(seconds: 30);

  // Retry configuration
  int _retryCount = 0;
  static const maxRetries = 3;
  Timer? _retryTimer;

  // Cached hub references
  Hub? _haHub;
  Hub? _hueHub;

  // Getters
  HubConnectionType get activeHubType => _activeHubType;
  ConnectionStatus get connectionStatus => _connectionStatus;
  String? get lastError => _lastError;
  DateTime? get lastVerified => _lastVerified;

  /// Whether any hub is connected (HA or Hue).
  bool get hasActiveHub =>
      (_activeHubType == HubConnectionType.homeAssistant && _connectionStatus == ConnectionStatus.connected) ||
      _hueConnectionStatus == ConnectionStatus.connected;

  /// Whether Home Assistant is specifically connected.
  bool get isHaConnected =>
      _activeHubType == HubConnectionType.homeAssistant && _connectionStatus == ConnectionStatus.connected;

  /// Whether Hue is specifically connected.
  bool get isHueConnected => _hueConnectionStatus == ConnectionStatus.connected;

  bool get isConnected => _connectionStatus == ConnectionStatus.connected;
  bool get isConnecting => _connectionStatus == ConnectionStatus.connecting;
  HaWebSocketProvider? get haWebSocket => _haWebSocket;
  int get retryCount => _retryCount;

  // Hue-specific getters
  ConnectionStatus get hueConnectionStatus => _hueConnectionStatus;
  String? get hueLastError => _hueLastError;

  HubConnectionProvider() {
    _startAutoRefresh();
  }

  /// Configure hubs from HomeProvider data.
  ///
  /// Call this when HomeProvider has loaded hub data.
  /// Only notifies listeners if the configuration actually changed.
  void configureHubs(List<Hub> hubs) {
    final newHaHub = hubs.where((h) => h.type == HubType.homeAssistant && h.hasCredentials).firstOrNull;
    final newHueHub = hubs.where((h) => h.type == HubType.hue && h.hasCredentials).firstOrNull;

    // Check if anything changed
    final haHubChanged = newHaHub?.id != _haHub?.id;
    final hueHubChanged = newHueHub?.id != _hueHub?.id;

    if (!haHubChanged && !hueHubChanged) {
      return; // No changes, don't trigger rebuild
    }

    _haHub = newHaHub;
    _hueHub = newHueHub;

    // Set active hub type based on what's configured
    if (_haHub != null) {
      _activeHubType = HubConnectionType.homeAssistant;
    } else if (_hueHub != null) {
      _activeHubType = HubConnectionType.hue;
    } else {
      _activeHubType = HubConnectionType.none;
    }

    notifyListeners();
  }

  /// Start auto-refresh timer for connection health monitoring.
  void _startAutoRefresh() {
    _refreshTimer?.cancel();
    _refreshTimer = Timer.periodic(_refreshInterval, (_) {
      if (_connectionStatus == ConnectionStatus.connected) {
        _verifyConnectionHealth();
      }
    });
  }

  /// Verify the current connection is still healthy.
  Future<void> _verifyConnectionHealth() async {
    // Only check WebSocket health for Home Assistant connections
    // Hue connections don't use WebSocket, so skip this check for them
    if (_activeHubType == HubConnectionType.homeAssistant) {
      if (_haWebSocket == null || !_haWebSocket!.isConnected) {
        if (_connectionStatus == ConnectionStatus.connected) {
          _connectionStatus = ConnectionStatus.error;
          _lastError = 'Connection lost';
          notifyListeners();
          _startReconnect();
        }
      }
    }
  }

  /// Verify connection to any configured hub.
  ///
  /// Tries to connect to both HA and Hue if configured.
  /// Returns true if at least one connection is successful.
  Future<bool> verifyConnection() async {
    bool anyConnected = false;

    // Try HA if configured
    if (_haHub != null) {
      final haSuccess = await _verifyHaConnection();
      if (haSuccess) anyConnected = true;
    }

    // Try Hue if configured
    if (_hueHub != null) {
      final hueSuccess = await _verifyHueConnection();
      if (hueSuccess) anyConnected = true;
    }

    if (!anyConnected && _activeHubType == HubConnectionType.none) {
      _lastError = 'No hub configured';
    }

    return anyConnected;
  }

  /// Verify Home Assistant WebSocket connection.
  Future<bool> _verifyHaConnection() async {
    if (_haHub == null) {
      _connectionStatus = ConnectionStatus.error;
      _lastError = 'Home Assistant not configured';
      notifyListeners();
      return false;
    }

    _connectionStatus = ConnectionStatus.connecting;
    _lastError = null;
    _retryCount = 0;
    notifyListeners();

    try {
      // Clean up existing connection
      await _disconnectHa();

      // Create new WebSocket connection from Hub model
      final config = HomeAssistantConfig(
        host: _haHub!.endpoint.host,
        port: _haHub!.endpoint.port,
        token: _haHub!.token!,
        useSsl: _haHub!.endpoint.useSsl,
      );
      _haWebSocket = HaWebSocketProvider(config);

      final connected = await _haWebSocket!.connect();
      if (!connected) {
        _connectionStatus = ConnectionStatus.error;
        _lastError = _haWebSocket!.lastError ?? 'Connection failed';
        _haWebSocket = null;
        notifyListeners();
        return false;
      }

      // Connection successful
      _connectionStatus = ConnectionStatus.connected;
      _lastVerified = DateTime.now();
      _lastError = null;
      notifyListeners();

      debugPrint('HubConnectionProvider: Connected to Home Assistant');
      return true;
    } catch (e) {
      _connectionStatus = ConnectionStatus.error;
      _lastError = e.toString();
      notifyListeners();
      return false;
    }
  }

  /// Verify Hue connection.
  Future<bool> _verifyHueConnection() async {
    if (_hueHub == null) {
      _hueConnectionStatus = ConnectionStatus.error;
      _hueLastError = 'Hue not configured';
      notifyListeners();
      return false;
    }

    _hueConnectionStatus = ConnectionStatus.connecting;
    _hueLastError = null;
    notifyListeners();

    try {
      final connected = await HueProvider.testBridgeConnection(
        bridgeIp: _hueHub!.endpoint.host,
        username: _hueHub!.token!,
      );

      if (connected) {
        _hueConnectionStatus = ConnectionStatus.connected;
        _hueLastError = null;
        // Also update the active hub status if Hue is selected
        if (_activeHubType == HubConnectionType.hue) {
          _connectionStatus = ConnectionStatus.connected;
          _lastVerified = DateTime.now();
        }
      } else {
        _hueConnectionStatus = ConnectionStatus.error;
        _hueLastError = 'Could not connect to Hue bridge';
      }

      notifyListeners();
      return connected;
    } catch (e) {
      _hueConnectionStatus = ConnectionStatus.error;
      _hueLastError = e.toString();
      notifyListeners();
      return false;
    }
  }

  /// Public method to verify Hue connection independently.
  Future<bool> verifyHueConnection() async {
    return await _verifyHueConnection();
  }

  /// Disconnect from Home Assistant.
  Future<void> _disconnectHa() async {
    _eventSubscription?.cancel();
    _eventSubscription = null;
    await _haWebSocket?.dispose();
    _haWebSocket = null;
  }

  /// Start reconnection attempts with exponential backoff.
  void _startReconnect() {
    if (_retryCount >= maxRetries) {
      debugPrint('HubConnectionProvider: Max retries reached');
      return;
    }

    _retryTimer?.cancel();
    final delay = Duration(seconds: (2 << _retryCount).clamp(2, 30));
    _retryCount++;

    debugPrint('HubConnectionProvider: Retry $_retryCount/$maxRetries in ${delay.inSeconds}s');

    _retryTimer = Timer(delay, () async {
      final success = await verifyConnection();
      if (!success && _retryCount < maxRetries) {
        _startReconnect();
      }
    });
  }

  /// Reset retry counter and attempt immediate reconnection.
  Future<bool> retryConnection() async {
    _retryTimer?.cancel();
    _retryCount = 0;
    return await verifyConnection();
  }

  /// Disconnect from the current hub.
  Future<void> disconnect() async {
    _retryTimer?.cancel();
    _retryCount = 0;

    switch (_activeHubType) {
      case HubConnectionType.homeAssistant:
        await _disconnectHa();
        break;
      case HubConnectionType.hue:
        _hueConnectionStatus = ConnectionStatus.disconnected;
        _hueLastError = null;
        if (_activeHubType == HubConnectionType.hue) {
          _activeHubType = HubConnectionType.none;
        }
        break;
      case HubConnectionType.none:
        break;
    }

    _connectionStatus = ConnectionStatus.disconnected;
    notifyListeners();
  }

  /// Reload hub configuration (call after settings change).
  Future<void> reload() async {
    await disconnect();
    // Note: Call configureHubs() with fresh hub data from HomeProvider
  }

  @override
  void dispose() {
    _refreshTimer?.cancel();
    _retryTimer?.cancel();
    _eventSubscription?.cancel();
    _haWebSocket?.dispose();
    super.dispose();
  }
}
