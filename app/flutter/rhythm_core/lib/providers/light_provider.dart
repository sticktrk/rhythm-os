/// Light provider interface and device models for rhythm lighting.
///
/// This module defines the abstract interface for controlling lights
/// through different backends (Home Assistant, Philips Hue, etc.).
library;

/// The type of light provider.
enum ProviderType {
  /// Home Assistant REST API
  homeAssistant,

  /// Philips Hue bridge
  hue,
}

/// A light device that can be controlled.
class LightDevice {
  /// Unique identifier for the device (entity_id for HA, light ID for Hue).
  final String id;

  /// Human-readable name of the device.
  final String name;

  /// Optional area/room ID the device belongs to.
  final String? areaId;

  /// Optional area/room name.
  final String? areaName;

  /// The provider type this device belongs to.
  final ProviderType provider;

  /// Whether the device supports color temperature.
  final bool supportsColorTemp;

  /// Whether the device supports brightness.
  final bool supportsBrightness;

  const LightDevice({
    required this.id,
    required this.name,
    this.areaId,
    this.areaName,
    required this.provider,
    this.supportsColorTemp = true,
    this.supportsBrightness = true,
  });

  @override
  String toString() => 'LightDevice($id: $name)';

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is LightDevice &&
          runtimeType == other.runtimeType &&
          id == other.id &&
          provider == other.provider;

  @override
  int get hashCode => id.hashCode ^ provider.hashCode;
}

/// Exception thrown when a light provider operation fails.
class LightProviderException implements Exception {
  final String message;
  final String? code;
  final Object? cause;

  const LightProviderException(this.message, {this.code, this.cause});

  @override
  String toString() => 'LightProviderException: $message';
}

/// Abstract interface for controlling lights.
///
/// Implementations should handle the specific API calls for their backend.
abstract class LightProvider {
  /// The type of this provider.
  ProviderType get type;

  /// A unique identifier for this provider instance.
  String get id;

  /// A human-readable name for this provider.
  String get name;

  /// Turn on a light with adaptive lighting values.
  ///
  /// [deviceId] - The device identifier (entity_id for HA, light ID for Hue).
  /// [brightness] - Brightness percentage (0-100).
  /// [kelvin] - Color temperature in Kelvin (typically 2000-6500).
  Future<void> turnOn(
    String deviceId, {
    required int brightness,
    required int kelvin,
  });

  /// Turn off a light.
  ///
  /// [deviceId] - The device identifier.
  Future<void> turnOff(String deviceId);

  /// Discover available light devices.
  ///
  /// Returns a list of light devices that can be controlled by this provider.
  Future<List<LightDevice>> discoverDevices();

  /// Test the connection to this provider.
  ///
  /// Returns true if the connection is successful, false otherwise.
  Future<bool> testConnection();

  /// Dispose of any resources held by this provider.
  Future<void> dispose();
}

/// Configuration for creating a light provider.
abstract class ProviderConfig {
  /// The provider type.
  ProviderType get type;

  /// Serialize the config to a JSON-compatible map.
  Map<String, dynamic> toJson();

  /// Create a config from a JSON-compatible map.
  static ProviderConfig fromJson(Map<String, dynamic> json) {
    final type = ProviderType.values.firstWhere(
      (t) => t.name == json['type'],
      orElse: () => throw ArgumentError('Unknown provider type: ${json['type']}'),
    );

    switch (type) {
      case ProviderType.homeAssistant:
        return HomeAssistantConfig.fromJson(json);
      case ProviderType.hue:
        return HueConfig.fromJson(json);
    }
  }
}

/// Configuration for Home Assistant provider.
class HomeAssistantConfig implements ProviderConfig {
  @override
  ProviderType get type => ProviderType.homeAssistant;

  /// The Home Assistant host (IP or hostname).
  final String host;

  /// The port number (default: 8123).
  final int port;

  /// Long-lived access token for authentication.
  final String token;

  /// Whether to use SSL/TLS.
  final bool useSsl;

  const HomeAssistantConfig({
    required this.host,
    this.port = 8123,
    required this.token,
    this.useSsl = false,
  });

  String get baseUrl {
    final protocol = useSsl ? 'https' : 'http';
    return '$protocol://$host:$port';
  }

  @override
  Map<String, dynamic> toJson() => {
        'type': type.name,
        'host': host,
        'port': port,
        'token': token,
        'useSsl': useSsl,
      };

  factory HomeAssistantConfig.fromJson(Map<String, dynamic> json) {
    return HomeAssistantConfig(
      host: json['host'] as String,
      port: json['port'] as int? ?? 8123,
      token: json['token'] as String,
      useSsl: json['useSsl'] as bool? ?? false,
    );
  }
}

/// Configuration for Philips Hue provider.
class HueConfig implements ProviderConfig {
  @override
  ProviderType get type => ProviderType.hue;

  /// The Hue bridge IP address.
  final String bridgeIp;

  /// The username/application key for authentication.
  final String username;

  const HueConfig({
    required this.bridgeIp,
    required this.username,
  });

  String get baseUrl => 'https://$bridgeIp/api/$username';

  @override
  Map<String, dynamic> toJson() => {
        'type': type.name,
        'bridgeIp': bridgeIp,
        'username': username,
      };

  factory HueConfig.fromJson(Map<String, dynamic> json) {
    return HueConfig(
      bridgeIp: json['bridgeIp'] as String,
      username: json['username'] as String,
    );
  }
}
