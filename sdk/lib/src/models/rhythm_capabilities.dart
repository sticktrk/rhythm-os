import '../json_parsing.dart';

/// Stable hub/device onboarding method ids from `/api/state.capabilities.hubs`.
abstract final class RhythmDeviceOnboardingMethod {
  static const String matterOnNetworkSetupCode = 'matter_on_network_setup_code';
  static const String matterBleWifiCommissioning =
      'matter_ble_wifi_commissioning';
}

/// Host capabilities advertised by the Rhythm server.
class RhythmCapabilities {
  final List<RhythmHubCapabilities> hubs;

  const RhythmCapabilities({
    this.hubs = const [],
  });

  factory RhythmCapabilities.fromJson(Map<String, dynamic> json) {
    return RhythmCapabilities(
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
}

/// Per-hub capability metadata from `/api/state.capabilities.hubs`.
class RhythmHubCapabilities {
  final String type;
  final bool configurable;
  final List<String> deviceOnboardingMethods;
  final bool supportsUnpairing;
  final bool supportsRoomlessDevices;
  final RhythmAddDeviceCapabilities addDevice;

  const RhythmHubCapabilities({
    required this.type,
    required this.configurable,
    this.deviceOnboardingMethods = const [],
    required this.supportsUnpairing,
    required this.supportsRoomlessDevices,
    this.addDevice = const RhythmAddDeviceCapabilities(),
  });

  bool get canAddDevice => addDevice.isSupported;

  bool supportsDeviceOnboardingMethod(String method) {
    return deviceOnboardingMethods.contains(method);
  }

  factory RhythmHubCapabilities.fromJson(Map<String, dynamic> json) {
    final legacyAddDevice =
        jsonMap(json['add_device']) ?? const <String, dynamic>{};
    final deviceOnboardingMethods = _parseDeviceOnboardingMethods(
      json['device_onboarding_methods'],
      legacyAddDevice: legacyAddDevice,
    );

    return RhythmHubCapabilities(
      type: json['type'] as String? ?? '',
      configurable: json['configurable'] as bool? ?? false,
      deviceOnboardingMethods: deviceOnboardingMethods,
      supportsUnpairing: json['supports_unpairing'] as bool? ?? false,
      supportsRoomlessDevices:
          json['supports_roomless_devices'] as bool? ?? false,
      addDevice: RhythmAddDeviceCapabilities.fromJson(
        legacyAddDevice,
        deviceOnboardingMethods: deviceOnboardingMethods,
      ),
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

  if (rawMethods is List) {
    for (final method in rawMethods) {
      if (method is String && method.isNotEmpty) {
        methods.add(method);
      }
    }
  }

  if ((legacyAddDevice['on_network_setup_code'] as bool?) == true) {
    methods.add(RhythmDeviceOnboardingMethod.matterOnNetworkSetupCode);
  }
  if ((legacyAddDevice['ble_wifi_commissioning'] as bool?) == true) {
    methods.add(RhythmDeviceOnboardingMethod.matterBleWifiCommissioning);
  }

  return methods.toList(growable: false);
}
