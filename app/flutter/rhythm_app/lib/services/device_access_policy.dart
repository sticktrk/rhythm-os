import 'dart:async';

import 'package:rhythm_core/rhythm_core.dart';

import '../config/platform_context.dart';
import 'settings_service.dart';

/// Remembers device ownership for each saved server without touching its
/// credentials. Losing a hello or restarting offline must not re-enable local
/// Hue/HA transports for a Home Assistant-owned home.
class DeviceAccessPolicy {
  DeviceAccessPolicy({
    Map<String, bool>? knownOwners,
    Future<void> Function(String key, bool ownedByHa)? persistOwner,
    bool? addonContext,
  })  : _knownOwners = Map.of(
          knownOwners ?? SettingsService.instance.knownDeviceOwners,
        ),
        _persistOwner =
            persistOwner ?? SettingsService.instance.saveDeviceOwner,
        _addonContext = addonContext ?? PlatformCtx.isHaAddon;

  final Map<String, bool> _knownOwners;
  final Future<void> Function(String key, bool ownedByHa) _persistOwner;
  final bool _addonContext;
  bool _ownedByHa = false;
  bool _allowsDirectAccess = true;

  bool get ownedByHomeAssistant => _ownedByHa;
  bool get allowsDirectAccess => _allowsDirectAccess;

  static String _hubKey(Hub hub) => 'home:${hub.homeId}:hub:${hub.id}';
  static String _instanceKey(Hub hub, String id) =>
      'home:${hub.homeId}:instance:$id';

  bool? _owner(Hub hub) =>
      _knownOwners[_hubKey(hub)] ??
      (hub.serverInstanceId == null
          ? null
          : _knownOwners[_instanceKey(hub, hub.serverInstanceId!)]);

  void suspendWhileLoading() {
    _allowsDirectAccess = false;
    DirectHubAccess.select(scope: 'home-loading', allowed: false);
  }

  void select({required String? homeId, required Hub? server}) {
    final known = server == null ? null : _owner(server);
    _ownedByHa = known ?? _addonContext;
    // A selected but as-yet unidentified server cannot authorize a parallel
    // device controller. A previously identified rpiz remains usable offline.
    _allowsDirectAccess = !_ownedByHa && (server == null || known == false);
    DirectHubAccess.select(
      scope: server == null ? 'home:$homeId:local' : _hubKey(server),
      allowed: _allowsDirectAccess,
    );
  }

  void observeServer(
    Hub hub, {
    required bool haDeviceManagement,
    required bool explicitDeployment,
    required String platformContext,
    String? serverInstanceId,
  }) {
    final owned = haDeviceManagement || platformContext == 'ha_addon'
        ? true
        : explicitDeployment || platformContext == 'rpiz'
            ? false
            : _owner(hub) ?? false;
    final keys = <String>{
      _hubKey(hub),
      if ((serverInstanceId ?? hub.serverInstanceId)?.isNotEmpty == true)
        _instanceKey(hub, serverInstanceId ?? hub.serverInstanceId!),
    };
    for (final key in keys) {
      if (_knownOwners[key] == owned) continue;
      _knownOwners[key] = owned;
      // Update the gate synchronously; persist independently of UI/network work.
      unawaited(_persistOwner(key, owned));
    }
  }
}
