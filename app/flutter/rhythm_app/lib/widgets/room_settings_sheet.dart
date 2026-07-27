import 'dart:async';
import 'package:flutter/material.dart';
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
        RoomModeState,
        RhythmNodeProfileSettings,
        RhythmTimerSetting;
import '../screens/settings/light_screen.dart';
import 'device_detail_sheet.dart';
import 'info_tooltip.dart';
import 'low_glow_switch.dart';
import 'segmented_tab_bar.dart';
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

  void _openRoomLightSettings() {
    HapticFeedback.lightImpact();
    LightScreen.showForRoom(
      context,
      roomId: room.id,
      roomName: room.name,
    );
  }

  void _showRoomLightSettingsUnavailable() {
    HapticFeedback.lightImpact();
    final version = context.read<ServerSyncProvider>().firmwareVersion;
    final versionSuffix = version == '0.0.0' ? '' : ' ($version)';
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(
          'Update the Rhythm appliance$versionSuffix to customize light settings for ${room.name}.',
        ),
      ),
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
            if (widget.enableLivePreview) ...[
              // Room orb preview with live CCT + light output
              Selector<RoomProvider, (int?, int?, (int, int, int)?, bool)>(
                selector: (_, rp) {
                  final r = rp.getRoom(room.id);
                  return (
                    rp.getBrightness(room.id),
                    rp.getKelvin(room.id),
                    rp.getRoomColor(room.id),
                    r?.lightsOn ?? false,
                  );
                },
                builder: (context, data, _) {
                  final (brightness, kelvin, color, lightsOn) = data;
                  return _AnimatedRoomOrb(
                    roomId: room.id,
                    roomName: room.name,
                    brightness: brightness,
                    kelvin: kelvin,
                    directColor: color != null
                        ? Color.fromARGB(255, color.$1, color.$2, color.$3)
                        : null,
                    lightsOn: lightsOn,
                  );
                },
              ),
              const SizedBox(height: 12),
            ],
            // Tab selector
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 20),
              child: SegmentedTabBar<_SheetTab>(
                selected: _selectedTab,
                onChanged: (tab) => setState(() => _selectedTab = tab),
                tabs: const [
                  SegmentedTab('Settings', _SheetTab.rhythm),
                  SegmentedTab('Devices', _SheetTab.devices),
                  SegmentedTab('Info', _SheetTab.settings),
                ],
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
    final hasMotionBehavior = context.select<RoomProvider, bool>(
      (provider) => provider.hasMotionSensor(room.id),
    );
    final node = syncProvider.nodeById(room.id);
    final settings = node?.profileSettings;
    final standbyEnabled = syncProvider.standbyEnabledForNode(room.id);
    final lightSettingsSupported =
        syncProvider.lightProfileOverridesSupportedForNode(room.id);
    final hasLightOverrides =
        syncProvider.hasNodeLightProfileOverrides(room.id);

    return ListView(
      key: const ValueKey('rhythm'),
      padding: const EdgeInsets.symmetric(horizontal: 20),
      children: [
        // Low glow is the user-facing name for the room's existing Standby
        // preference, independent of the Day/Sleep profile selection.
        _buildSettingsGroup('', [
          if (room.kind.isRoom)
            _buildRoomLightSettingsRow(
              supported: lightSettingsSupported,
              customized: hasLightOverrides,
            ),
          _SettingsRow(
            icon: Icons.bedtime_outlined,
            label: 'Low glow',
            labelInfo: const InfoTooltip(
              eyebrow: 'LOW GLOW',
              accentColor: Color(0xFF7C83FF),
              iconSize: 15,
              message: 'Keep this room softly lit when motion times out or '
                  'you turn it off. Motion or On restores normal lighting.',
            ),
            trailing: LowGlowSwitch(
              value: standbyEnabled,
              onChanged: (val) => _setStandbyEnabled(context, val),
            ),
            onTap: () => _setStandbyEnabled(context, !standbyEnabled),
          ),
        ]),
        if (hasMotionBehavior) ...[
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
      ],
    );
  }

  Widget _buildRoomLightSettingsRow({
    required bool supported,
    required bool customized,
  }) {
    final status = customized
        ? 'Custom'
        : supported
            ? 'Home'
            : 'Update required';
    final semanticsValue = customized
        ? 'Custom room settings'
        : supported
            ? 'Using home settings'
            : 'Appliance update required';
    final accent =
        customized ? const Color(0xFFF9A825) : CelestialColors.textSecondary;
    final onPressed =
        supported ? _openRoomLightSettings : _showRoomLightSettingsUnavailable;

    return Semantics(
      key: ValueKey('room-settings-light-settings-${room.id}'),
      button: true,
      enabled: supported,
      excludeSemantics: true,
      label: 'Light settings',
      value: semanticsValue,
      onTap: onPressed,
      child: _SettingsRow(
        icon: supported ? Icons.tune_rounded : Icons.system_update_rounded,
        label: 'Light settings',
        trailing: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              status,
              key: ValueKey('room-settings-light-status-${room.id}'),
              style: TextStyle(
                color: accent.withValues(
                  alpha: supported || customized ? 0.92 : 0.58,
                ),
                fontSize: 13,
                fontWeight: customized ? FontWeight.w700 : FontWeight.w500,
              ),
            ),
            const SizedBox(width: 6),
            Icon(
              supported
                  ? Icons.chevron_right_rounded
                  : Icons.info_outline_rounded,
              size: 18,
              color: accent.withValues(alpha: supported ? 0.72 : 0.46),
            ),
          ],
        ),
        onTap: onPressed,
      ),
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
    final canRename = room.kind.isRoom || room.kind == RoomNodeKind.lightDevice;
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
                if (canRename) ...[
                  const SizedBox(width: 4),
                  Icon(
                    Icons.edit_outlined,
                    color: CelestialColors.textSecondary.withValues(alpha: 0.5),
                    size: 14,
                  ),
                ],
              ],
            ),
            onTap: canRename ? () => _showRenameDialog(context) : null,
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
    final isRoom = room.kind.isRoom;
    final title = isRoom ? 'Rename Room' : 'Rename Bulb';
    final hintText = isRoom ? 'Room name' : 'Bulb name';
    final newName = await showDialog<String>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: Text(
          title,
          style: const TextStyle(color: CelestialColors.textPrimary),
        ),
        content: TextField(
          controller: controller,
          autofocus: true,
          style: const TextStyle(color: CelestialColors.textPrimary),
          decoration: InputDecoration(
            hintText: hintText,
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
      final success = isRoom
          ? await http.topologyRenameRoom(room.id, newName)
          : await http.renameCanonicalDevice(room.id, newName);
      if (!context.mounted) return;
      if (!success) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text('Failed to rename ${isRoom ? 'room' : 'bulb'}'),
          ),
        );
        return;
      }
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
          RhythmDeviceType.contact: 3,
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
      RhythmDeviceType.contact => 'Contact',
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
        RhythmDeviceType.contact => (
            Icons.sensor_door_outlined,
            const Color(0xFFFFB74D)
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

/// Room power control with live CCT and output visuals.
class _AnimatedRoomOrb extends StatefulWidget {
  final String roomId;
  final String roomName;
  final int? brightness;
  final int? kelvin;
  final Color? directColor;
  final bool lightsOn;

  const _AnimatedRoomOrb({
    required this.roomId,
    required this.roomName,
    required this.brightness,
    required this.kelvin,
    this.directColor,
    required this.lightsOn,
  });

  @override
  State<_AnimatedRoomOrb> createState() => _AnimatedRoomOrbState();
}

class _AnimatedRoomOrbState extends State<_AnimatedRoomOrb> {
  bool _pressed = false;

  void _onTap() {
    final roomProvider = context.read<RoomProvider>();
    final serverSync = context.read<ServerSyncProvider>();

    if (widget.lightsOn) {
      HapticFeedback.heavyImpact();
      roomProvider.setRoomLightsOnLocal(widget.roomId, false);
      roomProvider.setRoomStateLocal(widget.roomId, RoomModeState.hardOff);
      unawaited(
        serverSync.pushNodePreferences(
          widget.roomId,
          state: RoomModeState.hardOff,
        ),
      );
      return;
    }

    HapticFeedback.mediumImpact();
    roomProvider.setRoomLightsOnLocal(widget.roomId, true);
    roomProvider.setRoomRhythmEnabled(widget.roomId, true);
    roomProvider.setRoomStateLocal(widget.roomId, RoomModeState.active);
    serverSync.dispatchResetNode(widget.roomId);
  }

  @override
  Widget build(BuildContext context) {
    final effectiveBrightness = widget.brightness ?? 0;
    final effectiveKelvin = widget.kelvin ?? 3000;
    final liveColor =
        widget.directColor ?? ColorUtils.cctToColor(effectiveKelvin);
    final orbColor =
        widget.lightsOn ? liveColor : CelestialColors.textSecondary;
    final glowAlpha =
        widget.lightsOn ? 0.15 + (effectiveBrightness / 100.0) * 0.25 : 0.05;
    final borderAlpha =
        widget.lightsOn ? 0.4 + (effectiveBrightness / 100.0) * 0.4 : 0.2;

    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Semantics(
          button: true,
          toggled: widget.lightsOn,
          label: '${widget.roomName} power',
          hint: widget.lightsOn ? 'Turn lights off' : 'Turn lights on',
          child: GestureDetector(
            key: ValueKey('room-detail-power-${widget.roomId}'),
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
                child: Center(
                  child: AnimatedContainer(
                    duration: const Duration(milliseconds: 300),
                    curve: Curves.easeOut,
                    width: 80,
                    height: 80,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      color: orbColor.withValues(alpha: glowAlpha),
                      border: Border.all(
                        color: orbColor.withValues(alpha: borderAlpha),
                        width: 2,
                      ),
                      boxShadow: [
                        BoxShadow(
                          color: orbColor.withValues(
                            alpha: widget.lightsOn ? 0.24 : 0.04,
                          ),
                          blurRadius: widget.lightsOn ? 20 : 8,
                          spreadRadius: widget.lightsOn ? 4 : 0,
                        ),
                      ],
                    ),
                    child: Icon(
                      Icons.power_settings_new_rounded,
                      color: widget.lightsOn
                          ? liveColor
                          : CelestialColors.textSecondary
                              .withValues(alpha: 0.55),
                      size: 32,
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
            directColor: widget.directColor,
          ),
          const SizedBox(height: 6),
          Text(
            'On',
            key: ValueKey('room-detail-power-status-${widget.roomId}'),
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.8),
              fontSize: 12,
              fontWeight: FontWeight.w500,
            ),
          ),
        ] else
          Padding(
            padding: const EdgeInsets.only(top: 8),
            child: Text(
              'Off',
              key: ValueKey('room-detail-power-status-${widget.roomId}'),
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
