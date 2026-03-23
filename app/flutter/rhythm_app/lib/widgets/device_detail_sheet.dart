import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import '../providers/server_sync_provider.dart';
import '../services/server_http_client.dart';
import 'solar_orbit.dart'; // For CelestialColors

/// Bottom sheet showing canonical device details + connections.
///
/// Opened by tapping a device row in [RoomSettingsSheet].
class DeviceDetailSheet extends StatefulWidget {
  final TypedDevice device;
  final String roomId;

  const DeviceDetailSheet({
    super.key,
    required this.device,
    required this.roomId,
  });

  static Future<void> show(
    BuildContext context,
    TypedDevice device,
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

  @override
  void initState() {
    super.initState();
    _loadCanonicalData();
  }

  Future<void> _loadCanonicalData() async {
    final http = context.read<ServerSyncProvider>().httpClient;
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
              padding: const EdgeInsets.symmetric(vertical: 16),
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
                        style: TextStyle(
                          color: CelestialColors.textSecondary.withValues(alpha: 0.7),
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
                    _buildMoveButton(context),
                  ],
                  const SizedBox(height: 16),
                ],
              ),
            ),
            // Done button
            Padding(
              padding: EdgeInsets.fromLTRB(
                20, 12, 20, MediaQuery.of(context).padding.bottom + 16,
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

  Widget _buildInfoSection(TypedDevice device) {
    final typeLabel = switch (device.type) {
      ServerDeviceType.light => 'Light',
      ServerDeviceType.button => 'Button',
      ServerDeviceType.motion => 'Motion Sensor',
    };

    final rhythmId = _canonicalData?['id'] as String?;

    return _buildGroup('Device Info', [
      _InfoRow(label: 'Type', value: typeLabel),
      if (device.manufacturer != null)
        _InfoRow(label: 'Manufacturer', value: device.manufacturer!),
      if (device.model != null)
        _InfoRow(label: 'Model', value: device.model!),
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
            const Expanded(
              child: Text(
                'Move to Room...',
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

  Future<void> _showMoveDialog(BuildContext context) async {
    final syncProvider = context.read<ServerSyncProvider>();
    final rooms = syncProvider.helloRooms
        .where((r) => r.id != widget.roomId)
        .toList();

    if (rooms.isEmpty) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('No other rooms available')),
      );
      return;
    }

    final targetRoom = await showModalBottomSheet<ServerRoom>(
      context: context,
      backgroundColor: Colors.transparent,
      builder: (ctx) => Container(
        decoration: const BoxDecoration(
          color: CelestialColors.backgroundCard,
          borderRadius: BorderRadius.vertical(top: Radius.circular(24)),
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Padding(
              padding: const EdgeInsets.all(16),
              child: Text(
                'Move to Room',
                style: TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 16,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
            for (final room in rooms)
              ListTile(
                leading: const Icon(
                  Icons.meeting_room_rounded,
                  color: Color(0xFFFFC107),
                ),
                title: Text(
                  room.name,
                  style: const TextStyle(color: CelestialColors.textPrimary),
                ),
                subtitle: Text(
                  room.deviceSummary,
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.7),
                    fontSize: 12,
                  ),
                ),
                onTap: () => Navigator.of(ctx).pop(room),
              ),
            SizedBox(height: MediaQuery.of(ctx).padding.bottom + 16),
          ],
        ),
      ),
    );

    if (targetRoom == null || !context.mounted) return;

    final success = await syncProvider.httpClient.topologyMoveDevice(
      deviceId: widget.device.id,
      fromRoomId: widget.roomId,
      toRoomId: targetRoom.id,
    );

    if (context.mounted) {
      if (success) {
        Navigator.of(context).pop(); // Close detail sheet
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text('Moved ${widget.device.displayName} to ${targetRoom.name}'),
          ),
        );
        // Trigger re-sync to update state
        syncProvider.httpClient.triggerSync();
      } else {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(content: Text('Failed to move device')),
        );
      }
    }
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

  (IconData, Color) _iconForType(ServerDeviceType type) => switch (type) {
    ServerDeviceType.light => (Icons.lightbulb_outline, const Color(0xFFFFB74D)),
    ServerDeviceType.button => (Icons.touch_app_outlined, const Color(0xFF64B5F6)),
    ServerDeviceType.motion => (Icons.sensors_outlined, const Color(0xFF81C784)),
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
    final hubType = (hubKey['hub_type'] as Map<String, dynamic>?)?['0'] as String?
        ?? hubKey['hub_type']?.toString()
        ?? 'unknown';
    final address = hubKey['address'] as String? ?? '';
    final displayHub = switch (hubType) {
      'hue' => 'Hue Bridge',
      'ha' => 'Home Assistant',
      _ => hubType,
    };

    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
      child: Row(
        children: [
          Icon(
            hubType == 'hue' ? Icons.lightbulb : Icons.home,
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
