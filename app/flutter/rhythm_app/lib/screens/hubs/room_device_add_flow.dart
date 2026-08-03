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
  final method = await showRoomDeviceAddMethodChooser(
    context,
    deviceType: deviceType,
  );
  if (!context.mounted || method == null) return;

  unawaited(
    AnalyticsService().logRoomDeviceAddMethodSelected(
      source: analyticsSource,
      deviceType: deviceType.name,
      method: method.name,
    ),
  );

  final syncProvider = context.read<ServerSyncProvider>();
  switch (method) {
    case RoomDeviceAddMethod.scan:
      if (!syncProvider.canScanToAddDeviceType(deviceType)) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text(
              'Connect or update your Rhythm Box to scan for '
              '${_pluralDeviceLabel(deviceType).toLowerCase()}.',
            ),
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

    case RoomDeviceAddMethod.existing:
      final selected = await showExistingRoomDevicePicker(
        context,
        roomId: roomId,
        roomName: roomName,
        deviceType: deviceType,
      );
      if (!context.mounted || selected == null) return;
      await assignCanonicalDeviceToRoom(
        context,
        device: selected.device,
        currentParentNodeId: selected.parentNodeId,
        targetRoomId: roomId,
        targetRoomName: roomName,
        analyticsSource: analyticsSource,
      );
  }
}

@visibleForTesting
Future<RoomDeviceAddMethod?> showRoomDeviceAddMethodChooser(
  BuildContext context, {
  required RhythmDeviceType deviceType,
}) {
  final label = _singularDeviceLabel(deviceType).toLowerCase();
  return showModalBottomSheet<RoomDeviceAddMethod>(
    context: context,
    backgroundColor: CelestialColors.backgroundCard,
    showDragHandle: true,
    builder: (sheetContext) => SafeArea(
      child: Padding(
        key: const ValueKey('room-device-add-method-chooser'),
        padding: const EdgeInsets.fromLTRB(20, 4, 20, 20),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              'Add $label',
              style: const TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 21,
                fontWeight: FontWeight.w700,
              ),
            ),
            const SizedBox(height: 8),
            const Text(
              'Scan its code or choose a device Rhythm already knows about.',
              style: TextStyle(
                color: CelestialColors.textSecondary,
                fontSize: 14,
                height: 1.4,
              ),
            ),
            const SizedBox(height: 18),
            _MethodTile(
              key: const ValueKey('room-device-add-scan'),
              icon: Icons.qr_code_scanner_rounded,
              color: const Color(0xFF4DD0C8),
              title: 'Scan',
              subtitle: 'Use the QR or printed setup code',
              onTap: () => Navigator.of(sheetContext).pop(
                RoomDeviceAddMethod.scan,
              ),
            ),
            const SizedBox(height: 10),
            _MethodTile(
              key: const ValueKey('room-device-add-existing'),
              icon: Icons.devices_other_rounded,
              color: const Color(0xFFFFB74D),
              title: 'Select from existing',
              subtitle: 'Choose from devices already in Rhythm',
              onTap: () => Navigator.of(sheetContext).pop(
                RoomDeviceAddMethod.existing,
              ),
            ),
          ],
        ),
      ),
    ),
  );
}

Future<ExistingRoomDeviceCandidate?> showExistingRoomDevicePicker(
  BuildContext context, {
  required String roomId,
  required String roomName,
  required RhythmDeviceType deviceType,
}) {
  return showModalBottomSheet<ExistingRoomDeviceCandidate>(
    context: context,
    isScrollControlled: true,
    backgroundColor: CelestialColors.backgroundCard,
    showDragHandle: true,
    builder: (_) => _ExistingRoomDevicePicker(
      roomId: roomId,
      roomName: roomName,
      deviceType: deviceType,
    ),
  );
}

class _MethodTile extends StatelessWidget {
  const _MethodTile({
    super.key,
    required this.icon,
    required this.color,
    required this.title,
    required this.subtitle,
    required this.onTap,
  });

  final IconData icon;
  final Color color;
  final String title;
  final String subtitle;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return ListTile(
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(14)),
      tileColor: color.withValues(alpha: 0.1),
      leading: Icon(icon, color: color),
      title: Text(title, style: const TextStyle(color: Colors.white)),
      subtitle: Text(
        subtitle,
        style: const TextStyle(color: CelestialColors.textSecondary),
      ),
      trailing: const Icon(
        Icons.chevron_right_rounded,
        color: CelestialColors.textSecondary,
      ),
      onTap: onTap,
    );
  }
}

class _ExistingRoomDevicePicker extends StatefulWidget {
  const _ExistingRoomDevicePicker({
    required this.roomId,
    required this.roomName,
    required this.deviceType,
  });

  final String roomId;
  final String roomName;
  final RhythmDeviceType deviceType;

  @override
  State<_ExistingRoomDevicePicker> createState() =>
      _ExistingRoomDevicePickerState();
}

class _ExistingRoomDevicePickerState extends State<_ExistingRoomDevicePicker> {
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
    setState(() => _candidates = _load());
  }

  @override
  Widget build(BuildContext context) {
    final label = _pluralDeviceLabel(widget.deviceType).toLowerCase();
    return SafeArea(
      child: SizedBox(
        height: MediaQuery.sizeOf(context).height * 0.65,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(20, 4, 20, 20),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Text(
                'Select from existing',
                style: const TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 21,
                  fontWeight: FontWeight.w700,
                ),
              ),
              const SizedBox(height: 6),
              Text(
                'Choose one of your existing $label for ${widget.roomName}.',
                style: const TextStyle(
                  color: CelestialColors.textSecondary,
                  fontSize: 14,
                ),
              ),
              const SizedBox(height: 16),
              Expanded(
                child: FutureBuilder<List<ExistingRoomDeviceCandidate>?>(
                  future: _candidates,
                  builder: (context, snapshot) {
                    if (snapshot.connectionState != ConnectionState.done) {
                      return const Center(
                        child: CircularProgressIndicator(
                          key: ValueKey('existing-room-devices-loading'),
                        ),
                      );
                    }
                    final candidates = snapshot.data;
                    if (snapshot.hasError || candidates == null) {
                      return _PickerMessage(
                        key: const ValueKey('existing-room-devices-error'),
                        message: 'Could not load existing devices.',
                        actionLabel: 'Retry',
                        onAction: _retry,
                      );
                    }
                    if (candidates.isEmpty) {
                      return _PickerMessage(
                        key: const ValueKey('existing-room-devices-empty'),
                        message: 'No existing $label found.',
                      );
                    }
                    return ListView.separated(
                      itemCount: candidates.length,
                      separatorBuilder: (_, __) => const SizedBox(height: 8),
                      itemBuilder: (context, index) {
                        final candidate = candidates[index];
                        final alreadyInRoom =
                            candidate.parentNodeId == widget.roomId;
                        final subtitle = alreadyInRoom
                            ? 'Already in ${widget.roomName}'
                            : candidate.parentNodeId.isEmpty
                                ? 'Unassigned'
                                : 'Currently in ${candidate.parentLabel}';
                        return ListTile(
                          key: ValueKey(
                            'existing-room-device-${candidate.device.id}',
                          ),
                          shape: RoundedRectangleBorder(
                            borderRadius: BorderRadius.circular(14),
                          ),
                          tileColor: Colors.white.withValues(alpha: 0.06),
                          leading: Icon(
                            _iconForDeviceType(widget.deviceType),
                            color: _colorForDeviceType(widget.deviceType),
                          ),
                          title: Text(
                            candidate.device.displayName,
                            style: const TextStyle(color: Colors.white),
                          ),
                          subtitle: Text(
                            subtitle,
                            style: const TextStyle(
                              color: CelestialColors.textSecondary,
                            ),
                          ),
                          trailing: Icon(
                            alreadyInRoom
                                ? Icons.check_circle_outline_rounded
                                : Icons.chevron_right_rounded,
                            color: alreadyInRoom
                                ? const Color(0xFF81C784)
                                : CelestialColors.textSecondary,
                          ),
                          onTap: () => Navigator.of(context).pop(candidate),
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

class _PickerMessage extends StatelessWidget {
  const _PickerMessage({
    super.key,
    required this.message,
    this.actionLabel,
    this.onAction,
  });

  final String message;
  final String? actionLabel;
  final VoidCallback? onAction;

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(
            message,
            textAlign: TextAlign.center,
            style: const TextStyle(color: CelestialColors.textSecondary),
          ),
          if (actionLabel != null && onAction != null) ...[
            const SizedBox(height: 12),
            OutlinedButton(onPressed: onAction, child: Text(actionLabel!)),
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
