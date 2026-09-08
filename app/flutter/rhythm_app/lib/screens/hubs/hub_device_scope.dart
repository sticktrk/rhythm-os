/// Presentation groups retain the real endpoint ownership of every device.
/// The third-party group is never sent to an appliance as a synthetic hub.
class HubDeviceScope {
  HubDeviceScope.single(Map<String, dynamic> hub)
      : hubs = [hub],
        isThirdParty = false;

  HubDeviceScope.thirdParty(this.hubs) : isThirdParty = true;

  static const thirdPartyLabel = 'Third party Hubs';
  final List<Map<String, dynamic>> hubs;
  final bool isThirdParty;

  /// Existing dedicated integrations keep their presentation. Additional
  /// provider types share one device destination without a vendor allowlist.
  static bool isThirdPartyType(String type) => switch (type) {
        '' ||
        'none' ||
        'hue' ||
        'hue_ble' ||
        'local_ble' ||
        'matter' ||
        'ha' ||
        'homeassistant' ||
        'home_assistant' ||
        'zigbee' =>
          false,
        _ => true,
      };

  static List<HubDeviceScope> group(List<Map<String, dynamic>> hubs) {
    final thirdParty = hubs
        .where((hub) => isThirdPartyType(hub['type']?.toString() ?? ''))
        .toList();
    final scopes = <HubDeviceScope>[];
    var addedThirdParty = false;
    for (final hub in hubs) {
      if (!isThirdPartyType(hub['type']?.toString() ?? '')) {
        scopes.add(HubDeviceScope.single(hub));
      } else if (!addedThirdParty) {
        scopes.add(HubDeviceScope.thirdParty(thirdParty));
        addedThirdParty = true;
      }
    }
    return scopes;
  }

  static String hubIdentity(Map<String, dynamic> hub) {
    final type = hub['type']?.toString().trim().toLowerCase() ?? '';
    final address = hub['address']?.toString().trim().toLowerCase() ?? '';
    return '${type.length}:$type|${address.length}:$address';
  }

  String get identity => isThirdParty
      ? 'third-party:${(hubs.map(hubIdentity).toList()..sort()).join(';')}'
      : hubIdentity(hubs.single);

  bool matchesEndpoint(Map<String, dynamic> endpoint) {
    final key = endpoint['hub_key'];
    if (key is! Map) return false;
    return hubs.any((hub) {
      if (key['hub_type']?.toString() != hub['type']?.toString()) return false;
      final address = hub['address']?.toString().trim().toLowerCase() ?? '';
      return address.isEmpty ||
          key['address']?.toString().trim().toLowerCase() == address;
    });
  }

  bool containsDevice(Map<String, dynamic> device) =>
      (device['endpoints'] as List? ?? const []).whereType<Map>().any(
          (endpoint) => matchesEndpoint(Map<String, dynamic>.from(endpoint)));

  /// Count canonical devices once even if they have several matching endpoints.
  static String summarize(Iterable<Map<String, dynamic>> devices,
      {int? roomCount}) {
    var lights = 0, buttons = 0, sensors = 0, other = 0;
    for (final device in devices) {
      switch (device['device_type'] ?? 'light') {
        case 'light':
          lights++;
        case 'button':
          buttons++;
        case 'motion' || 'contact':
          sensors++;
        default:
          other++;
      }
    }
    final parts = [
      if (lights > 0) '$lights light${lights == 1 ? '' : 's'}',
      if (buttons > 0) '$buttons button${buttons == 1 ? '' : 's'}',
      if (sensors > 0) '$sensors sensor${sensors == 1 ? '' : 's'}',
      if (other > 0) '$other device${other == 1 ? '' : 's'}',
    ];
    if (parts.isEmpty) return 'No devices';
    final rooms = roomCount == null
        ? ''
        : ' across $roomCount room${roomCount == 1 ? '' : 's'}';
    return '${parts.join(', ')}$rooms';
  }
}
