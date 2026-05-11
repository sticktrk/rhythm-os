import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmDevice, RhythmDeviceType;
import '../providers/server_sync_provider.dart';
import 'room_picker_sheet.dart';
import 'solar_orbit.dart'; // For CelestialColors

Future<bool> showDeviceNodeAssignmentFlow(
  BuildContext context, {
  required RhythmDevice device,
  required String currentParentNodeId,
  bool allowNoRoom = false,
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

  if (targetParentNodeId == normalizedCurrentParentNodeId) {
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

  final success =
      await syncProvider.api.assignDeviceParent(device.id, targetParentNodeId);

  if (!context.mounted) return false;

  if (!success) {
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(
          isUnassigned
              ? 'Failed to assign ${device.displayName}'
              : 'Failed to move ${device.displayName}',
        ),
      ),
    );
    return false;
  }

  await syncProvider.connection.reconnect();
  if (!context.mounted) return true;

  ScaffoldMessenger.of(context).showSnackBar(
    SnackBar(
      content: Text(
        selectedIsUnassigned
            ? 'Removed ${device.displayName} from its room'
            : isUnassigned
                ? 'Assigned ${device.displayName} to $selectedLabel'
                : 'Moved ${device.displayName} to $selectedLabel',
      ),
    ),
  );
  return true;
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

/// Bottom sheet showing canonical device details + connections.
///
/// Opened by tapping a device row in [RoomSettingsSheet].
class DeviceDetailSheet extends StatefulWidget {
  final RhythmDevice device;
  final String roomId;

  const DeviceDetailSheet({
    super.key,
    required this.device,
    required this.roomId,
  });

  static Future<void> show(
    BuildContext context,
    RhythmDevice device,
    String roomId,
  ) {
    HapticFeedback.lightImpact();
    return showModalBottomSheet(
      context: context,
      isScrollControlled: true,
      backgroundColor: Colors.transparent,
      builder: (context) => DeviceDetailSheet(
        device: device,
        roomId: roomId,
      ),
    );
  }

  @override
  State<DeviceDetailSheet> createState() => _DeviceDetailSheetState();
}

class _DeviceDetailSheetState extends State<DeviceDetailSheet> {
  Map<String, dynamic>? _canonicalData;
  bool _loading = true;
  bool _removing = false;

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

  @override
  Widget build(BuildContext context) {
    final topPad = MediaQuery.of(context).padding.top;
    final device = widget.device;
    final (icon, iconColor) = _iconForType(device.type);
    final canUnpairMatter = context.select<ServerSyncProvider, bool>(
      (sync) => sync.canUnpairMatterDevices,
    );

    return Padding(
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
          mainAxisSize: MainAxisSize.min,
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
                  Text(
                    device.displayName,
                    textAlign: TextAlign.center,
                    style: const TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 18,
                      fontWeight: FontWeight.w600,
                    ),
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
            // Content
            Flexible(
              child: ListView(
                shrinkWrap: true,
                padding: const EdgeInsets.symmetric(horizontal: 20),
                children: [
                  _buildInfoSection(device),
                  const SizedBox(height: 16),
                  if (_loading)
                    const Center(
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
                    )
                  else if (_canonicalData != null) ...[
                    _buildConnectionsSection(),
                    const SizedBox(height: 16),
                    if (device.type == RhythmDeviceType.light) ...[
                      _FlashButton(
                        deviceLabel: device.displayName,
                        onFlash: () => context
                            .read<ServerSyncProvider>()
                            .api
                            .flashCanonicalDevice(device.id),
                      ),
                      const SizedBox(height: 12),
                    ],
                    _buildMoveButton(context),
                    if (_matterNativeId != null && canUnpairMatter) ...[
                      const SizedBox(height: 12),
                      _buildRemoveButton(context),
                    ],
                  ],
                  const SizedBox(height: 16),
                ],
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
                  onPressed: () => Navigator.of(context).pop(),
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
    );
  }

  Widget _buildInfoSection(RhythmDevice device) {
    final typeLabel = switch (device.type) {
      RhythmDeviceType.light => 'Light',
      RhythmDeviceType.button => 'Button',
      RhythmDeviceType.motion => 'Motion Sensor',
    };

    final rhythmId = _canonicalData?['id'] as String?;

    return _buildGroup('Device Info', [
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

    return GestureDetector(
      onTap: () => _showMoveDialog(context),
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
              Icons.swap_horiz,
              color: CelestialColors.sunWarm.withValues(alpha: 0.8),
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

  /// The matter native ID from the canonical endpoints, or null if not a matter device.
  String? get _matterNativeId {
    final endpoints = _canonicalData?['endpoints'] as List<dynamic>? ?? [];
    for (final ep in endpoints) {
      final nativeId =
          (ep as Map<String, dynamic>)['native_id'] as String? ?? '';
      if (nativeId.startsWith('matter-')) return nativeId;
    }
    return null;
  }

  Widget _buildRemoveButton(BuildContext context) {
    return GestureDetector(
      onTap: _removing ? null : () => _confirmRemoveDevice(context),
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
            if (_removing)
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
                _removing ? 'Removing...' : 'Remove Device',
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

  Future<void> _confirmRemoveDevice(BuildContext context) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: const Text(
          'Remove Device?',
          style: TextStyle(color: CelestialColors.textPrimary),
        ),
        content: Text(
          'This will decommission "${widget.device.displayName}" and remove it from your system. The device can be re-paired afterwards.',
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
    await _removeDevice(context, force: true);
  }

  Future<void> _removeDevice(BuildContext context,
      {required bool force}) async {
    final nativeId = _matterNativeId;
    if (nativeId == null) return;

    setState(() => _removing = true);

    final syncProvider = context.read<ServerSyncProvider>();
    final result = await syncProvider.api.unpairDevice(
      hubType: 'matter',
      deviceId: nativeId,
      force: force,
    );

    if (!context.mounted) return;

    final status = result?['status'] as String?;
    if (status == 'complete') {
      await syncProvider.connection.reconnect();
      if (!context.mounted) return;
      Navigator.of(context).pop();
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Removed ${widget.device.displayName}')),
      );
    } else {
      final error = result?['error'] as String? ?? 'Unknown error';
      setState(() => _removing = false);
      if (!context.mounted) return;
      // Offer force-remove if the device is unreachable.
      final forceRemove = await showDialog<bool>(
        context: context,
        builder: (ctx) => AlertDialog(
          backgroundColor: CelestialColors.backgroundCard,
          title: const Text(
            'Removal Failed',
            style: TextStyle(color: CelestialColors.textPrimary),
          ),
          content: Text(
            '$error\n\nForce remove? This cleans up local state without contacting the device.',
            style: const TextStyle(color: CelestialColors.textSecondary),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(ctx).pop(false),
              child: const Text('Cancel'),
            ),
            TextButton(
              onPressed: () => Navigator.of(ctx).pop(true),
              child: Text('Force Remove',
                  style: TextStyle(color: Colors.red.shade300)),
            ),
          ],
        ),
      );
      if (forceRemove == true && context.mounted) {
        await _removeDevice(context, force: true);
      }
    }
  }

  Future<void> _showMoveDialog(BuildContext context) async {
    final syncProvider = context.read<ServerSyncProvider>();
    final success = await showDeviceNodeAssignmentFlow(
      context,
      device: widget.device,
      currentParentNodeId: widget.roomId,
      allowNoRoom: _canLeaveUnassigned(syncProvider),
    );

    if (success && context.mounted) {
      Navigator.of(context).pop();
    }
  }

  bool _canLeaveUnassigned(ServerSyncProvider syncProvider) {
    return switch (widget.device.type) {
      RhythmDeviceType.button || RhythmDeviceType.motion => true,
      RhythmDeviceType.light =>
        _matterNativeId != null && syncProvider.supportsMatterRoomlessDevices,
    };
  }

  Widget _buildGroup(String title, List<Widget> children) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
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
      };
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
              'hue' => Icons.lightbulb,
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
