import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show RhythmDevice, RhythmDeviceType, RhythmHubInfo, RhythmPairedDevice;
import 'package:uuid/uuid.dart';

import '../../providers/server_sync_provider.dart';
import '../../widgets/device_detail_sheet.dart';
import 'device_pairing_code_entry_screen.dart';
import 'device_pairing_scanner_screen.dart';
import 'hue_ble_device_add_screen.dart';
import 'hue_bridge_light_add_screen.dart';
import 'local_ble_device_add_screen.dart';
import 'matter_pairing_flow.dart';

enum DevicePairingTarget { any, hueBle, hueBridge }

class HueBridgePairingTarget {
  const HueBridgePairingTarget({
    required this.address,
    required this.label,
  });

  final String address;
  final String label;
}

typedef _ResolvedPairedDevice = ({
  RhythmDevice device,
  String parentNodeId,
});

@visibleForTesting
String pairingJourneyIdForIntake(
  DevicePairingScannerResult intake, {
  required String fallbackPrefix,
}) {
  final journeyId = intake.journeyId;
  if (journeyId != null && journeyId.isNotEmpty) return journeyId;
  return '$fallbackPrefix-${const Uuid().v4()}';
}

@visibleForTesting
List<HueBridgePairingTarget> connectedHueBridgePairingTargets({
  required List<RhythmHubInfo> serverHubs,
  required List<Map<String, dynamic>> serverHubInfos,
}) {
  final targetsByAddress = <String, HueBridgePairingTarget>{};
  for (final hub in serverHubs) {
    if (hub.type != 'hue' || !hub.connected) continue;
    final address = hub.address?.trim() ?? '';
    if (address.isEmpty) continue;
    final rawInfo = serverHubInfos.cast<Map<String, dynamic>?>().firstWhere(
          (info) =>
              info?['type'] == 'hue' &&
              info?['address']?.toString().trim().toLowerCase() ==
                  address.toLowerCase(),
          orElse: () => null,
        );
    final configuredLabel = [
      rawInfo?['name'],
      rawInfo?['label'],
      rawInfo?['bridge_name'],
    ]
        .map((value) => value?.toString().trim() ?? '')
        .firstWhere((value) => value.isNotEmpty, orElse: () => '');
    targetsByAddress[address.toLowerCase()] = HueBridgePairingTarget(
      address: address,
      label: configuredLabel.isEmpty ? address : '$configuredLabel · $address',
    );
  }
  final targets = targetsByAddress.values.toList(growable: false)
    ..sort((left, right) => left.label.compareTo(right.label));
  return targets;
}

/// Starts universal code intake, direct Hue BLE scan, or Hue Bridge search.
///
/// Hue Bluetooth bulbs retain their nearby-scan flow. Other local-BLE profiles
/// may use universal QR/manual intake when explicitly advertised by the host.
Future<void> startDevicePairingFlow(
  BuildContext context, {
  DevicePairingTarget target = DevicePairingTarget.any,
  String analyticsSource = 'unknown',
  String? hubAddress,
}) async {
  final syncProvider = context.read<ServerSyncProvider>();

  if (target == DevicePairingTarget.hueBle) {
    if (!syncProvider.canAddHueBleDevice) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text(
            'Update your Rhythm Box to scan for Hue Bluetooth bulbs.',
          ),
        ),
      );
      return;
    }
    await _startHueBlePairing(
      context,
      analyticsSource: analyticsSource,
    );
    return;
  }

  final hueBridgeOnly = target == DevicePairingTarget.hueBridge;
  if (hueBridgeOnly && !syncProvider.canAddHueBridgeDeviceBySerial) {
    ScaffoldMessenger.of(context).showSnackBar(
      const SnackBar(
        content: Text(
          'Connect a supported Hue Bridge before adding a bulb by serial.',
        ),
      ),
    );
    return;
  }
  if (!hueBridgeOnly && !syncProvider.canScanToAddDevice) return;
  final journeyId = 'device-pair-${const Uuid().v4()}';

  if (!hueBridgeOnly && syncProvider.canAddHueBleDevice) {
    final canUseSetupCode = syncProvider.canAddMatterDevice ||
        syncProvider.canAddHueBridgeDeviceBySerial ||
        syncProvider.canAddLocalBleDevice;
    if (!canUseSetupCode) {
      await _startHueBlePairing(
        context,
        analyticsSource: analyticsSource,
      );
      return;
    }
    final intake = await showUniversalDevicePairingIntakeChooser(context);
    if (!context.mounted || intake == null) return;
    if (intake == DevicePairingTarget.hueBle) {
      await _startHueBlePairing(
        context,
        analyticsSource: analyticsSource,
      );
      return;
    }
  }

  final hueBridgeSerialSearchAvailable =
      syncProvider.canAddHueBridgeDeviceBySerial;

  while (true) {
    if (!context.mounted) return;
    final intake = await _capturePairingCode(
      context,
      hueBridgeSerialSearchAvailable: hueBridgeSerialSearchAvailable,
      supportedLocalBleProfileIds: syncProvider.supportedLocalBleProfileIds,
      hueBridgeOnly: hueBridgeOnly,
      journeyId: journeyId,
    );
    if (!context.mounted || intake == null) return;

    switch (intake.action) {
      case DevicePairingScannerAction.matter:
        if (hueBridgeOnly) {
          ScaffoldMessenger.of(context).showSnackBar(
            const SnackBar(
              content: Text(
                'Scan or enter the six-character serial printed on the Hue '
                'bulb.',
              ),
            ),
          );
          continue;
        }
        if (!syncProvider.canAddMatterDevice) {
          ScaffoldMessenger.of(context).showSnackBar(
            const SnackBar(
              content: Text(
                'This Rhythm Box does not support adding Matter devices.',
              ),
            ),
          );
          continue;
        }
        await startMatterPairingFlow(
          context,
          analyticsSource: analyticsSource,
          initialIntakeResult: intake,
          journeyId: journeyId,
        );
        return;

      case DevicePairingScannerAction.hueBridge:
        if (!syncProvider.canAddHueBridgeDeviceBySerial) {
          ScaffoldMessenger.of(context).showSnackBar(
            const SnackBar(
              content: Text(
                'Connect a Hue Bridge to search for a bulb by serial. For '
                'direct Bluetooth, use the nearby Hue scan.',
              ),
            ),
          );
          continue;
        }
        await _startHueBridgePairing(
          context,
          intake: intake,
          analyticsSource: analyticsSource,
          hubAddress: hubAddress,
        );
        return;

      case DevicePairingScannerAction.localBle:
        if (hueBridgeOnly) continue;
        final profileId = intake.localBleSetup?.profileId;
        if (profileId == null ||
            !syncProvider.canAddLocalBleProfile(profileId)) {
          ScaffoldMessenger.of(context).showSnackBar(
            const SnackBar(
              content: Text(
                'Update your Rhythm Box before adding this Bluetooth device.',
              ),
            ),
          );
          continue;
        }
        await _startLocalBlePairing(
          context,
          intake: intake,
          analyticsSource: analyticsSource,
        );
        return;

      case DevicePairingScannerAction.enterCode:
        continue;
    }
  }
}

@visibleForTesting
Future<DevicePairingTarget?> showUniversalDevicePairingIntakeChooser(
  BuildContext context,
) {
  return showModalBottomSheet<DevicePairingTarget>(
    context: context,
    backgroundColor: const Color(0xFF151923),
    showDragHandle: true,
    builder: (sheetContext) => SafeArea(
      child: Padding(
        key: const ValueKey('device-pairing-method-chooser'),
        padding: const EdgeInsets.fromLTRB(20, 4, 20, 20),
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              const Text(
                'How do you want to add it?',
                style: TextStyle(
                  color: Colors.white,
                  fontSize: 21,
                  fontWeight: FontWeight.w700,
                ),
              ),
              const SizedBox(height: 8),
              Text(
                'Hue Bluetooth bulbs are found nearby without a setup code. '
                'Other supported devices use their QR or printed code.',
                style: TextStyle(
                  color: Colors.white.withValues(alpha: 0.7),
                  fontSize: 14,
                  height: 1.4,
                ),
              ),
              const SizedBox(height: 18),
              ListTile(
                key: const ValueKey('pairing-method-hue-ble'),
                shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(14),
                ),
                tileColor: const Color(0xFFFFB900).withValues(alpha: 0.12),
                leading: const Icon(
                  Icons.bluetooth_searching_rounded,
                  color: Color(0xFFFFB900),
                ),
                title: const Text(
                  'Find nearby Hue Bluetooth bulbs',
                  style: TextStyle(color: Colors.white),
                ),
                subtitle: Text(
                  'No QR code or serial required',
                  style: TextStyle(color: Colors.white.withValues(alpha: 0.62)),
                ),
                onTap: () =>
                    Navigator.of(sheetContext).pop(DevicePairingTarget.hueBle),
              ),
              const SizedBox(height: 10),
              ListTile(
                key: const ValueKey('pairing-method-code'),
                shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(14),
                ),
                tileColor: Colors.white.withValues(alpha: 0.06),
                leading: const Icon(
                  Icons.qr_code_scanner_rounded,
                  color: Color(0xFF4DD0C8),
                ),
                title: const Text(
                  'Scan or enter a setup code',
                  style: TextStyle(color: Colors.white),
                ),
                subtitle: Text(
                  'Matter, Bluetooth devices, or a Hue Bridge bulb serial',
                  style: TextStyle(color: Colors.white.withValues(alpha: 0.62)),
                ),
                onTap: () =>
                    Navigator.of(sheetContext).pop(DevicePairingTarget.any),
              ),
            ],
          ),
        ),
      ),
    ),
  );
}

Future<DevicePairingScannerResult?> _capturePairingCode(
  BuildContext context, {
  required bool hueBridgeSerialSearchAvailable,
  required Set<String> supportedLocalBleProfileIds,
  bool hueBridgeOnly = false,
  required String journeyId,
}) async {
  if (supportsDevicePairingCamera) {
    final scanned = await DevicePairingScannerScreen.show(
      context,
      hueBridgeSerialSearchAvailable: hueBridgeSerialSearchAvailable,
      supportedLocalBleProfileIds: supportedLocalBleProfileIds,
      hueBridgeOnly: hueBridgeOnly,
      journeyId: journeyId,
    );
    if (!context.mounted || scanned == null) return null;
    if (scanned.action == DevicePairingScannerAction.enterCode) {
      return DevicePairingCodeEntryScreen.show(
        context,
        hueBridgeSerialSearchAvailable: hueBridgeSerialSearchAvailable,
        supportedLocalBleProfileIds: supportedLocalBleProfileIds,
        hueBridgeOnly: hueBridgeOnly,
        journeyId: journeyId,
      );
    }
    return scanned;
  }
  return DevicePairingCodeEntryScreen.show(
    context,
    hueBridgeSerialSearchAvailable: hueBridgeSerialSearchAvailable,
    supportedLocalBleProfileIds: supportedLocalBleProfileIds,
    hueBridgeOnly: hueBridgeOnly,
    journeyId: journeyId,
  );
}

Future<void> _startLocalBlePairing(
  BuildContext context, {
  required DevicePairingScannerResult intake,
  required String analyticsSource,
}) async {
  final setup = intake.localBleSetup;
  if (setup == null) return;
  final syncProvider = context.read<ServerSyncProvider>();
  final pairingProfileId =
      syncProvider.canonicalLocalBleProfileId(setup.profileId);
  if (pairingProfileId == null) return;
  final result = await LocalBleDeviceAddScreen.show(
    context,
    setup: setup,
    pairingProfileId: pairingProfileId,
    inputMethod: intake.inputMethod,
    analyticsSource: analyticsSource,
    journeyId: pairingJourneyIdForIntake(
      intake,
      fallbackPrefix: 'device-pair',
    ),
  );
  if (!context.mounted || result == null) return;

  await continueRecoveredLocalBlePairingFlow(context, result);
}

/// Continues the normal post-pairing sync and room-assignment flow for a
/// device recovered from the durable app-shell pairing pointer.
Future<void> continueRecoveredLocalBlePairingFlow(
  BuildContext context,
  RhythmPairedDevice result,
) async {
  final syncProvider = context.read<ServerSyncProvider>();

  try {
    await syncProvider.connection.reconnect();
  } catch (_) {
    if (!context.mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(
          '${result.name} was added. It will appear in Devices after the '
          'Rhythm Box reconnects.',
        ),
      ),
    );
    return;
  }
  if (!context.mounted) return;
  final resolved = await _resolvePairedDevices(
    context,
    [result],
    hubType: 'local_ble',
  );
  if (!context.mounted) return;
  if (resolved.isEmpty) {
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(
          '${result.name} was added. It will appear in Devices after the next sync.',
        ),
      ),
    );
    return;
  }
  await _offerRoomAssignments(
    context,
    resolved,
    allowNoRoom: syncProvider.supportsLocalBleRoomlessDevices,
    sourceLabel: 'Bluetooth device',
    expectedCount: 1,
  );
}

Future<void> _startHueBridgePairing(
  BuildContext context, {
  required DevicePairingScannerResult intake,
  required String analyticsSource,
  String? hubAddress,
}) async {
  final serial = intake.payload;
  if (serial == null) return;
  final syncProvider = context.read<ServerSyncProvider>();
  final selectedBridge = await _selectHueBridgeForPairing(
    context,
    syncProvider,
    requestedAddress: hubAddress,
  );
  if (!context.mounted || selectedBridge.cancelled) return;
  final result = await HueBridgeLightAddScreen.show(
    context,
    serial: serial,
    analyticsSource: analyticsSource,
    journeyId: pairingJourneyIdForIntake(
      intake,
      fallbackPrefix: 'hue-bridge-add',
    ),
    hubAddress: selectedBridge.address,
  );
  if (!context.mounted || result == null) return;

  await syncProvider.api.triggerSync();
  await syncProvider.connection.reconnect();
  if (!context.mounted) return;

  final count = result.addedCount;
  final warningSuffix = _pairingWarningSuffix(result.warnings);
  ScaffoldMessenger.of(context).showSnackBar(
    SnackBar(
      content: Text(
        count == null
            ? 'Hue Bridge search completed. Synced your Hue devices.'
                '$warningSuffix'
            : count == 1
                ? 'Added 1 light through your Hue Bridge.$warningSuffix'
                : 'Added $count lights through your Hue Bridge.$warningSuffix',
      ),
    ),
  );

  if (result.devices.isEmpty) return;
  final resolved = await _resolvePairedDevices(
    context,
    result.devices,
    hubType: 'hue',
    hubAddress: selectedBridge.address,
  );
  if (!context.mounted || resolved.isEmpty) return;
  await _offerRoomAssignments(
    context,
    resolved,
    allowNoRoom:
        syncProvider.hueBridgeCapabilities?.supportsRoomlessDevices ?? false,
    sourceLabel: 'Hue Bridge',
  );
}

Future<({bool cancelled, String? address})> _selectHueBridgeForPairing(
  BuildContext context,
  ServerSyncProvider syncProvider, {
  String? requestedAddress,
}) async {
  final exactAddress = requestedAddress?.trim() ?? '';
  if (exactAddress.isNotEmpty) {
    return (cancelled: false, address: exactAddress);
  }

  final connectedHubs = syncProvider.serverHubs
      .where((hub) => hub.type == 'hue' && hub.connected)
      .toList(growable: false);
  final targets = connectedHueBridgePairingTargets(
    serverHubs: syncProvider.serverHubs,
    serverHubInfos: syncProvider.serverHubInfos,
  );
  if (connectedHubs.length <= 1) {
    return (
      cancelled: false,
      address: targets.isNotEmpty
          ? targets.single.address
          : connectedHubs.isEmpty
              ? null
              : connectedHubs.single.address,
    );
  }
  if (targets.length != connectedHubs.length) {
    ScaffoldMessenger.of(context).showSnackBar(
      const SnackBar(
        content: Text(
          'Rhythm could not identify every connected Hue Bridge. Open the '
          'specific Bridge in Devices and add the bulb there.',
        ),
      ),
    );
    return (cancelled: true, address: null);
  }

  final selectedAddress = await showHueBridgePairingTargetChooser(
    context,
    targets,
  );
  return (
    cancelled: selectedAddress == null,
    address: selectedAddress,
  );
}

@visibleForTesting
Future<String?> showHueBridgePairingTargetChooser(
  BuildContext context,
  List<HueBridgePairingTarget> targets,
) {
  return showDialog<String>(
    context: context,
    builder: (dialogContext) => SimpleDialog(
      key: const ValueKey('hue-bridge-pairing-chooser'),
      backgroundColor: const Color(0xFF151923),
      title: const Text(
        'Choose a Hue Bridge',
        style: TextStyle(color: Colors.white),
      ),
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(24, 0, 24, 12),
          child: Text(
            'The selected Bridge will search for and take ownership of this '
            'Zigbee bulb.',
            style: TextStyle(
              color: Colors.white.withValues(alpha: 0.7),
              height: 1.4,
            ),
          ),
        ),
        for (final target in targets)
          SimpleDialogOption(
            key: ValueKey('hue-bridge-choice-${target.address}'),
            onPressed: () => Navigator.of(dialogContext).pop(target.address),
            child: Row(
              children: [
                const Icon(Icons.hub_rounded, color: Color(0xFFFFB900)),
                const SizedBox(width: 12),
                Expanded(
                  child: Text(
                    target.label,
                    style: const TextStyle(color: Colors.white),
                  ),
                ),
              ],
            ),
          ),
      ],
    ),
  );
}

Future<void> _startHueBlePairing(
  BuildContext context, {
  required String analyticsSource,
}) async {
  final syncProvider = context.read<ServerSyncProvider>();
  final result = await HueBleDeviceAddScreen.show(
    context,
    analyticsSource: analyticsSource,
    journeyId: 'hue-ble-pair-${const Uuid().v4()}',
  );
  if (!context.mounted || result == null) return;

  await syncProvider.connection.reconnect();
  if (!context.mounted) return;

  final resolved = await _resolvePairedHueBleDevices(context, result);
  if (!context.mounted) return;
  final addedCount = result.devices.length;
  final warningSuffix = _pairingWarningSuffix(result.warnings);
  if (resolved.isEmpty) {
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(
          addedCount == 1
              ? '${result.devices.single.name} was added. It will appear in '
                  'Devices after the next sync.$warningSuffix'
              : '$addedCount Hue Bluetooth lights were added. They will '
                  'appear in Devices after the next sync.$warningSuffix',
        ),
      ),
    );
    return;
  }

  if (result.warnings.isNotEmpty) {
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(
          '$addedCount Hue Bluetooth '
          '${addedCount == 1 ? 'light was' : 'lights were'} added.'
          '$warningSuffix',
        ),
      ),
    );
  }
  await _offerRoomAssignments(
    context,
    resolved,
    allowNoRoom: syncProvider.supportsHueBleRoomlessDevices,
    sourceLabel: 'Hue Bluetooth',
    expectedCount: addedCount,
  );
}

Future<List<_ResolvedPairedDevice>> _resolvePairedHueBleDevices(
  BuildContext context,
  HueBleDevicePairingResult pairingResult,
) {
  return _resolvePairedDevices(
    context,
    [
      for (final device in pairingResult.devices)
        RhythmPairedDevice(
          deviceId: device.nativeDeviceId,
          name: device.name,
          deviceType: device.deviceType,
          manufacturer: device.manufacturer,
          model: device.model,
        ),
    ],
    hubType: 'hue_ble',
  );
}

Future<List<_ResolvedPairedDevice>> _resolvePairedDevices(
  BuildContext context,
  List<RhythmPairedDevice> pairedDevices, {
  required String hubType,
  String? hubAddress,
}) async {
  final syncProvider = context.read<ServerSyncProvider>();
  final pairingByNativeId = {
    for (final device in pairedDevices) device.deviceId: device,
  };
  var resolvedByNativeId = <String, _ResolvedPairedDevice>{};

  for (int attempt = 0; attempt < 5; attempt++) {
    final devices = await syncProvider.api.getCanonicalDevices();
    if (!context.mounted) return const [];

    if (devices != null) {
      for (final device in devices) {
        final endpoints = device['endpoints'] as List<dynamic>? ?? const [];
        String? matchedNativeId;
        for (final endpoint in endpoints.whereType<Map>()) {
          final nativeId = endpoint['native_id']?.toString();
          if (nativeId != null &&
              pairingByNativeId.containsKey(nativeId) &&
              pairedEndpointMatchesTarget(
                endpoint,
                hubType: hubType,
                hubAddress: hubAddress,
              )) {
            matchedNativeId = nativeId;
            break;
          }
        }
        if (matchedNativeId == null) continue;

        final canonicalId = device['id'] as String?;
        if (canonicalId == null || canonicalId.isEmpty) continue;
        final pairingDevice = pairingByNativeId[matchedNativeId]!;
        resolvedByNativeId[matchedNativeId] = (
          device: RhythmDevice(
            id: canonicalId,
            type: RhythmDeviceType.fromString(
              device['device_type'] as String? ?? pairingDevice.deviceType,
            ),
            name: device['name'] as String? ?? pairingDevice.name,
            manufacturer:
                device['manufacturer'] as String? ?? pairingDevice.manufacturer,
            model: device['model'] as String? ?? pairingDevice.model,
          ),
          parentNodeId: resolvePairedDeviceParentNodeId(
            device,
            topologyParentId:
                syncProvider.topologyNodeById(canonicalId)?.parentId,
          ),
        );
      }
    }

    if (resolvedByNativeId.length == pairingByNativeId.length) break;
    if (attempt < 4) {
      await Future<void>.delayed(const Duration(seconds: 1));
    }
  }

  return [
    for (final pairingDevice in pairedDevices)
      if (resolvedByNativeId[pairingDevice.deviceId] case final resolved?)
        resolved,
  ];
}

Future<void> _offerRoomAssignments(
  BuildContext context,
  List<_ResolvedPairedDevice> resolved, {
  required bool allowNoRoom,
  required String sourceLabel,
  int? expectedCount,
}) async {
  final pairedCount = expectedCount ?? resolved.length;
  if (pairedCount > 1) {
    final assignNow = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        backgroundColor: const Color(0xFF151923),
        title: Text('$pairedCount $sourceLabel lights added'),
        content: Text(
          resolved.length == pairedCount
              ? 'Assign each new light to a room now, or leave them '
                  'unassigned and manage them later from Devices.'
              : '${resolved.length} of $pairedCount lights are ready to assign. '
                  'The others will appear in Devices after sync.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(false),
            child: const Text('Later'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(dialogContext).pop(true),
            child: const Text('Assign Lights'),
          ),
        ],
      ),
    );
    if (!context.mounted || assignNow != true) return;
  }

  for (final pairedDevice in resolved) {
    if (!context.mounted) return;
    await showDeviceNodeAssignmentFlow(
      context,
      device: pairedDevice.device,
      currentParentNodeId: pairedDevice.parentNodeId,
      allowNoRoom: allowNoRoom,
    );
  }
}

String _pairingWarningSuffix(List<String> warnings) {
  if (warnings.isEmpty) return '';
  final detail = warnings.first.trim();
  return ' Some bulbs could not be added.'
      '${detail.isEmpty ? '' : ' $detail'}';
}

@visibleForTesting
bool pairedEndpointMatchesTarget(
  Map endpoint, {
  required String hubType,
  String? hubAddress,
}) {
  final hubKey = endpoint['hub_key'];
  if (hubKey is! Map || hubKey['hub_type']?.toString() != hubType) {
    return false;
  }
  final normalizedAddress = hubAddress?.trim().toLowerCase() ?? '';
  if (normalizedAddress.isEmpty) return true;
  return hubKey['address']?.toString().trim().toLowerCase() ==
      normalizedAddress;
}

@visibleForTesting
String resolvePairedDeviceParentNodeId(
  Map<String, dynamic> canonicalDevice, {
  String? topologyParentId,
}) {
  final canonicalRoomId = canonicalDevice['room_id']?.toString().trim() ?? '';
  if (canonicalRoomId.isNotEmpty) return canonicalRoomId;
  return topologyParentId ?? '';
}
