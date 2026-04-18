import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show RhythmConnection, RhythmDevice, RhythmDeviceType;

import '../../providers/home_provider.dart';
import '../../providers/server_sync_provider.dart';
import '../../widgets/device_detail_sheet.dart';
import '../../widgets/solar_orbit.dart';
import 'matter_add_method.dart';
import 'matter_device_add_screen.dart';

Future<void> startMatterPairingFlow(
  BuildContext context, {
  MatterAddMethod? preferredMethod,
}) async {
  final homeProvider = context.read<HomeProvider>();
  final syncProvider = context.read<ServerSyncProvider>();
  final serverHub = homeProvider.currentHomeHubs
      .where((hub) => hub.type == HubType.server)
      .firstOrNull;
  if (serverHub == null) return;

  final addMethod = await _resolveMatterAddMethod(
    context,
    syncProvider: syncProvider,
    preferredMethod: preferredMethod,
  );
  if (!context.mounted || addMethod == null) return;

  final pairingResult = await MatterDeviceAddScreen.show(
    context,
    endpoint: serverHub.endpoint,
    addMethod: addMethod,
  );
  if (!context.mounted || pairingResult == null) return;

  await syncProvider.connection.reconnect();
  if (!context.mounted) return;

  final resolved = await _resolvePairedMatterDevice(context, pairingResult);
  if (!context.mounted) return;

  if (resolved == null) {
    final allowsRoomless = syncProvider.supportsMatterRoomlessDevices;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(
          allowsRoomless
              ? '${pairingResult.name} was added. You can leave it unassigned or move it into a room from the Matter device list once it appears.'
              : '${pairingResult.name} was added. You can assign it to a room from the Matter device list once it appears.',
        ),
      ),
    );
    return;
  }

  await showDeviceRoomAssignmentFlow(
    context,
    device: resolved.device,
    currentRoomId: resolved.roomId,
  );
}

Future<({RhythmDevice device, String roomId})?> _resolvePairedMatterDevice(
  BuildContext context,
  MatterDevicePairingResult pairingResult,
) async {
  final connection = context.read<RhythmConnection>();

  for (int attempt = 0; attempt < 5; attempt++) {
    final devices = await connection.api.getCanonicalDevices();
    if (!context.mounted) return null;

    if (devices != null) {
      for (final device in devices) {
        final endpoints = device['endpoints'] as List<dynamic>? ?? const [];
        final matchesNativeId = endpoints.any((endpoint) {
          final nativeId =
              (endpoint as Map<String, dynamic>)['native_id'] as String?;
          return nativeId == pairingResult.nativeDeviceId;
        });
        if (!matchesNativeId) continue;

        final canonicalId = device['id'] as String?;
        if (canonicalId == null || canonicalId.isEmpty) continue;

        return (
          device: RhythmDevice(
            id: canonicalId,
            type: RhythmDeviceType.fromString(
              device['device_type'] as String? ?? pairingResult.deviceType,
            ),
            name: device['name'] as String? ?? pairingResult.name,
            manufacturer:
                device['manufacturer'] as String? ?? pairingResult.manufacturer,
            model: device['model'] as String? ?? pairingResult.model,
          ),
          roomId: device['room_id'] as String? ?? '',
        );
      }
    }

    if (attempt < 4) {
      await Future.delayed(const Duration(seconds: 1));
    }
  }

  return null;
}

Future<MatterAddMethod?> _resolveMatterAddMethod(
  BuildContext context, {
  required ServerSyncProvider syncProvider,
  MatterAddMethod? preferredMethod,
}) async {
  if (preferredMethod != null) return preferredMethod;

  final methods = _availableMatterAddMethods(syncProvider);
  if (methods.isEmpty) {
    return syncProvider.canAddMatterDevice ? MatterAddMethod.automatic : null;
  }
  if (methods.length == 1) return methods.single;

  return showModalBottomSheet<MatterAddMethod>(
    context: context,
    backgroundColor: Colors.transparent,
    builder: (ctx) => Container(
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.vertical(top: Radius.circular(24)),
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          const SizedBox(height: 12),
          Container(
            width: 36,
            height: 4,
            decoration: BoxDecoration(
              color: CelestialColors.orbitRing,
              borderRadius: BorderRadius.circular(2),
            ),
          ),
          const SizedBox(height: 18),
          Text(
            'Add Matter Device',
            style: const TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 18,
              fontWeight: FontWeight.w600,
            ),
          ),
          const SizedBox(height: 8),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 24),
            child: Text(
              'Choose how Rhythm should add this Matter device.',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.72),
                fontSize: 14,
                height: 1.4,
              ),
            ),
          ),
          const SizedBox(height: 16),
          for (final method in methods) ...[
            ListTile(
              leading: Icon(
                method == MatterAddMethod.onNetworkSetupCode
                    ? Icons.wifi_tethering_rounded
                    : Icons.bluetooth_searching_rounded,
                color: const Color(0xFF26A69A),
              ),
              title: Text(
                method.actionLabel,
                style: const TextStyle(color: CelestialColors.textPrimary),
              ),
              subtitle: Text(
                method.description,
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.72),
                  fontSize: 12,
                  height: 1.35,
                ),
              ),
              onTap: () => Navigator.of(ctx).pop(method),
            ),
            if (method != methods.last)
              Divider(
                height: 1,
                color: CelestialColors.orbitRing.withValues(alpha: 0.3),
              ),
          ],
          SizedBox(height: MediaQuery.of(ctx).padding.bottom + 16),
        ],
      ),
    ),
  );
}

List<MatterAddMethod> _availableMatterAddMethods(
  ServerSyncProvider syncProvider,
) {
  final methods = <MatterAddMethod>[];
  if (syncProvider.canAddMatterOnNetworkDevice) {
    methods.add(MatterAddMethod.onNetworkSetupCode);
  }
  if (syncProvider.canCommissionMatterBleWifi) {
    methods.add(MatterAddMethod.bleWifiCommissioning);
  }
  return methods;
}
