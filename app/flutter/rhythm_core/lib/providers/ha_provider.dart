/// Home Assistant light provider implementation.
///
/// This provider connects to Home Assistant via its REST API
/// to control lights with adaptive lighting values.
library;

import 'dart:convert';
import 'package:http/http.dart' as http;

import 'light_provider.dart';

/// Home Assistant light provider.
///
/// Controls lights via Home Assistant's REST API.
class HomeAssistantProvider implements LightProvider {
  final HomeAssistantConfig config;
  final http.Client _client;

  @override
  ProviderType get type => ProviderType.homeAssistant;

  @override
  String get id => 'ha_${config.host}';

  @override
  String get name => 'Home Assistant (${config.host})';

  HomeAssistantProvider(this.config, {http.Client? client})
      : _client = client ?? http.Client();

  Map<String, String> get _headers => {
        'Authorization': 'Bearer ${config.token}',
        'Content-Type': 'application/json',
      };

  @override
  Future<void> turnOn(
    String entityId, {
    required int brightness,
    required int kelvin,
  }) async {
    // Home Assistant expects brightness_pct (0-100) and kelvin
    final response = await _callService('light', 'turn_on', {
      'entity_id': entityId,
      'brightness_pct': brightness.clamp(0, 100),
      'kelvin': kelvin.clamp(1000, 10000),
    });

    if (response.statusCode != 200) {
      throw LightProviderException(
        'Failed to turn on light: ${response.body}',
        code: response.statusCode.toString(),
      );
    }
  }

  @override
  Future<void> turnOff(String entityId) async {
    final response = await _callService('light', 'turn_off', {
      'entity_id': entityId,
    });

    if (response.statusCode != 200) {
      throw LightProviderException(
        'Failed to turn off light: ${response.body}',
        code: response.statusCode.toString(),
      );
    }
  }

  @override
  Future<List<LightDevice>> discoverDevices() async {
    final uri = Uri.parse('${config.baseUrl}/api/states');
    final response = await _client.get(uri, headers: _headers);

    if (response.statusCode != 200) {
      throw LightProviderException(
        'Failed to get states: ${response.body}',
        code: response.statusCode.toString(),
      );
    }

    final List<dynamic> states = jsonDecode(response.body);

    // Filter for light entities
    final lights = states
        .where((state) => (state['entity_id'] as String).startsWith('light.'))
        .map((state) {
      final entityId = state['entity_id'] as String;
      final attributes = state['attributes'] as Map<String, dynamic>? ?? {};
      final friendlyName = attributes['friendly_name'] as String? ?? entityId;

      // Check supported features
      final supportedFeatures = attributes['supported_features'] as int? ?? 0;
      // Brightness is bit 0, color temp is bit 1
      final supportsBrightness = (supportedFeatures & 1) != 0;
      final supportsColorTemp = (supportedFeatures & 2) != 0 ||
          attributes.containsKey('color_temp_kelvin') ||
          attributes.containsKey('min_color_temp_kelvin');

      return LightDevice(
        id: entityId,
        name: friendlyName,
        provider: ProviderType.homeAssistant,
        supportsColorTemp: supportsColorTemp,
        supportsBrightness: supportsBrightness,
      );
    }).toList();

    // Try to get area information
    await _enrichWithAreas(lights);

    return lights;
  }

  /// Enrich light devices with area information from Home Assistant.
  Future<void> _enrichWithAreas(List<LightDevice> lights) async {
    try {
      // Get entity registry
      final entityRegistry = await _getEntityRegistry();
      // Get area registry
      final areaRegistry = await _getAreaRegistry();

      // Build area name lookup
      final areaNames = <String, String>{};
      for (final area in areaRegistry) {
        areaNames[area['area_id'] as String] = area['name'] as String;
      }

      // Enrich devices with area info
      for (var i = 0; i < lights.length; i++) {
        final entry = entityRegistry.firstWhere(
          (e) => e['entity_id'] == lights[i].id,
          orElse: () => {},
        );

        if (entry.isNotEmpty && entry['area_id'] != null) {
          final areaId = entry['area_id'] as String;
          final areaName = areaNames[areaId];

          lights[i] = LightDevice(
            id: lights[i].id,
            name: lights[i].name,
            areaId: areaId,
            areaName: areaName,
            provider: lights[i].provider,
            supportsColorTemp: lights[i].supportsColorTemp,
            supportsBrightness: lights[i].supportsBrightness,
          );
        }
      }
    } catch (e) {
      // Area enrichment is optional, don't fail if it doesn't work
    }
  }

  Future<List<Map<String, dynamic>>> _getEntityRegistry() async {
    final uri = Uri.parse('${config.baseUrl}/api/config/entity_registry/list');
    final response = await _client.get(uri, headers: _headers);

    if (response.statusCode != 200) {
      return [];
    }

    return (jsonDecode(response.body) as List<dynamic>)
        .cast<Map<String, dynamic>>();
  }

  Future<List<Map<String, dynamic>>> _getAreaRegistry() async {
    final uri = Uri.parse('${config.baseUrl}/api/config/area_registry/list');
    final response = await _client.get(uri, headers: _headers);

    if (response.statusCode != 200) {
      return [];
    }

    return (jsonDecode(response.body) as List<dynamic>)
        .cast<Map<String, dynamic>>();
  }

  @override
  Future<bool> testConnection() async {
    try {
      final uri = Uri.parse('${config.baseUrl}/api/');
      final response = await _client.get(uri, headers: _headers);
      return response.statusCode == 200;
    } catch (e) {
      return false;
    }
  }

  Future<http.Response> _callService(
    String domain,
    String service,
    Map<String, dynamic> data,
  ) async {
    final uri = Uri.parse('${config.baseUrl}/api/services/$domain/$service');
    return await _client.post(
      uri,
      headers: _headers,
      body: jsonEncode(data),
    );
  }

  /// Get the current state of a light entity.
  Future<Map<String, dynamic>?> getLightState(String entityId) async {
    final uri = Uri.parse('${config.baseUrl}/api/states/$entityId');
    final response = await _client.get(uri, headers: _headers);

    if (response.statusCode != 200) {
      return null;
    }

    return jsonDecode(response.body) as Map<String, dynamic>;
  }

  /// Get all lights in a specific area.
  Future<List<LightDevice>> getLightsInArea(String areaId) async {
    final devices = await discoverDevices();
    return devices.where((d) => d.areaId == areaId).toList();
  }

  /// Get all available areas from Home Assistant.
  Future<List<({String id, String name})>> getAreas() async {
    final areaRegistry = await _getAreaRegistry();
    return areaRegistry
        .map((a) => (id: a['area_id'] as String, name: a['name'] as String))
        .toList();
  }

  @override
  Future<void> dispose() async {
    _client.close();
  }
}
