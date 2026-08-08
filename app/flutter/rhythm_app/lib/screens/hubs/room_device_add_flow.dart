import 'dart:async';

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show RhythmDevice, RhythmDeviceType, RhythmTopologyNode;

import '../../providers/server_sync_provider.dart';
import '../../services/analytics_service.dart';
import '../../widgets/device_detail_sheet.dart';
import '../../widgets/solar_orbit.dart';
import 'device_pairing_flow.dart';

enum RoomDeviceAddMethod { scan, existing }

class ExistingRoomDeviceCandidate {
  const ExistingRoomDeviceCandidate({
    required this.device,
    required this.parentNodeId,
    required this.parentLabel,
  });

  final RhythmDevice device;
  final String parentNodeId;
  final String parentLabel;
}

class RoomDeviceAddSelection {
  const RoomDeviceAddSelection.scan() : candidate = null;

  const RoomDeviceAddSelection.existing(this.candidate)
      : assert(candidate != null);

  final ExistingRoomDeviceCandidate? candidate;

  bool get isScan => candidate == null;
}

@visibleForTesting
List<ExistingRoomDeviceCandidate> existingRoomDeviceCandidates({
  required List<Map<String, dynamic>> canonicalDevices,
  required List<RhythmTopologyNode> topologyNodes,
  required Map<String, String> roomNamesById,
  required RhythmDeviceType deviceType,
}) {
  final topologyById = {
    for (final node in topologyNodes) node.id: node,
  };
  final candidates = <ExistingRoomDeviceCandidate>[];
  for (final canonical in canonicalDevices) {
    final id = canonical['id']?.toString().trim() ?? '';
    if (id.isEmpty) continue;
    final topologyNode = topologyById[id];
    final rawType = canonical['device_type']?.toString().trim();
    final resolvedType = rawType != null && rawType.isNotEmpty
        ? _parseExactDeviceType(rawType)
        : topologyNode == null
            ? null
            : RhythmDeviceType.fromNodeKind(topologyNode.kind);
    if (resolvedType != deviceType) continue;

    final canonicalRoomId = canonical['room_id']?.toString().trim() ?? '';
    final canonicalParentId = canonical['parent_id']?.toString().trim() ?? '';
    final topologyParentId = topologyNode?.parentId?.trim() ?? '';
    final parentNodeId = canonicalRoomId.isNotEmpty
        ? canonicalRoomId
        : canonicalParentId.isNotEmpty
            ? canonicalParentId
            : topologyParentId;
    final name = canonical['name']?.toString().trim();
    final manufacturer = canonical['manufacturer']?.toString().trim();
    final model = canonical['model']?.toString().trim();
    candidates.add(
      ExistingRoomDeviceCandidate(
        device: RhythmDevice(
          id: id,
          type: resolvedType!,
          name: name == null || name.isEmpty ? topologyNode?.name : name,
          manufacturer: manufacturer == null || manufacturer.isEmpty
              ? topologyNode?.manufacturer
              : manufacturer,
          model: model == null || model.isEmpty ? topologyNode?.model : model,
        ),
        parentNodeId: parentNodeId,
        parentLabel: parentNodeId.isEmpty
            ? 'Unassigned'
            : roomNamesById[parentNodeId] ?? 'Another room',
      ),
    );
  }
  candidates.sort(
    (left, right) => left.device.displayName.toLowerCase().compareTo(
          right.device.displayName.toLowerCase(),
        ),
  );
  return candidates;
}

Future<void> startRoomDeviceAddFlow(
  BuildContext context, {
  required String roomId,
  required String roomName,
  required RhythmDeviceType deviceType,
  required String analyticsSource,
}) async {
  final selection = await showRoomDeviceAddSheet(
    context,
    roomId: roomId,
    roomName: roomName,
    deviceType: deviceType,
  );
  if (!context.mounted || selection == null) return;

  final method = selection.isScan
      ? RoomDeviceAddMethod.scan
      : RoomDeviceAddMethod.existing;

  unawaited(
    AnalyticsService().logRoomDeviceAddMethodSelected(
      source: analyticsSource,
      deviceType: deviceType.name,
      method: method.name,
    ),
  );

  if (selection.isScan) {
    final syncProvider = context.read<ServerSyncProvider>();
    if (!syncProvider.canScanToAddDevice) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text('Scanning is not available on this Rhythm Box yet.'),
        ),
      );
      return;
    }
    await startDevicePairingFlow(
      context,
      analyticsSource: analyticsSource,
      roomAssignment: DevicePairingRoomAssignment(
        roomId: roomId,
        roomName: roomName,
        expectedDeviceType: deviceType,
      ),
    );
    return;
  }

  final selected = selection.candidate!;
  if (deviceType == RhythmDeviceType.motion) {
    final syncProvider = context.read<ServerSyncProvider>();
    final targetRoomIds = syncProvider
        .controlTargetNodeIds(
          sourceNodeId: selected.device.id,
          controlKind: 'motion',
        )
        .toSet();
    if (selected.parentNodeId.isNotEmpty) {
      targetRoomIds.add(selected.parentNodeId);
    }
    targetRoomIds.add(roomId);
    final success = await syncProvider.setNodeControlTargets(
      sourceNodeId: selected.device.id,
      controlKind: 'motion',
      targetNodeIds: targetRoomIds,
    );
    if (!context.mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(
          success
              ? 'Added ${selected.device.displayName} as additional motion for $roomName'
              : 'Failed to add ${selected.device.displayName}',
        ),
      ),
    );
    return;
  }
  await assignCanonicalDeviceToRoom(
    context,
    device: selected.device,
    currentParentNodeId: selected.parentNodeId,
    targetRoomId: roomId,
    targetRoomName: roomName,
    analyticsSource: analyticsSource,
  );
}

@visibleForTesting
Future<RoomDeviceAddSelection?> showRoomDeviceAddSheet(
  BuildContext context, {
  required String roomId,
  required String roomName,
  required RhythmDeviceType deviceType,
}) {
  return showModalBottomSheet<RoomDeviceAddSelection>(
    context: context,
    isScrollControlled: true,
    backgroundColor: CelestialColors.backgroundCard,
    showDragHandle: true,
    builder: (_) => _RoomDeviceAddSheet(
      roomId: roomId,
      roomName: roomName,
      deviceType: deviceType,
    ),
  );
}

class _RoomDeviceAddSheet extends StatefulWidget {
  const _RoomDeviceAddSheet({
    required this.roomId,
    required this.roomName,
    required this.deviceType,
  });

  final String roomId;
  final String roomName;
  final RhythmDeviceType deviceType;

  @override
  State<_RoomDeviceAddSheet> createState() => _RoomDeviceAddSheetState();
}

class _RoomDeviceAddSheetState extends State<_RoomDeviceAddSheet> {
  late Future<List<ExistingRoomDeviceCandidate>?> _candidates;

  @override
  void initState() {
    super.initState();
    _candidates = _load();
  }

  Future<List<ExistingRoomDeviceCandidate>?> _load() async {
    final syncProvider = context.read<ServerSyncProvider>();
    final canonicalDevices = await syncProvider.api.getCanonicalDevices();
    if (canonicalDevices == null) return null;
    final roomNamesById = <String, String>{
      for (final room in syncProvider.helloRooms) room.id: room.name,
      for (final node
          in syncProvider.topologyNodes.where((node) => node.isRoom))
        node.id: node.name,
    };
    return existingRoomDeviceCandidates(
      canonicalDevices: canonicalDevices,
      topologyNodes: syncProvider.topologyNodes,
      roomNamesById: roomNamesById,
      deviceType: widget.deviceType,
    );
  }

  void _retry() {
    final candidates = _load();
    setState(() {
      _candidates = candidates;
    });
  }

  @override
  Widget build(BuildContext context) {
    final singularLabel = _singularDeviceLabel(widget.deviceType).toLowerCase();
    final label = _pluralDeviceLabel(widget.deviceType).toLowerCase();
    final accent = _colorForDeviceType(widget.deviceType);
    return SafeArea(
      child: SizedBox(
        height: MediaQuery.sizeOf(context).height * 0.8,
        child: Padding(
          key: const ValueKey('room-device-add-sheet'),
          padding: const EdgeInsets.fromLTRB(20, 4, 20, 20),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Row(
                children: [
                  Container(
                    width: 42,
                    height: 42,
                    decoration: BoxDecoration(
                      color: accent.withValues(alpha: 0.14),
                      borderRadius: BorderRadius.circular(13),
                    ),
                    child: Icon(
                      _iconForDeviceType(widget.deviceType),
                      color: accent,
                      size: 22,
                    ),
                  ),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          'Add $singularLabel',
                          style: const TextStyle(
                            color: CelestialColors.textPrimary,
                            fontSize: 21,
                            fontWeight: FontWeight.w700,
                          ),
                        ),
                        Text(
                          'Place it in ${widget.roomName}',
                          style: const TextStyle(
                            color: CelestialColors.textSecondary,
                            fontSize: 13,
                          ),
                        ),
                      ],
                    ),
                  ),
                ],
              ),
              const SizedBox(height: 18),
              _ScanAction(
                key: const ValueKey('room-device-add-scan'),
                onTap: () => Navigator.of(context).pop(
                  const RoomDeviceAddSelection.scan(),
                ),
              ),
              const SizedBox(height: 22),
              Row(
                children: [
                  const Text(
                    'Existing devices',
                    style: TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 15,
                      fontWeight: FontWeight.w700,
                    ),
                  ),
                  const SizedBox(width: 9),
                  Expanded(
                    child: Container(
                      height: 1,
                      color: Colors.white.withValues(alpha: 0.09),
                    ),
                  ),
                ],
              ),
              const SizedBox(height: 5),
              Text(
                'Choose a $singularLabel Rhythm already knows about.',
                style: const TextStyle(
                  color: CelestialColors.textSecondary,
                  fontSize: 12,
                ),
              ),
              const SizedBox(height: 12),
              Expanded(
                child: FutureBuilder<List<ExistingRoomDeviceCandidate>?>(
                  future: _candidates,
                  builder: (context, snapshot) {
                    if (snapshot.connectionState != ConnectionState.done) {
                      return const _PickerMessage(
                        key: ValueKey('existing-room-devices-loading'),
                        icon: Icons.manage_search_rounded,
                        message: 'Finding existing devices…',
                        loading: true,
                      );
                    }
                    final candidates = snapshot.data;
                    if (snapshot.hasError || candidates == null) {
                      return _PickerMessage(
                        key: const ValueKey('existing-room-devices-error'),
                        icon: Icons.cloud_off_rounded,
                        message: 'Could not load existing devices.',
                        actionLabel: 'Try again',
                        onAction: _retry,
                      );
                    }
                    if (candidates.isEmpty) {
                      return _PickerMessage(
                        key: const ValueKey('existing-room-devices-empty'),
                        icon: Icons.devices_other_rounded,
                        message: 'No existing $label found.',
                      );
                    }
                    return ListView.separated(
                      key: const ValueKey('existing-room-devices-list'),
                      padding: const EdgeInsets.only(bottom: 4),
                      itemCount: candidates.length,
                      separatorBuilder: (_, __) => const SizedBox(height: 9),
                      itemBuilder: (context, index) {
                        final candidate = candidates[index];
                        final alreadyInRoom =
                            candidate.parentNodeId == widget.roomId;
                        final status = alreadyInRoom
                            ? 'Already in ${widget.roomName}'
                            : candidate.parentNodeId.isEmpty
                                ? 'Unassigned'
                                : 'Currently in ${candidate.parentLabel}';
                        final statusColor = alreadyInRoom
                            ? const Color(0xFF81C784)
                            : candidate.parentNodeId.isEmpty
                                ? const Color(0xFFFFB74D)
                                : CelestialColors.textSecondary;
                        return Material(
                          key: ValueKey(
                            'existing-room-device-${candidate.device.id}',
                          ),
                          color: Colors.white.withValues(alpha: 0.055),
                          shape: RoundedRectangleBorder(
                            borderRadius: BorderRadius.circular(16),
                            side: BorderSide(
                              color: alreadyInRoom
                                  ? statusColor.withValues(alpha: 0.28)
                                  : Colors.white.withValues(alpha: 0.07),
                            ),
                          ),
                          clipBehavior: Clip.antiAlias,
                          child: InkWell(
                            onTap: () => Navigator.of(context).pop(
                              RoomDeviceAddSelection.existing(candidate),
                            ),
                            child: Padding(
                              padding: const EdgeInsets.symmetric(
                                horizontal: 14,
                                vertical: 12,
                              ),
                              child: Row(
                                children: [
                                  Container(
                                    width: 40,
                                    height: 40,
                                    decoration: BoxDecoration(
                                      color: accent.withValues(alpha: 0.12),
                                      shape: BoxShape.circle,
                                    ),
                                    child: Icon(
                                      _iconForDeviceType(widget.deviceType),
                                      color: accent,
                                      size: 20,
                                    ),
                                  ),
                                  const SizedBox(width: 12),
                                  Expanded(
                                    child: Column(
                                      crossAxisAlignment:
                                          CrossAxisAlignment.start,
                                      children: [
                                        Text(
                                          candidate.device.displayName,
                                          maxLines: 1,
                                          overflow: TextOverflow.ellipsis,
                                          style: const TextStyle(
                                            color: Colors.white,
                                            fontSize: 15,
                                            fontWeight: FontWeight.w600,
                                          ),
                                        ),
                                        const SizedBox(height: 3),
                                        Text(
                                          status,
                                          maxLines: 1,
                                          overflow: TextOverflow.ellipsis,
                                          style: TextStyle(
                                            color: statusColor,
                                            fontSize: 12,
                                            fontWeight: FontWeight.w500,
                                          ),
                                        ),
                                      ],
                                    ),
                                  ),
                                  const SizedBox(width: 8),
                                  Icon(
                                    alreadyInRoom
                                        ? Icons.check_circle_rounded
                                        : Icons.add_circle_outline_rounded,
                                    color: alreadyInRoom
                                        ? statusColor
                                        : CelestialColors.textSecondary,
                                    size: 22,
                                  ),
                                ],
                              ),
                            ),
                          ),
                        );
                      },
                    );
                  },
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _ScanAction extends StatelessWidget {
  const _ScanAction({super.key, required this.onTap});

  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    const scanColor = Color(0xFF4DD0C8);
    return Semantics(
      button: true,
      label: 'Scan a device code',
      hint: 'Opens the camera or manual code entry',
      child: Material(
        color: Colors.transparent,
        borderRadius: BorderRadius.circular(18),
        clipBehavior: Clip.antiAlias,
        child: Ink(
          decoration: BoxDecoration(
            gradient: LinearGradient(
              colors: [
                scanColor.withValues(alpha: 0.20),
                const Color(0xFF26A69A).withValues(alpha: 0.08),
              ],
            ),
            borderRadius: BorderRadius.circular(18),
            border: Border.all(color: scanColor.withValues(alpha: 0.3)),
          ),
          child: InkWell(
            onTap: onTap,
            child: const Padding(
              padding: EdgeInsets.symmetric(horizontal: 16, vertical: 15),
              child: Row(
                children: [
                  DecoratedBox(
                    decoration: BoxDecoration(
                      color: Color(0x294DD0C8),
                      shape: BoxShape.circle,
                    ),
                    child: SizedBox(
                      width: 44,
                      height: 44,
                      child: Icon(
                        Icons.qr_code_scanner_rounded,
                        color: scanColor,
                        size: 23,
                      ),
                    ),
                  ),
                  SizedBox(width: 13),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          'Scan',
                          style: TextStyle(
                            color: Colors.white,
                            fontSize: 16,
                            fontWeight: FontWeight.w700,
                          ),
                        ),
                        SizedBox(height: 2),
                        Text(
                          'Use a QR or printed setup code',
                          style: TextStyle(
                            color: CelestialColors.textSecondary,
                            fontSize: 12,
                          ),
                        ),
                      ],
                    ),
                  ),
                  Icon(
                    Icons.arrow_forward_rounded,
                    color: scanColor,
                    size: 21,
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _PickerMessage extends StatelessWidget {
  const _PickerMessage({
    super.key,
    required this.icon,
    required this.message,
    this.actionLabel,
    this.onAction,
    this.loading = false,
  });

  final IconData icon;
  final String message;
  final String? actionLabel;
  final VoidCallback? onAction;
  final bool loading;

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (loading)
            const SizedBox(
              width: 24,
              height: 24,
              child: CircularProgressIndicator(strokeWidth: 2.4),
            )
          else
            Icon(
              icon,
              color: CelestialColors.textSecondary,
              size: 28,
            ),
          const SizedBox(height: 10),
          Text(
            message,
            textAlign: TextAlign.center,
            style: const TextStyle(color: CelestialColors.textSecondary),
          ),
          if (actionLabel != null && onAction != null) ...[
            const SizedBox(height: 9),
            OutlinedButton(
              style: OutlinedButton.styleFrom(
                minimumSize: const Size(0, 36),
                visualDensity: VisualDensity.compact,
              ),
              onPressed: onAction,
              child: Text(actionLabel!),
            ),
          ],
        ],
      ),
    );
  }
}

String _singularDeviceLabel(RhythmDeviceType deviceType) =>
    switch (deviceType) {
      RhythmDeviceType.motion => 'Motion sensor',
      RhythmDeviceType.button => 'Button',
      RhythmDeviceType.contact => 'Contact sensor',
      RhythmDeviceType.light => 'Light',
    };

String _pluralDeviceLabel(RhythmDeviceType deviceType) => switch (deviceType) {
      RhythmDeviceType.motion => 'Motion sensors',
      RhythmDeviceType.button => 'Buttons',
      RhythmDeviceType.contact => 'Contact sensors',
      RhythmDeviceType.light => 'Lights',
    };

IconData _iconForDeviceType(RhythmDeviceType deviceType) =>
    switch (deviceType) {
      RhythmDeviceType.motion => Icons.sensors_outlined,
      RhythmDeviceType.button => Icons.touch_app_outlined,
      RhythmDeviceType.contact => Icons.sensor_door_outlined,
      RhythmDeviceType.light => Icons.lightbulb_outline_rounded,
    };

Color _colorForDeviceType(RhythmDeviceType deviceType) => switch (deviceType) {
      RhythmDeviceType.motion => const Color(0xFF81C784),
      RhythmDeviceType.button => const Color(0xFF64B5F6),
      RhythmDeviceType.contact => const Color(0xFFFFB74D),
      RhythmDeviceType.light => const Color(0xFFFFB74D),
    };

RhythmDeviceType? _parseExactDeviceType(String value) => switch (value) {
      'light' => RhythmDeviceType.light,
      'button' => RhythmDeviceType.button,
      'motion' => RhythmDeviceType.motion,
      'contact' => RhythmDeviceType.contact,
      _ => null,
    };
