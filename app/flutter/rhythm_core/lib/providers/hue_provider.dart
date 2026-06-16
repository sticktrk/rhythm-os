/// Philips Hue light provider implementation.
///
/// This provider connects to Philips Hue bridges via their REST API
/// to control lights with adaptive lighting values.
library;

import 'dart:convert';
import 'dart:io';
import 'package:http/http.dart' as http;
import 'package:http/io_client.dart';
import 'package:multicast_dns/multicast_dns.dart';

import 'light_provider.dart';

/// Philips Hue light provider.
///
/// Controls lights via the Hue bridge REST API.
/// Uses CIE xy color space for color temperature control.
class HueProvider implements LightProvider {
  final HueConfig config;
  final http.Client _client;

  @override
  ProviderType get type => ProviderType.hue;

  @override
  String get id => 'hue_${config.bridgeIp}';

  @override
  String get name => 'Philips Hue (${config.bridgeIp})';

  HueProvider(this.config, {http.Client? client})
      : _client = client ?? _createSecureClient();

  /// Create an HTTP client that accepts self-signed certificates.
  /// Hue bridges use self-signed certs for HTTPS.
  static http.Client _createSecureClient() {
    final httpClient = HttpClient()
      ..badCertificateCallback = (cert, host, port) => true;
    return IOClient(httpClient);
  }

  @override
  Future<void> turnOn(
    String lightId, {
    required int brightness,
    required int kelvin,
  }) async {
    // Convert Kelvin to CIE xy color coordinates
    final xy = _kelvinToXy(kelvin);

    // Hue uses 0-254 for brightness
    final hueBrightness = ((brightness / 100.0) * 254).round().clamp(1, 254);

    final uri = Uri.parse('${config.baseUrl}/lights/$lightId/state');
    final response = await _client.put(
      uri,
      body: jsonEncode({
        'on': true,
        'bri': hueBrightness,
        'xy': [xy.$1, xy.$2],
      }),
    );

    if (response.statusCode != 200) {
      throw LightProviderException(
        'Failed to turn on light: ${response.body}',
        code: response.statusCode.toString(),
      );
    }

    // Check for errors in the Hue response
    final body = jsonDecode(response.body) as List<dynamic>;
    for (final item in body) {
      if (item is Map && item.containsKey('error')) {
        throw LightProviderException(
          'Hue error: ${item['error']['description']}',
          code: item['error']['type'].toString(),
        );
      }
    }
  }

  @override
  Future<void> turnOff(String lightId) async {
    final uri = Uri.parse('${config.baseUrl}/lights/$lightId/state');
    final response = await _client.put(
      uri,
      body: jsonEncode({'on': false}),
    );

    if (response.statusCode != 200) {
      throw LightProviderException(
        'Failed to turn off light: ${response.body}',
        code: response.statusCode.toString(),
      );
    }
  }

  @override
  Future<List<LightDevice>> discoverDevices() async {
    final uri = Uri.parse('${config.baseUrl}/lights');
    final response = await _client.get(uri);

    if (response.statusCode != 200) {
      throw LightProviderException(
        'Failed to get lights: ${response.body}',
        code: response.statusCode.toString(),
      );
    }

    final Map<String, dynamic> lights = jsonDecode(response.body);

    // Also get groups/rooms for area information
    final rooms = await _getRooms();

    return lights.entries.map((entry) {
      final lightId = entry.key;
      final lightData = entry.value as Map<String, dynamic>;
      final name = lightData['name'] as String? ?? 'Light $lightId';

      // Check capabilities
      final type = lightData['type'] as String? ?? '';
      final capabilities = lightData['capabilities'] as Map<String, dynamic>? ?? {};
      final control = capabilities['control'] as Map<String, dynamic>? ?? {};

      // Extended color lights support xy, color temp lights support ct
      final supportsColorTemp = type.contains('Color') ||
          type.contains('color temperature') ||
          control.containsKey('ct');

      // Find which room this light belongs to
      String? roomId;
      String? roomName;
      for (final room in rooms.entries) {
        final roomData = room.value as Map<String, dynamic>;
        final roomLights = roomData['lights'] as List<dynamic>? ?? [];
        if (roomLights.contains(lightId)) {
          roomId = room.key;
          roomName = roomData['name'] as String?;
          break;
        }
      }

      return LightDevice(
        id: lightId,
        name: name,
        areaId: roomId,
        areaName: roomName,
        provider: ProviderType.hue,
        supportsColorTemp: supportsColorTemp,
        supportsBrightness: true, // All Hue lights support brightness
      );
    }).toList();
  }

  Future<Map<String, dynamic>> _getRooms() async {
    final uri = Uri.parse('${config.baseUrl}/groups');
    final response = await _client.get(uri);

    if (response.statusCode != 200) {
      return {};
    }

    final Map<String, dynamic> groups = jsonDecode(response.body);

    // Filter for Room type groups
    return Map.fromEntries(
      groups.entries.where((e) {
        final data = e.value as Map<String, dynamic>;
        return data['type'] == 'Room';
      }),
    );
  }

  @override
  Future<bool> testConnection() async {
    try {
      final uri = Uri.parse('${config.baseUrl}/config');
      final response = await _client.get(uri);
      return response.statusCode == 200;
    } catch (e) {
      return false;
    }
  }

  /// Test connection to a Hue bridge using stored credentials.
  ///
  /// This is a static method that creates a temporary client with
  /// self-signed certificate support for HTTPS connections.
  static Future<bool> testBridgeConnection({
    required String bridgeIp,
    required String username,
  }) async {
    print('[HueTest] Testing connection to $bridgeIp');

    // Create an HTTP client that accepts self-signed certificates
    final httpClient = HttpClient()
      ..badCertificateCallback = (cert, host, port) => true;
    final client = IOClient(httpClient);

    try {
      final uri = Uri.parse('https://$bridgeIp/api/$username/config');
      print('[HueTest] GET $uri');

      final response = await client.get(uri);
      print('[HueTest] Status: ${response.statusCode}');

      if (response.statusCode == 200) {
        // Verify we got valid JSON (not an error page)
        try {
          final data = jsonDecode(response.body);
          // Check for a known field in the config response
          if (data is Map && data.containsKey('name')) {
            print('[HueTest] Connection successful, bridge name: ${data['name']}');
            return true;
          }
        } catch (e) {
          print('[HueTest] Invalid JSON response');
        }
      }
      return false;
    } catch (e) {
      print('[HueTest] Exception: $e');
      return false;
    } finally {
      client.close();
    }
  }

  /// Get the current state of a light.
  Future<Map<String, dynamic>?> getLightState(String lightId) async {
    final uri = Uri.parse('${config.baseUrl}/lights/$lightId');
    final response = await _client.get(uri);

    if (response.statusCode != 200) {
      return null;
    }

    return jsonDecode(response.body) as Map<String, dynamic>;
  }

  /// Get all lights in a specific room.
  Future<List<LightDevice>> getLightsInRoom(String roomId) async {
    final devices = await discoverDevices();
    return devices.where((d) => d.areaId == roomId).toList();
  }

  /// Get all rooms from the Hue bridge.
  Future<List<({String id, String name})>> getRooms() async {
    final rooms = await _getRooms();
    return rooms.entries
        .map((e) => (
              id: e.key,
              name: (e.value as Map<String, dynamic>)['name'] as String
            ))
        .toList();
  }

  @override
  Future<void> dispose() async {
    _client.close();
  }

  /// Convert Kelvin color temperature to CIE xy coordinates.
  ///
  /// This uses the algorithm from the Philips Hue documentation.
  (double, double) _kelvinToXy(int kelvin) {
    // Clamp to reasonable range
    final k = kelvin.clamp(2000, 6500).toDouble();

    // Calculate x using the algorithm from Philips
    double x;
    if (k >= 4000) {
      x = -0.2661239e9 / (k * k * k) -
          0.2343589e6 / (k * k) +
          0.8776956e3 / k +
          0.179910;
    } else {
      x = -0.4743006e9 / (k * k * k) -
          0.5852078e6 / (k * k) +
          0.9153307e3 / k +
          0.137964;
    }

    // Calculate y
    double y;
    if (k >= 4000) {
      y = 3.0817580 * x * x * x -
          5.8733867 * x * x +
          3.7514764 * x -
          0.3700098;
    } else if (k >= 2222) {
      y = -0.9549476 * x * x * x +
          2.0266423 * x * x -
          0.2755856 * x -
          0.0182259;
    } else {
      y = -1.1063814 * x * x * x +
          2.3520033 * x * x -
          0.3927534 * x -
          0.0041788;
    }

    // Clamp to valid range
    return (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0));
  }

  /// Discover Hue bridges on the local network using mDNS.
  ///
  /// Returns a list of bridge IP addresses found.
  static Future<List<String>> discoverBridges({
    Duration timeout = const Duration(seconds: 5),
  }) async {
    final bridges = <String>[];

    try {
      final client = MDnsClient();
      await client.start();

      await for (final ptr in client.lookup<PtrResourceRecord>(
        ResourceRecordQuery.serverPointer('_hue._tcp.local'),
      ).timeout(timeout, onTimeout: (sink) => sink.close())) {
        await for (final srv in client.lookup<SrvResourceRecord>(
          ResourceRecordQuery.service(ptr.domainName),
        )) {
          await for (final ip in client.lookup<IPAddressResourceRecord>(
            ResourceRecordQuery.addressIPv4(srv.target),
          )) {
            bridges.add(ip.address.address);
          }
        }
      }

      client.stop();
    } catch (e) {
      // mDNS discovery failed, try alternative method
    }

    // Also try the Philips discovery endpoint as fallback
    if (bridges.isEmpty) {
      try {
        final response = await http.get(
          Uri.parse('https://discovery.meethue.com'),
        );
        if (response.statusCode == 200) {
          final List<dynamic> discovered = jsonDecode(response.body);
          for (final bridge in discovered) {
            final ip = bridge['internalipaddress'] as String?;
            if (ip != null && !bridges.contains(ip)) {
              bridges.add(ip);
            }
          }
        }
      } catch (e) {
        // Discovery endpoint failed
      }
    }

    return bridges;
  }

  /// Start the link button pairing process with a Hue bridge.
  ///
  /// Call this after the user presses the link button on the bridge.
  /// Returns the username/application key if successful.
  static Future<String?> pair(
    String bridgeIp, {
    // Keep 'rhythm_lighting' for backwards compat — changing breaks existing Hue pairings.
    String appName = 'rhythm_lighting',
    String deviceName = 'mobile_app',
  }) async {
    print('[HuePair] Attempting to pair with bridge at $bridgeIp');

    // Create an HTTP client that accepts self-signed certificates
    // (Hue bridges use self-signed certs for HTTPS)
    final httpClient = HttpClient()
      ..badCertificateCallback = (cert, host, port) => true;
    final client = IOClient(httpClient);

    try {
      // Modern Hue bridges require HTTPS
      final uri = Uri.parse('https://$bridgeIp/api');
      print('[HuePair] POST $uri');

      final response = await client.post(
        uri,
        headers: {'Content-Type': 'application/json'},
        body: jsonEncode({
          'devicetype': '$appName#$deviceName',
          'generateclientkey': true,
        }),
      );

      print('[HuePair] Status: ${response.statusCode}');
      print('[HuePair] Body: ${response.body}');

      if (response.statusCode != 200) {
        print('[HuePair] Non-200 status, returning null');
        return null;
      }

      final List<dynamic> result = jsonDecode(response.body);
      print('[HuePair] Parsed result: $result');

      if (result.isEmpty) {
        print('[HuePair] Empty result, returning null');
        return null;
      }

      final first = result[0] as Map<String, dynamic>;

      // Check for error (link button not pressed)
      if (first.containsKey('error')) {
        final error = first['error'];
        print('[HuePair] Error response: type=${error['type']}, description=${error['description']}');
        return null;
      }

      // Success - return the username
      if (first.containsKey('success')) {
        final username = first['success']['username'] as String?;
        print('[HuePair] Success! Username: $username');
        return username;
      }

      print('[HuePair] Unexpected response format, returning null');
      return null;
    } catch (e, stack) {
      print('[HuePair] Exception: $e');
      print('[HuePair] Stack: $stack');
      return null;
    } finally {
      client.close();
    }
  }
}
