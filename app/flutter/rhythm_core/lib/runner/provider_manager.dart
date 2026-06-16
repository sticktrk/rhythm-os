/// Provider manager for rhythm runner.
///
/// Manages light provider instances and room-to-provider mappings.
library;

import 'dart:convert';
import 'package:shared_preferences/shared_preferences.dart';

import '../providers/light_provider.dart';
import '../providers/ha_provider.dart';
import '../providers/hue_provider.dart';

/// Manages light provider instances and room-to-provider mappings.
///
/// Each room can be assigned to a specific provider, enabling mixed setups
/// where some rooms use Home Assistant and others use Hue directly.
class ProviderManager {
  final Map<String, LightProvider> _providers = {};
  final Map<String, String> _roomProviders = {};
  final Map<String, List<String>> _roomDevices = {};

  static const String _providerConfigKey = 'rhythm_provider_configs';
  static const String _roomMappingKey = 'rhythm_room_mappings';
  static const String _roomDevicesKey = 'rhythm_room_devices';

  /// Get all registered providers.
  Map<String, LightProvider> get providers => Map.unmodifiable(_providers);

  /// Get all room-to-provider mappings.
  Map<String, String> get roomProviders => Map.unmodifiable(_roomProviders);

  /// Get all room-to-devices mappings.
  Map<String, List<String>> get roomDevices =>
      _roomDevices.map((k, v) => MapEntry(k, List.unmodifiable(v)));

  /// Register a provider with a unique ID.
  ///
  /// If a provider with the same ID already exists, it will be replaced.
  void registerProvider(String id, LightProvider provider) {
    _providers[id] = provider;
  }

  /// Unregister a provider by ID.
  ///
  /// Also removes any room mappings that reference this provider.
  Future<void> unregisterProvider(String id) async {
    final provider = _providers.remove(id);
    await provider?.dispose();

    // Remove room mappings that reference this provider
    _roomProviders.removeWhere((_, providerId) => providerId == id);
  }

  /// Get a provider by ID.
  LightProvider? getProvider(String id) => _providers[id];

  /// Assign a room to use a specific provider.
  void assignRoomToProvider(String roomId, String providerId) {
    if (!_providers.containsKey(providerId)) {
      throw ArgumentError('Provider $providerId not registered');
    }
    _roomProviders[roomId] = providerId;
  }

  /// Unassign a room from its provider.
  void unassignRoom(String roomId) {
    _roomProviders.remove(roomId);
    _roomDevices.remove(roomId);
  }

  /// Set the devices for a room.
  void setRoomDevices(String roomId, List<String> deviceIds) {
    _roomDevices[roomId] = List.from(deviceIds);
  }

  /// Add a device to a room.
  void addDeviceToRoom(String roomId, String deviceId) {
    _roomDevices.putIfAbsent(roomId, () => []);
    if (!_roomDevices[roomId]!.contains(deviceId)) {
      _roomDevices[roomId]!.add(deviceId);
    }
  }

  /// Remove a device from a room.
  void removeDeviceFromRoom(String roomId, String deviceId) {
    _roomDevices[roomId]?.remove(deviceId);
  }

  /// Get the provider for a room.
  LightProvider? getProviderForRoom(String roomId) {
    final providerId = _roomProviders[roomId];
    return providerId != null ? _providers[providerId] : null;
  }

  /// Get the devices for a room.
  List<String> getDevicesForRoom(String roomId) {
    return List.unmodifiable(_roomDevices[roomId] ?? []);
  }

  /// Get all configured room IDs.
  List<String> get configuredRooms => _roomProviders.keys.toList();

  /// Create a provider from configuration and register it.
  Future<LightProvider> createAndRegisterProvider(ProviderConfig config) async {
    LightProvider provider;

    switch (config.type) {
      case ProviderType.homeAssistant:
        provider = HomeAssistantProvider(config as HomeAssistantConfig);
      case ProviderType.hue:
        provider = HueProvider(config as HueConfig);
    }

    registerProvider(provider.id, provider);
    return provider;
  }

  /// Save provider configurations and room mappings to persistent storage.
  Future<void> save() async {
    final prefs = await SharedPreferences.getInstance();

    // Save provider configs (we need to recreate them, so save the config)
    final providerConfigs = <Map<String, dynamic>>[];
    for (final provider in _providers.values) {
      Map<String, dynamic> config;
      switch (provider.type) {
        case ProviderType.homeAssistant:
          final ha = provider as HomeAssistantProvider;
          config = ha.config.toJson();
        case ProviderType.hue:
          final hue = provider as HueProvider;
          config = hue.config.toJson();
      }
      providerConfigs.add(config);
    }
    await prefs.setString(_providerConfigKey, jsonEncode(providerConfigs));

    // Save room mappings
    await prefs.setString(_roomMappingKey, jsonEncode(_roomProviders));

    // Save room devices
    await prefs.setString(_roomDevicesKey, jsonEncode(_roomDevices));
  }

  /// Load provider configurations and room mappings from persistent storage.
  Future<void> load() async {
    final prefs = await SharedPreferences.getInstance();

    // Load provider configs
    final configsJson = prefs.getString(_providerConfigKey);
    if (configsJson != null) {
      final List<dynamic> configs = jsonDecode(configsJson);
      for (final configMap in configs) {
        try {
          final config =
              ProviderConfig.fromJson(configMap as Map<String, dynamic>);
          await createAndRegisterProvider(config);
        } catch (e) {
          // Skip invalid configs
        }
      }
    }

    // Load room mappings
    final mappingsJson = prefs.getString(_roomMappingKey);
    if (mappingsJson != null) {
      final Map<String, dynamic> mappings = jsonDecode(mappingsJson);
      _roomProviders.clear();
      mappings.forEach((roomId, providerId) {
        if (_providers.containsKey(providerId)) {
          _roomProviders[roomId] = providerId as String;
        }
      });
    }

    // Load room devices
    final devicesJson = prefs.getString(_roomDevicesKey);
    if (devicesJson != null) {
      final Map<String, dynamic> devices = jsonDecode(devicesJson);
      _roomDevices.clear();
      devices.forEach((roomId, deviceList) {
        _roomDevices[roomId] =
            (deviceList as List<dynamic>).cast<String>().toList();
      });
    }
  }

  /// Clear all providers and mappings.
  Future<void> clear() async {
    for (final provider in _providers.values) {
      await provider.dispose();
    }
    _providers.clear();
    _roomProviders.clear();
    _roomDevices.clear();

    final prefs = await SharedPreferences.getInstance();
    await prefs.remove(_providerConfigKey);
    await prefs.remove(_roomMappingKey);
    await prefs.remove(_roomDevicesKey);
  }

  /// Dispose all providers.
  Future<void> dispose() async {
    for (final provider in _providers.values) {
      await provider.dispose();
    }
    _providers.clear();
  }
}
