import '../json_parsing.dart';

const int _deviceProfileIdMaxLength = 80;
final RegExp _deviceProfileIdPattern = RegExp(
  r'^([a-z0-9_-]+(?:\.[a-z0-9_-]+)+)\.v([1-9][0-9]*)$',
);

class _ParsedDeviceProfileId {
  const _ParsedDeviceProfileId(this.family, this.version);

  final String family;
  final int version;
}

_ParsedDeviceProfileId? _parseDeviceProfileId(String value) {
  if (value.isEmpty || value.length > _deviceProfileIdMaxLength) return null;
  final match = _deviceProfileIdPattern.firstMatch(value);
  if (match == null) return null;
  final version = int.tryParse(match.group(2)!);
  if (version == null || version > 0xffffffff) return null;
  return _ParsedDeviceProfileId(match.group(1)!, version);
}

/// Stable hub/device onboarding method ids from `/api/state.capabilities.hubs`.
abstract final class RhythmDeviceOnboardingMethod {
  static const String matterOnNetworkSetupCode = 'matter_on_network_setup_code';
  static const String matterBleWifiCommissioning =
      'matter_ble_wifi_commissioning';
  static const String hueBleNearbyScan = 'hue_ble_nearby_scan';
  static const String localBleQr = 'local_ble_qr';
  static const String hueBridgeSerialSearch = 'hue_bridge_serial_search';
  static const String hueBridgeButtonSearch = 'hue_bridge_button_search';
}

/// Stable local-device profile IDs advertised by a Rhythm appliance.
abstract final class RhythmDeviceProfileId {
  static const String oreinOc02001Button = 'orein.oc02001.button.v1';
}

/// Stable feature IDs advertised by `/api/state.capabilities.features`.
abstract final class RhythmFeature {
  static const String asyncDebugBundleUpload = 'async_debug_bundle_upload';
  static const String motionActivationToggle = 'motion_activation_toggle';
  static const String roomScheduleV1 = 'room_schedule_v1';
  static const String lightSchedulesV1 = 'light_schedules_v1';
  static const String lightScheduleOverridesV1 = 'light_schedule_overrides_v1';
  static const String lightScheduleSolarOffsetsV1 =
      'light_schedule_solar_offsets_v1';
  static const String roomLightProfileOverrides =
      'room_light_profile_overrides';
  static const String roomDayIdleProfileOverrides =
      'room_day_idle_profile_overrides_v1';
  static const String guardedRoomLightProfileOverrides =
      'guarded_room_light_profile_overrides';
  static const String targetGuardedRoomLightProfileOverrides =
      'target_guarded_room_light_profile_overrides';
  static const String hueRoomAuthorityConsent = 'hue_room_authority_consent_v1';
  static const String matterSetupCodeRecovery = 'matter_setup_code_recovery_v1';
  static const String removedDeviceArchive = 'removed_device_archive_v1';
  static const String hueRoomTopologySync = 'hue_room_topology_sync_v1';
  static const String sceneMotionSuppression = 'scene_motion_suppression_v1';
  static const String buttonMultiRoomControls = 'button_multi_room_controls_v1';
}

/// Host capabilities advertised by the Rhythm server.
class RhythmCapabilities {
  final int? apiSchemaVersion;
  final List<String> features;
  final List<RhythmHubCapabilities> hubs;

  const RhythmCapabilities({
    this.apiSchemaVersion,
    this.features = const [],
    this.hubs = const [],
  });

  factory RhythmCapabilities.fromJson(Map<String, dynamic> json) {
    return RhythmCapabilities(
      apiSchemaVersion: jsonInt(
        json['api_schema_version'],
        preferredKeys: const ['api_schema_version'],
      ),
      features: _parseStringList(json['features']),
      hubs: ((json['hubs'] as List<dynamic>?) ?? const <dynamic>[])
          .map(jsonMap)
          .nonNulls
          .map(RhythmHubCapabilities.fromJson)
          .where((hub) => hub.type.isNotEmpty)
          .toList(),
    );
  }

  RhythmHubCapabilities? hub(String type) {
    for (final hub in hubs) {
      if (hub.type == type) return hub;
    }
    return null;
  }

  bool supportsFeature(String feature) => features.contains(feature);
}

/// Per-hub capability metadata from `/api/state.capabilities.hubs`.
class RhythmHubCapabilities {
  final String type;
  final bool configurable;
  final List<String> deviceOnboardingMethods;
  final bool supportsUnpairing;
  final List<String> unpairableDeviceTypes;
  final bool supportsRoomlessDevices;
  final bool blocksRoomReadiness;
  final List<RhythmDeviceProfile> deviceProfiles;
  final RhythmAddDeviceCapabilities addDevice;

  const RhythmHubCapabilities({
    required this.type,
    required this.configurable,
    this.deviceOnboardingMethods = const [],
    required this.supportsUnpairing,
    this.unpairableDeviceTypes = const [],
    required this.supportsRoomlessDevices,
    this.blocksRoomReadiness = true,
    this.deviceProfiles = const [],
    this.addDevice = const RhythmAddDeviceCapabilities(),
  });

  bool get canAddDevice => addDevice.isSupported;

  bool supportsDeviceOnboardingMethod(String method) {
    return deviceOnboardingMethods.contains(method);
  }

  bool supportsDeviceProfile(String profileId) {
    return deviceProfiles.any((profile) => profile.acceptsProfileId(profileId));
  }

  bool supportsUnpairingDeviceType(String deviceType) {
    return supportsUnpairing && unpairableDeviceTypes.contains(deviceType);
  }

  factory RhythmHubCapabilities.fromJson(Map<String, dynamic> json) {
    final legacyAddDevice =
        jsonMap(json['add_device']) ?? const <String, dynamic>{};
    final deviceOnboardingMethods = _parseDeviceOnboardingMethods(
      json['device_onboarding_methods'],
      legacyAddDevice: legacyAddDevice,
    );

    final type = json['type'] as String? ?? '';
    final supportsUnpairing = json['supports_unpairing'] as bool? ?? false;
    var unpairableDeviceTypes = _parseStringList(
      json['unpairable_device_types'],
    );
    // Hue Bridge appliances before typed unpair metadata only implemented
    // exact V1 light deletion. Preserve that path without exposing their
    // broken switch/sensor removal behavior.
    if (unpairableDeviceTypes.isEmpty && supportsUnpairing && type == 'hue') {
      unpairableDeviceTypes = const ['light'];
    }

    return RhythmHubCapabilities(
      type: type,
      configurable: json['configurable'] as bool? ?? false,
      deviceOnboardingMethods: deviceOnboardingMethods,
      supportsUnpairing: supportsUnpairing,
      unpairableDeviceTypes: unpairableDeviceTypes,
      supportsRoomlessDevices:
          json['supports_roomless_devices'] as bool? ?? false,
      blocksRoomReadiness: json['blocks_room_readiness'] as bool? ?? true,
      deviceProfiles: _parseDeviceProfiles(json['device_profiles']),
      addDevice: RhythmAddDeviceCapabilities.fromJson(
        legacyAddDevice,
        deviceOnboardingMethods: deviceOnboardingMethods,
      ),
    );
  }
}

/// A bounded, non-secret device profile descriptor advertised by a hub.
class RhythmDeviceProfile {
  final String id;

  /// Older parser/storage IDs explicitly accepted by this current profile.
  final List<String> compatibleProfileIds;
  final String deviceType;
  final String displayName;
  final bool inputOnly;
  final List<String> onboardingMethods;

  const RhythmDeviceProfile({
    required this.id,
    this.compatibleProfileIds = const [],
    required this.deviceType,
    required this.displayName,
    required this.inputOnly,
    this.onboardingMethods = const [],
  });

  bool supportsOnboardingMethod(String method) {
    return onboardingMethods.contains(method);
  }

  bool acceptsProfileId(String profileId) {
    return id == profileId || compatibleProfileIds.contains(profileId);
  }

  factory RhythmDeviceProfile.fromJson(Map<String, dynamic> json) {
    final rawId = json['id'];
    final id = rawId is String ? rawId : '';
    final currentId = _parseDeviceProfileId(id);
    final compatibleProfileIds = <String>[];
    if (currentId != null) {
      for (final compatibleId in _parseStringList(
        json['compatible_profile_ids'],
      )) {
        final parsed = _parseDeviceProfileId(compatibleId);
        if (parsed != null &&
            parsed.family == currentId.family &&
            parsed.version < currentId.version &&
            !compatibleProfileIds.contains(compatibleId)) {
          compatibleProfileIds.add(compatibleId);
        }
      }
    }
    return RhythmDeviceProfile(
      id: id,
      compatibleProfileIds: compatibleProfileIds,
      deviceType:
          json['device_type'] is String ? json['device_type'] as String : '',
      displayName:
          json['display_name'] is String ? json['display_name'] as String : '',
      inputOnly:
          json['input_only'] is bool ? json['input_only'] as bool : false,
      onboardingMethods: _parseStringList(json['onboarding_methods']),
    );
  }
}

/// Explicit add-device methods for a host-managed hub like Matter.
class RhythmAddDeviceCapabilities {
  final bool onNetworkSetupCode;
  final bool bleWifiCommissioning;

  const RhythmAddDeviceCapabilities({
    this.onNetworkSetupCode = false,
    this.bleWifiCommissioning = false,
  });

  bool get isSupported => onNetworkSetupCode || bleWifiCommissioning;

  factory RhythmAddDeviceCapabilities.fromJson(
    Map<String, dynamic> json, {
    Iterable<String> deviceOnboardingMethods = const [],
  }) {
    final onboardingMethodSet = deviceOnboardingMethods.toSet();
    return RhythmAddDeviceCapabilities(
      onNetworkSetupCode: onboardingMethodSet.contains(
            RhythmDeviceOnboardingMethod.matterOnNetworkSetupCode,
          ) ||
          (json['on_network_setup_code'] as bool?) == true,
      bleWifiCommissioning: onboardingMethodSet.contains(
            RhythmDeviceOnboardingMethod.matterBleWifiCommissioning,
          ) ||
          (json['ble_wifi_commissioning'] as bool?) == true,
    );
  }
}

List<String> _parseDeviceOnboardingMethods(
  Object? rawMethods, {
  Map<String, dynamic> legacyAddDevice = const <String, dynamic>{},
}) {
  final methods = <String>{};

  methods.addAll(_parseStringList(rawMethods));

  if ((legacyAddDevice['on_network_setup_code'] as bool?) == true) {
    methods.add(RhythmDeviceOnboardingMethod.matterOnNetworkSetupCode);
  }
  if ((legacyAddDevice['ble_wifi_commissioning'] as bool?) == true) {
    methods.add(RhythmDeviceOnboardingMethod.matterBleWifiCommissioning);
  }

  return methods.toList(growable: false);
}

List<String> _parseStringList(Object? value) {
  if (value is! List) return const [];
  return value
      .whereType<String>()
      .where((value) => value.isNotEmpty)
      .toList(growable: false);
}

List<RhythmDeviceProfile> _parseDeviceProfiles(Object? value) {
  if (value is! List) return const [];
  final profiles = <RhythmDeviceProfile>[];
  for (final rawProfile in value) {
    final profileJson = jsonMap(rawProfile);
    if (profileJson == null ||
        !_hasSupportedDeviceProfileFieldTypes(profileJson)) {
      continue;
    }
    final profile = RhythmDeviceProfile.fromJson(profileJson);
    if (_parseDeviceProfileId(profile.id) != null) {
      profiles.add(profile);
    }
  }
  return profiles;
}

bool _hasSupportedDeviceProfileFieldTypes(Map<String, dynamic> json) {
  if (json['id'] is! String) return false;
  if (!_hasOptionalType<String>(json, 'device_type') ||
      !_hasOptionalType<String>(json, 'display_name') ||
      !_hasOptionalType<bool>(json, 'input_only') ||
      !_hasOptionalType<List>(json, 'compatible_profile_ids') ||
      !_hasOptionalType<List>(json, 'onboarding_methods')) {
    return false;
  }
  return true;
}

bool _hasOptionalType<T>(Map<String, dynamic> json, String key) {
  final value = json[key];
  return value == null || value is T;
}
