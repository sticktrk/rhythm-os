import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show
        RhythmDevice,
        RhythmDeviceType,
        RhythmPairingRecoverySecret,
        RhythmRoomProjectionStatus;
import 'package:uuid/uuid.dart';
import '../providers/server_sync_provider.dart';
import '../screens/hubs/matter_bulb_tester_screen.dart';
import '../screens/settings/light_screen.dart';
import '../services/analytics_service.dart';
import '../services/matter_removal_flow.dart';
import 'low_glow_switch.dart';
import 'blocking_operation_overlay.dart';
import 'matter_setup_code_dialog.dart';
import 'room_picker_sheet.dart';
import 'segmented_tab_bar.dart';
import 'solar_orbit.dart'; // For CelestialColors

const _deviceRoomMoveUuid = Uuid();
const _deviceControlTargetsUuid = Uuid();

bool _roomProjectionIsUnresolved(RhythmRoomProjectionStatus status) =>
    status == RhythmRoomProjectionStatus.pending ||
    status == RhythmRoomProjectionStatus.attention ||
    status == RhythmRoomProjectionStatus.blocked;

String _roomProjectionMessage(
  String committedMessage,
  RhythmRoomProjectionStatus status,
) =>
    switch (status) {
      RhythmRoomProjectionStatus.pending =>
        '$committedMessage. Hue room sync is still pending; individual bulb control remains available.',
      RhythmRoomProjectionStatus.attention =>
        '$committedMessage. Hue room sync needs attention; individual bulb control remains available.',
      RhythmRoomProjectionStatus.blocked =>
        '$committedMessage. Hue room sync is blocked until room ownership is reviewed.',
      _ => committedMessage,
    };

/// Request the server's existing physical Identify behavior for one bulb.
///
/// Callers own their local haptic/visual feedback. Analytics is deliberately
/// identifier-free and cannot change the product result.
Future<bool> identifyCanonicalBulb(
  BuildContext context, {
  required RhythmDevice device,
  required String source,
}) async {
  if (device.type != RhythmDeviceType.light) return false;
  final success = await context
      .read<ServerSyncProvider>()
      .api
      .flashCanonicalDevice(device.id);
  unawaited(
    AnalyticsService().logBulbIdentifyCompleted(
      source: source,
      outcome: success ? 'succeeded' : 'failed',
    ),
  );
  return success;
}

/// Assign an existing canonical device to a known room without reopening the
/// general room picker.
///
/// Room-scoped add and scan flows already know their destination. The server
/// remains authoritative: this helper does not mutate local membership until
/// the assignment is acknowledged and topology refreshes successfully.
Future<bool> assignCanonicalDeviceToRoom(
  BuildContext context, {
  required RhythmDevice device,
  required String currentParentNodeId,
  required String targetRoomId,
  required String targetRoomName,
  required String analyticsSource,
}) async {
  final normalizedCurrentParentNodeId = currentParentNodeId.trim();
  if (normalizedCurrentParentNodeId == targetRoomId) {
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text('${device.displayName} is already in $targetRoomName'),
      ),
    );
    return true;
  }

  final syncProvider = context.read<ServerSyncProvider>();
  final journeyId = 'device-room-move-${_deviceRoomMoveUuid.v4()}';
  final assignment = await syncProvider.api.assignDeviceParentResult(
    device.id,
    targetRoomId,
  );
  if (!context.mounted) return false;

  if (assignment?.canonicalCommitted != true) {
    unawaited(
      AnalyticsService().logDeviceRoomMoveCompleted(
        journeyId: journeyId,
        source: analyticsSource,
        destination: 'room',
        outcome: 'failed',
        failureStage: 'assignment_request',
      ),
    );
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(content: Text('Failed to place ${device.displayName}')),
    );
    return false;
  }

  final refreshed = await syncProvider.refreshAfterTopologyMutation();
  if (!context.mounted) return refreshed;
  if (!refreshed) {
    unawaited(
      AnalyticsService().logDeviceRoomMoveCompleted(
        journeyId: journeyId,
        source: analyticsSource,
        destination: 'room',
        outcome: 'partial',
        failureStage: 'authoritative_refresh',
      ),
    );
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(
          '${device.displayName} was assigned, but rooms could not refresh. '
          'Pull to refresh and confirm its room.',
        ),
      ),
    );
    return false;
  }

  final committedMessage = normalizedCurrentParentNodeId.isEmpty
      ? 'Assigned ${device.displayName} to $targetRoomName'
      : 'Moved ${device.displayName} to $targetRoomName';
  final projectionStatus = assignment!.projectionStatus;
  final projectionUnresolved = _roomProjectionIsUnresolved(projectionStatus);
  unawaited(
    AnalyticsService().logDeviceRoomMoveCompleted(
      journeyId: journeyId,
      source: analyticsSource,
      destination: 'room',
      outcome: projectionUnresolved ? 'partial' : 'succeeded',
      failureStage: projectionUnresolved
          ? 'external_projection_${projectionStatus.name}'
          : null,
    ),
  );
  ScaffoldMessenger.of(context).showSnackBar(
    SnackBar(
      content: Text(_roomProjectionMessage(
        committedMessage,
        projectionStatus,
      )),
    ),
  );
  return true;
}

Future<bool> showDeviceNodeAssignmentFlow(
  BuildContext context, {
  required RhythmDevice device,
  required String currentParentNodeId,
  bool allowNoRoom = false,
  VoidCallback? onAssignmentStarted,
  String analyticsSource = 'device_detail',
}) async {
  final syncProvider = context.read<ServerSyncProvider>();
  final normalizedCurrentParentNodeId =
      currentParentNodeId.isEmpty ? null : currentParentNodeId;
  final roomSummariesById = {
    for (final room in syncProvider.helloRooms) room.id: room,
  };
  final topologyRooms = syncProvider.topologyNodes
      .where((node) => node.isRoom && node.id.isNotEmpty)
      .map(
        (node) => RoomPickerOption(
          id: node.id,
          name: node.name,
          subtitle: roomSummariesById[node.id]?.deviceSummary,
        ),
      )
      .toList();
  final rooms = (topologyRooms.isNotEmpty
          ? topologyRooms
          : syncProvider.helloRooms.map(
              (room) => RoomPickerOption(
                id: room.id,
                name: room.name,
                subtitle: room.deviceSummary,
              ),
            ))
      .where((room) => room.id != normalizedCurrentParentNodeId)
      .toList()
    ..sort(
      (left, right) => left.name.toLowerCase().compareTo(
            right.name.toLowerCase(),
          ),
    );

  final isUnassigned = normalizedCurrentParentNodeId == null;
  final title = isUnassigned ? 'Assign to Room' : 'Move to Room';
  RoomPickerOption? createdRoom;

  final targetRoomId = await showRoomPickerSheet(
    context,
    title: title,
    currentRoomId: normalizedCurrentParentNodeId,
    rooms: rooms,
    allowUnassigned: allowNoRoom,
    allowCreateRoom: true,
    unassignedLabel: isUnassigned ? 'Unassigned' : 'Remove from Room',
    unassignedSubtitle: isUnassigned
        ? 'Keep this device unassigned.'
        : 'Remove this device from its current room.',
    emptyMessage:
        'No rooms exist yet. Create one now to place this device in a room.',
    onCreateRoom: () async {
      createdRoom = await createTopologyRoomOptionFromPrompt(context);
      return createdRoom?.id;
    },
    noOptionsMessage: 'No other rooms available',
  );

  if (targetRoomId == null || !context.mounted) return false;
  final targetParentNodeId = targetRoomId.isEmpty ? null : targetRoomId;
  final selectedIsUnassigned = targetParentNodeId == null;
  final assignmentChanged = targetParentNodeId != normalizedCurrentParentNodeId;
  final activatesStandalone = selectedIsUnassigned &&
      allowNoRoom &&
      device.type == RhythmDeviceType.light;
  final roomNamesById = {
    for (final room in rooms) room.id: room.name,
    if (createdRoom != null) createdRoom!.id: createdRoom!.name,
  };
  var selectedLabel = 'Unassigned';
  if (targetParentNodeId != null) {
    selectedLabel = roomNamesById[targetParentNodeId] ??
        roomSummariesById[targetParentNodeId]?.name ??
        'selected room';
  }

  if (!assignmentChanged && !activatesStandalone) {
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(
          selectedIsUnassigned
              ? '${device.displayName} has no room assignment'
              : '${device.displayName} is already in $selectedLabel',
        ),
      ),
    );
    return true;
  }

  onAssignmentStarted?.call();
  final assignmentOverlay = showBlockingOperationOverlay(
    context,
    message: 'Saving room assignment…',
  );
  final journeyId = 'device-room-move-${_deviceRoomMoveUuid.v4()}';
  final destination = selectedIsUnassigned ? 'unassigned' : 'room';
  var success = true;
  var refreshed = false;
  var failureStage = 'assignment_request';
  var projectionStatus = RhythmRoomProjectionStatus.notReported;
  try {
    if (assignmentChanged) {
      final assignment = await syncProvider.api.assignDeviceParentResult(
        device.id,
        targetParentNodeId,
      );
      success = assignment?.canonicalCommitted == true;
      projectionStatus = assignment?.projectionStatus ??
          RhythmRoomProjectionStatus.notReported;
    }

    if (success && activatesStandalone) {
      failureStage = 'standalone_activation';
      success = await _resolveUnassignedDeviceAsStandalone(
        syncProvider,
        device.id,
      );
    }

    if (success) {
      refreshed = await syncProvider.refreshAfterTopologyMutation();
    }
  } finally {
    assignmentOverlay.remove();
  }

  if (!context.mounted) return false;

  if (!success) {
    unawaited(
      AnalyticsService().logDeviceRoomMoveCompleted(
        journeyId: journeyId,
        source: analyticsSource,
        destination: destination,
        outcome: 'failed',
        failureStage: failureStage,
      ),
    );
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(
          activatesStandalone
              ? 'Failed to activate ${device.displayName} as standalone'
              : isUnassigned
                  ? 'Failed to assign ${device.displayName}'
                  : 'Failed to move ${device.displayName}',
        ),
      ),
    );
    return false;
  }

  if (!refreshed) {
    unawaited(
      AnalyticsService().logDeviceRoomMoveCompleted(
        journeyId: journeyId,
        source: analyticsSource,
        destination: destination,
        outcome: 'partial',
        failureStage: 'authoritative_refresh',
      ),
    );
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(
          '${device.displayName} was updated, but rooms could not refresh. '
          'Pull to refresh and confirm its room.',
        ),
      ),
    );
    return false;
  }

  final projectionUnresolved = _roomProjectionIsUnresolved(projectionStatus);
  if (projectionUnresolved) {
    unawaited(
      AnalyticsService().logDeviceRoomMoveCompleted(
        journeyId: journeyId,
        source: analyticsSource,
        destination: destination,
        outcome: 'partial',
        failureStage: 'external_projection_${projectionStatus.name}',
      ),
    );
    final committedMessage = selectedIsUnassigned
        ? 'Removed ${device.displayName} from its room'
        : isUnassigned
            ? 'Assigned ${device.displayName} to $selectedLabel'
            : 'Moved ${device.displayName} to $selectedLabel';
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(_roomProjectionMessage(
          committedMessage,
          projectionStatus,
        )),
      ),
    );
    return true;
  }

  unawaited(
    AnalyticsService().logDeviceRoomMoveCompleted(
      journeyId: journeyId,
      source: analyticsSource,
      destination: destination,
      outcome: 'succeeded',
    ),
  );

  ScaffoldMessenger.of(context).showSnackBar(
    SnackBar(
      content: Text(
        activatesStandalone
            ? '${device.displayName} is ready to use standalone'
            : selectedIsUnassigned
                ? 'Removed ${device.displayName} from its room'
                : isUnassigned
                    ? 'Assigned ${device.displayName} to $selectedLabel'
                    : 'Moved ${device.displayName} to $selectedLabel',
      ),
    ),
  );
  return true;
}

/// Moves or removes one room-control relationship without changing the
/// device's parent room or its other control targets.
Future<bool> showDeviceControlRoomRelationshipFlow(
  BuildContext context, {
  required RhythmDevice device,
  required String currentRoomId,
  required String controlKind,
  VoidCallback? onAssignmentStarted,
  String analyticsSource = 'device_detail',
}) async {
  final syncProvider = context.read<ServerSyncProvider>();
  final roomSummariesById = {
    for (final room in syncProvider.helloRooms) room.id: room,
  };
  final topologyRooms = syncProvider.topologyNodes
      .where((node) => node.isRoom && node.id.isNotEmpty)
      .map(
        (node) => RoomPickerOption(
          id: node.id,
          name: node.name,
          subtitle: roomSummariesById[node.id]?.deviceSummary,
        ),
      )
      .toList();
  final rooms = (topologyRooms.isNotEmpty
          ? topologyRooms
          : syncProvider.helloRooms.map(
              (room) => RoomPickerOption(
                id: room.id,
                name: room.name,
                subtitle: room.deviceSummary,
              ),
            ))
      .toList()
    ..sort(
      (left, right) => left.name.toLowerCase().compareTo(
            right.name.toLowerCase(),
          ),
    );
  RoomPickerOption? createdRoom;

  final targetRoomId = await showRoomPickerSheet(
    context,
    title: 'Move to Room',
    currentRoomId: currentRoomId,
    rooms: rooms,
    allowUnassigned: true,
    allowCreateRoom: true,
    unassignedLabel: 'Remove from Room',
    unassignedSubtitle: 'Stop this device from controlling this room.',
    emptyMessage: 'No other rooms exist yet.',
    onCreateRoom: () async {
      createdRoom = await createTopologyRoomOptionFromPrompt(context);
      return createdRoom?.id;
    },
  );
  if (targetRoomId == null || !context.mounted) return false;

  final currentTargets = syncProvider.controlTargetNodeIds(
    sourceNodeId: device.id,
    controlKind: controlKind,
  );
  final updatedTargets = <String>{...currentTargets}..remove(currentRoomId);
  if (targetRoomId.isNotEmpty) updatedTargets.add(targetRoomId);
  if (updatedTargets.length == currentTargets.length &&
      updatedTargets.containsAll(currentTargets)) {
    return true;
  }

  onAssignmentStarted?.call();
  final assignmentOverlay = showBlockingOperationOverlay(
    context,
    message: 'Saving room assignment…',
  );
  final success = await syncProvider.setNodeControlTargets(
    sourceNodeId: device.id,
    controlKind: controlKind,
    targetNodeIds: updatedTargets,
  );
  assignmentOverlay.remove();
  if (!context.mounted) return success;

  final destination = targetRoomId.isEmpty ? 'unassigned' : 'room';
  unawaited(
    AnalyticsService().logDeviceRoomMoveCompleted(
      journeyId: 'device-room-move-${_deviceRoomMoveUuid.v4()}',
      source: analyticsSource,
      destination: destination,
      outcome: success ? 'succeeded' : 'failed',
      failureStage: success ? null : 'control_targets_request_or_refresh',
    ),
  );

  final roomNamesById = {
    for (final room in rooms) room.id: room.name,
    if (createdRoom != null) createdRoom!.id: createdRoom!.name,
  };
  final currentRoomName = roomNamesById[currentRoomId] ?? 'this room';
  final targetRoomName = roomNamesById[targetRoomId] ?? 'the selected room';
  ScaffoldMessenger.of(context).showSnackBar(
    SnackBar(
      content: Text(
        success
            ? targetRoomId.isEmpty
                ? 'Removed ${device.displayName} from $currentRoomName'
                : 'Moved ${device.displayName} to $targetRoomName'
            : 'Failed to update ${device.displayName}',
      ),
    ),
  );
  return success;
}

Future<bool> _resolveUnassignedDeviceAsStandalone(
  ServerSyncProvider syncProvider,
  String deviceId,
) async {
  final entries = await syncProvider.api.getTriageEntries();
  if (entries == null) return false;

  Map<String, dynamic>? pendingEntry;
  for (final entry in entries) {
    if (entry['kind'] == 'unassigned_device' &&
        entry['canonical_id'] == deviceId) {
      pendingEntry = entry;
      break;
    }
  }

  // No pending entry means this device already has a durable standalone
  // decision (or is connected to a server that did not quarantine it).
  if (pendingEntry == null) return true;

  final entryId = pendingEntry['id']?.toString();
  if (entryId == null || entryId.isEmpty) return false;

  final result = await syncProvider.api.resolveTriageNewResult(entryId);
  return result?['status'] == 'standalone';
}

Future<bool> showDeviceRoomAssignmentFlow(
  BuildContext context, {
  required RhythmDevice device,
  required String currentRoomId,
  bool allowNoRoom = false,
}) {
  return showDeviceNodeAssignmentFlow(
    context,
    device: device,
    currentParentNodeId: currentRoomId,
    allowNoRoom: allowNoRoom,
  );
}

Future<bool> showMotionTargetRoomsFlow(
  BuildContext context, {
  required RhythmDevice device,
  required String currentParentNodeId,
}) =>
    _showControlTargetRoomsFlow(
      context,
      device: device,
      currentParentNodeId: currentParentNodeId,
      controlKind: 'motion',
      title: 'Motion Controls',
      description: 'Turn on every selected room when motion is detected.',
    );

Future<bool> showButtonTargetRoomsFlow(
  BuildContext context, {
  required RhythmDevice device,
  required String currentParentNodeId,
}) =>
    _showControlTargetRoomsFlow(
      context,
      device: device,
      currentParentNodeId: currentParentNodeId,
      controlKind: 'button',
      title: 'Button Controls',
      description: 'Apply each button press to every selected room.',
    );

Future<bool> _showControlTargetRoomsFlow(
  BuildContext context, {
  required RhythmDevice device,
  required String currentParentNodeId,
  required String controlKind,
  required String title,
  required String description,
}) async {
  final syncProvider = context.read<ServerSyncProvider>();
  final roomSummariesById = {
    for (final room in syncProvider.helloRooms) room.id: room,
  };
  final topologyRooms = syncProvider.topologyNodes
      .where((node) => node.isRoom && node.id.isNotEmpty)
      .map(
        (node) => RoomPickerOption(
          id: node.id,
          name: node.name,
          subtitle: roomSummariesById[node.id]?.deviceSummary,
        ),
      )
      .toList();
  final rooms = (topologyRooms.isNotEmpty
          ? topologyRooms
          : syncProvider.helloRooms.map(
              (room) => RoomPickerOption(
                id: room.id,
                name: room.name,
                subtitle: room.deviceSummary,
              ),
            ))
      .toList()
    ..sort(
      (left, right) => left.name.toLowerCase().compareTo(
            right.name.toLowerCase(),
          ),
    );

  final currentTargets = syncProvider.controlTargetNodeIds(
    sourceNodeId: device.id,
    controlKind: controlKind,
  );
  final initialTargets = <String>{...currentTargets};
  if (initialTargets.isEmpty && currentParentNodeId.isNotEmpty) {
    initialTargets.add(currentParentNodeId);
  }

  final selectedTargets = await showMultiRoomPickerSheet(
    context,
    title: title,
    rooms: rooms,
    selectedRoomIds: initialTargets,
    description: description,
  );
  if (selectedTargets == null || !context.mounted) return false;
  if (selectedTargets.length == initialTargets.length &&
      selectedTargets.containsAll(initialTargets)) {
    return true;
  }

  final success = await syncProvider.setNodeControlTargets(
    sourceNodeId: device.id,
    controlKind: controlKind,
    targetNodeIds: selectedTargets,
  );
  if (controlKind == 'button') {
    unawaited(
      AnalyticsService().logButtonControlTargetsSaveCompleted(
        journeyId: 'button-control-targets-${_deviceControlTargetsUuid.v4()}',
        source: 'device_detail',
        targetCountBucket: _targetCountBucket(selectedTargets.length),
        outcome: success ? 'succeeded' : 'failed',
        failureStage: success ? null : 'request_or_refresh',
      ),
    );
  }
  if (!context.mounted) return success;

  ScaffoldMessenger.of(context).showSnackBar(
    SnackBar(
      content: Text(
        success
            ? 'Updated ${device.displayName} $controlKind controls'
            : 'Failed to update ${device.displayName}',
      ),
    ),
  );
  return success;
}

String _targetCountBucket(int count) => switch (count) {
      <= 0 => 'none',
      1 => 'one',
      2 || 3 => 'two_to_three',
      _ => 'four_plus',
    };

/// Bottom sheet showing canonical device details + connections.
///
/// Opened by tapping a device row in [RoomSettingsSheet].
/// Tabs on the device (bulb) detail sheet — mirrors the room settings sheet.
enum _DeviceTab { settings, network, info }

enum _HueBleRemovalFailureChoice { cancel, retry, forget }

typedef _DeviceEndpoint = ({
  String hubType,
  String hubAddress,
  String nativeId,
});

class DeviceDetailSheet extends StatefulWidget {
  final RhythmDevice device;
  final String roomId;
  final String? parentRoomId;

  const DeviceDetailSheet({
    super.key,
    required this.device,
    required this.roomId,
    this.parentRoomId,
  });

  static Future<void> show(
    BuildContext context,
    RhythmDevice device,
    String roomId, {
    String? parentRoomId,
  }) {
    HapticFeedback.lightImpact();
    return showModalBottomSheet(
      context: context,
      isScrollControlled: true,
      backgroundColor: Colors.transparent,
      builder: (context) => DeviceDetailSheet(
        device: device,
        roomId: roomId,
        parentRoomId: parentRoomId,
      ),
    );
  }

  @override
  State<DeviceDetailSheet> createState() => _DeviceDetailSheetState();
}

class _DeviceDetailSheetState extends State<DeviceDetailSheet> {
  Map<String, dynamic>? _canonicalData;
  bool _loading = true;
  bool _moving = false;
  bool _loadingMatterSetupCode = false;
  _DeviceEndpoint? _removingEndpoint;
  _DeviceTab _selectedTab = _DeviceTab.settings;

  @override
  void initState() {
    super.initState();
    _loadCanonicalData();
  }

  Future<void> _loadCanonicalData() async {
    final http = context.read<ServerSyncProvider>().api;
    final data = await http.getCanonicalDevice(widget.device.id);
    if (mounted) {
      setState(() {
        _canonicalData = data;
        _loading = false;
      });
    }
  }

  String get _deviceDisplayName {
    final canonicalName = (_canonicalData?['name'] as String?)?.trim();
    if (canonicalName != null && canonicalName.isNotEmpty) {
      return canonicalName;
    }
    return widget.device.displayName;
  }

  @override
  Widget build(BuildContext context) {
    final topPad = MediaQuery.of(context).padding.top;
    final device = widget.device;
    final deviceDisplayName = _deviceDisplayName;
    final (icon, iconColor) = _iconForType(device.type);
    final canUnpairMatter = context.select<ServerSyncProvider, bool>(
      (sync) => sync.canUnpairMatterDevices,
    );
    final canRecoverMatterSetupCode = context.select<ServerSyncProvider, bool>(
      (sync) => sync.canRecoverMatterSetupCode,
    );
    final canUnpairHueBle = context.select<ServerSyncProvider, bool>(
      (sync) => sync.canUnpairHueBleDevices,
    );
    final canUnpairLocalBle = context.select<ServerSyncProvider, bool>(
      (sync) => sync.canUnpairLocalBleDevices,
    );
    final canUnpairHueBridge = context.select<ServerSyncProvider, bool>(
      (sync) => sync.canUnpairHueBridgeDeviceType(device.type),
    );
    // Low glow and profile overrides are node-level settings. Only expose
    // them for bulbs that the server reports as independently addressable
    // light nodes.
    final isLightNode = device.type == RhythmDeviceType.light &&
        context.select<ServerSyncProvider, bool>(
          (sync) => sync.nodeById(device.id) != null,
        );
    final lightSettingsSupported = isLightNode &&
        context.select<ServerSyncProvider, bool>(
          (sync) => sync.lightProfileOverridesSupportedForNode(device.id),
        );
    final individualProfileRoute = isLightNode
        ? context.select<ServerSyncProvider, bool?>(
            (sync) => sync
                .nodeById(device.id)
                ?.lightCapabilities
                ?.individualProfileOverrides,
          )
        : null;
    final groupedLightSettings = isLightNode && individualProfileRoute == false;
    final hasLightOverrides = isLightNode &&
        context.select<ServerSyncProvider, bool>(
          (sync) => sync.hasNodeLightProfileOverrides(device.id),
        );

    return PopScope(
      canPop: _removingEndpoint == null && !_moving,
      child: Padding(
        padding: EdgeInsets.only(top: topPad + 100),
        child: Container(
          decoration: const BoxDecoration(
            color: CelestialColors.backgroundCard,
            borderRadius: BorderRadius.vertical(top: Radius.circular(24)),
            boxShadow: [
              BoxShadow(
                color: Color(0x40000000),
                blurRadius: 20,
                offset: Offset(0, -4),
              ),
            ],
          ),
          child: Column(
            children: [
              // Drag handle
              Padding(
                padding: const EdgeInsets.only(top: 12, bottom: 8),
                child: Container(
                  width: 36,
                  height: 4,
                  decoration: BoxDecoration(
                    color: CelestialColors.orbitRing,
                    borderRadius: BorderRadius.circular(2),
                  ),
                ),
              ),
              // Device icon + name
              Padding(
                padding: const EdgeInsets.fromLTRB(20, 16, 20, 16),
                child: Column(
                  children: [
                    Container(
                      width: 56,
                      height: 56,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        color: iconColor.withValues(alpha: 0.15),
                        border: Border.all(
                          color: iconColor.withValues(alpha: 0.4),
                          width: 2,
                        ),
                      ),
                      child: Icon(icon, color: iconColor, size: 24),
                    ),
                    const SizedBox(height: 12),
                    // The name is a plain title here; renaming lives on the
                    // Device Info row (Info tab) with a pencil.
                    _DeviceNameButton(
                      label: deviceDisplayName,
                      onTap: null,
                    ),
                    if (device.productInfo != null)
                      Padding(
                        padding: const EdgeInsets.only(top: 4),
                        child: Text(
                          device.productInfo!,
                          textAlign: TextAlign.center,
                          style: TextStyle(
                            color: CelestialColors.textSecondary
                                .withValues(alpha: 0.7),
                            fontSize: 13,
                          ),
                        ),
                      ),
                  ],
                ),
              ),
              // Tab selector
              Padding(
                padding: const EdgeInsets.symmetric(horizontal: 20),
                child: SegmentedTabBar<_DeviceTab>(
                  selected: _selectedTab,
                  onChanged: (tab) => setState(() => _selectedTab = tab),
                  tabs: const [
                    SegmentedTab('Settings', _DeviceTab.settings),
                    SegmentedTab('Network', _DeviceTab.network),
                    SegmentedTab('Info', _DeviceTab.info),
                  ],
                ),
              ),
              const SizedBox(height: 16),
              // Tab content — Expanded so the sheet keeps a stable height across
              // tabs (short tabs fill/scroll instead of shrinking the card).
              Expanded(
                child: AnimatedSwitcher(
                  duration: const Duration(milliseconds: 200),
                  child: switch (_selectedTab) {
                    _DeviceTab.settings => _buildSettingsTab(
                        context,
                        device,
                        isLightNode,
                        lightSettingsSupported,
                        hasLightOverrides,
                        groupedLightSettings,
                      ),
                    _DeviceTab.network => _buildNetworkTab(
                        context,
                        device,
                        canUnpairMatter,
                        canRecoverMatterSetupCode,
                        canUnpairHueBle,
                        canUnpairLocalBle,
                        canUnpairHueBridge,
                      ),
                    _DeviceTab.info => _buildInfoTab(context, device),
                  },
                ),
              ),
              // Done button
              Padding(
                padding: EdgeInsets.fromLTRB(
                  20,
                  12,
                  20,
                  MediaQuery.of(context).padding.bottom + 16,
                ),
                child: SizedBox(
                  width: double.infinity,
                  height: 50,
                  child: ElevatedButton(
                    onPressed: _removingEndpoint == null
                        ? _moving
                            ? null
                            : () => Navigator.of(context).pop()
                        : null,
                    style: ElevatedButton.styleFrom(
                      backgroundColor: CelestialColors.sunWarm,
                      foregroundColor: CelestialColors.backgroundDark,
                      shape: RoundedRectangleBorder(
                        borderRadius: BorderRadius.circular(25),
                      ),
                      elevation: 0,
                    ),
                    child: const Text(
                      'DONE',
                      style: TextStyle(
                        fontSize: 15,
                        fontWeight: FontWeight.w700,
                        letterSpacing: 2.0,
                      ),
                    ),
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _buildInfoSection(BuildContext context, RhythmDevice device) {
    final typeLabel = switch (device.type) {
      RhythmDeviceType.light => 'Light',
      RhythmDeviceType.button => 'Button',
      RhythmDeviceType.motion => 'Motion Sensor',
      RhythmDeviceType.contact => 'Contact Sensor',
    };

    final rhythmId = _canonicalData?['id'] as String?;

    return _buildGroup('Device Info', [
      // The canonical rename endpoint owns names for every device type.
      _buildNameEditRow(context),
      _InfoRow(label: 'Type', value: typeLabel),
      if (device.manufacturer != null)
        _InfoRow(label: 'Manufacturer', value: device.manufacturer!),
      if (device.model != null) _InfoRow(label: 'Model', value: device.model!),
      if (rhythmId != null)
        _InfoRow(
          label: 'Rhythm ID',
          value: rhythmId.length > 12
              ? '${rhythmId.substring(0, 12)}...'
              : rhythmId,
        ),
    ]);
  }

  void _setStandbyEnabled(RhythmDevice device, bool enabled) {
    final sync = context.read<ServerSyncProvider>();
    sync.setNodeStandbyEnabledLocal(device.id, enabled);
    sync.pushNodePreferences(device.id, standbyEnabled: enabled);
    HapticFeedback.selectionClick();
  }

  void _openBulbLightSettings(RhythmDevice device) {
    HapticFeedback.lightImpact();
    LightScreen.showForBulb(
      context,
      nodeId: device.id,
      bulbName: _deviceDisplayName,
    );
  }

  void _showBulbLightSettingsUnavailable() {
    HapticFeedback.lightImpact();
    final version = context.read<ServerSyncProvider>().firmwareVersion;
    final versionSuffix = version == '0.0.0' ? '' : ' ($version)';
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(
          'Update the Rhythm appliance$versionSuffix to customize lighting '
          'for $_deviceDisplayName.',
        ),
      ),
    );
  }

  // ── Tab content ──────────────────────────────────────────────────────────

  /// Settings tab: Low glow (Standby preference) + Move.
  Widget _buildSettingsTab(
    BuildContext context,
    RhythmDevice device,
    bool isLightNode,
    bool lightSettingsSupported,
    bool hasLightOverrides,
    bool groupedLightSettings,
  ) {
    final standbyEnabled = context.select<ServerSyncProvider, bool>(
      (sync) => sync.standbyEnabledForNode(device.id),
    );
    final buttonMultiRoomControlsSupported =
        context.select<ServerSyncProvider, bool>(
      (sync) => sync.buttonMultiRoomControlsSupported,
    );
    return ListView(
      key: const ValueKey('settings'),
      padding: const EdgeInsets.fromLTRB(20, 0, 20, 16),
      children: [
        if (isLightNode) ...[
          _buildGroup('', [
            LightingOverrideRow(
              nodeId: device.id,
              supported: lightSettingsSupported,
              customized: hasLightOverrides,
              settingsKeyPrefix: 'device-settings-light',
              unsupportedStatus: groupedLightSettings ? 'Room only' : null,
              unsupportedSemanticsValue:
                  groupedLightSettings ? 'Controlled by room' : null,
              unsupportedIcon: groupedLightSettings ? Icons.home_rounded : null,
              onPressed: lightSettingsSupported
                  ? () => _openBulbLightSettings(device)
                  : groupedLightSettings
                      ? null
                      : _showBulbLightSettingsUnavailable,
            ),
            LowGlowSettingRow(
              value: standbyEnabled,
              onChanged: (value) => _setStandbyEnabled(device, value),
            ),
          ]),
          const SizedBox(height: 16),
        ],
        if (device.type == RhythmDeviceType.motion) ...[
          _buildMotionTargetsButton(context),
          const SizedBox(height: 12),
        ],
        if (device.type == RhythmDeviceType.button &&
            buttonMultiRoomControlsSupported) ...[
          _buildButtonTargetsButton(context),
          const SizedBox(height: 12),
        ],
        if (_loading)
          _tabLoadingIndicator()
        else if (_canonicalData != null)
          _buildMoveButton(context),
      ],
    );
  }

  /// Network tab: how this device connects (its hub / bridge), the bulb tester
  /// for Matter lights, Identify, and Remove.
  Widget _buildNetworkTab(
    BuildContext context,
    RhythmDevice device,
    bool canUnpairMatter,
    bool canRecoverMatterSetupCode,
    bool canUnpairHueBle,
    bool canUnpairLocalBle,
    bool canUnpairHueBridge,
  ) {
    final isLight = device.type == RhythmDeviceType.light;
    final removableEndpoints = _removableEndpoints.where((endpoint) {
      return switch (endpoint.hubType) {
        'matter' => canUnpairMatter,
        'hue_ble' => canUnpairHueBle,
        'local_ble' => canUnpairLocalBle,
        'hue' => canUnpairHueBridge,
        _ => false,
      };
    }).toList(growable: false);
    final hasMultipleConnections = _allEndpoints.length > 1;
    return ListView(
      key: const ValueKey('network'),
      padding: const EdgeInsets.fromLTRB(20, 0, 20, 16),
      children: [
        if (_loading)
          _tabLoadingIndicator()
        else if (_canonicalData != null) ...[
          _buildConnectionsSection(),
          if (isLight && _matterNativeId != null) ...[
            const SizedBox(height: 12),
            if (canRecoverMatterSetupCode) ...[
              _buildMatterSetupCodeButton(context, _matterNativeId!),
              const SizedBox(height: 12),
            ],
            _buildMatterTesterButton(context, _matterNativeId!),
          ],
          if (isLight) ...[
            const SizedBox(height: 12),
            _FlashButton(
              deviceLabel: _deviceDisplayName,
              onFlash: () => identifyCanonicalBulb(
                context,
                device: device,
                source: 'network_button',
              ),
            ),
          ],
          for (final endpoint in removableEndpoints) ...[
            const SizedBox(height: 12),
            _buildRemoveButton(
              context,
              endpoint,
              removesConnectionOnly: hasMultipleConnections,
            ),
          ],
        ] else
          _tabHint('No connection info available.'),
      ],
    );
  }

  /// Info tab: read-only device details.
  Widget _buildInfoTab(BuildContext context, RhythmDevice device) {
    return ListView(
      key: const ValueKey('info'),
      padding: const EdgeInsets.fromLTRB(20, 0, 20, 16),
      children: [
        _buildInfoSection(context, device),
      ],
    );
  }

  /// Editable "Name" row for the Device Info group — value plus a pencil that
  /// opens the canonical-device rename dialog.
  Widget _buildNameEditRow(BuildContext context) {
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: () => _showRenameDialog(context),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 11),
        child: Row(
          children: [
            const Text(
              'Name',
              style: TextStyle(
                color: CelestialColors.textSecondary,
                fontSize: 14,
              ),
            ),
            const SizedBox(width: 16),
            Expanded(
              child: Text(
                _deviceDisplayName,
                textAlign: TextAlign.right,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 14,
                ),
              ),
            ),
            const SizedBox(width: 8),
            Icon(
              Icons.edit_outlined,
              size: 16,
              color: CelestialColors.sunWarm.withValues(alpha: 0.85),
            ),
          ],
        ),
      ),
    );
  }

  Widget _tabLoadingIndicator() => const Center(
        child: Padding(
          padding: EdgeInsets.all(20),
          child: SizedBox(
            width: 20,
            height: 20,
            child: CircularProgressIndicator(
              strokeWidth: 2,
              color: CelestialColors.sunWarm,
            ),
          ),
        ),
      );

  Widget _tabHint(String text) => Padding(
        padding: const EdgeInsets.all(20),
        child: Center(
          child: Text(
            text,
            textAlign: TextAlign.center,
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.5),
              fontSize: 13,
            ),
          ),
        ),
      );

  Future<void> _showRenameDialog(BuildContext context) async {
    final deviceLabel = switch (widget.device.type) {
      RhythmDeviceType.light => 'Bulb',
      RhythmDeviceType.button => 'Button',
      RhythmDeviceType.motion => 'Motion Sensor',
      RhythmDeviceType.contact => 'Contact Sensor',
    };
    final controller = TextEditingController(text: _deviceDisplayName);
    final newName = await showDialog<String>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: Text(
          'Rename $deviceLabel',
          style: const TextStyle(color: CelestialColors.textPrimary),
        ),
        content: TextField(
          controller: controller,
          autofocus: true,
          style: const TextStyle(color: CelestialColors.textPrimary),
          decoration: InputDecoration(
            hintText: '$deviceLabel name',
            hintStyle: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.5),
            ),
            enabledBorder: UnderlineInputBorder(
              borderSide: BorderSide(
                color: CelestialColors.orbitRing.withValues(alpha: 0.3),
              ),
            ),
            focusedBorder: const UnderlineInputBorder(
              borderSide: BorderSide(color: CelestialColors.sunWarm),
            ),
          ),
          onSubmitted: (value) => Navigator.of(ctx).pop(value.trim()),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(),
            child: Text(
              'Cancel',
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.7),
              ),
            ),
          ),
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(controller.text.trim()),
            child: const Text(
              'Rename',
              style: TextStyle(color: CelestialColors.sunWarm),
            ),
          ),
        ],
      ),
    );

    if (newName == null ||
        newName.isEmpty ||
        newName == _deviceDisplayName ||
        !context.mounted) {
      return;
    }

    final syncProvider = context.read<ServerSyncProvider>();
    final success =
        await syncProvider.api.renameCanonicalDevice(widget.device.id, newName);
    if (!context.mounted) return;

    if (!success) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text('Failed to rename ${deviceLabel.toLowerCase()}'),
        ),
      );
      return;
    }

    setState(() {
      _canonicalData = {
        ...?_canonicalData,
        'name': newName,
      };
    });
    syncProvider.api.triggerSync();
  }

  Widget _buildConnectionsSection() {
    final endpoints = (_canonicalData?['endpoints'] as List<dynamic>?) ?? [];
    if (endpoints.isEmpty) return const SizedBox.shrink();

    return _buildGroup('Connections', [
      for (final ep in endpoints)
        _ConnectionRow(
          hubKey: (ep['hub_key'] as Map<String, dynamic>?) ?? {},
          nativeId: ep['native_id'] as String? ?? '',
          preferred: ep['preferred'] as bool? ?? false,
        ),
    ]);
  }

  Widget _buildMoveButton(BuildContext context) {
    final syncProvider = context.read<ServerSyncProvider>();
    final isAssigned = widget.roomId.isNotEmpty;
    final canLeaveUnassigned = isAssigned && _canLeaveUnassigned(syncProvider);
    final label = isAssigned
        ? canLeaveUnassigned
            ? 'Move or Remove...'
            : 'Move to Room...'
        : 'Assign to Room...';

    return Semantics(
      key: ValueKey('device-room-move-button-${widget.device.id}'),
      button: true,
      enabled: !_moving,
      label: _moving ? 'Updating room' : label,
      child: GestureDetector(
        onTap: _moving ? null : () => _showMoveDialog(context),
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
          decoration: BoxDecoration(
            color: CelestialColors.backgroundDark.withValues(alpha: 0.5),
            borderRadius: BorderRadius.circular(14),
            border: Border.all(
              color: CelestialColors.orbitRing.withValues(alpha: 0.3),
            ),
          ),
          child: Row(
            children: [
              if (_moving)
                const SizedBox(
                  key: ValueKey('device-room-move-progress'),
                  width: 20,
                  height: 20,
                  child: CircularProgressIndicator(
                    strokeWidth: 2,
                    color: CelestialColors.sunWarm,
                  ),
                )
              else
                Icon(
                  Icons.swap_horiz,
                  color: CelestialColors.sunWarm.withValues(alpha: 0.8),
                  size: 20,
                ),
              const SizedBox(width: 12),
              Expanded(
                child: Text(
                  _moving ? 'Updating room…' : label,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 15,
                  ),
                ),
              ),
              if (!_moving)
                const Icon(
                  Icons.chevron_right,
                  color: CelestialColors.textSecondary,
                  size: 20,
                ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _buildMotionTargetsButton(BuildContext context) {
    return _buildControlTargetsButton(
      context,
      controlKind: 'motion',
      label: 'Motion controls',
      icon: Icons.sensor_occupied_outlined,
      iconColor: const Color(0xFF81C784),
      onTap: () => _showMotionTargetsDialog(context),
    );
  }

  Widget _buildButtonTargetsButton(BuildContext context) {
    return _buildControlTargetsButton(
      context,
      controlKind: 'button',
      label: 'Button controls',
      icon: Icons.smart_button_outlined,
      iconColor: CelestialColors.sunWarm,
      onTap: () => _showButtonTargetsDialog(context),
    );
  }

  Widget _buildControlTargetsButton(
    BuildContext context, {
    required String controlKind,
    required String label,
    required IconData icon,
    required Color iconColor,
    required VoidCallback onTap,
  }) {
    final targetCount = context.select<ServerSyncProvider, int>(
      (sync) => sync
          .controlTargetNodeIds(
            sourceNodeId: widget.device.id,
            controlKind: controlKind,
          )
          .length,
    );
    final targetSummary = switch (targetCount) {
      0 => widget.roomId.isEmpty ? 'Choose rooms' : 'Current room',
      1 => '1 room',
      _ => '$targetCount rooms',
    };

    return Semantics(
      button: true,
      label: '$label, $targetSummary',
      child: InkWell(
        onTap: onTap,
        borderRadius: BorderRadius.circular(14),
        child: Ink(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
          decoration: BoxDecoration(
            color: CelestialColors.backgroundDark.withValues(alpha: 0.5),
            borderRadius: BorderRadius.circular(14),
            border: Border.all(
              color: CelestialColors.orbitRing.withValues(alpha: 0.3),
            ),
          ),
          child: Row(
            children: [
              Icon(
                icon,
                color: iconColor.withValues(alpha: 0.9),
                size: 20,
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Text(
                  label,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 15,
                  ),
                ),
              ),
              Text(
                targetSummary,
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                  fontSize: 13,
                ),
              ),
              const SizedBox(width: 4),
              const Icon(
                Icons.chevron_right,
                color: CelestialColors.textSecondary,
                size: 20,
              ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _buildMatterTesterButton(BuildContext context, String nativeId) {
    return GestureDetector(
      onTap: () {
        Navigator.of(context).push(
          MaterialPageRoute(
            builder: (_) => MatterBulbTesterScreen(
              device: widget.device,
              nativeDeviceId: nativeId,
            ),
          ),
        );
      },
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
        decoration: BoxDecoration(
          color: CelestialColors.backgroundDark.withValues(alpha: 0.5),
          borderRadius: BorderRadius.circular(14),
          border: Border.all(
            color: CelestialColors.orbitRing.withValues(alpha: 0.3),
          ),
        ),
        child: Row(
          children: [
            Icon(
              Icons.science_outlined,
              color: CelestialColors.accentBlue.withValues(alpha: 0.9),
              size: 20,
            ),
            const SizedBox(width: 12),
            const Expanded(
              child: Text(
                'Matter Bulb Tester',
                style: TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 15,
                ),
              ),
            ),
            const Icon(
              Icons.chevron_right,
              color: CelestialColors.textSecondary,
              size: 20,
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildMatterSetupCodeButton(BuildContext context, String nativeId) {
    return GestureDetector(
      key: const ValueKey('matter-setup-code-recovery'),
      onTap: _loadingMatterSetupCode
          ? null
          : () => _showMatterSetupCode(context, nativeId),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
        decoration: BoxDecoration(
          color: CelestialColors.backgroundDark.withValues(alpha: 0.5),
          borderRadius: BorderRadius.circular(14),
          border: Border.all(
            color: CelestialColors.orbitRing.withValues(alpha: 0.3),
          ),
        ),
        child: Row(
          children: [
            if (_loadingMatterSetupCode)
              const SizedBox(
                width: 20,
                height: 20,
                child: CircularProgressIndicator(strokeWidth: 2),
              )
            else
              Icon(
                Icons.key_outlined,
                color: CelestialColors.sunWarm.withValues(alpha: 0.9),
                size: 20,
              ),
            const SizedBox(width: 12),
            const Expanded(
              child: Text(
                'Matter setup code',
                style: TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 15,
                ),
              ),
            ),
            const Icon(
              Icons.chevron_right,
              color: CelestialColors.textSecondary,
              size: 20,
            ),
          ],
        ),
      ),
    );
  }

  Future<void> _showMatterSetupCode(
    BuildContext context,
    String nativeId,
  ) async {
    const source = 'device_network';
    setState(() => _loadingMatterSetupCode = true);
    unawaited(
      AnalyticsService().logMatterSetupCodeRecoveryAttempted(source: source),
    );

    RhythmPairingRecoverySecret? secret;
    String? failureStage;
    try {
      secret = await context
          .read<ServerSyncProvider>()
          .api
          .getMatterSetupCode(nativeId);
      if (secret == null) failureStage = 'not_available';
    } catch (_) {
      failureStage = 'request';
    } finally {
      if (mounted) setState(() => _loadingMatterSetupCode = false);
    }
    if (!context.mounted) return;

    if (secret == null) {
      unawaited(
        AnalyticsService().logMatterSetupCodeRecoveryCompleted(
          source: source,
          outcome: 'failed',
          failureStage: failureStage,
        ),
      );
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content:
              Text('No saved Matter setup code is available for this device.'),
        ),
      );
      return;
    }

    unawaited(
      AnalyticsService().logMatterSetupCodeRecoveryCompleted(
        source: source,
        outcome: 'succeeded',
      ),
    );
    await _showMatterSetupCodeDialog(context, secret);
  }

  Future<void> _showMatterSetupCodeDialog(
    BuildContext context,
    RhythmPairingRecoverySecret secret,
  ) async {
    final messenger = ScaffoldMessenger.of(context);
    await showDialog<void>(
      context: context,
      builder: (_) => MatterSetupCodeDialog(
        secret: secret,
        onCopied: () => messenger.showSnackBar(
          const SnackBar(content: Text('Matter setup code copied')),
        ),
      ),
    );
  }

  List<Map<String, dynamic>> get _allEndpoints {
    final endpoints = _canonicalData?['endpoints'] as List<dynamic>? ?? [];
    return [
      for (final endpoint in endpoints)
        if (endpoint is Map<String, dynamic>) endpoint,
    ];
  }

  _DeviceEndpoint? _endpointForHub(String hubType) {
    for (final endpoint in _allEndpoints) {
      final hubKey = endpoint['hub_key'] as Map<String, dynamic>? ?? const {};
      if (hubKey['hub_type']?.toString() != hubType) continue;
      final hubAddress = hubKey['address']?.toString() ?? '';
      final nativeId = endpoint['native_id']?.toString() ?? '';
      if (nativeId.isNotEmpty) {
        return (
          hubType: hubType,
          hubAddress: hubAddress,
          nativeId: nativeId,
        );
      }
    }
    return null;
  }

  /// The Matter native ID from the canonical endpoints, when present.
  String? get _matterNativeId => _endpointForHub('matter')?.nativeId;

  /// Endpoint connections the appliance can remove through their integration.
  ///
  /// Keep this as a list instead of picking a preferred transport: one
  /// canonical device may intentionally contain Bridge, Matter, and BLE paths.
  List<_DeviceEndpoint> get _removableEndpoints {
    final removable = <_DeviceEndpoint>[];
    for (final endpoint in _allEndpoints) {
      final hubKey = endpoint['hub_key'] as Map<String, dynamic>? ?? const {};
      final hubType = hubKey['hub_type']?.toString() ?? '';
      final hubAddress = hubKey['address']?.toString() ?? '';
      final nativeId = endpoint['native_id']?.toString() ?? '';
      if ((hubType == 'matter' ||
              hubType == 'hue_ble' ||
              hubType == 'local_ble' ||
              hubType == 'hue') &&
          nativeId.isNotEmpty) {
        removable.add((
          hubType: hubType,
          hubAddress: hubAddress,
          nativeId: nativeId,
        ));
      }
    }
    return removable;
  }

  String _endpointLabel(_DeviceEndpoint endpoint) => switch (endpoint.hubType) {
        'hue' => 'Hue Bridge',
        'hue_ble' => 'Hue Bluetooth',
        'local_ble' => 'Local Bluetooth',
        _ => 'Matter',
      };

  String get _deviceTypeAnalyticsLabel => switch (widget.device.type) {
        RhythmDeviceType.light => 'light',
        RhythmDeviceType.button => 'button',
        RhythmDeviceType.motion => 'motion',
        RhythmDeviceType.contact => 'contact',
      };

  String get _hueRemovalNoun => switch (widget.device.type) {
        RhythmDeviceType.light => 'bulb',
        RhythmDeviceType.button => 'switch',
        RhythmDeviceType.motion => 'motion sensor',
        RhythmDeviceType.contact => 'contact sensor',
      };

  String _removeConfirmation(
    _DeviceEndpoint endpoint, {
    required bool removesConnectionOnly,
  }) {
    final name = _deviceDisplayName;
    final connectionLabel = _endpointLabel(endpoint);
    if (endpoint.hubType == 'hue') {
      final otherConnectionNote = removesConnectionOnly
          ? ' The device will remain in Rhythm through its other connection.'
          : '';
      return 'This will ask your Hue Bridge to remove "$name", then remove '
          'the $connectionLabel connection from Rhythm.$otherConnectionNote'
          '\n\nIf bridge removal does not return cleanly, Rhythm can check '
          'whether the $_hueRemovalNoun is already absent before finishing '
          'cleanup.';
    }
    if (removesConnectionOnly) {
      final handoffNote = endpoint.hubType == 'hue_ble'
          ? '\n\nKeep the bulb powered on and nearby while Rhythm performs an '
              'authenticated Bluetooth release. The bulb will become '
              'discoverable and can be paired again without a factory reset.'
          : '';
      return 'This will remove the $connectionLabel connection from "$name". '
          'The device will remain in Rhythm through its other '
          'connection.$handoffNote';
    }
    if (endpoint.hubType == 'hue_ble') {
      return 'This will ask "$name" to perform an authenticated Bluetooth '
          'release, then remove it from Rhythm.\n\nKeep the bulb powered on '
          'and nearby. After removal, it can be paired again without a '
          'factory reset.';
    }
    if (endpoint.hubType == 'local_ble') {
      return 'This will remove "$name" from Rhythm. Put the device back in '
          'pairing mode and scan its setup code to add it again.';
    }
    return 'This will decommission "$name" and remove it from your system. '
        'The device can be re-paired afterwards.\n\nIf the device is offline '
        'this can take a minute, after which you can force-remove it.';
  }

  Widget _buildRemoveButton(
    BuildContext context,
    _DeviceEndpoint endpoint, {
    required bool removesConnectionOnly,
  }) {
    final isRemoving = _removingEndpoint == endpoint;
    final removalInProgress = _removingEndpoint != null;
    final idleLabel = removesConnectionOnly
        ? 'Remove ${_endpointLabel(endpoint)} Connection'
        : 'Remove Device';
    return GestureDetector(
      key: ValueKey(
        'remove-device-endpoint-${endpoint.hubType}-${endpoint.nativeId}',
      ),
      onTap: removalInProgress
          ? null
          : () => _confirmRemoveDevice(
                context,
                endpoint,
                removesConnectionOnly: removesConnectionOnly,
              ),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
        decoration: BoxDecoration(
          color: Colors.red.shade900.withValues(alpha: 0.3),
          borderRadius: BorderRadius.circular(14),
          border: Border.all(
            color: Colors.red.shade400.withValues(alpha: 0.4),
          ),
        ),
        child: Row(
          children: [
            if (isRemoving)
              SizedBox(
                width: 18,
                height: 18,
                child: CircularProgressIndicator(
                  strokeWidth: 2,
                  color: Colors.red.shade300,
                ),
              )
            else
              Icon(
                Icons.delete_outline,
                color: Colors.red.shade300,
                size: 20,
              ),
            const SizedBox(width: 12),
            Expanded(
              child: Text(
                isRemoving ? 'Removing...' : idleLabel,
                style: TextStyle(
                  color: Colors.red.shade300,
                  fontSize: 15,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Future<void> _confirmRemoveDevice(
    BuildContext context,
    _DeviceEndpoint endpoint, {
    required bool removesConnectionOnly,
  }) async {
    final actionLabel = removesConnectionOnly
        ? 'Remove ${_endpointLabel(endpoint)} Connection'
        : 'Remove Device';
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: Text(
          '$actionLabel?',
          style: const TextStyle(color: CelestialColors.textPrimary),
        ),
        content: Text(
          _removeConfirmation(
            endpoint,
            removesConnectionOnly: removesConnectionOnly,
          ),
          style: const TextStyle(color: CelestialColors.textSecondary),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: const Text('Cancel'),
          ),
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: Text('Remove', style: TextStyle(color: Colors.red.shade300)),
          ),
        ],
      ),
    );

    if (confirmed != true || !context.mounted) return;
    await _removeDevice(
      context,
      endpoint,
      removesConnectionOnly: removesConnectionOnly,
    );
  }

  Future<void> _removeDevice(
    BuildContext context,
    _DeviceEndpoint endpoint, {
    required bool removesConnectionOnly,
  }) async {
    final syncProvider = context.read<ServerSyncProvider>();
    final api = syncProvider.api;
    final hueJourneyId = endpoint.hubType == 'hue'
        ? 'hue-bridge-remove-${const Uuid().v4()}'
        : null;

    var retryHueBleRelease = false;
    var forceAttempted = false;
    Map<String, dynamic>? completionResult;
    late MatterRemovalOutcome outcome;
    do {
      retryHueBleRelease = false;
      outcome = await runMatterRemovalFlow(
        unpair: ({required bool force}) {
          forceAttempted = forceAttempted || force;
          if (mounted) setState(() => _removingEndpoint = endpoint);
          return api.unpairDevice(
            hubType: endpoint.hubType,
            deviceId: endpoint.nativeId,
            hubAddress:
                endpoint.hubType == 'hue' && endpoint.hubAddress.isNotEmpty
                    ? endpoint.hubAddress
                    : null,
            deviceType: _deviceTypeAnalyticsLabel,
            correlationId: hueJourneyId,
            force: force,
          );
        },
        confirmForceRemove: (error) async {
          if (!context.mounted) return false;
          setState(() => _removingEndpoint = null);

          if (endpoint.hubType == 'hue_ble') {
            final choice = await showDialog<_HueBleRemovalFailureChoice>(
              context: context,
              builder: (ctx) => AlertDialog(
                backgroundColor: CelestialColors.backgroundCard,
                title: const Text(
                  'Couldn’t release the bulb',
                  style: TextStyle(color: CelestialColors.textPrimary),
                ),
                content: Text(
                  '$error\n\nKeep the bulb powered on and nearby. Try Again '
                  'attempts the authenticated Bluetooth release again.\n\n'
                  'Forget Anyway lets Rhythm retry the release, then remove '
                  'only its local device record if release is still '
                  'unavailable. If this Rhythm Box retains the Bluetooth '
                  'bond, Rhythm can explicitly re-adopt it later. Moving the '
                  'bulb to another controller may require a factory reset if '
                  'the old key cannot be released.',
                  style: const TextStyle(color: CelestialColors.textSecondary),
                ),
                actions: [
                  TextButton(
                    onPressed: () => Navigator.of(ctx)
                        .pop(_HueBleRemovalFailureChoice.cancel),
                    child: const Text('Cancel'),
                  ),
                  TextButton(
                    onPressed: () => Navigator.of(ctx)
                        .pop(_HueBleRemovalFailureChoice.retry),
                    child: const Text('Try Again'),
                  ),
                  TextButton(
                    onPressed: () => Navigator.of(ctx)
                        .pop(_HueBleRemovalFailureChoice.forget),
                    child: Text(
                      'Forget Anyway',
                      style: TextStyle(color: Colors.red.shade300),
                    ),
                  ),
                ],
              ),
            );
            if (choice == _HueBleRemovalFailureChoice.retry) {
              retryHueBleRelease = true;
              return false;
            }
            return choice == _HueBleRemovalFailureChoice.forget &&
                context.mounted;
          }

          // A Hue Bridge "force" request is intentionally only an
          // absence-confirmation fallback: the server must not remove local
          // state while the bridge still owns the bulb, because the next sync
          // would recreate the endpoint.
          final isHueBridge = endpoint.hubType == 'hue';
          final forceRemovalExplanation = isHueBridge
              ? 'Check the Hue Bridge and finish removal? Rhythm will clean '
                  'up its connection only after the bridge confirms the '
                  '$_hueRemovalNoun is absent. If the bridge still owns it, '
                  'this fails safely.'
              : 'Force remove? This cleans up local state without contacting '
                  'the device.';
          final forceRemove = await showDialog<bool>(
            context: context,
            builder: (ctx) => AlertDialog(
              backgroundColor: CelestialColors.backgroundCard,
              title: Text(
                isHueBridge ? 'Removal Not Confirmed' : 'Removal Failed',
                style: TextStyle(color: CelestialColors.textPrimary),
              ),
              content: Text(
                '$error\n\n$forceRemovalExplanation',
                style: const TextStyle(color: CelestialColors.textSecondary),
              ),
              actions: [
                TextButton(
                  onPressed: () => Navigator.of(ctx).pop(false),
                  child: const Text('Cancel'),
                ),
                TextButton(
                  onPressed: () => Navigator.of(ctx).pop(true),
                  child: Text(
                      isHueBridge ? 'Check and Finish Removal' : 'Force Remove',
                      style: TextStyle(color: Colors.red.shade300)),
                ),
              ],
            ),
          );
          return forceRemove == true && context.mounted;
        },
        onComplete: (result) => completionResult = result,
      );
    } while (retryHueBleRelease && context.mounted);

    if (hueJourneyId != null) {
      unawaited(
        AnalyticsService().logHueBridgeDeviceRemovalCompleted(
          journeyId: hueJourneyId,
          deviceType: _deviceTypeAnalyticsLabel,
          outcome: outcome == MatterRemovalOutcome.removed
              ? 'succeeded'
              : 'cancelled',
          force: forceAttempted,
        ),
      );
    }

    if (!context.mounted) return;

    if (outcome == MatterRemovalOutcome.removed) {
      final lifecycleWarning = completionResult?['warning']?.toString().trim();
      await syncProvider.connection.reconnect();
      if (!context.mounted) return;
      if (removesConnectionOnly) {
        final remainingEndpoints = [
          for (final candidate in _allEndpoints)
            if (!_matchesEndpoint(candidate, endpoint)) candidate,
        ];
        setState(() {
          _canonicalData = {
            ...?_canonicalData,
            'endpoints': remainingEndpoints,
          };
          _removingEndpoint = null;
        });
        await _refreshCanonicalDataPreservingCurrent();
        if (!context.mounted) return;
        if (lifecycleWarning?.isNotEmpty == true) {
          await _showAcknowledgedRemovalWarning(
            context,
            title: 'Removed ${_endpointLabel(endpoint)} connection',
            warning: lifecycleWarning!,
          );
        } else {
          ScaffoldMessenger.of(context).showSnackBar(
            SnackBar(
              content: Text(
                'Removed ${_endpointLabel(endpoint)} connection from '
                '$_deviceDisplayName',
              ),
            ),
          );
        }
      } else {
        if (lifecycleWarning?.isNotEmpty == true) {
          setState(() => _removingEndpoint = null);
          await _showAcknowledgedRemovalWarning(
            context,
            title: 'Removed from Rhythm',
            warning: lifecycleWarning!,
          );
          if (!context.mounted) return;
          Navigator.of(context).pop();
          return;
        }
        final messenger = ScaffoldMessenger.of(context);
        Navigator.of(context).pop();
        messenger.showSnackBar(
          SnackBar(
            content: Text('Removed ${widget.device.displayName}'),
          ),
        );
      }
    } else {
      setState(() => _removingEndpoint = null);
    }
  }

  Future<void> _showAcknowledgedRemovalWarning(
    BuildContext context, {
    required String title,
    required String warning,
  }) {
    return showDialog<void>(
      context: context,
      barrierDismissible: false,
      builder: (ctx) => PopScope(
        canPop: false,
        child: AlertDialog(
          backgroundColor: CelestialColors.backgroundCard,
          title: Text(
            title,
            style: const TextStyle(color: CelestialColors.textPrimary),
          ),
          content: Text(
            warning,
            style: const TextStyle(color: CelestialColors.textSecondary),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(ctx).pop(),
              child: const Text('Done'),
            ),
          ],
        ),
      ),
    );
  }

  bool _matchesEndpoint(
    Map<String, dynamic> candidate,
    _DeviceEndpoint endpoint,
  ) {
    final hubKey = candidate['hub_key'] as Map<String, dynamic>? ?? const {};
    return hubKey['hub_type']?.toString() == endpoint.hubType &&
        (hubKey['address']?.toString() ?? '') == endpoint.hubAddress &&
        candidate['native_id']?.toString() == endpoint.nativeId;
  }

  Future<void> _refreshCanonicalDataPreservingCurrent() async {
    final data = await context
        .read<ServerSyncProvider>()
        .api
        .getCanonicalDevice(widget.device.id);
    if (!mounted || data == null) return;
    setState(() => _canonicalData = data);
  }

  Future<void> _showMoveDialog(BuildContext context) async {
    if (_moving) return;
    final syncProvider = context.read<ServerSyncProvider>();
    if (widget.device.type == RhythmDeviceType.light) {
      HapticFeedback.mediumImpact();
      unawaited(
        identifyCanonicalBulb(
          context,
          device: widget.device,
          source: 'move_to_room',
        ),
      );
    }

    var dismissing = false;
    try {
      final parentRoomId = widget.parentRoomId ?? widget.roomId;
      final isControlledRoomRelationship =
          widget.device.type == RhythmDeviceType.motion &&
              widget.roomId.isNotEmpty &&
              widget.roomId != parentRoomId &&
              syncProvider
                  .controlTargetNodeIds(
                    sourceNodeId: widget.device.id,
                    controlKind: 'motion',
                  )
                  .contains(widget.roomId);
      late final bool success;
      if (isControlledRoomRelationship) {
        success = await showDeviceControlRoomRelationshipFlow(
          context,
          device: widget.device,
          currentRoomId: widget.roomId,
          controlKind: 'motion',
          onAssignmentStarted: () {
            if (mounted) setState(() => _moving = true);
          },
        );
      } else {
        success = await showDeviceNodeAssignmentFlow(
          context,
          device: widget.device,
          currentParentNodeId: parentRoomId,
          allowNoRoom: _canLeaveUnassigned(syncProvider),
          onAssignmentStarted: () {
            if (mounted) setState(() => _moving = true);
          },
        );
      }

      if (success && context.mounted) {
        dismissing = true;
        Navigator.of(context).pop();
      } else if (_moving && context.mounted) {
        // The shared ScaffoldMessenger lives below this modal sheet. Dismiss
        // the sheet after a started assignment fails so its snackbar cannot be
        // hidden behind the card. A cancelled room picker leaves it open.
        dismissing = true;
        Navigator.of(context).pop();
      }
    } finally {
      if (!dismissing && mounted && _moving) {
        setState(() => _moving = false);
      }
    }
  }

  Future<void> _showMotionTargetsDialog(BuildContext context) async {
    await showMotionTargetRoomsFlow(
      context,
      device: widget.device,
      currentParentNodeId: widget.parentRoomId ?? widget.roomId,
    );
  }

  Future<void> _showButtonTargetsDialog(BuildContext context) async {
    await showButtonTargetRoomsFlow(
      context,
      device: widget.device,
      currentParentNodeId: widget.parentRoomId ?? widget.roomId,
    );
  }

  bool _canLeaveUnassigned(ServerSyncProvider syncProvider) {
    return switch (widget.device.type) {
      RhythmDeviceType.button ||
      RhythmDeviceType.motion ||
      RhythmDeviceType.contact =>
        true,
      RhythmDeviceType.light => _removableEndpoints.any(
          (endpoint) => switch (endpoint.hubType) {
            'matter' => syncProvider.supportsMatterRoomlessDevices,
            'hue_ble' => syncProvider.supportsHueBleRoomlessDevices,
            'local_ble' => syncProvider.supportsLocalBleRoomlessDevices,
            'hue' => false,
            _ => false,
          },
        ),
    };
  }

  Widget _buildGroup(String title, List<Widget> children) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (title.isNotEmpty)
          Padding(
            padding: const EdgeInsets.only(left: 4, bottom: 8),
            child: Text(
              title.toUpperCase(),
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.6),
                fontSize: 11,
                fontWeight: FontWeight.w600,
                letterSpacing: 1.5,
              ),
            ),
          ),
        Container(
          decoration: BoxDecoration(
            color: CelestialColors.backgroundDark.withValues(alpha: 0.5),
            borderRadius: BorderRadius.circular(14),
            border: Border.all(
              color: CelestialColors.orbitRing.withValues(alpha: 0.3),
              width: 1,
            ),
          ),
          child: Column(
            children: [
              for (int i = 0; i < children.length; i++) ...[
                children[i],
                if (i < children.length - 1)
                  Divider(
                    height: 1,
                    indent: 16,
                    color: CelestialColors.orbitRing.withValues(alpha: 0.2),
                  ),
              ],
            ],
          ),
        ),
      ],
    );
  }

  (IconData, Color) _iconForType(RhythmDeviceType type) => switch (type) {
        RhythmDeviceType.light => (
            Icons.lightbulb_outline,
            const Color(0xFFFFB74D)
          ),
        RhythmDeviceType.button => (
            Icons.touch_app_outlined,
            const Color(0xFF64B5F6)
          ),
        RhythmDeviceType.motion => (
            Icons.sensors_outlined,
            const Color(0xFF81C784)
          ),
        RhythmDeviceType.contact => (
            Icons.sensor_door_outlined,
            const Color(0xFFFFB74D)
          ),
      };
}

class _DeviceNameButton extends StatelessWidget {
  final String label;
  final VoidCallback? onTap;

  const _DeviceNameButton({
    required this.label,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final content = ConstrainedBox(
      constraints: BoxConstraints(
        maxWidth: MediaQuery.sizeOf(context).width - 64,
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          Flexible(
            fit: FlexFit.loose,
            child: Text(
              label,
              textAlign: TextAlign.center,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 18,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
          if (onTap != null) ...[
            const SizedBox(width: 6),
            Icon(
              Icons.edit_outlined,
              color: CelestialColors.textSecondary.withValues(alpha: 0.65),
              size: 16,
            ),
          ],
        ],
      ),
    );

    if (onTap == null) return content;

    return Semantics(
      button: true,
      label: 'Rename bulb',
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
          child: content,
        ),
      ),
    );
  }
}

class _InfoRow extends StatelessWidget {
  final String label;
  final String value;

  const _InfoRow({required this.label, required this.value});

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 11),
      child: Row(
        children: [
          Expanded(
            child: Text(
              label,
              style: const TextStyle(
                color: CelestialColors.textSecondary,
                fontSize: 14,
              ),
            ),
          ),
          Text(
            value,
            style: const TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 14,
            ),
          ),
        ],
      ),
    );
  }
}

/// Action button that asks the server to briefly pulse a Light so the user
/// can see which physical bulb maps to this entry. Plays a layered "sonar"
/// animation in sync with the request so the on-screen feedback feels
/// continuous with the bulb pulsing in the room.
class _FlashButton extends StatefulWidget {
  final String deviceLabel;
  final Future<bool> Function() onFlash;

  const _FlashButton({
    required this.deviceLabel,
    required this.onFlash,
  });

  @override
  State<_FlashButton> createState() => _FlashButtonState();
}

class _FlashButtonState extends State<_FlashButton>
    with SingleTickerProviderStateMixin {
  static const _amber = Color(0xFFFFB74D);
  static const _hot = Color(0xFFFFE082);

  late final AnimationController _flash;
  bool _busy = false;

  @override
  void initState() {
    super.initState();
    _flash = AnimationController(
      duration: const Duration(milliseconds: 950),
      vsync: this,
    );
  }

  @override
  void dispose() {
    _flash.dispose();
    super.dispose();
  }

  Future<void> _trigger() async {
    if (_busy) return;
    setState(() => _busy = true);
    HapticFeedback.mediumImpact();
    final animFuture = _flash.forward(from: 0);
    final success = await widget.onFlash();
    await animFuture;
    if (!mounted) return;
    if (!success) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Could not identify ${widget.deviceLabel}')),
      );
    }
    _flash.value = 0;
    setState(() => _busy = false);
  }

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: _busy ? null : _trigger,
      behavior: HitTestBehavior.opaque,
      child: AnimatedBuilder(
        animation: _flash,
        builder: (context, _) {
          final raw = _flash.value;
          // Bell curve: 0..0.5 ramp up, 0.5..1.0 fade out.
          final intensity = raw < 0.5
              ? Curves.easeOutCubic.transform(raw * 2)
              : Curves.easeInCubic.transform(1 - (raw - 0.5) * 2);
          return Container(
            decoration: BoxDecoration(
              color: CelestialColors.backgroundDark.withValues(alpha: 0.5),
              borderRadius: BorderRadius.circular(14),
              border: Border.all(
                color: Color.lerp(
                  CelestialColors.orbitRing.withValues(alpha: 0.3),
                  _hot.withValues(alpha: 0.85),
                  intensity,
                )!,
                width: 1.0 + intensity * 0.6,
              ),
            ),
            child: ClipRRect(
              borderRadius: BorderRadius.circular(13),
              child: Stack(
                children: [
                  if (intensity > 0.01)
                    Positioned.fill(
                      child: CustomPaint(
                        painter: _FlashHaloPainter(
                          progress: raw,
                          intensity: intensity,
                          color: _hot,
                        ),
                      ),
                    ),
                  Padding(
                    padding: const EdgeInsets.symmetric(
                      horizontal: 16,
                      vertical: 14,
                    ),
                    child: Row(
                      children: [
                        SizedBox(
                          width: 20,
                          height: 20,
                          child: Stack(
                            alignment: Alignment.center,
                            children: [
                              Icon(
                                Icons.lightbulb_outline,
                                size: 20,
                                color: _amber.withValues(
                                  alpha: 0.9 - intensity * 0.5,
                                ),
                                shadows: [
                                  Shadow(
                                    color: _hot.withValues(
                                      alpha: 0.7 * intensity,
                                    ),
                                    blurRadius: 16 * intensity,
                                  ),
                                ],
                              ),
                              Opacity(
                                opacity: intensity,
                                child: const Icon(
                                  Icons.lightbulb,
                                  color: _hot,
                                  size: 20,
                                ),
                              ),
                            ],
                          ),
                        ),
                        const SizedBox(width: 12),
                        Expanded(
                          child: Text(
                            _busy ? 'Identifying…' : 'Identify',
                            style: TextStyle(
                              color: Color.lerp(
                                CelestialColors.textPrimary,
                                _hot,
                                intensity * 0.5,
                              ),
                              fontSize: 15,
                            ),
                          ),
                        ),
                        Opacity(
                          opacity: 1.0 - intensity,
                          child: const Icon(
                            Icons.chevron_right,
                            color: CelestialColors.textSecondary,
                            size: 20,
                          ),
                        ),
                      ],
                    ),
                  ),
                ],
              ),
            ),
          );
        },
      ),
    );
  }
}

class _FlashHaloPainter extends CustomPainter {
  final double progress;
  final double intensity;
  final Color color;

  _FlashHaloPainter({
    required this.progress,
    required this.intensity,
    required this.color,
  });

  @override
  void paint(Canvas canvas, Size size) {
    // Origin is the icon's center: padding 16 + half-icon 10.
    final origin = Offset(26, size.height / 2);
    final maxR = size.width;

    // Radial bloom that brightens the whole button from the icon outward.
    final bloomR = maxR * (0.35 + progress * 1.1);
    final bloomPaint = Paint()
      ..shader = RadialGradient(
        colors: [
          color.withValues(alpha: 0.30 * intensity),
          color.withValues(alpha: 0.10 * intensity),
          color.withValues(alpha: 0.0),
        ],
        stops: const [0.0, 0.45, 1.0],
      ).createShader(Rect.fromCircle(center: origin, radius: bloomR));
    canvas.drawRect(Offset.zero & size, bloomPaint);

    // Leading sonar ring.
    _drawRing(canvas, origin, maxR, progress, 0.65, 1.6);

    // Trailing ring (delayed start) for layered depth.
    if (progress > 0.18) {
      final p2 = ((progress - 0.18) / 0.82).clamp(0.0, 1.0);
      _drawRing(canvas, origin, maxR, p2, 0.40, 1.0);
    }
  }

  void _drawRing(
    Canvas canvas,
    Offset origin,
    double maxR,
    double p,
    double startAlpha,
    double width,
  ) {
    final r = maxR * (0.05 + p * 0.95);
    final fade = (1 - p) * (1 - p);
    final paint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = width * fade
      ..color = color.withValues(alpha: startAlpha * fade);
    canvas.drawCircle(origin, r, paint);
  }

  @override
  bool shouldRepaint(covariant _FlashHaloPainter old) =>
      old.progress != progress ||
      old.intensity != intensity ||
      old.color != color;
}

class _ConnectionRow extends StatelessWidget {
  final Map<String, dynamic> hubKey;
  final String nativeId;
  final bool preferred;

  const _ConnectionRow({
    required this.hubKey,
    required this.nativeId,
    required this.preferred,
  });

  @override
  Widget build(BuildContext context) {
    final hubType = hubKey['hub_type']?.toString() ?? 'unknown';
    final address = hubKey['address'] as String? ?? '';
    final displayHub = switch (hubType) {
      'hue' => 'Hue Bridge',
      'hue_ble' => 'Hue Bluetooth',
      'local_ble' => 'Local Bluetooth',
      'ha' || 'homeassistant' || 'home_assistant' => 'Home Assistant',
      'matter' => 'Matter',
      _ => hubType,
    };

    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
      child: Row(
        children: [
          Icon(
            switch (hubType) {
              'hue' || 'hue_ble' => Icons.lightbulb,
              'local_ble' => Icons.bluetooth_rounded,
              'matter' => Icons.memory_outlined,
              _ => Icons.home,
            },
            color: CelestialColors.sunWarm.withValues(alpha: 0.7),
            size: 18,
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  displayHub,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 14,
                  ),
                ),
                Text(
                  address,
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.6),
                    fontSize: 11,
                  ),
                ),
              ],
            ),
          ),
          if (preferred)
            Container(
              padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
              decoration: BoxDecoration(
                color: CelestialColors.sunWarm.withValues(alpha: 0.15),
                borderRadius: BorderRadius.circular(8),
              ),
              child: const Text(
                'Preferred',
                style: TextStyle(
                  color: CelestialColors.sunWarm,
                  fontSize: 10,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
        ],
      ),
    );
  }
}
