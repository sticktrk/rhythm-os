import 'dart:async';

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show RhythmConnection, RhythmDevice, RhythmDeviceType;

import '../../../providers/server_sync_provider.dart';
import '../../../services/analytics_service.dart';
import '../../../widgets/device_detail_sheet.dart';
import '../../../widgets/room_picker_sheet.dart';
import '../../../widgets/settings_row.dart';
import '../../../widgets/solar_orbit.dart';
import '../../hubs/rhythmserver_settings_screen.dart';

/// "Add a Device" flow — the pairing/add options only (Rhythm hub, Hue, Home
/// Assistant, Matter, …). Reached from the floating "+" on the home screen.
///
/// Backed by [RhythmServerHubManagementSection] with `showConfigured: false`
/// so it reuses the exact capability-gated add options without listing the
/// hubs already paired.
class AddDeviceScreen extends StatelessWidget {
  const AddDeviceScreen({super.key});

  static Future<void> show(BuildContext context) {
    AnalyticsService().logScreenView('add_device');
    return Navigator.of(context).push(
      MaterialPageRoute(builder: (_) => const AddDeviceScreen()),
    );
  }

  Future<void> _addRoom(BuildContext context) async {
    final room = await createTopologyRoomOptionFromPrompt(context);
    if (room != null && context.mounted) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Added room "${room.name}"')),
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            devicesBackHeader(context, 'Add a Device'),
            Expanded(
              child: SingleChildScrollView(
                padding: const EdgeInsets.symmetric(horizontal: 20),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    const SizedBox(height: 8),
                    const RhythmServerHubManagementSection(
                      showConfigured: false,
                    ),
                    const SettingsSectionHeader(title: 'Rooms'),
                    SettingsGroup(
                      children: [
                        SettingsRow(
                          icon: Icons.meeting_room_outlined,
                          iconColor: const Color(0xFF7C83FF),
                          label: 'Add a Room',
                          onTap: () => _addRoom(context),
                        ),
                      ],
                    ),
                    const SizedBox(height: 40),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// "Devices" list — an accordion grouped by hub. Each hub is a collapsible
/// section; expanding reveals its devices inline. Tapping a device opens its
/// detail sheet. Reached from Settings → Hardware. New devices are added from
/// the home screen's floating "+", so there are no add options here.
class DevicesListScreen extends StatefulWidget {
  const DevicesListScreen({super.key});

  static Future<void> show(BuildContext context) {
    AnalyticsService().logScreenView('devices_list');
    return Navigator.of(context).push(
      MaterialPageRoute(builder: (_) => const DevicesListScreen()),
    );
  }

  @override
  State<DevicesListScreen> createState() => _DevicesListScreenState();
}

/// A device plus the room context needed to open its detail sheet.
class _DeviceEntry {
  final RhythmDevice device;
  final String roomId;
  final String roomName;

  const _DeviceEntry({
    required this.device,
    required this.roomId,
    required this.roomName,
  });
}

/// One configured hub and the devices that live behind it.
class _HubGroup {
  final String type;
  final String label;
  final bool connected;
  final List<_DeviceEntry> devices;

  const _HubGroup({
    required this.type,
    required this.label,
    required this.connected,
    required this.devices,
  });
}

class _DevicesListScreenState extends State<DevicesListScreen> {
  bool _loading = true;
  List<_HubGroup> _hubs = const [];
  // Accordion: one hub open at a time. Defaults to the first hub.
  String? _expandedType;

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  Future<void> _load() async {
    final http = context.read<RhythmConnection>();
    final sync = context.read<ServerSyncProvider>();

    final raw = await http.api.getCanonicalDevices();
    if (!mounted) return;

    // room-node-id -> room-name, and device-id -> parent-node-id, so each
    // device carries the room context DeviceDetailSheet needs.
    final roomNames = <String, String>{
      for (final r in sync.helloRooms)
        if (r.id.isNotEmpty) r.id: r.name,
    };
    final parentByDevice = <String, String?>{
      for (final node in sync.topologyNodes.where((n) => n.isDevice))
        node.id: node.parentId,
    };

    final configuredHubs = sync.serverHubInfos
        .where((h) => h['type'] != null && h['type'] != 'none')
        .toList();

    final hubs = <_HubGroup>[];
    for (final h in configuredHubs) {
      final type = h['type'] as String;
      final connected = h['connected'] as bool? ?? false;

      final entries = <_DeviceEntry>[];
      for (final d in raw ?? const <Map<String, dynamic>>[]) {
        final endpoints = d['endpoints'] as List<dynamic>? ?? const [];
        final belongs = endpoints.any((ep) {
          final hubKey = (ep as Map<String, dynamic>)['hub_key']
                  as Map<String, dynamic>? ??
              const {};
          return hubKey['hub_type']?.toString() == type;
        });
        if (!belongs) continue;

        final id = d['id'] as String? ?? '';
        final parentId = id.isEmpty
            ? (d['parent_id'] as String? ?? d['room_id'] as String?)
            : (parentByDevice[id] ??
                d['parent_id'] as String? ??
                d['room_id'] as String?);

        entries.add(_DeviceEntry(
          device: RhythmDevice(
            id: id,
            type: RhythmDeviceType.fromString(
                d['device_type'] as String? ?? 'light'),
            name: d['name'] as String?,
            manufacturer: d['manufacturer'] as String?,
            model: d['model'] as String?,
          ),
          roomId: parentId ?? '',
          roomName:
              (parentId != null ? roomNames[parentId] : null) ?? 'Unassigned',
        ));
      }

      _sortEntries(entries);
      hubs.add(_HubGroup(
        type: type,
        label: _hubLabel(type),
        connected: connected,
        devices: entries,
      ));
    }

    setState(() {
      _hubs = hubs;
      _loading = false;
      _expandedType ??= hubs.isNotEmpty ? hubs.first.type : null;
    });
  }

  void _sortEntries(List<_DeviceEntry> entries) {
    const order = {
      RhythmDeviceType.light: 0,
      RhythmDeviceType.button: 1,
      RhythmDeviceType.motion: 2,
      RhythmDeviceType.contact: 3,
    };
    entries.sort((a, b) =>
        (order[a.device.type] ?? 3).compareTo(order[b.device.type] ?? 3));
  }

  Future<void> _openDevice(_DeviceEntry entry) async {
    await DeviceDetailSheet.show(context, entry.device, entry.roomId);
    if (!mounted) return;
    // A rename / room move in the sheet may change what we show.
    await _load();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            devicesBackHeader(context, 'Devices'),
            Expanded(child: _buildBody()),
          ],
        ),
      ),
    );
  }

  Widget _buildBody() {
    if (_loading) {
      return const Center(
        child: SizedBox(
          width: 26,
          height: 26,
          child: CircularProgressIndicator(strokeWidth: 2.4),
        ),
      );
    }

    if (_hubs.isEmpty) {
      return _EmptyState(
        icon: Icons.hub_outlined,
        title: 'No devices yet',
        message: 'Tap the + on the home screen to pair a hub or device.',
      );
    }

    return RefreshIndicator(
      onRefresh: _load,
      color: CelestialColors.accentBlue,
      backgroundColor: CelestialColors.backgroundCard,
      child: ListView(
        padding: const EdgeInsets.fromLTRB(20, 8, 20, 40),
        children: [
          for (final hub in _hubs)
            _HubAccordionTile(
              hub: hub,
              expanded: _expandedType == hub.type,
              onToggle: () => setState(() {
                _expandedType = _expandedType == hub.type ? null : hub.type;
              }),
              onDeviceTap: _openDevice,
            ),
        ],
      ),
    );
  }
}

/// Collapsible hub section: header (icon, name, device summary, status dot,
/// chevron) plus an inline device list revealed when expanded.
class _HubAccordionTile extends StatelessWidget {
  const _HubAccordionTile({
    required this.hub,
    required this.expanded,
    required this.onToggle,
    required this.onDeviceTap,
  });

  final _HubGroup hub;
  final bool expanded;
  final VoidCallback onToggle;
  final ValueChanged<_DeviceEntry> onDeviceTap;

  static const _accent = Color(0xFF00BCD4);

  @override
  Widget build(BuildContext context) {
    return Container(
      margin: const EdgeInsets.only(bottom: 12),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(16),
        border: Border.all(
          color: expanded
              ? _accent.withValues(alpha: 0.3)
              : CelestialColors.orbitRing.withValues(alpha: 0.4),
        ),
      ),
      clipBehavior: Clip.antiAlias,
      child: Column(
        children: [
          _buildHeader(),
          ClipRect(
            child: AnimatedAlign(
              alignment: Alignment.topCenter,
              heightFactor: expanded ? 1 : 0,
              duration: const Duration(milliseconds: 260),
              curve: Curves.easeOutCubic,
              child: _buildDeviceList(),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildHeader() {
    return GestureDetector(
      onTap: onToggle,
      behavior: HitTestBehavior.opaque,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(14, 14, 12, 14),
        child: Row(
          children: [
            Container(
              width: 34,
              height: 34,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: _accent.withValues(alpha: 0.14),
                border: Border.all(color: _accent.withValues(alpha: 0.28)),
              ),
              child: const Icon(Icons.hub_rounded, color: _accent, size: 18),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    hub.label,
                    style: const TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 16,
                      fontWeight: FontWeight.w600,
                      letterSpacing: -0.1,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    _summary(hub.devices),
                    style: TextStyle(
                      color:
                          CelestialColors.textSecondary.withValues(alpha: 0.7),
                      fontSize: 12.5,
                    ),
                  ),
                ],
              ),
            ),
            Container(
              width: 8,
              height: 8,
              margin: const EdgeInsets.only(right: 10),
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: hub.connected
                    ? const Color(0xFF22C55E)
                    : CelestialColors.textSecondary.withValues(alpha: 0.4),
              ),
            ),
            AnimatedRotation(
              turns: expanded ? 0.5 : 0,
              duration: const Duration(milliseconds: 220),
              child: Icon(
                Icons.keyboard_arrow_down_rounded,
                color: CelestialColors.textSecondary.withValues(alpha: 0.5),
                size: 22,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildDeviceList() {
    if (hub.devices.isEmpty) {
      return Padding(
        padding: const EdgeInsets.fromLTRB(16, 0, 16, 16),
        child: Row(
          children: [
            Icon(
              Icons.info_outline_rounded,
              size: 15,
              color: CelestialColors.textSecondary.withValues(alpha: 0.5),
            ),
            const SizedBox(width: 8),
            Text(
              'No devices on this hub yet.',
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.6),
                fontSize: 13,
              ),
            ),
          ],
        ),
      );
    }

    return Column(
      children: [
        Divider(
          height: 1,
          thickness: 0.5,
          color: CelestialColors.orbitRing.withValues(alpha: 0.25),
        ),
        for (final entry in hub.devices)
          _DeviceRow(entry: entry, onTap: onDeviceTap),
        const SizedBox(height: 6),
      ],
    );
  }

  String _summary(List<_DeviceEntry> devices) {
    if (devices.isEmpty) return 'No devices';
    int lights = 0, buttons = 0, motion = 0, contact = 0;
    for (final e in devices) {
      switch (e.device.type) {
        case RhythmDeviceType.light:
          lights++;
        case RhythmDeviceType.button:
          buttons++;
        case RhythmDeviceType.motion:
          motion++;
        case RhythmDeviceType.contact:
          contact++;
      }
    }
    final parts = <String>[];
    if (lights > 0) parts.add('$lights light${lights == 1 ? '' : 's'}');
    if (buttons > 0) parts.add('$buttons button${buttons == 1 ? '' : 's'}');
    if (motion > 0) parts.add('$motion sensor${motion == 1 ? '' : 's'}');
    if (contact > 0) parts.add('$contact contact${contact == 1 ? '' : 's'}');
    return parts.join(' · ');
  }
}

/// A single tappable device row inside a hub accordion.
class _DeviceRow extends StatelessWidget {
  const _DeviceRow({required this.entry, required this.onTap});

  final _DeviceEntry entry;
  final ValueChanged<_DeviceEntry> onTap;

  @override
  Widget build(BuildContext context) {
    final device = entry.device;
    final (icon, iconColor) = _iconForDeviceType(device.type);
    final subtitle = [
      entry.roomName,
      if (device.productInfo != null) device.productInfo!,
    ].join(' · ');

    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: () => onTap(entry),
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 11, 14, 11),
        child: Row(
          children: [
            Icon(icon, color: iconColor, size: 19),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    device.displayName,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 14,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    subtitle,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color:
                          CelestialColors.textSecondary.withValues(alpha: 0.5),
                      fontSize: 11.5,
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(width: 8),
            Text(
              _labelForDeviceType(device.type),
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.55),
                fontSize: 11,
              ),
            ),
            Icon(
              Icons.chevron_right,
              color: CelestialColors.textSecondary.withValues(alpha: 0.35),
              size: 20,
            ),
          ],
        ),
      ),
    );
  }
}

class _EmptyState extends StatelessWidget {
  const _EmptyState({
    required this.icon,
    required this.title,
    required this.message,
  });

  final IconData icon;
  final String title;
  final String message;

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 40),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon,
                size: 44,
                color: CelestialColors.textSecondary.withValues(alpha: 0.4)),
            const SizedBox(height: 16),
            Text(
              title,
              style: const TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 18,
                fontWeight: FontWeight.w600,
              ),
            ),
            const SizedBox(height: 8),
            Text(
              message,
              textAlign: TextAlign.center,
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.65),
                fontSize: 14,
                height: 1.4,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// Back-chevron + centered title header shared by the device screens.
Widget devicesBackHeader(BuildContext context, String title) {
  return Container(
    padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
    child: Row(
      children: [
        GestureDetector(
          onTap: () => Navigator.of(context).pop(),
          child: Container(
            width: 40,
            height: 40,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: CelestialColors.accentBlue.withValues(alpha: 0.2),
            ),
            child: const Icon(
              Icons.chevron_left,
              color: CelestialColors.accentBlue,
              size: 24,
            ),
          ),
        ),
        Expanded(
          child: Text(
            title,
            textAlign: TextAlign.center,
            style: const TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 18,
              fontWeight: FontWeight.w600,
            ),
          ),
        ),
        const SizedBox(width: 40),
      ],
    ),
  );
}

String _hubLabel(String type) {
  switch (type) {
    case 'matter':
      return 'Matter';
    case 'hue':
      return 'Philips Hue';
    case 'hue_ble':
      return 'Hue Bluetooth';
    case 'home_assistant':
    case 'homeassistant':
    case 'ha':
      return 'Home Assistant';
    case 'zigbee':
      return 'Zigbee';
    default:
      if (type.isEmpty) return 'Hub';
      return type[0].toUpperCase() + type.substring(1);
  }
}

String _labelForDeviceType(RhythmDeviceType type) => switch (type) {
      RhythmDeviceType.light => 'Light',
      RhythmDeviceType.button => 'Button',
      RhythmDeviceType.motion => 'Motion',
      RhythmDeviceType.contact => 'Contact',
    };

(IconData, Color) _iconForDeviceType(RhythmDeviceType type) => switch (type) {
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
