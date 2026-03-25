import 'dart:math' as math;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import '../models/config_model.dart';
import '../providers/room_provider.dart';
import '../providers/server_sync_provider.dart';
import '../services/server_http_client.dart';
import 'device_detail_sheet.dart';
import 'light_output_display.dart';
import 'solar_orbit.dart'; // For CelestialColors

/// Bottom sheet with per-room settings.
///
/// Currently shows placeholder settings. Opened by tapping the ellipsis
/// satellite on a room orb while in settings mode.
class RoomSettingsSheet extends StatelessWidget {
  final RoomDto room;

  const RoomSettingsSheet({super.key, required this.room});

  static Future<void> show(BuildContext context, RoomDto room) {
    HapticFeedback.mediumImpact();
    return showModalBottomSheet(
      context: context,
      isScrollControlled: true,
      backgroundColor: Colors.transparent,
      builder: (context) => RoomSettingsSheet(room: room),
    );
  }

  @override
  Widget build(BuildContext context) {
    final topPad = MediaQuery.of(context).padding.top;

    return Padding(
      // Leave room for status bar + a small gap
      padding: EdgeInsets.only(top: topPad + 44),
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
            // Header
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 8),
              child: Text(
                room.name,
                style: TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 18,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 0.5,
                ),
              ),
            ),
            // Room orb preview with live CCT + light output
            Selector<RoomProvider, (int?, int?, bool, bool)>(
              selector: (_, rp) {
                final r = rp.getRoom(room.id);
                return (
                  rp.getBrightness(room.id),
                  rp.getKelvin(room.id),
                  r?.lightsOn ?? false,
                  r?.rhythmEnabled ?? false,
                );
              },
              builder: (context, data, _) {
                final (brightness, kelvin, lightsOn, rhythmEnabled) = data;
                return _AnimatedRoomOrb(
                  roomId: room.id,
                  roomName: room.name,
                  brightness: brightness,
                  kelvin: kelvin,
                  lightsOn: lightsOn,
                  rhythmEnabled: rhythmEnabled,
                );
              },
            ),
            const SizedBox(height: 16),
            // Settings list
            Expanded(
              child: ListView(
                padding: const EdgeInsets.symmetric(horizontal: 20),
                children: [
                  _buildSettingsGroup('General', [
                    _SettingsRow(
                      icon: Icons.label_outline,
                      label: 'Name',
                      trailing: Row(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          Text(
                            room.name,
                            style: const TextStyle(
                              color: CelestialColors.textSecondary,
                              fontSize: 14,
                            ),
                          ),
                          const SizedBox(width: 4),
                          Icon(
                            Icons.edit_outlined,
                            color: CelestialColors.textSecondary.withValues(alpha: 0.5),
                            size: 14,
                          ),
                        ],
                      ),
                      onTap: () => _showRenameDialog(context),
                    ),
                    _SettingsRow(
                      icon: Icons.hub_outlined,
                      label: 'Source',
                      trailing: Text(
                        _sourceLabel(room.source),
                        style: const TextStyle(
                          color: CelestialColors.textSecondary,
                          fontSize: 14,
                        ),
                      ),
                    ),
                    Selector<RoomProvider, bool>(
                      selector: (_, rp) =>
                          rp.getRoom(room.id)?.disabled ?? room.disabled,
                      builder: (context, isHidden, _) {
                        return _SettingsRow(
                          icon: Icons.visibility_off_outlined,
                          label: 'Hide this room',
                          trailing: _ToggleSwitch(
                            value: isHidden,
                            onChanged: (val) {
                              context.read<RoomProvider>().toggleDisabled(room.id);
                              HapticFeedback.selectionClick();
                            },
                          ),
                          onTap: () {
                            context.read<RoomProvider>().toggleDisabled(room.id);
                            HapticFeedback.selectionClick();
                          },
                        );
                      },
                    ),
                    _SettingsRow(
                      icon: Icons.merge_type_outlined,
                      label: 'Merge with...',
                      trailing: const Icon(
                        Icons.chevron_right,
                        color: CelestialColors.textSecondary,
                        size: 20,
                      ),
                      onTap: () => _showMergeDialog(context),
                    ),
                  ]),
                  const SizedBox(height: 16),
                  _buildDevicesSection(context),
                  const SizedBox(height: 16),
                  Consumer<ConfigModel>(
                    builder: (context, configModel, _) {
                      final cfg = room.curveConfig ?? configModel.config;
                      return _buildSettingsGroup('Adaptive Lighting', [
                        _SettingsRow(
                          icon: Icons.brightness_6_outlined,
                          label: 'Brightness Range',
                          trailing: Text(
                            '${cfg.minBrightness}% – ${cfg.maxBrightness}%',
                            style: const TextStyle(
                              color: CelestialColors.textSecondary,
                              fontSize: 14,
                            ),
                          ),
                          onTap: () {},
                        ),
                        _SettingsRow(
                          icon: Icons.thermostat_outlined,
                          label: 'Color Temp Range',
                          trailing: Text(
                            '${cfg.minColorTemp}K – ${cfg.maxColorTemp}K',
                            style: const TextStyle(
                              color: CelestialColors.textSecondary,
                              fontSize: 14,
                            ),
                          ),
                          onTap: () {},
                        ),
                      ]);
                    },
                  ),
                  // const SizedBox(height: 16),
                  // _buildSettingsGroup('Behavior', [
                  //   _SettingsRow(
                  //     icon: Icons.nightlight_outlined,
                  //     label: 'Sleep Schedule',
                  //     trailing: const Icon(
                  //       Icons.chevron_right,
                  //       color: CelestialColors.textSecondary,
                  //       size: 20,
                  //     ),
                  //     onTap: () {},
                  //   ),
                  //   _SettingsRow(
                  //     icon: Icons.schedule_outlined,
                  //     label: 'Transition Speed',
                  //     trailing: const Text(
                  //       'Normal',
                  //       style: TextStyle(
                  //         color: CelestialColors.textSecondary,
                  //         fontSize: 14,
                  //       ),
                  //     ),
                  //     onTap: () {},
                  //   ),
                  // ]),
                ],
              ),
            ),
            // Done button — pinned at bottom
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

  Widget _buildSettingsGroup(String title, List<Widget> children) {
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
                    indent: 48,
                    color: CelestialColors.orbitRing.withValues(alpha: 0.2),
                  ),
              ],
            ],
          ),
        ),
      ],
    );
  }

  // _buildTogglePlaceholder removed — replaced by _ToggleSwitch widget

  Future<void> _showRenameDialog(BuildContext context) async {
    final controller = TextEditingController(text: room.name);
    final newName = await showDialog<String>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: const Text(
          'Rename Room',
          style: TextStyle(color: CelestialColors.textPrimary),
        ),
        content: TextField(
          controller: controller,
          autofocus: true,
          style: const TextStyle(color: CelestialColors.textPrimary),
          decoration: InputDecoration(
            hintText: 'Room name',
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

    if (newName != null && newName.isNotEmpty && newName != room.name && context.mounted) {
      final http = context.read<ServerSyncProvider>().httpClient;
      await http.topologyRenameRoom(room.id, newName);
      // Trigger re-sync so the name updates
      http.triggerSync();
    }
  }

  Future<void> _showMergeDialog(BuildContext context) async {
    final syncProvider = context.read<ServerSyncProvider>();
    final rooms = syncProvider.helloRooms
        .where((r) => r.id != room.id)
        .toList();

    if (rooms.isEmpty) {
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(content: Text('No other rooms available')),
        );
      }
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
                'Merge "${room.name}" into...',
                style: const TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 16,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
            for (final r in rooms)
              ListTile(
                leading: const Icon(
                  Icons.meeting_room_rounded,
                  color: Color(0xFFFFC107),
                ),
                title: Text(
                  r.name,
                  style: const TextStyle(color: CelestialColors.textPrimary),
                ),
                subtitle: Text(
                  r.deviceSummary,
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.7),
                    fontSize: 12,
                  ),
                ),
                onTap: () => Navigator.of(ctx).pop(r),
              ),
            SizedBox(height: MediaQuery.of(ctx).padding.bottom + 16),
          ],
        ),
      ),
    );

    if (targetRoom == null || !context.mounted) return;

    // Confirm merge
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: const Text(
          'Merge Rooms',
          style: TextStyle(color: CelestialColors.textPrimary),
        ),
        content: Text(
          'Merge "${room.name}" into "${targetRoom.name}"? '
          'All devices and hub targets will be combined.',
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.8),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: Text(
              'Cancel',
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.7),
              ),
            ),
          ),
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: const Text(
              'Merge',
              style: TextStyle(color: CelestialColors.sunWarm),
            ),
          ),
        ],
      ),
    );

    if (confirmed != true || !context.mounted) return;

    final http = context.read<ServerSyncProvider>().httpClient;
    final success = await http.topologyMergeRooms(targetRoom.id, room.id);
    if (context.mounted) {
      if (success) {
        Navigator.of(context).pop(); // Close settings sheet
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text('Merged "${room.name}" into "${targetRoom.name}"'),
          ),
        );
        http.triggerSync();
      } else {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(content: Text('Failed to merge rooms')),
        );
      }
    }
  }

  Widget _buildDevicesSection(BuildContext context) {
    final devices = context.read<ServerSyncProvider>().devicesForRoom(room.id);
    if (devices.isEmpty) {
      // Fallback: show summary from device IDs count
      final summary = context.read<ServerSyncProvider>().deviceSummaryForRoom(room.id);
      if (summary.isEmpty) return const SizedBox.shrink();
      return _buildSettingsGroup('Devices', [
        _SettingsRow(
          icon: Icons.devices_outlined,
          label: summary,
          trailing: const SizedBox.shrink(),
        ),
      ]);
    }

    // Sort: lights → buttons → motion
    final sorted = List<TypedDevice>.from(devices)
      ..sort((a, b) {
        const order = {
          ServerDeviceType.light: 0,
          ServerDeviceType.button: 1,
          ServerDeviceType.motion: 2,
        };
        return (order[a.type] ?? 3).compareTo(order[b.type] ?? 3);
      });

    return _buildSettingsGroup('Devices', [
      for (final device in sorted)
        _DeviceRow(device: device, roomId: room.id),
    ]);
  }

  String _sourceLabel(RoomSourceDto source) {
    switch (source) {
      case RoomSourceDto.hue:
        return 'Philips Hue';
      case RoomSourceDto.homeAssistant:
        return 'Home Assistant';
      case RoomSourceDto.esp32:
        return 'ESP32';
      case RoomSourceDto.unknown:
        return 'Unknown';
    }
  }
}

/// A single row in the settings sheet.
class _SettingsRow extends StatelessWidget {
  final IconData icon;
  final String label;
  final Widget trailing;
  final VoidCallback? onTap;

  const _SettingsRow({
    required this.icon,
    required this.label,
    required this.trailing,
    this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: onTap,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 13),
        child: Row(
          children: [
            Icon(
              icon,
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
            trailing,
          ],
        ),
      ),
    );
  }
}

/// A device row in the room settings Devices section.
class _DeviceRow extends StatelessWidget {
  final TypedDevice device;
  final String roomId;

  const _DeviceRow({required this.device, required this.roomId});

  @override
  Widget build(BuildContext context) {
    final (icon, iconColor) = _iconForType(device.type);
    final typeLabel = switch (device.type) {
      ServerDeviceType.light => 'Light',
      ServerDeviceType.button => 'Button',
      ServerDeviceType.motion => 'Motion',
    };

    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: () => DeviceDetailSheet.show(context, device, roomId),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
        child: Row(
          children: [
            Icon(icon, color: iconColor, size: 20),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    device.displayName,
                    style: const TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 14,
                    ),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                  if (device.productInfo != null)
                    Padding(
                      padding: const EdgeInsets.only(top: 2),
                      child: Text(
                        device.productInfo!,
                        style: TextStyle(
                          color: CelestialColors.textSecondary.withValues(alpha: 0.6),
                          fontSize: 11,
                        ),
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                      ),
                    ),
                ],
              ),
            ),
            Text(
              typeLabel,
              style: const TextStyle(
                color: CelestialColors.textSecondary,
                fontSize: 12,
              ),
            ),
          ],
        ),
      ),
    );
  }

  (IconData, Color) _iconForType(ServerDeviceType type) => switch (type) {
    ServerDeviceType.light => (Icons.lightbulb_outline, const Color(0xFFFFB74D)),
    ServerDeviceType.button => (Icons.touch_app_outlined, const Color(0xFF64B5F6)),
    ServerDeviceType.motion => (Icons.sensors_outlined, const Color(0xFF81C784)),
  };
}

/// Custom toggle switch matching the celestial design system.
class _ToggleSwitch extends StatelessWidget {
  final bool value;
  final ValueChanged<bool> onChanged;

  const _ToggleSwitch({required this.value, required this.onChanged});

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: () => onChanged(!value),
      child: Container(
        width: 44,
        height: 26,
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(13),
          color: value
              ? CelestialColors.sunWarm.withValues(alpha: 0.3)
              : CelestialColors.orbitRing.withValues(alpha: 0.3),
          border: Border.all(
            color: value
                ? CelestialColors.sunWarm.withValues(alpha: 0.6)
                : CelestialColors.orbitRing.withValues(alpha: 0.5),
            width: 1,
          ),
        ),
        child: AnimatedAlign(
          duration: const Duration(milliseconds: 200),
          alignment: value ? Alignment.centerRight : Alignment.centerLeft,
          child: Container(
            width: 20,
            height: 20,
            margin: const EdgeInsets.symmetric(horizontal: 2),
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: value
                  ? CelestialColors.sunWarm
                  : CelestialColors.textSecondary,
            ),
          ),
        ),
      ),
    );
  }
}

/// Animated room orb with orbiting arcs when rhythm is active.
///
/// Tap to toggle rhythm on/off. Shows:
/// - Active: rotating comet-tail arcs + breathing glow + "Rhythm" label
/// - Paused: static orb + "Paused ▶" hint
/// - Off: grey circle + "Off"
class _AnimatedRoomOrb extends StatefulWidget {
  final String roomId;
  final String roomName;
  final int? brightness;
  final int? kelvin;
  final bool lightsOn;
  final bool rhythmEnabled;

  const _AnimatedRoomOrb({
    required this.roomId,
    required this.roomName,
    required this.brightness,
    required this.kelvin,
    required this.lightsOn,
    required this.rhythmEnabled,
  });

  @override
  State<_AnimatedRoomOrb> createState() => _AnimatedRoomOrbState();
}

class _AnimatedRoomOrbState extends State<_AnimatedRoomOrb>
    with TickerProviderStateMixin {
  late AnimationController _rotationController;
  late AnimationController _pulseController;
  late AnimationController _activeController;
  bool _pressed = false;

  bool get _active => widget.rhythmEnabled && widget.lightsOn;

  @override
  void initState() {
    super.initState();
    _rotationController = AnimationController(
      vsync: this,
      duration: const Duration(seconds: 8),
    );
    _pulseController = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 3000),
    );
    _activeController = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 500),
      value: _active ? 1.0 : 0.0,
    );
    _activeController.addStatusListener(_onActiveStatus);
    if (_active) {
      _rotationController.repeat();
      _pulseController.repeat(reverse: true);
    }
  }

  void _onActiveStatus(AnimationStatus status) {
    if (status == AnimationStatus.dismissed) {
      _rotationController.stop();
      _pulseController.stop();
    }
  }

  @override
  void didUpdateWidget(_AnimatedRoomOrb old) {
    super.didUpdateWidget(old);
    if (old.rhythmEnabled != widget.rhythmEnabled ||
        old.lightsOn != widget.lightsOn) {
      _syncAnimations();
    }
  }

  void _syncAnimations() {
    if (_active) {
      if (!_rotationController.isAnimating) _rotationController.repeat();
      if (!_pulseController.isAnimating) {
        _pulseController.repeat(reverse: true);
      }
      _activeController.animateTo(1.0,
          duration: const Duration(milliseconds: 500),
          curve: Curves.easeOut);
    } else {
      _activeController.animateTo(0.0,
          duration: const Duration(milliseconds: 600),
          curve: Curves.easeIn);
    }
  }

  void _onTap() {
    HapticFeedback.lightImpact();
    final newEnabled = !widget.rhythmEnabled;
    final roomProvider = context.read<RoomProvider>();
    roomProvider.setRoomRhythmEnabled(widget.roomId, newEnabled);
    final serverSync = context.read<ServerSyncProvider>();
    serverSync.pushRoomPreferences(widget.roomId, rhythmEnabled: newEnabled);
  }

  @override
  void dispose() {
    _activeController.removeStatusListener(_onActiveStatus);
    _rotationController.dispose();
    _pulseController.dispose();
    _activeController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final effectiveBrightness = widget.brightness ?? 0;
    final effectiveKelvin = widget.kelvin ?? 3000;
    final cctColor = widget.lightsOn
        ? ColorUtils.cctToColor(effectiveKelvin)
        : CelestialColors.textSecondary;
    final glowAlpha =
        widget.lightsOn ? 0.15 + (effectiveBrightness / 100.0) * 0.25 : 0.05;
    final borderAlpha =
        widget.lightsOn ? 0.4 + (effectiveBrightness / 100.0) * 0.4 : 0.2;

    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTap: _onTap,
          onTapDown: (_) => setState(() => _pressed = true),
          onTapUp: (_) => setState(() => _pressed = false),
          onTapCancel: () => setState(() => _pressed = false),
          child: AnimatedScale(
            scale: _pressed ? 0.93 : 1.0,
            duration: const Duration(milliseconds: 100),
            curve: Curves.easeInOut,
            child: SizedBox(
              width: 100,
              height: 100,
              child: AnimatedBuilder(
                animation: Listenable.merge([
                  _rotationController,
                  _pulseController,
                  _activeController,
                ]),
                builder: (context, child) {
                  final activeT = _activeController.value;
                  final pulse = _pulseController.value;
                  final extraGlow = activeT * pulse * 0.12;
                  final extraSpread = activeT * pulse * 3.0;

                  return CustomPaint(
                    painter: _OrbitArcPainter(
                      rotation: _rotationController.value,
                      color: cctColor,
                      opacity: activeT * (0.5 + pulse * 0.4),
                    ),
                    child: Center(
                      child: Container(
                        width: 80,
                        height: 80,
                        decoration: BoxDecoration(
                          shape: BoxShape.circle,
                          color:
                              cctColor.withValues(alpha: glowAlpha + extraGlow),
                          border: Border.all(
                            color: cctColor.withValues(alpha: borderAlpha),
                            width: 2,
                          ),
                          boxShadow: [
                            BoxShadow(
                              color: cctColor.withValues(
                                alpha: (widget.lightsOn ? 0.2 : 0.05) +
                                    extraGlow,
                              ),
                              blurRadius: 16 + extraSpread * 2,
                              spreadRadius: 2 + extraSpread,
                            ),
                          ],
                        ),
                        child: child,
                      ),
                    ),
                  );
                },
                child: Center(
                  child: Text(
                    widget.roomName.isNotEmpty
                        ? widget.roomName[0].toUpperCase()
                        : '?',
                    style: TextStyle(
                      color: cctColor,
                      fontSize: 28,
                      fontWeight: FontWeight.w700,
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
        if (widget.lightsOn) ...[
          const SizedBox(height: 12),
          LightOutputCompact(
            brightness: effectiveBrightness,
            kelvin: effectiveKelvin,
          ),
          const SizedBox(height: 6),
          GestureDetector(
            behavior: HitTestBehavior.opaque,
            onTap: _onTap,
            child: _RhythmStatusLabel(
              active: _active,
              pulseAnimation: _pulseController,
              color: cctColor,
            ),
          ),
        ] else
          Padding(
            padding: const EdgeInsets.only(top: 8),
            child: Text(
              'Off',
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.6),
                fontSize: 14,
                fontWeight: FontWeight.w500,
              ),
            ),
          ),
      ],
    );
  }
}

/// Label below the orb showing rhythm state with visual hints.
class _RhythmStatusLabel extends StatelessWidget {
  final bool active;
  final Animation<double> pulseAnimation;
  final Color color;

  const _RhythmStatusLabel({
    required this.active,
    required this.pulseAnimation,
    required this.color,
  });

  @override
  Widget build(BuildContext context) {
    return AnimatedSwitcher(
      duration: const Duration(milliseconds: 250),
      child: active
          ? Row(
              key: const ValueKey(true),
              mainAxisSize: MainAxisSize.min,
              children: [
                AnimatedBuilder(
                  animation: pulseAnimation,
                  builder: (context, _) => Container(
                    width: 6,
                    height: 6,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      color: color.withValues(
                          alpha: 0.4 + pulseAnimation.value * 0.6),
                      boxShadow: [
                        BoxShadow(
                          color: color.withValues(
                              alpha: pulseAnimation.value * 0.4),
                          blurRadius: 4,
                        ),
                      ],
                    ),
                  ),
                ),
                const SizedBox(width: 6),
                Text(
                  'Rhythm',
                  style: TextStyle(
                    color:
                        CelestialColors.textSecondary.withValues(alpha: 0.8),
                    fontSize: 12,
                    fontWeight: FontWeight.w500,
                  ),
                ),
              ],
            )
          : Row(
              key: const ValueKey(false),
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(
                  Icons.play_arrow_rounded,
                  color:
                      CelestialColors.textSecondary.withValues(alpha: 0.5),
                  size: 14,
                ),
                const SizedBox(width: 2),
                Text(
                  'Paused',
                  style: TextStyle(
                    color:
                        CelestialColors.textSecondary.withValues(alpha: 0.6),
                    fontSize: 12,
                    fontWeight: FontWeight.w500,
                  ),
                ),
              ],
            ),
    );
  }
}

/// Paints orbiting gradient arcs (comet tails) around the room orb.
class _OrbitArcPainter extends CustomPainter {
  final double rotation;
  final Color color;
  final double opacity;

  static const _arcs = [
    (offset: 0.0, sweepDeg: 80.0, widthFactor: 1.0, alphaFactor: 1.0),
    (offset: 2.5, sweepDeg: 50.0, widthFactor: 0.8, alphaFactor: 0.55),
    (offset: 4.3, sweepDeg: 30.0, widthFactor: 0.65, alphaFactor: 0.3),
  ];

  _OrbitArcPainter({
    required this.rotation,
    required this.color,
    required this.opacity,
  });

  @override
  void paint(Canvas canvas, Size size) {
    if (opacity <= 0.01) return;

    final center = Offset(size.width / 2, size.height / 2);
    final radius = size.width / 2 - 3;
    final rect = Rect.fromCircle(center: center, radius: radius);
    final baseAngle = rotation * 2 * math.pi;

    for (final arc in _arcs) {
      _drawFadingArc(
        canvas,
        rect,
        baseAngle + arc.offset,
        arc.sweepDeg,
        2.5 * arc.widthFactor,
        opacity * arc.alphaFactor,
      );
    }
  }

  void _drawFadingArc(
    Canvas canvas,
    Rect rect,
    double startAngle,
    double sweepDegrees,
    double width,
    double alpha,
  ) {
    const segments = 10;
    final sweepRad = sweepDegrees * math.pi / 180;
    final segmentSweep = sweepRad / segments;

    for (int i = 0; i < segments; i++) {
      final t = i / segments;
      final segAlpha = (alpha * (1.0 - t * 0.85)).clamp(0.0, 1.0);
      final paint = Paint()
        ..color = color.withValues(alpha: segAlpha)
        ..style = PaintingStyle.stroke
        ..strokeWidth = width
        ..strokeCap = StrokeCap.round;

      canvas.drawArc(
        rect,
        startAngle + i * segmentSweep,
        segmentSweep + 0.015,
        false,
        paint,
      );
    }
  }

  @override
  bool shouldRepaint(_OrbitArcPainter old) =>
      rotation != old.rotation ||
      opacity != old.opacity ||
      color != old.color;
}
