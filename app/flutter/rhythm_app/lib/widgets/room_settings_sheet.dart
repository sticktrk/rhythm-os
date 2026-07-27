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

enum _SheetTab { light, motion, buttons }

class _RoomSettingsSheetState extends State<RoomSettingsSheet> {
  _SheetTab _selectedTab = _SheetTab.light;
  late String _roomName;
  final Map<String, RhythmTimerSetting> _motionTimeoutDrafts = {};

  RoomDto get room => widget.room;

  @override
  void initState() {
    super.initState();
    _roomName = room.name;
    _refreshLivePreviewState();
  }

  @override
  void didUpdateWidget(RoomSettingsSheet oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.room.id != widget.room.id) {
      _motionTimeoutDrafts.clear();
      _roomName = room.name;
    } else if (oldWidget.room.name != widget.room.name) {
      _roomName = room.name;
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
      roomName: _roomName,
    );
  }

  void _showRoomLightSettingsUnavailable() {
    HapticFeedback.lightImpact();
    final version = context.read<ServerSyncProvider>().firmwareVersion;
    final versionSuffix = version == '0.0.0' ? '' : ' ($version)';
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(
          'Update the Rhythm appliance$versionSuffix to customize light settings for $_roomName.',
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
        child: Stack(
          children: [
            Column(
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
                _buildRoomHeader(context),
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
                        roomName: _roomName,
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
                    key: const ValueKey('room-settings-tabs'),
                    selected: _selectedTab,
                    onChanged: (tab) => setState(() => _selectedTab = tab),
                    tabs: const [
                      SegmentedTab('Light', _SheetTab.light),
                      SegmentedTab('Motion', _SheetTab.motion),
                      SegmentedTab('Buttons', _SheetTab.buttons),
                    ],
                  ),
                ),
                const SizedBox(height: 16),
                // Tab content
                Expanded(
                  child: AnimatedSwitcher(
                    duration: const Duration(milliseconds: 200),
                    child: switch (_selectedTab) {
                      _SheetTab.light => _buildLightContent(context),
                      _SheetTab.motion => _buildMotionContent(context),
                      _SheetTab.buttons => _buildButtonsContent(context),
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
            Positioned(
              top: 8,
              right: 16,
              child: _buildSourceBadge(),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildRoomHeader(BuildContext context) {
    final canRename = room.kind.isRoom || room.kind == RoomNodeKind.lightDevice;
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 4, 20, 12),
      child: Row(
        mainAxisAlignment: MainAxisAlignment.center,
        mainAxisSize: MainAxisSize.min,
        children: [
          Flexible(
            child: Text(
              _roomName,
              key: const ValueKey('room-settings-room-name'),
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 19,
                fontWeight: FontWeight.w600,
                letterSpacing: 0.3,
              ),
            ),
          ),
          if (canRename) ...[
            const SizedBox(width: 5),
            SizedBox(
              width: 32,
              height: 32,
              child: IconButton(
                key: const ValueKey('room-settings-rename'),
                tooltip: room.kind.isRoom ? 'Rename room' : 'Rename light',
                padding: EdgeInsets.zero,
                onPressed: () => _showRenameDialog(context),
                icon: Icon(
                  Icons.edit_outlined,
                  size: 17,
                  color: CelestialColors.sunWarm.withValues(alpha: 0.84),
                ),
              ),
            ),
          ],
        ],
      ),
    );
  }

  Widget _buildSourceBadge() {
    final source = _sourceLabel(room.source);
    return Semantics(
      label: 'Source: $source',
      child: Container(
        key: const ValueKey('room-settings-source'),
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
        decoration: BoxDecoration(
          color: CelestialColors.backgroundDark.withValues(alpha: 0.46),
          borderRadius: BorderRadius.circular(99),
          border: Border.all(
            color: CelestialColors.orbitRing.withValues(alpha: 0.32),
          ),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              Icons.hub_outlined,
              size: 11,
              color: CelestialColors.textSecondary.withValues(alpha: 0.72),
            ),
            const SizedBox(width: 4),
            Text(
              source,
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                fontSize: 11,
                fontWeight: FontWeight.w500,
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

  Widget _buildLightContent(BuildContext context) {
    final syncProvider = context.watch<ServerSyncProvider>();
    final standbyEnabled = syncProvider.standbyEnabledForNode(room.id);
    final lightSettingsSupported =
        syncProvider.lightProfileOverridesSupportedForNode(room.id);
    final hasLightOverrides =
        syncProvider.hasNodeLightProfileOverrides(room.id);
    final lights = syncProvider
        .devicesForRoom(room.id)
        .where((device) => device.type == RhythmDeviceType.light)
        .toList(growable: false);

    return ListView(
      key: const ValueKey('light'),
      padding: const EdgeInsets.symmetric(horizontal: 20),
      children: [
        // Low glow is the user-facing name for the room's existing Standby
        // preference, independent of the Day/Sleep profile selection.
        _buildSettingsGroup('', [
          if (room.kind.isRoom)
            LightingOverrideRow(
              nodeId: room.id,
              supported: lightSettingsSupported,
              customized: hasLightOverrides,
              settingsKeyPrefix: 'room-settings-light',
              onPressed: lightSettingsSupported
                  ? _openRoomLightSettings
                  : _showRoomLightSettingsUnavailable,
            ),
          LowGlowSettingRow(
            value: standbyEnabled,
            onChanged: (val) => _setStandbyEnabled(context, val),
          ),
        ]),
        if (lights.isNotEmpty) ...[
          const SizedBox(height: 16),
          _buildDeviceGroup('Lights', lights),
        ],
      ],
    );
  }

  Widget _buildMotionContent(BuildContext context) {
    final syncProvider = context.watch<ServerSyncProvider>();
    final hasMotionBehavior = context.select<RoomProvider, bool>(
      (provider) => provider.hasMotionSensor(room.id),
    );
    final settings = syncProvider.nodeById(room.id)?.profileSettings;
    final devices = syncProvider.devicesForRoom(room.id);
    final motionSensors = devices
        .where((device) => device.type == RhythmDeviceType.motion)
        .toList(growable: false);
    final contactSensors = devices
        .where((device) => device.type == RhythmDeviceType.contact)
        .toList(growable: false);

    return ListView(
      key: const ValueKey('motion'),
      padding: const EdgeInsets.symmetric(horizontal: 20),
      children: [
        if (motionSensors.isNotEmpty)
          _buildDeviceGroup('Motion Sensors', motionSensors),
        if (hasMotionBehavior) ...[
          if (motionSensors.isNotEmpty) const SizedBox(height: 16),
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
        if (contactSensors.isNotEmpty) ...[
          if (motionSensors.isNotEmpty || hasMotionBehavior)
            const SizedBox(height: 16),
          _buildDeviceGroup('Contact Sensors', contactSensors),
        ],
      ],
    );
  }

  Widget _buildButtonsContent(BuildContext context) {
    final buttons = context
        .watch<ServerSyncProvider>()
        .devicesForRoom(room.id)
        .where((device) => device.type == RhythmDeviceType.button)
        .toList(growable: false);

    return ListView(
      key: const ValueKey('buttons'),
      padding: const EdgeInsets.symmetric(horizontal: 20),
      children: [
        if (buttons.isNotEmpty) _buildDeviceGroup('Buttons', buttons),
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

  void _setStandbyEnabled(BuildContext context, bool enabled) {
    final syncProvider = context.read<ServerSyncProvider>();
    syncProvider.setNodeStandbyEnabledLocal(room.id, enabled);
    syncProvider.pushNodePreferences(room.id, standbyEnabled: enabled);
    HapticFeedback.selectionClick();
  }

  Future<void> _showRenameDialog(BuildContext context) async {
    final controller = TextEditingController(text: _roomName);
    final isRoom = room.kind.isRoom;
    final title = isRoom ? 'Rename Room' : 'Rename Bulb';
    final hintText = isRoom ? 'Room name' : 'Bulb name';
    final canDeleteRoom = _canDeleteRoom(context.read<ServerSyncProvider>());
    final result = await showDialog<({String? name, bool delete})>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: Text(
          title,
          style: const TextStyle(color: CelestialColors.textPrimary),
        ),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            TextField(
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
              onSubmitted: (value) => Navigator.of(ctx).pop(
                (name: value.trim(), delete: false),
              ),
            ),
            if (canDeleteRoom) ...[
              const SizedBox(height: 20),
              TextButton.icon(
                key: const ValueKey('room-settings-delete-room'),
                onPressed: () => Navigator.of(ctx).pop(
                  (name: null, delete: true),
                ),
                style: TextButton.styleFrom(
                  foregroundColor: Colors.red.shade300,
                  padding: const EdgeInsets.symmetric(horizontal: 8),
                ),
                icon: const Icon(Icons.delete_outline_rounded, size: 18),
                label: const Text('Delete Room'),
              ),
            ],
          ],
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
            onPressed: () => Navigator.of(ctx).pop(
              (name: controller.text.trim(), delete: false),
            ),
            child: const Text(
              'Rename',
              style: TextStyle(color: CelestialColors.sunWarm),
            ),
          ),
        ],
      ),
    );

    if (result?.delete == true && context.mounted) {
      await _confirmDeleteRoom(context);
      return;
    }

    final newName = result?.name;
    if (newName != null &&
        newName.isNotEmpty &&
        newName != _roomName &&
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
      setState(() => _roomName = newName);
      unawaited(http.triggerSync());
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
          'Delete "$_roomName" from the topology? Any remaining devices in this room will become unassigned.',
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

    final success = await syncProvider.api.topologyDeleteRoom(room.id);

    if (!mounted) return;

    if (!success) {
      messenger.showSnackBar(
        const SnackBar(content: Text('Room deletion failed')),
      );
      return;
    }

    await syncProvider.fullRefresh();
    if (!mounted) return;

    navigator.pop();
    messenger.showSnackBar(
      SnackBar(content: Text('Deleted $_roomName')),
    );
  }

  Widget _buildDeviceGroup(String title, List<RhythmDevice> devices) {
    return _buildSettingsGroup(title, [
      for (final device in devices) _DeviceRow(device: device, roomId: room.id),
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

/// A device row in the matching room settings device section.
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
