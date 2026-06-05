import 'dart:async';
import 'dart:math' as math;
import 'package:flutter/material.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import '../providers/room_provider.dart';
import '../providers/server_sync_provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show
        RhythmCurveConfig,
        RhythmDevice,
        RhythmDeviceType,
        RhythmMode,
        RhythmNodeProfileSettings,
        RhythmTimerSetting;
import 'device_detail_sheet.dart';
import 'info_tooltip.dart';
import 'light_output_display.dart';
import 'auto_slider_setting_row.dart';
import 'solar_orbit.dart'; // For CelestialColors

/// Bottom sheet with per-room settings.
///
/// Currently shows placeholder settings. Opened by tapping the ellipsis
/// satellite on a room orb while in settings mode.
class RoomSettingsSheet extends StatefulWidget {
  final RoomDto room;
  final bool enableLivePreview;

  const RoomSettingsSheet({
    super.key,
    required this.room,
    this.enableLivePreview = true,
  });

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
  State<RoomSettingsSheet> createState() => _RoomSettingsSheetState();
}

enum _SheetTab { rhythm, devices, settings }

class _RoomSettingsSheetState extends State<RoomSettingsSheet> {
  // Opens on the "Settings" tab (internally `rhythm`).
  _SheetTab _selectedTab = _SheetTab.rhythm;
  bool _deletingRoom = false;
  final Map<String, RhythmTimerSetting> _motionTimeoutDrafts = {};

  RoomDto get room => widget.room;

  @override
  void initState() {
    super.initState();
    _refreshLivePreviewState();
  }

  @override
  void didUpdateWidget(RoomSettingsSheet oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.room.id != widget.room.id) {
      _motionTimeoutDrafts.clear();
    }
    if (oldWidget.room.id != widget.room.id ||
        oldWidget.enableLivePreview != widget.enableLivePreview) {
      _refreshLivePreviewState();
    }
  }

  void _refreshLivePreviewState() {
    if (!widget.enableLivePreview) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      unawaited(
        context.read<ServerSyncProvider>().ensureRoomPreviewStateFresh(room.id),
      );
    });
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
            if (widget.enableLivePreview) ...[
              // Room orb preview with live CCT + light output
              Selector<RoomProvider,
                  (int?, int?, (int, int, int)?, bool, bool, DateTime?)>(
                selector: (_, rp) {
                  final r = rp.getRoom(room.id);
                  return (
                    rp.getBrightness(room.id),
                    rp.getKelvin(room.id),
                    rp.getRoomColor(room.id),
                    r?.lightsOn ?? false,
                    r?.rhythmEnabled ?? false,
                    rp.getLastTickTime(room.id),
                  );
                },
                builder: (context, data, _) {
                  final (
                    brightness,
                    kelvin,
                    color,
                    lightsOn,
                    rhythmEnabled,
                    lastTickTime
                  ) = data;
                  final intervalSecs =
                      context.read<ServerSyncProvider>().rhythmIntervalSecs;
                  return _AnimatedRoomOrb(
                    roomId: room.id,
                    roomName: room.name,
                    brightness: brightness,
                    kelvin: kelvin,
                    directColor: color != null
                        ? Color.fromARGB(255, color.$1, color.$2, color.$3)
                        : null,
                    lightsOn: lightsOn,
                    rhythmEnabled: rhythmEnabled,
                    rhythmIntervalSecs: intervalSecs,
                    lastTickTime: lastTickTime,
                  );
                },
              ),
              const SizedBox(height: 12),
            ],
            // Tab selector
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 20),
              child: _TabSelector(
                selected: _selectedTab,
                onChanged: (tab) => setState(() => _selectedTab = tab),
              ),
            ),
            const SizedBox(height: 16),
            // Tab content
            Expanded(
              child: AnimatedSwitcher(
                duration: const Duration(milliseconds: 200),
                child: switch (_selectedTab) {
                  _SheetTab.rhythm => _buildRhythmContent(context),
                  _SheetTab.devices => _buildDevicesContent(context),
                  _SheetTab.settings => _buildSettingsContent(context),
                },
              ),
            ),
            // Done button — pinned at bottom
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

  Widget _buildSettingsGroup(String title, List<Widget> children) {
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

  Widget _buildRhythmContent(BuildContext context) {
    final syncProvider = context.watch<ServerSyncProvider>();
    final node = syncProvider.nodeById(room.id);
    final settings = node?.profileSettings;
    final standbyEnabled = syncProvider.standbyEnabledForNode(room.id);

    return ListView(
      key: const ValueKey('rhythm'),
      padding: const EdgeInsets.symmetric(horizontal: 20),
      children: [
        // Standby is its own room-level behavior, not a Day/Sleep override —
        // so it sits above the per-mode profile groups as a standalone card.
        _buildSettingsGroup('', [
          _SettingsRow(
            icon: Icons.bedtime_outlined,
            label: 'Off Behavior',
            labelInfo: const InfoTooltip(
              eyebrow: 'OFF BEHAVIOR',
              accentColor: Color(0xFF7C83FF),
              iconSize: 15,
              message:
                  'What this room does when it switches off. Off cuts the '
                  'lights fully. Standby keeps them in a low, ready state '
                  'instead of going dark.',
            ),
            trailing: _OffBehaviorSwitch(
              standby: standbyEnabled,
              onChanged: (val) => _setStandbyEnabled(context, val),
            ),
            onTap: () => _setStandbyEnabled(context, !standbyEnabled),
          ),
        ]),
        const SizedBox(height: 16),
        _buildSettingsGroup('Day Profile', [
          _buildMotionTimeoutRow(
            context,
            mode: RhythmMode.day,
            profileSettings: settings,
          ),
        ]),
        const SizedBox(height: 16),
        _buildSettingsGroup('Sleep Profile', [
          _buildMotionTimeoutRow(
            context,
            mode: RhythmMode.sleep,
            profileSettings: settings,
          ),
        ]),
      ],
    );
  }

  Widget _buildMotionTimeoutRow(
    BuildContext context, {
    required RhythmMode mode,
    required RhythmNodeProfileSettings? profileSettings,
  }) {
    final syncProvider = context.read<ServerSyncProvider>();
    final profileId = _activeProfileIdForMode(syncProvider, mode);
    final draftKey = _motionTimeoutDraftKey(mode, profileId);
    final committedSetting =
        _motionTimeoutSettingForProfile(profileId, profileSettings);
    final draftSetting = _motionTimeoutDrafts[draftKey];
    final setting = draftSetting ?? committedSetting;
    final inheritedSecs = _defaultMotionTimeoutSecs(context, mode);
    final currentSecs = setting?.fixedValue ?? inheritedSecs;
    final sliderValue = currentSecs.clamp(30, 1800).toDouble();

    void push(int? secs) {
      syncProvider.pushNodeProfileOverrides(
        room.id,
        profileOverrides: {
          profileId: secs == null
              ? null
              : {
                  'motion_timeout_secs':
                      RhythmTimerSetting.fixed(secs).toJson(),
                },
        },
      );
      HapticFeedback.selectionClick();
    }

    return Padding(
      padding: const EdgeInsets.fromLTRB(14, 13, 14, 12),
      child: AutoSliderSettingRow(
        icon: mode == RhythmMode.sleep
            ? Icons.nightlight_outlined
            : Icons.wb_sunny_outlined,
        title: 'Motion Timeout',
        titleColor: CelestialColors.textPrimary,
        color: const Color(0xFF4ADE80),
        isAuto: setting == null || setting.isAuto,
        sliderValue: sliderValue,
        effectiveValue: inheritedSecs.toDouble(),
        sliderMin: 30,
        sliderMax: 1800,
        divisions: 59,
        format: (value) => _formatMotionTimeout(value.round()),
        tooltip: 'How long the lights stay on after motion is last detected '
            'before timing out to mood.',
        onAuto: () {
          _commitMotionTimeoutSetting(mode, profileId, null);
          push(null);
        },
        onManual: () {
          _setMotionTimeoutDraft(
            mode,
            profileId,
            RhythmTimerSetting.fixed(inheritedSecs),
          );
          HapticFeedback.selectionClick();
        },
        onSliderChanged: (value) {
          _setMotionTimeoutDraft(
            mode,
            profileId,
            RhythmTimerSetting.fixed(value.round()),
          );
        },
        onSliderChangeEnd: (value) {
          final secs = value.round();
          _commitMotionTimeoutSetting(
            mode,
            profileId,
            RhythmTimerSetting.fixed(secs),
          );
          push(secs);
        },
      ),
    );
  }

  RhythmTimerSetting? _motionTimeoutSettingForProfile(
    String profileId,
    RhythmNodeProfileSettings? profileSettings,
  ) {
    return profileSettings?.profileOverrides[profileId]?.motionTimeoutSetting;
  }

  void _setMotionTimeoutDraft(
    RhythmMode mode,
    String profileId,
    RhythmTimerSetting setting,
  ) {
    setState(() {
      _motionTimeoutDrafts[_motionTimeoutDraftKey(mode, profileId)] = setting;
    });
  }

  void _commitMotionTimeoutSetting(
    RhythmMode mode,
    String profileId,
    RhythmTimerSetting? setting,
  ) {
    setState(() {
      _motionTimeoutDrafts.remove(_motionTimeoutDraftKey(mode, profileId));
    });
    context.read<ServerSyncProvider>().setNodeProfileMotionTimeoutLocal(
          room.id,
          mode: mode,
          profileId: profileId,
          setting: setting,
        );
  }

  String _motionTimeoutDraftKey(RhythmMode mode, String profileId) =>
      '${mode.wireValue}:$profileId';

  int _defaultMotionTimeoutSecs(BuildContext context, RhythmMode mode) {
    final syncProvider = context.read<ServerSyncProvider>();
    if (syncProvider.activeMode == mode &&
        syncProvider.effectiveMotionTimeoutSecs != null) {
      return syncProvider.effectiveMotionTimeoutSecs!;
    }

    final profileId = _activeProfileIdForMode(syncProvider, mode);
    for (final profile in syncProvider.profiles) {
      if (profile.id == profileId) {
        return profile.motionTimeoutSecs ??
            RhythmCurveConfig.defaultMotionTimeoutSecs;
      }
    }
    return RhythmCurveConfig.defaultMotionTimeoutSecs;
  }

  String _activeProfileIdForMode(
      ServerSyncProvider syncProvider, RhythmMode mode) {
    for (final config in syncProvider.modeConfigs) {
      if (config.mode == mode && config.activeProfileId.isNotEmpty) {
        return config.activeProfileId;
      }
    }
    return mode == RhythmMode.sleep ? 'sleep' : 'rhythm';
  }

  String _formatMotionTimeout(int secs) {
    if (secs == 0) return 'Off';
    if (secs >= 60) return '${(secs / 60).round()}m';
    return '${secs}s';
  }

  Widget _buildSettingsContent(BuildContext context) {
    final syncProvider = context.watch<ServerSyncProvider>();
    final renameLabel = room.kind.isRoom ? 'Name' : 'Node Name';
    final hideLabel = room.kind == RoomNodeKind.lightDevice
        ? 'Hide this light'
        : 'Hide this room';
    final canDeleteRoom = _canDeleteRoom(syncProvider);
    return ListView(
      key: const ValueKey('settings'),
      padding: const EdgeInsets.symmetric(horizontal: 20),
      children: [
        _buildSettingsGroup('General', [
          _SettingsRow(
            icon: Icons.label_outline,
            label: renameLabel,
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
                if (room.kind.isRoom) ...[
                  const SizedBox(width: 4),
                  Icon(
                    Icons.edit_outlined,
                    color: CelestialColors.textSecondary.withValues(alpha: 0.5),
                    size: 14,
                  ),
                ],
              ],
            ),
            onTap: room.kind.isRoom ? () => _showRenameDialog(context) : null,
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
            selector: (_, rp) => rp.getRoom(room.id)?.disabled ?? room.disabled,
            builder: (context, isHidden, _) {
              return _SettingsRow(
                icon: Icons.visibility_off_outlined,
                label: hideLabel,
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
        ]),
        if (canDeleteRoom) ...[
          const SizedBox(height: 16),
          _buildSettingsGroup('Danger', [
            _SettingsRow(
              icon: Icons.delete_outline,
              label: _deletingRoom ? 'Deleting Room...' : 'Delete Room',
              trailing: _deletingRoom
                  ? SizedBox(
                      width: 16,
                      height: 16,
                      child: CircularProgressIndicator(
                        strokeWidth: 2,
                        color: Colors.red.shade300,
                      ),
                    )
                  : Text(
                      'Delete',
                      style: TextStyle(
                        color: Colors.red.shade300,
                        fontSize: 14,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
              onTap: _deletingRoom ? null : () => _confirmDeleteRoom(context),
            ),
          ]),
        ],
      ],
    );
  }

  void _setStandbyEnabled(BuildContext context, bool enabled) {
    final syncProvider = context.read<ServerSyncProvider>();
    syncProvider.setNodeStandbyEnabledLocal(room.id, enabled);
    syncProvider.pushNodePreferences(room.id, standbyEnabled: enabled);
    HapticFeedback.selectionClick();
  }

  Widget _buildDevicesContent(BuildContext context) {
    return ListView(
      key: const ValueKey('devices'),
      padding: const EdgeInsets.symmetric(horizontal: 20),
      children: [
        _buildDevicesSection(context),
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

    if (newName != null &&
        newName.isNotEmpty &&
        newName != room.name &&
        context.mounted) {
      final http = context.read<ServerSyncProvider>().api;
      await http.topologyRenameRoom(room.id, newName);
      // Trigger re-sync so the name updates
      http.triggerSync();
    }
  }

  bool _canDeleteRoom(ServerSyncProvider syncProvider) {
    if (!room.kind.isRoom) return false;
    final roomSummary =
        syncProvider.helloRooms.where((r) => r.id == room.id).firstOrNull;
    if (roomSummary == null) return false;
    final hasProtectedHubBulbs = roomSummary.lightCount > 0 &&
        roomSummary.hubTypes.any(_isProtectedLightHubType);
    return !hasProtectedHubBulbs;
  }

  bool _isProtectedLightHubType(String hubType) {
    return switch (hubType) {
      'hue' || 'homeassistant' || 'home_assistant' || 'ha' => true,
      _ => false,
    };
  }

  Future<void> _confirmDeleteRoom(BuildContext context) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: const Text(
          'Delete Room?',
          style: TextStyle(color: CelestialColors.textPrimary),
        ),
        content: Text(
          'Delete "${room.name}" from the topology? Any remaining devices in this room will become unassigned.',
          style: const TextStyle(color: CelestialColors.textSecondary),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: Text(
              'Cancel',
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.8),
              ),
            ),
          ),
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: Text(
              'Delete',
              style: TextStyle(color: Colors.red.shade300),
            ),
          ),
        ],
      ),
    );

    if (confirmed != true || !context.mounted) return;
    await _deleteRoom(context);
  }

  Future<void> _deleteRoom(BuildContext context) async {
    final syncProvider = context.read<ServerSyncProvider>();
    final messenger = ScaffoldMessenger.of(context);
    final navigator = Navigator.of(context);
    setState(() => _deletingRoom = true);

    final success = await syncProvider.api.topologyDeleteRoom(room.id);

    if (!mounted) return;

    if (!success) {
      setState(() => _deletingRoom = false);
      messenger.showSnackBar(
        const SnackBar(content: Text('Room deletion failed')),
      );
      return;
    }

    await syncProvider.fullRefresh();
    if (!mounted) return;

    navigator.pop();
    messenger.showSnackBar(
      SnackBar(content: Text('Deleted ${room.name}')),
    );
  }

  Widget _buildDevicesSection(BuildContext context) {
    final devices = context.read<ServerSyncProvider>().devicesForRoom(room.id);
    if (devices.isEmpty) {
      // Fallback: show summary from device IDs count
      final summary =
          context.read<ServerSyncProvider>().deviceSummaryForRoom(room.id);
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
    final sorted = List<RhythmDevice>.from(devices)
      ..sort((a, b) {
        const order = {
          RhythmDeviceType.light: 0,
          RhythmDeviceType.button: 1,
          RhythmDeviceType.motion: 2,
        };
        return (order[a.type] ?? 3).compareTo(order[b.type] ?? 3);
      });

    return _buildSettingsGroup('Devices', [
      for (final device in sorted) _DeviceRow(device: device, roomId: room.id),
    ]);
  }

  String _sourceLabel(RoomSourceDto source) {
    switch (source) {
      case RoomSourceDto.matter:
        return 'Matter';
      case RoomSourceDto.hue:
        return 'Philips Hue';
      case RoomSourceDto.homeAssistant:
        return 'Home Assistant';
      case RoomSourceDto.bridge:
        return 'Bridge';
      case RoomSourceDto.unknown:
        return 'Unknown';
    }
  }
}

/// Segmented tab selector for Settings / Devices.
class _TabSelector extends StatelessWidget {
  final _SheetTab selected;
  final ValueChanged<_SheetTab> onChanged;

  const _TabSelector({required this.selected, required this.onChanged});

  @override
  Widget build(BuildContext context) {
    return Container(
      height: 36,
      decoration: BoxDecoration(
        color: CelestialColors.backgroundDark.withValues(alpha: 0.5),
        borderRadius: BorderRadius.circular(18),
        border: Border.all(
          color: CelestialColors.orbitRing.withValues(alpha: 0.3),
          width: 1,
        ),
      ),
      child: Row(
        children: [
          _tabItem('Settings', _SheetTab.rhythm),
          _tabItem('Devices', _SheetTab.devices),
          _tabItem('Info', _SheetTab.settings),
        ],
      ),
    );
  }

  Widget _tabItem(String label, _SheetTab tab) {
    final isSelected = selected == tab;
    return Expanded(
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: () {
          HapticFeedback.selectionClick();
          onChanged(tab);
        },
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 200),
          margin: const EdgeInsets.all(3),
          decoration: BoxDecoration(
            color: isSelected
                ? CelestialColors.sunWarm.withValues(alpha: 0.2)
                : Colors.transparent,
            borderRadius: BorderRadius.circular(15),
            border: isSelected
                ? Border.all(
                    color: CelestialColors.sunWarm.withValues(alpha: 0.4),
                    width: 1,
                  )
                : null,
          ),
          alignment: Alignment.center,
          child: Text(
            label,
            style: TextStyle(
              color: isSelected
                  ? CelestialColors.sunWarm
                  : CelestialColors.textSecondary.withValues(alpha: 0.7),
              fontSize: 13,
              fontWeight: isSelected ? FontWeight.w600 : FontWeight.w500,
              letterSpacing: 0.5,
            ),
          ),
        ),
      ),
    );
  }
}

/// A single row in the settings sheet.
class _SettingsRow extends StatelessWidget {
  final IconData icon;
  final String label;
  final Widget trailing;

  /// Optional widget rendered immediately to the right of the label (e.g. an
  /// info affordance), kept on the left side of the row.
  final Widget? labelInfo;
  final VoidCallback? onTap;

  const _SettingsRow({
    required this.icon,
    required this.label,
    required this.trailing,
    this.labelInfo,
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
              child: Row(
                children: [
                  Flexible(
                    child: Text(
                      label,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 15,
                      ),
                    ),
                  ),
                  if (labelInfo != null) ...[
                    const SizedBox(width: 4),
                    labelInfo!,
                  ],
                ],
              ),
            ),
            trailing,
          ],
        ),
      ),
    );
  }
}

/// A two-segment switch for a room's off behavior — both options stay visible
/// ("Off" | "Standby") with the active one tinted. Tapping a segment selects
/// that state.
class _OffBehaviorSwitch extends StatelessWidget {
  final bool standby;
  final ValueChanged<bool> onChanged;

  const _OffBehaviorSwitch({required this.standby, required this.onChanged});

  static const Color _standbyColor = Color(0xFF7C83FF);

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.all(3),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundDark.withValues(alpha: 0.55),
        borderRadius: BorderRadius.circular(999),
        border: Border.all(
          color: CelestialColors.orbitRing.withValues(alpha: 0.4),
        ),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          _segment(
            label: 'Off',
            selected: !standby,
            color: CelestialColors.textSecondary,
            onTap: () => onChanged(false),
          ),
          _segment(
            label: 'Standby',
            selected: standby,
            color: _standbyColor,
            onTap: () => onChanged(true),
          ),
        ],
      ),
    );
  }

  Widget _segment({
    required String label,
    required bool selected,
    required Color color,
    required VoidCallback onTap,
  }) {
    return GestureDetector(
      onTap: onTap,
      behavior: HitTestBehavior.opaque,
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 180),
        curve: Curves.easeOut,
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 5),
        decoration: BoxDecoration(
          color: selected ? color.withValues(alpha: 0.18) : Colors.transparent,
          borderRadius: BorderRadius.circular(999),
        ),
        child: Text(
          label,
          style: TextStyle(
            color: selected
                ? color
                : CelestialColors.textSecondary.withValues(alpha: 0.5),
            fontSize: 13,
            fontWeight: FontWeight.w600,
            letterSpacing: 0.2,
          ),
        ),
      ),
    );
  }
}

/// A device row in the room settings Devices section.
class _DeviceRow extends StatelessWidget {
  final RhythmDevice device;
  final String roomId;

  const _DeviceRow({required this.device, required this.roomId});

  @override
  Widget build(BuildContext context) {
    final (icon, iconColor) = _iconForType(device.type);
    final typeLabel = switch (device.type) {
      RhythmDeviceType.light => 'Light',
      RhythmDeviceType.button => 'Button',
      RhythmDeviceType.motion => 'Motion',
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
                          color: CelestialColors.textSecondary
                              .withValues(alpha: 0.6),
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

/// Animated room orb with countdown ring when rhythm is active.
///
/// Tap to toggle rhythm on/off. Shows:
/// - Active: depleting countdown ring + breathing glow + "Rhythm" label
/// - Paused: static orb + "Paused ▶" hint
/// - Off: grey circle + "Off"
class _AnimatedRoomOrb extends StatefulWidget {
  final String roomId;
  final String roomName;
  final int? brightness;
  final int? kelvin;
  final Color? directColor;
  final bool lightsOn;
  final bool rhythmEnabled;
  final int rhythmIntervalSecs;
  final DateTime? lastTickTime;

  const _AnimatedRoomOrb({
    required this.roomId,
    required this.roomName,
    required this.brightness,
    required this.kelvin,
    this.directColor,
    required this.lightsOn,
    required this.rhythmEnabled,
    required this.rhythmIntervalSecs,
    this.lastTickTime,
  });

  @override
  State<_AnimatedRoomOrb> createState() => _AnimatedRoomOrbState();
}

class _AnimatedRoomOrbState extends State<_AnimatedRoomOrb>
    with TickerProviderStateMixin {
  late Ticker _ticker;
  late AnimationController _pulseController;
  late AnimationController _activeController;
  bool _pressed = false;
  double _countdownProgress = 1.0;

  bool get _active => widget.rhythmEnabled && widget.lightsOn;

  @override
  void initState() {
    super.initState();
    _ticker = createTicker(_onTick);
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
      _ticker.start();
      _pulseController.repeat(reverse: true);
    }
  }

  void _onTick(Duration elapsed) {
    final lastTick = widget.lastTickTime;
    if (lastTick == null || widget.rhythmIntervalSecs <= 0) {
      if (_countdownProgress != 1.0) {
        setState(() => _countdownProgress = 1.0);
      }
      return;
    }
    final elapsedSecs =
        DateTime.now().difference(lastTick).inMilliseconds / 1000.0;
    final progress =
        (1.0 - elapsedSecs / widget.rhythmIntervalSecs).clamp(0.0, 1.0);
    if ((_countdownProgress - progress).abs() > 0.001) {
      setState(() => _countdownProgress = progress);
    }
  }

  void _onActiveStatus(AnimationStatus status) {
    if (status == AnimationStatus.dismissed) {
      _ticker.stop();
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
      if (!_ticker.isActive) _ticker.start();
      if (!_pulseController.isAnimating) {
        _pulseController.repeat(reverse: true);
      }
      _activeController.animateTo(1.0,
          duration: const Duration(milliseconds: 500), curve: Curves.easeOut);
    } else {
      _activeController.animateTo(0.0,
          duration: const Duration(milliseconds: 600), curve: Curves.easeIn);
    }
  }

  void _onTap() {
    HapticFeedback.lightImpact();
    final newEnabled = !widget.rhythmEnabled;
    final roomProvider = context.read<RoomProvider>();
    roomProvider.setRoomRhythmEnabled(widget.roomId, newEnabled);
    final serverSync = context.read<ServerSyncProvider>();
    serverSync.pushNodePreferences(widget.roomId, rhythmEnabled: newEnabled);
  }

  @override
  void dispose() {
    _activeController.removeStatusListener(_onActiveStatus);
    _ticker.dispose();
    _pulseController.dispose();
    _activeController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final effectiveBrightness = widget.brightness ?? 0;
    final effectiveKelvin = widget.kelvin ?? 3000;
    final cctColor = !widget.lightsOn
        ? CelestialColors.textSecondary
        : widget.directColor ?? ColorUtils.cctToColor(effectiveKelvin);
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
                  _pulseController,
                  _activeController,
                ]),
                builder: (context, child) {
                  final activeT = _activeController.value;
                  final pulse = _pulseController.value;
                  final extraGlow = activeT * pulse * 0.12;
                  final extraSpread = activeT * pulse * 3.0;

                  return CustomPaint(
                    foregroundPainter: _CountdownRingPainter(
                      progress: _countdownProgress,
                      color: cctColor,
                      opacity: activeT,
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
                                alpha:
                                    (widget.lightsOn ? 0.2 : 0.05) + extraGlow,
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
                  child: Icon(
                    _active ? Icons.pause_rounded : Icons.play_arrow_rounded,
                    color: cctColor,
                    size: 32,
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
            directColor: widget.directColor,
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
                    color: CelestialColors.textSecondary.withValues(alpha: 0.8),
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
                  color: CelestialColors.textSecondary.withValues(alpha: 0.5),
                  size: 14,
                ),
                const SizedBox(width: 2),
                Text(
                  'Paused',
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.6),
                    fontSize: 12,
                    fontWeight: FontWeight.w500,
                  ),
                ),
              ],
            ),
    );
  }
}

/// Countdown ring that depletes over the rhythm interval.
class _CountdownRingPainter extends CustomPainter {
  final double progress; // 1.0 = full, 0.0 = empty
  final Color color;
  final double opacity;

  _CountdownRingPainter({
    required this.progress,
    required this.color,
    required this.opacity,
  });

  @override
  void paint(Canvas canvas, Size size) {
    if (opacity <= 0.01) return;

    final center = Offset(size.width / 2, size.height / 2);
    final radius = size.width / 2 - 3;
    const strokeWidth = 3.0;

    // Background track
    canvas.drawCircle(
      center,
      radius,
      Paint()
        ..color = color.withValues(alpha: 0.15 * opacity)
        ..style = PaintingStyle.stroke
        ..strokeWidth = strokeWidth,
    );

    // Depleting arc — starts at 12-o'clock, sweeps clockwise
    if (progress > 0.005) {
      canvas.drawArc(
        Rect.fromCircle(center: center, radius: radius),
        -math.pi / 2,
        2 * math.pi * progress,
        false,
        Paint()
          ..color = color.withValues(alpha: 0.7 * opacity)
          ..style = PaintingStyle.stroke
          ..strokeWidth = strokeWidth
          ..strokeCap = StrokeCap.round,
      );
    }
  }

  @override
  bool shouldRepaint(_CountdownRingPainter old) =>
      progress != old.progress || opacity != old.opacity || color != old.color;
}
