/// Hue device registry for mapping switches to rooms.
///
/// Queries the Hue V2 API to discover button devices and their
/// associated rooms, enabling automatic room mapping for SSE events.
library;

import 'dart:convert';
import 'dart:io';

// Import the generated Rust bindings for behavior tracking
import '../src/rust/api/hue_registry.dart' as rust_hue;
import '../src/rust/api/dto/hue_registry.dart' show HueBehaviorTrackerDto;
import 'behavior_tracker_json.dart' as tracker_json;

/// A Hue button resource from the V2 API.
class HueButton {
  /// V2 resource ID of the button.
  final String id;

  /// Control ID (button index, 1-4 for dimmer switch).
  final int controlId;

  /// Owner device resource ID.
  final String ownerDeviceId;

  const HueButton({
    required this.id,
    required this.controlId,
    required this.ownerDeviceId,
  });
}

/// A Hue switch/button device discovered from the V2 API.
class HueSwitchDevice {
  /// V2 resource ID of the device.
  final String id;

  /// Human-readable name.
  final String name;

  /// Product name/model.
  final String? productName;

  /// List of button service IDs belonging to this device.
  final List<String> buttonServiceIds;

  /// V2 resource ID of the room this device is assigned to.
  final String? roomId;

  const HueSwitchDevice({
    required this.id,
    required this.name,
    this.productName,
    required this.buttonServiceIds,
    this.roomId,
  });

  @override
  String toString() =>
      'HueSwitchDevice($name, room: $roomId, buttons: ${buttonServiceIds.length})';
}

/// A Hue motion sensor resource from the V2 API.
class HueMotionSensor {
  /// V2 resource ID of the motion service.
  final String id;

  /// Owner device resource ID.
  final String ownerDeviceId;

  /// V2 resource ID of the room this sensor is in.
  final String? roomId;

  const HueMotionSensor({
    required this.id,
    required this.ownerDeviceId,
    this.roomId,
  });
}

/// A room from the Hue V2 API.
class HueRoom {
  /// V2 resource ID.
  final String id;

  /// Human-readable name.
  final String name;

  const HueRoom({
    required this.id,
    required this.name,
  });

  @override
  String toString() => 'HueRoom($name)';
}

/// Registry for Hue devices and room mappings.
///
/// Provides device discovery via the V2 API and maintains a mapping
/// from device IDs to Rhythm room IDs.
///
/// Also tracks which devices are configured in the Hue app via behavior_instance
/// resources, so we can avoid processing events for switches that Hue handles.
///
/// Usage:
/// ```dart
/// final registry = HueDeviceRegistry(
///   bridgeIp: '192.168.1.100',
///   applicationKey: 'your-app-key',
/// );
///
/// await registry.discover();
/// await registry.fetchBehaviorInstances();
///
/// // Check if a device is configured in Hue app
/// if (!registry.isDeviceConfigured('device-uuid')) {
///   // Process with Rhythm
/// }
///
/// // Configure mapping from Hue room to Rhythm room
/// registry.setRoomMapping('hue-room-uuid', 'rhythm-living-room');
///
/// // Get room for device
/// final roomId = registry.getRoomForDevice('device-uuid');
/// ```
class HueDeviceRegistry {
  final String bridgeIp;
  final String applicationKey;
  final bool acceptSelfSignedCerts;

  /// All discovered switch devices, keyed by device ID.
  final Map<String, HueSwitchDevice> _devices = {};

  /// All discovered buttons, keyed by button resource ID.
  final Map<String, HueButton> _buttons = {};

  /// All discovered rooms, keyed by room ID.
  final Map<String, HueRoom> _rooms = {};

  /// All discovered motion sensors, keyed by motion service ID.
  final Map<String, HueMotionSensor> _motionSensors = {};

  /// Mapping from Hue room ID to Rhythm room ID.
  final Map<String, String> _roomMappings = {};

  /// Rust-based behavior tracker for device configuration status.
  /// Replaces the previous Dart-based _behaviorToDevice and _configuredDeviceIds.
  HueBehaviorTrackerDto _behaviorTracker = rust_hue.createBehaviorTracker();

  HueDeviceRegistry({
    required this.bridgeIp,
    required this.applicationKey,
    this.acceptSelfSignedCerts = true,
  });

  /// Get all discovered switch devices.
  Iterable<HueSwitchDevice> get devices => _devices.values;

  /// Get all discovered buttons.
  Iterable<HueButton> get buttons => _buttons.values;

  /// Get all discovered Hue rooms.
  Iterable<HueRoom> get rooms => _rooms.values;

  /// Get all discovered motion sensors.
  Iterable<HueMotionSensor> get motionSensors => _motionSensors.values;

  /// Get device by ID.
  HueSwitchDevice? getDevice(String deviceId) => _devices[deviceId];

  /// Get button by resource ID.
  HueButton? getButton(String buttonId) => _buttons[buttonId];

  /// Get Hue room by ID.
  HueRoom? getHueRoom(String roomId) => _rooms[roomId];

  /// Check if a device is configured in the Hue app.
  ///
  /// Returns true if a behavior_instance exists for this device,
  /// meaning Hue will handle events from this device.
  bool isDeviceConfigured(String deviceId) {
    return rust_hue.behaviorTrackerIsConfigured(
      tracker: _behaviorTracker,
      deviceId: deviceId,
    );
  }

  /// Get all configured device IDs (for debugging).
  Set<String> get configuredDeviceIds {
    final ids = rust_hue.behaviorTrackerConfiguredDevices(
      tracker: _behaviorTracker,
    );
    return Set.from(ids);
  }

  /// Get the Rhythm room ID for a device.
  ///
  /// First looks up the device to find its Hue room,
  /// then uses the room mapping to get the Rhythm room ID.
  String? getRoomForDevice(String deviceId) {
    final device = _devices[deviceId];
    if (device == null || device.roomId == null) return null;

    return _roomMappings[device.roomId];
  }

  /// Set a mapping from Hue room ID to Rhythm room ID.
  void setRoomMapping(String hueRoomId, String rhythmRoomId) {
    _roomMappings[hueRoomId] = rhythmRoomId;
  }

  /// Remove a room mapping.
  void removeRoomMapping(String hueRoomId) {
    _roomMappings.remove(hueRoomId);
  }

  /// Get all room mappings.
  Map<String, String> get roomMappings => Map.unmodifiable(_roomMappings);

  /// Load room mappings from a saved state.
  void loadRoomMappings(Map<String, String> mappings) {
    _roomMappings.clear();
    _roomMappings.addAll(mappings);
  }

  /// Discover devices and rooms from the Hue bridge.
  ///
  /// This queries the V2 API to find all button devices and rooms,
  /// building the mapping infrastructure.
  Future<void> discover() async {
    final client = HttpClient();
    if (acceptSelfSignedCerts) {
      client.badCertificateCallback = (cert, host, port) => true;
    }

    try {
      // Fetch devices, rooms, buttons, and motion sensors in parallel
      final results = await Future.wait([
        _fetchResource(client, 'device'),
        _fetchResource(client, 'room'),
        _fetchResource(client, 'button'),
        _fetchResource(client, 'motion'),
      ]);

      final deviceData = results[0];
      final roomData = results[1];
      final buttonData = results[2];
      final motionData = results[3];

      // Parse rooms first
      _rooms.clear();
      for (final room in roomData) {
        final id = room['id'] as String?;
        final metadata = room['metadata'] as Map<String, dynamic>?;
        final name = metadata?['name'] as String?;

        if (id != null && name != null) {
          _rooms[id] = HueRoom(id: id, name: name);
        }
      }

      // Build button-to-device mapping and store button metadata
      _buttons.clear();
      final buttonToDevice = <String, String>{};
      for (final button in buttonData) {
        final buttonId = button['id'] as String?;
        final owner = button['owner'] as Map<String, dynamic>?;
        final ownerRid = owner?['rid'] as String?;
        final metadata = button['metadata'] as Map<String, dynamic>?;
        final controlId = metadata?['control_id'] as int?;

        if (buttonId != null && ownerRid != null) {
          buttonToDevice[buttonId] = ownerRid;

          // Store button with control_id for SSE event mapping
          if (controlId != null) {
            _buttons[buttonId] = HueButton(
              id: buttonId,
              controlId: controlId,
              ownerDeviceId: ownerRid,
            );
          }
        }
      }

      // Parse devices - find ones with button services
      _devices.clear();
      for (final device in deviceData) {
        final id = device['id'] as String?;
        final metadata = device['metadata'] as Map<String, dynamic>?;
        final name = metadata?['name'] as String?;
        final productData = device['product_data'] as Map<String, dynamic>?;
        final productName = productData?['product_name'] as String?;

        if (id == null || name == null) continue;

        // Get button services for this device
        final services = device['services'] as List<dynamic>? ?? [];
        final buttonIds = <String>[];
        String? roomId;

        for (final service in services) {
          if (service is! Map<String, dynamic>) continue;

          final rtype = service['rtype'] as String?;
          final rid = service['rid'] as String?;
          if (rid == null) continue;

          if (rtype == 'button') {
            buttonIds.add(rid);
          } else if (rtype == 'device_power' || rtype == 'zigbee_connectivity') {
            // These don't help us find the room
          }
        }

        // Only care about devices with buttons
        if (buttonIds.isEmpty) continue;

        // Find room for this device by checking grouped_light in rooms
        roomId = _findRoomForDevice(id, roomData);

        _devices[id] = HueSwitchDevice(
          id: id,
          name: name,
          productName: productName,
          buttonServiceIds: buttonIds,
          roomId: roomId,
        );
      }

      // Parse motion sensors
      _motionSensors.clear();
      for (final motion in motionData) {
        final motionId = motion['id'] as String?;
        final owner = motion['owner'] as Map<String, dynamic>?;
        final ownerRid = owner?['rid'] as String?;
        if (motionId == null || ownerRid == null) continue;

        // Find room for the owner device
        final roomId = _findRoomForDevice(ownerRid, roomData);

        _motionSensors[motionId] = HueMotionSensor(
          id: motionId,
          ownerDeviceId: ownerRid,
          roomId: roomId,
        );
      }
    } finally {
      client.close();
    }
  }

  /// Fetch behavior_instance resources to determine which devices are configured in Hue.
  ///
  /// A device with a behavior_instance is configured in the Hue app and
  /// Rhythm should not process its button events.
  Future<void> fetchBehaviorInstances() async {
    final client = HttpClient();
    if (acceptSelfSignedCerts) {
      client.badCertificateCallback = (cert, host, port) => true;
    }

    try {
      final behaviorData = await _fetchResource(client, 'behavior_instance');

      // Clear and rebuild the tracker
      _behaviorTracker = rust_hue.behaviorTrackerClear(tracker: _behaviorTracker);

      for (final behavior in behaviorData) {
        final behaviorId = behavior['id'] as String?;
        final config = behavior['configuration'] as Map<String, dynamic>?;
        final device = config?['device'] as Map<String, dynamic>?;
        final deviceId = device?['rid'] as String?;

        if (behaviorId != null && deviceId != null) {
          _behaviorTracker = rust_hue.behaviorTrackerAdd(
            tracker: _behaviorTracker,
            behaviorId: behaviorId,
            deviceId: deviceId,
          );
        }
      }
    } finally {
      client.close();
    }
  }

  /// Handle a behavior_instance event from SSE.
  ///
  /// Call this when the SSE stream receives behavior_instance events
  /// to keep the configured device set up to date.
  ///
  /// [eventType] is "add", "update", or "delete".
  /// [data] is the behavior_instance resource data.
  void handleBehaviorInstanceEvent(String eventType, Map<String, dynamic> data) {
    final behaviorId = data['id'] as String?;
    if (behaviorId == null) return;

    switch (eventType) {
      case 'add':
      case 'update':
        // Extract device ID from configuration
        final config = data['configuration'] as Map<String, dynamic>?;
        final device = config?['device'] as Map<String, dynamic>?;
        final deviceId = device?['rid'] as String?;

        if (deviceId != null) {
          _behaviorTracker = rust_hue.behaviorTrackerAdd(
            tracker: _behaviorTracker,
            behaviorId: behaviorId,
            deviceId: deviceId,
          );
        }
        break;

      case 'delete':
        final result = rust_hue.behaviorTrackerRemove(
          tracker: _behaviorTracker,
          behaviorId: behaviorId,
        );
        _behaviorTracker = result.tracker;
        // result.deviceUnconfigured indicates if device is now available for Rhythm
        break;
    }
  }

  /// Find which room contains a device.
  String? _findRoomForDevice(String deviceId, List<dynamic> roomData) {
    for (final room in roomData) {
      final roomId = room['id'] as String?;
      if (roomId == null) continue;

      final children = room['children'] as List<dynamic>? ?? [];
      for (final child in children) {
        if (child is! Map<String, dynamic>) continue;

        final rtype = child['rtype'] as String?;
        final rid = child['rid'] as String?;

        if (rtype == 'device' && rid == deviceId) {
          return roomId;
        }
      }
    }
    return null;
  }

  Future<List<dynamic>> _fetchResource(
    HttpClient client,
    String resourceType,
  ) async {
    final uri = Uri.parse(
      'https://$bridgeIp/clip/v2/resource/$resourceType',
    );

    final request = await client.getUrl(uri);
    request.headers.set('hue-application-key', applicationKey);

    final response = await request.close();

    if (response.statusCode != 200) {
      throw HttpException(
        'Failed to fetch $resourceType: ${response.statusCode}',
        uri: uri,
      );
    }

    final body = await response.transform(utf8.decoder).join();
    final json = jsonDecode(body) as Map<String, dynamic>;

    return json['data'] as List<dynamic>? ?? [];
  }

  /// Serialize registry state for persistence.
  Map<String, dynamic> toJson() {
    // Serialize behavior tracker using Dart JSON
    final behaviorTrackerJson = tracker_json.behaviorTrackerToJson(
      _behaviorTracker,
    );

    return {
      'roomMappings': _roomMappings,
      'devices': _devices.values
          .map((d) => <String, dynamic>{
                'id': d.id,
                'name': d.name,
                'productName': d.productName,
                'buttonServiceIds': d.buttonServiceIds,
                'roomId': d.roomId,
              })
          .toList(),
      'rooms': _rooms.values
          .map((r) => <String, dynamic>{
                'id': r.id,
                'name': r.name,
              })
          .toList(),
      'motionSensors': _motionSensors.values
          .map((m) => <String, dynamic>{
                'id': m.id,
                'ownerDeviceId': m.ownerDeviceId,
                'roomId': m.roomId,
              })
          .toList(),
      'behaviorTracker': behaviorTrackerJson,
    };
  }

  /// Load registry state from persisted JSON.
  void loadFromJson(Map<String, dynamic> json) {
    // Load room mappings
    final mappings = json['roomMappings'] as Map<String, dynamic>?;
    if (mappings != null) {
      _roomMappings.clear();
      mappings.forEach((key, value) {
        if (value is String) {
          _roomMappings[key] = value;
        }
      });
    }

    // Load cached devices
    final devices = json['devices'] as List<dynamic>?;
    if (devices != null) {
      _devices.clear();
      for (final d in devices) {
        if (d is! Map<String, dynamic>) continue;

        final id = d['id'] as String?;
        final name = d['name'] as String?;
        if (id == null || name == null) continue;

        _devices[id] = HueSwitchDevice(
          id: id,
          name: name,
          productName: d['productName'] as String?,
          buttonServiceIds: (d['buttonServiceIds'] as List<dynamic>?)
                  ?.cast<String>() ??
              [],
          roomId: d['roomId'] as String?,
        );
      }
    }

    // Load cached rooms
    final rooms = json['rooms'] as List<dynamic>?;
    if (rooms != null) {
      _rooms.clear();
      for (final r in rooms) {
        if (r is! Map<String, dynamic>) continue;

        final id = r['id'] as String?;
        final name = r['name'] as String?;
        if (id == null || name == null) continue;

        _rooms[id] = HueRoom(id: id, name: name);
      }
    }

    // Load cached motion sensors
    final motionSensors = json['motionSensors'] as List<dynamic>?;
    if (motionSensors != null) {
      _motionSensors.clear();
      for (final m in motionSensors) {
        if (m is! Map<String, dynamic>) continue;

        final id = m['id'] as String?;
        final ownerDeviceId = m['ownerDeviceId'] as String?;
        if (id == null || ownerDeviceId == null) continue;

        _motionSensors[id] = HueMotionSensor(
          id: id,
          ownerDeviceId: ownerDeviceId,
          roomId: m['roomId'] as String?,
        );
      }
    }

    // Load behavior tracker from persisted JSON
    final behaviorTrackerJson = json['behaviorTracker'] as String?;
    if (behaviorTrackerJson != null) {
      final tracker = tracker_json.behaviorTrackerFromJson(behaviorTrackerJson);
      if (tracker != null) {
        _behaviorTracker = tracker;
      }
    } else {
      // Handle legacy format: migrate from old behaviorToDevice/configuredDeviceIds
      final behaviorToDevice = json['behaviorToDevice'] as Map<String, dynamic>?;
      if (behaviorToDevice != null) {
        _behaviorTracker = rust_hue.createBehaviorTracker();
        behaviorToDevice.forEach((behaviorId, deviceId) {
          if (deviceId is String) {
            _behaviorTracker = rust_hue.behaviorTrackerAdd(
              tracker: _behaviorTracker,
              behaviorId: behaviorId,
              deviceId: deviceId,
            );
          }
        });
      }
    }
  }
}
