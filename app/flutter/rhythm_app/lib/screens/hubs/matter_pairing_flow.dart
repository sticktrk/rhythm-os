import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmDevice, RhythmDeviceType;
import 'package:uuid/uuid.dart';

import '../../providers/home_provider.dart';
import '../../providers/server_sync_provider.dart';
import '../../services/hue/hue_service_locator.dart';
import '../../services/server_endpoint_resolver.dart';
import '../../widgets/device_detail_sheet.dart';
import 'device_pairing_scanner_screen.dart';
import 'matter_add_method.dart';
import 'matter_device_add_screen.dart';

Future<void> startMatterPairingFlow(
  BuildContext context, {
  MatterAddMethod? preferredMethod,
  String analyticsSource = 'unknown',
}) async {
  final homeProvider = context.read<HomeProvider>();
  final syncProvider = context.read<ServerSyncProvider>();
  final serverHub = homeProvider.activeServerHub;
  if (serverHub == null) return;
  final journeyId = 'matter-pair-${const Uuid().v4()}';

  final addMethod = await _resolveMatterAddMethod(
    context,
    syncProvider: syncProvider,
    preferredMethod: preferredMethod,
  );
  if (!context.mounted || addMethod == null) return;

  final serverEndpoint = await ServerEndpointResolver.resolve(
    serverHub,
    syncProvider: syncProvider,
  );
  if (!context.mounted) return;

  MatterDevicePairingResult? pairingResult;
  if (!supportsDevicePairingCamera) {
    pairingResult = await MatterDeviceAddScreen.show(
      context,
      endpoint: serverEndpoint.endpoint,
      authToken: serverEndpoint.hub.token,
      addMethod: addMethod,
      analyticsSource: analyticsSource,
      journeyId: journeyId,
    );
  } else {
    while (pairingResult == null) {
      if (!context.mounted) return;
      final scanResult = await DevicePairingScannerScreen.show(context);
      if (!context.mounted || scanResult == null) return;

      pairingResult = await MatterDeviceAddScreen.show(
        context,
        endpoint: serverEndpoint.endpoint,
        authToken: serverEndpoint.hub.token,
        addMethod: addMethod,
        analyticsSource: analyticsSource,
        journeyId: journeyId,
        initialSetupPayload:
            scanResult.action == DevicePairingScannerAction.matter
                ? scanResult.payload
                : null,
      );
    }
  }
  if (!context.mounted || pairingResult == null) return;

  if (HueServiceLocator.isDemoMode) {
    await syncProvider.fullRefresh();
  } else {
    await syncProvider.connection.reconnect();
  }
  if (!context.mounted) return;

  final pairedDevice = await _resolvePairedMatterDevice(context, pairingResult);
  if (!context.mounted) return;

  if (pairedDevice == null) {
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

  await showDeviceNodeAssignmentFlow(
    context,
    device: pairedDevice.device,
    currentParentNodeId: pairedDevice.parentNodeId,
    allowNoRoom: syncProvider.supportsMatterRoomlessDevices,
  );
}

Future<({RhythmDevice device, String parentNodeId})?>
    _resolvePairedMatterDevice(
  BuildContext context,
  MatterDevicePairingResult pairingResult,
) async {
  final syncProvider = context.read<ServerSyncProvider>();

  for (int attempt = 0; attempt < 5; attempt++) {
    final devices = await syncProvider.api.getCanonicalDevices();
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
          parentNodeId:
              syncProvider.topologyNodeById(canonicalId)?.parentId ?? '',
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
  return syncProvider.canAddMatterDevice ? MatterAddMethod.automatic : null;
}
