import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart' show SolarUtils;
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:uuid/uuid.dart';

import '../providers/home_provider.dart';
import '../providers/server_sync_provider.dart';
import '../screens/settings/light_schedules_screen.dart';
import '../screens/settings/light_screen.dart';
import '../services/analytics_service.dart';
import 'automation_section_header.dart';
import 'mode_summary_chip.dart';
import 'low_glow_switch.dart';
import 'light_schedule_times.dart';
import 'rhythm_clock/rhythm_clock_visuals.dart';
import 'rhythm_clock/rhythm_schedule_clock.dart';
import 'room_schedule_behavior_control.dart';
import 'solar_clock/solar_clock_exports.dart';
import 'solar_orbit.dart';

// Step accent mirroring the Presets screen's numbered sequence.
const Color _scheduleAccent = CelestialColors.accentBlue;

/// What decides when a room wakes and sleeps. The appliance stores these as
/// two separate authorities (a legacy room schedule and a named-schedule
/// assignment) that can never be combined, so the tab presents them as one
/// exclusive choice.
enum _ChoiceKind { home, named, custom, manual }

typedef _ScheduleChoice = ({_ChoiceKind kind, String? scheduleId});

const _ScheduleChoice _homeChoice = (kind: _ChoiceKind.home, scheduleId: null);
const _ScheduleChoice _customChoice =
    (kind: _ChoiceKind.custom, scheduleId: null);
const _ScheduleChoice _manualChoice =
    (kind: _ChoiceKind.manual, scheduleId: null);

/// Room-scoped lighting settings and capability-gated schedule editor.
///
/// The room-level Lighting override and Low glow preference lead into two
/// numbered steps sharing the whole-house Presets screen's visual grammar:
/// one schedule chooser for WHEN the room wakes and sleeps, then the
/// Wake / Sleep presets for WHAT it does.
class RoomScheduleTab extends StatefulWidget {
  const RoomScheduleTab({
    super.key,
    required this.roomId,
    required this.roomName,
    required this.showRoomLightingOverride,
    this.solarClockDataOverride,
  });

  final String roomId;
  final String roomName;
  final bool showRoomLightingOverride;

  /// Deterministic solar fixture for widget evidence. Production callers use
  /// the current home's location-derived data.
  final SolarClockData? solarClockDataOverride;

  @override
  State<RoomScheduleTab> createState() => _RoomScheduleTabState();
}

class _RoomScheduleTabState extends State<RoomScheduleTab> {
  final Uuid _uuid = const Uuid();
  String? _failure;
  _ScheduleChoice? _busyChoice;
  String? _saveJourneyId;
  String? _failedSaveSignature;
  int _saveAttemptNumber = 0;

  @override
  void initState() {
    super.initState();
    unawaited(
        AnalyticsService().logRoomScheduleOpened(source: 'room_settings'));
    WidgetsBinding.instance.addPostFrameCallback((_) {
      unawaited(context.read<ServerSyncProvider>().loadLightSchedules());
    });
  }

  Future<bool> _save(
    RhythmRoomSchedule schedule, {
    required String changeKind,
    required String inputMethod,
  }) async {
    final signature = [
      schedule.source.wireValue,
      schedule.wakeTime,
      schedule.sleepTime,
    ].join('|');
    if (_failedSaveSignature == signature && _saveJourneyId != null) {
      _saveAttemptNumber += 1;
    } else {
      _saveJourneyId = 'room-schedule-save-${_uuid.v4()}';
      _saveAttemptNumber = 1;
    }
    final journeyId = _saveJourneyId!;
    final attemptNumber = _saveAttemptNumber;
    setState(() => _failure = null);
    unawaited(AnalyticsService().logRoomScheduleSaveAttempted(
      journeyId: journeyId,
      attemptNumber: attemptNumber,
      inputMethod: inputMethod,
      changeKind: changeKind,
      source: schedule.source.wireValue,
    ));
    final ok = await context.read<ServerSyncProvider>().setRoomSchedule(
          widget.roomId,
          schedule,
          requestId: journeyId,
        );
    unawaited(AnalyticsService().logRoomScheduleSaveCompleted(
      journeyId: journeyId,
      attemptNumber: attemptNumber,
      inputMethod: inputMethod,
      changeKind: changeKind,
      source: schedule.source.wireValue,
      outcome: ok ? 'succeeded' : 'failed',
      failureStage: ok ? null : 'appliance_ack',
    ));
    if (ok) {
      _saveJourneyId = null;
      _failedSaveSignature = null;
      _saveAttemptNumber = 0;
    } else {
      _failedSaveSignature = signature;
    }
    if (mounted && !ok) {
      setState(() => _failure = 'Could not save. Try again.');
    }
    return ok;
  }

  /// The named or unscheduled assignment stored on this node itself, as
  /// opposed to one it merely inherits from a parent.
  RhythmLightScheduleAssignment? _ownAssignment(ServerSyncProvider sync) {
    final node = sync.nodeById(widget.roomId);
    final assignment = node?.profileSettings?.lightSchedule;
    if (assignment == null) return null;
    final inherited = node?.localProfileSettings?.lightSchedule == null &&
        (node?.parentId?.isNotEmpty ?? false);
    return inherited ? null : assignment;
  }

  /// Names the two presets after what actually fires them under the room's
  /// current schedule.
  (String, String) _presetTitles(ServerSyncProvider sync) {
    final node = sync.nodeById(widget.roomId);
    if (_currentChoice(sync).kind == _ChoiceKind.custom) {
      final schedule = sync.scheduleForRoom(widget.roomId);
      return (schedule.wakeTime, schedule.sleepTime);
    }
    final scheduleId =
        sync.lightScheduleTargetSupportedForNode(widget.roomId)
            ? node?.profileSettings?.lightScheduleId
            : null;
    final schedule = sync.lightSchedules
        .where((value) => value.id == scheduleId)
        .firstOrNull;
    if (schedule == null || !schedule.enabled) return ('Wake', 'Sleep');
    return (
      lightScheduleTriggerForMode(node, schedule, RhythmMode.day) ?? 'Wake',
      lightScheduleTriggerForMode(node, schedule, RhythmMode.sleep) ?? 'Sleep',
    );
  }

  _ScheduleChoice _currentChoice(ServerSyncProvider sync) {
    final own = sync.lightScheduleTargetSupportedForNode(widget.roomId)
        ? _ownAssignment(sync)
        : null;
    if (own != null) {
      return own.isUnscheduled
          ? _manualChoice
          : (kind: _ChoiceKind.named, scheduleId: own.scheduleId);
    }
    final followsOwnTimes = sync.roomScheduleSupportedForNode(widget.roomId) &&
        sync.nodeById(widget.roomId)?.profileSettings?.roomSchedule?.source ==
            RhythmRoomScheduleSource.followTime;
    return followsOwnTimes ? _customChoice : _homeChoice;
  }

  /// An explicit tap always dispatches, even on the current choice — that is
  /// the natural retry after a failed write.
  Future<void> _choose(_ScheduleChoice choice) async {
    final sync = context.read<ServerSyncProvider>();
    final namedSupported =
        sync.lightScheduleTargetSupportedForNode(widget.roomId);
    HapticFeedback.selectionClick();
    setState(() {
      _busyChoice = choice;
      _failure = null;
    });
    var assigned = true;
    switch (choice.kind) {
      case _ChoiceKind.named:
        assigned =
            await assignLightSchedule(sync, widget.roomId, choice.scheduleId);
      case _ChoiceKind.manual:
        assigned = await assignLightSchedule(sync, widget.roomId, null);
      case _ChoiceKind.home:
        if (namedSupported) {
          // Clears both authorities, so the room truly follows its parent or
          // the whole-home schedule again.
          assigned = await assignLightSchedule(
            sync,
            widget.roomId,
            null,
            legacy: true,
          );
        } else {
          await _save(
            sync.scheduleForRoom(widget.roomId).copyWith(
                  source: RhythmRoomScheduleSource.wakeSleepPresets,
                ),
            changeKind: 'source',
            inputMethod: 'choice_chip',
          );
        }
      case _ChoiceKind.custom:
        // The appliance rejects custom times while this room still holds its
        // own named or manual assignment, so release that first.
        if (namedSupported && _ownAssignment(sync) != null) {
          assigned = await assignLightSchedule(
            sync,
            widget.roomId,
            null,
            legacy: true,
          );
        }
        if (assigned && mounted) {
          await _save(
            sync.scheduleForRoom(widget.roomId).copyWith(
                  source: RhythmRoomScheduleSource.followTime,
                ),
            changeKind: 'source',
            inputMethod: 'choice_chip',
          );
        }
    }
    if (!mounted) return;
    setState(() {
      _busyChoice = null;
      if (!assigned) _failure = 'Could not change the schedule. Try again.';
    });
  }

  /// Named schedules are shared by the whole home, so they are created and
  /// edited in one place rather than per room.
  void _openSchedules() {
    HapticFeedback.lightImpact();
    Navigator.of(context).push(
      MaterialPageRoute(builder: (_) => const LightSchedulesScreen()),
    );
  }

  void _openRoomLightSettings() {
    HapticFeedback.lightImpact();
    LightScreen.showForRoom(
      context,
      roomId: widget.roomId,
      roomName: widget.roomName,
    );
  }

  void _showRoomLightSettingsUnavailable() {
    HapticFeedback.lightImpact();
    final version = context.read<ServerSyncProvider>().firmwareVersion;
    final versionSuffix = version == '0.0.0' ? '' : ' ($version)';
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(
          'Update the Rhythm appliance$versionSuffix to customize light settings for ${widget.roomName}.',
        ),
      ),
    );
  }

  void _setStandbyEnabled(bool enabled) {
    final sync = context.read<ServerSyncProvider>();
    sync.setNodeStandbyEnabledLocal(widget.roomId, enabled);
    sync.pushNodePreferences(widget.roomId, standbyEnabled: enabled);
    HapticFeedback.selectionClick();
  }

  Widget _buildLightingSettings(ServerSyncProvider sync) {
    final lightSettingsSupported =
        sync.lightProfileOverridesSupportedForNode(widget.roomId);
    final children = <Widget>[
      if (widget.showRoomLightingOverride)
        LightingOverrideRow(
          nodeId: widget.roomId,
          supported: lightSettingsSupported,
          customized: sync.hasNodeLightProfileOverrides(widget.roomId),
          settingsKeyPrefix: 'room-settings-light',
          onPressed: lightSettingsSupported
              ? _openRoomLightSettings
              : _showRoomLightSettingsUnavailable,
        ),
      LowGlowSettingRow(
        key: ValueKey('room-settings-low-glow-${widget.roomId}'),
        value: sync.standbyEnabledForNode(widget.roomId),
        onChanged: _setStandbyEnabled,
      ),
    ];

    return _GroupCard(
      padding: EdgeInsets.zero,
      child: Column(
        children: [
          for (var index = 0; index < children.length; index++) ...[
            children[index],
            if (index < children.length - 1)
              Divider(
                height: 1,
                indent: 48,
                color: CelestialColors.orbitRing.withValues(alpha: 0.2),
              ),
          ],
        ],
      ),
    );
  }

  Widget _buildChooser(
    ServerSyncProvider sync, {
    required String target,
    required bool namedSupported,
    required bool customSupported,
  }) {
    final node = sync.nodeById(widget.roomId);
    final current = _currentChoice(sync);
    final saving = sync.roomSchedulePendingForRoom(widget.roomId);
    final busy = saving ||
        sync.lightScheduleWritePendingForNode(widget.roomId) ||
        _busyChoice != null;
    final schedule = sync.scheduleForRoom(widget.roomId);

    // A schedule handed down by a parent keeps Home schedule selected; name
    // it so the Wake and Sleep times below are not a surprise.
    final effectiveAssignment =
        namedSupported ? node?.profileSettings?.lightSchedule : null;
    final inheritedSchedule =
        _ownAssignment(sync) == null && effectiveAssignment != null
            ? sync.lightSchedules
                .where((value) => value.id == effectiveAssignment.scheduleId)
                .firstOrNull
            : null;
    final parentName = sync.nodeById(node?.parentId ?? '')?.name;
    final homeSubtitle = inheritedSchedule != null
        ? 'Following ${inheritedSchedule.name} from ${parentName ?? 'its parent'}'
        : effectiveAssignment?.isUnscheduled == true &&
                _ownAssignment(sync) == null
            ? 'Manual only, like ${parentName ?? 'its parent'}'
            : 'Follows the rest of the home';

    Widget option({
      required _ScheduleChoice choice,
      required Key key,
      required IconData icon,
      required String title,
      required String subtitle,
      Widget? detail,
    }) {
      final selected = choice == current;
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _ScheduleOptionRow(
            key: key,
            icon: icon,
            title: title,
            subtitle: subtitle,
            selected: selected,
            busy: _busyChoice == choice,
            enabled: !busy,
            onTap: () => _choose(choice),
          ),
          AnimatedSize(
            duration: const Duration(milliseconds: 220),
            curve: Curves.easeOutCubic,
            alignment: Alignment.topCenter,
            child: selected && detail != null
                ? detail
                : const SizedBox(width: double.infinity, height: 0),
          ),
        ],
      );
    }

    final scheduleTimes = LightScheduleTimes(nodeId: widget.roomId);

    return _GroupCard(
      key: ValueKey('room-schedule-chooser-${widget.roomId}'),
      padding: const EdgeInsets.all(8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          option(
            choice: _homeChoice,
            key: const ValueKey('room-schedule-source-presets'),
            icon: Icons.home_rounded,
            title: 'Home schedule',
            subtitle: homeSubtitle,
            detail: inheritedSchedule != null ? scheduleTimes : null,
          ),
          if (namedSupported)
            for (final named in sync.lightSchedules)
              option(
                choice: (kind: _ChoiceKind.named, scheduleId: named.id),
                key: ValueKey('light-schedule-assignment-${named.id}'),
                icon: Icons.event_repeat_rounded,
                title: named.name,
                subtitle: lightScheduleSummary(named),
                detail: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    scheduleTimes,
                    // Adjustments saved by older builds still apply until the
                    // room is handed back to the shared schedule.
                    if (node?.localProfileSettings
                            ?.lightScheduleOverrides[named.id]?.isEmpty ==
                        false)
                      Align(
                        alignment: Alignment.centerLeft,
                        child: TextButton.icon(
                          key: const ValueKey('light-schedule-overrides-reset'),
                          onPressed: busy
                              ? null
                              : () => _choose(
                                    (
                                      kind: _ChoiceKind.named,
                                      scheduleId: named.id,
                                    ),
                                  ),
                          icon: const Icon(Icons.restart_alt_rounded, size: 18),
                          label: Text('Use the ${named.name} times'),
                        ),
                      ),
                  ],
                ),
              ),
          if (customSupported)
            option(
              choice: _customChoice,
              key: const ValueKey('room-schedule-source-follow-time'),
              icon: Icons.schedule_rounded,
              title: 'Custom times',
              subtitle: 'Set times just for this $target',
              detail: Padding(
                padding: const EdgeInsets.fromLTRB(4, 10, 4, 4),
                child: _RoomTimeEditor(
                  wakeTime: schedule.wakeTime,
                  sleepTime: schedule.sleepTime,
                  enabled: !busy,
                  solarClockDataOverride: widget.solarClockDataOverride,
                  onChanged: (wake, sleep, inputMethod) => _save(
                    schedule.copyWith(wakeTime: wake, sleepTime: sleep),
                    changeKind: 'times',
                    inputMethod: inputMethod,
                  ),
                ),
              ),
            ),
          if (namedSupported)
            option(
              choice: _manualChoice,
              key: const ValueKey('light-schedule-assignment-none'),
              icon: Icons.touch_app_rounded,
              title: 'Manual only',
              subtitle: 'Never switches on its own',
            ),
          AnimatedSize(
            duration: const Duration(milliseconds: 180),
            alignment: Alignment.topCenter,
            child: saving && _busyChoice == null
                ? Padding(
                    key: const ValueKey('room-schedule-save-pending'),
                    padding: const EdgeInsets.only(top: 8, bottom: 4),
                    child: Row(
                      mainAxisAlignment: MainAxisAlignment.center,
                      children: [
                        const SizedBox.square(
                          dimension: 12,
                          child: CircularProgressIndicator(
                            strokeWidth: 1.6,
                            color: _scheduleAccent,
                          ),
                        ),
                        const SizedBox(width: 8),
                        Text(
                          'Saving…',
                          style: TextStyle(
                            color: CelestialColors.textSecondary
                                .withValues(alpha: 0.8),
                            fontSize: 12,
                            fontWeight: FontWeight.w500,
                          ),
                        ),
                      ],
                    ),
                  )
                : const SizedBox(width: double.infinity, height: 0),
          ),
          if (_failure != null)
            Padding(
              padding: const EdgeInsets.fromLTRB(4, 8, 4, 4),
              child: _FailureBanner(message: _failure!),
            ),
          if (namedSupported)
            Align(
              alignment: Alignment.centerLeft,
              child: TextButton.icon(
                key: const ValueKey('room-schedule-manage-schedules'),
                onPressed: _openSchedules,
                icon: const Icon(Icons.edit_calendar_rounded, size: 18),
                label: const Text('Create / edit schedules'),
              ),
            ),
        ],
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final sync = context.watch<ServerSyncProvider>();
    final node = sync.nodeById(widget.roomId);
    final isLightNode = node?.kind == RhythmNodeKind.lightDevice;
    final target = isLightNode ? 'light' : 'room';
    final customSupported = sync.roomScheduleSupportedForNode(widget.roomId);
    final namedSupported =
        sync.lightScheduleTargetSupportedForNode(widget.roomId);
    if (!customSupported && !namedSupported) {
      final parentId = node?.parentId;
      final inheritsFromRoom =
          isLightNode && parentId != null && parentId.isNotEmpty;
      final parentName =
          inheritsFromRoom ? sync.nodeById(parentId)?.name : null;
      return ListView(
        key: const ValueKey('lighting'),
        padding: const EdgeInsets.symmetric(horizontal: 20),
        children: [
          _buildLightingSettings(sync),
          const SizedBox(height: 16),
          _GroupCard(
            padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 24),
            child: Column(
              children: [
                Icon(
                  inheritsFromRoom
                      ? Icons.account_tree_outlined
                      : Icons.system_update_alt,
                  color: CelestialColors.sunWarm,
                  size: 32,
                ),
                const SizedBox(height: 10),
                Text(
                  inheritsFromRoom ? 'Inherited from room' : 'Update required',
                  key: ValueKey(inheritsFromRoom
                      ? 'light-schedule-inherited'
                      : 'room-schedule-update-required'),
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                const SizedBox(height: 6),
                Text(
                  inheritsFromRoom
                      ? 'This bulb uses the custom light settings from ${parentName ?? 'its assigned room'}.'
                      : 'Update this Rhythm appliance to set a schedule for this $target.',
                  textAlign: TextAlign.center,
                  style: const TextStyle(color: CelestialColors.textSecondary),
                ),
              ],
            ),
          ),
        ],
      );
    }

    final (wakeTitle, sleepTitle) = _presetTitles(sync);

    return ListView(
      key: const ValueKey('lighting'),
      padding: const EdgeInsets.symmetric(horizontal: 20),
      children: [
        _buildLightingSettings(sync),
        const SizedBox(height: 4),
        AutomationSectionHeader(
          step: 1,
          title: 'Schedule',
          subtitle: 'Choose when this $target wakes and sleeps.',
          accent: _scheduleAccent,
        ),
        _buildChooser(
          sync,
          target: target,
          namedSupported: namedSupported,
          customSupported: customSupported,
        ),
        if (customSupported) ...[
          AutomationSectionHeader(
            step: 2,
            title: '$wakeTitle / $sleepTitle Presets',
            subtitle: 'Set what this $target does at $wakeTitle and at '
                '$sleepTitle. Changes to the current one apply right away.',
            accent: CelestialColors.sunWarm,
          ),
          _GroupCard(
            padding: const EdgeInsets.all(12),
            child: RoomScheduleBehaviorSegments(
              roomId: widget.roomId,
              wakeTitle: wakeTitle,
              sleepTitle: sleepTitle,
            ),
          ),
        ],
        const SizedBox(height: 24),
      ],
    );
  }
}

/// One exclusive choice in the schedule chooser: tinted icon disc, name and
/// one-line meaning, with the current choice filled in the schedule accent.
class _ScheduleOptionRow extends StatelessWidget {
  const _ScheduleOptionRow({
    super.key,
    required this.icon,
    required this.title,
    required this.subtitle,
    required this.selected,
    required this.busy,
    required this.enabled,
    required this.onTap,
  });

  final IconData icon;
  final String title;
  final String subtitle;
  final bool selected;
  final bool busy;
  final bool enabled;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final active = selected || busy;
    final iconColor = active
        ? _scheduleAccent
        : CelestialColors.textSecondary.withValues(alpha: 0.8);
    return Semantics(
      button: true,
      selected: selected,
      enabled: enabled,
      label: '$title. $subtitle',
      excludeSemantics: true,
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: enabled ? onTap : null,
        child: AnimatedOpacity(
          duration: const Duration(milliseconds: 150),
          opacity: enabled || busy ? 1.0 : 0.55,
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 250),
            curve: Curves.easeOut,
            constraints: const BoxConstraints(minHeight: 56),
            margin: const EdgeInsets.symmetric(vertical: 2),
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 9),
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(11),
              color: active
                  ? _scheduleAccent.withValues(alpha: 0.12)
                  : Colors.transparent,
              border: Border.all(
                color: active
                    ? _scheduleAccent.withValues(alpha: 0.45)
                    : Colors.transparent,
              ),
            ),
            child: Row(
              children: [
                Container(
                  width: 32,
                  height: 32,
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: iconColor.withValues(alpha: 0.14),
                    border:
                        Border.all(color: iconColor.withValues(alpha: 0.32)),
                  ),
                  child: busy
                      ? const SizedBox.square(
                          dimension: 14,
                          child: CircularProgressIndicator(
                            strokeWidth: 2,
                            color: _scheduleAccent,
                          ),
                        )
                      : Icon(icon, size: 16, color: iconColor),
                ),
                const SizedBox(width: 12),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Text(
                        title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: CelestialColors.textPrimary
                              .withValues(alpha: selected ? 1.0 : 0.92),
                          fontSize: 14.5,
                          fontWeight: FontWeight.w600,
                          letterSpacing: 0.1,
                        ),
                      ),
                      const SizedBox(height: 2),
                      Text(
                        subtitle,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: CelestialColors.textSecondary
                              .withValues(alpha: selected ? 0.9 : 0.7),
                          fontSize: 12,
                          height: 1.25,
                        ),
                      ),
                    ],
                  ),
                ),
                const SizedBox(width: 8),
                Icon(
                  selected
                      ? Icons.check_circle_rounded
                      : Icons.radio_button_unchecked_rounded,
                  key: selected
                      ? const ValueKey('room-schedule-choice-selected')
                      : null,
                  size: 20,
                  color: selected
                      ? _scheduleAccent
                      : CelestialColors.textSecondary.withValues(alpha: 0.35),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// Inline error surface rendered inside the schedule chooser, so the message
/// appears next to the control the user just touched.
class _FailureBanner extends StatelessWidget {
  const _FailureBanner({required this.message});

  final String message;

  @override
  Widget build(BuildContext context) {
    return Container(
      key: const ValueKey('room-schedule-failure'),
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
      decoration: BoxDecoration(
        color: Colors.redAccent.withValues(alpha: 0.10),
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: Colors.redAccent.withValues(alpha: 0.35)),
      ),
      child: Row(
        children: [
          const Icon(Icons.error_outline_rounded,
              color: Colors.redAccent, size: 16),
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              message,
              style: const TextStyle(
                color: Colors.redAccent,
                fontSize: 13,
                height: 1.3,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// The sheet's shared card surface — same geometry as the Bulbs/Motion/Buttons
/// tabs' settings groups so all four tabs read as one surface.
class _GroupCard extends StatelessWidget {
  const _GroupCard({super.key, required this.child, required this.padding});

  final Widget child;
  final EdgeInsetsGeometry padding;

  @override
  Widget build(BuildContext context) => Container(
        padding: padding,
        decoration: BoxDecoration(
          color: CelestialColors.backgroundDark.withValues(alpha: 0.5),
          borderRadius: BorderRadius.circular(14),
          border: Border.all(
              color: CelestialColors.orbitRing.withValues(alpha: 0.3)),
        ),
        child: child,
      );
}

/// The Custom-times editor: Wake/Sleep chips with steppers above the SAME
/// interactive orbital clock the whole-house Alarm Schedule uses
/// ([RhythmScheduleClock]) — rhythm-curve ring, draggable orbs, solar-anchor
/// snapping. Committing here writes fixed room times rather than triggers.
class _RoomTimeEditor extends StatefulWidget {
  const _RoomTimeEditor({
    required this.wakeTime,
    required this.sleepTime,
    required this.enabled,
    required this.solarClockDataOverride,
    required this.onChanged,
  });
  final String wakeTime;
  final String sleepTime;
  final bool enabled;
  final SolarClockData? solarClockDataOverride;
  final void Function(String wakeTime, String sleepTime, String inputMethod)
      onChanged;

  @override
  State<_RoomTimeEditor> createState() => _RoomTimeEditorState();
}

class _RoomTimeEditorState extends State<_RoomTimeEditor> {
  late int wake = _parse(widget.wakeTime);
  late int sleep = _parse(widget.sleepTime);
  RhythmMode _clockMode = RhythmMode.day;
  RhythmClockDragPreview? _preview;

  // Memoized solar data + per-mode colors/curve visuals — the solar/curve
  // math is native work that must not rerun on every drag-frame rebuild.
  String? _visualsKey;
  SolarClockData? _solarClockData;
  // Mode colors come from the active lighting profiles (direct color or
  // midpoint CCT), exactly like the Alarm Schedule screen — a 1800 K sleep
  // profile renders warm red, never the cool UI accent.
  Color _dayColor = fallbackDayColor;
  Color _sleepColor = fallbackSleepColor;
  ModeCurveVisual _dayVisual =
      const ModeCurveVisual(fallbackColor: fallbackDayColor);
  ModeCurveVisual _sleepVisual =
      const ModeCurveVisual(fallbackColor: fallbackSleepColor);

  @override
  void didUpdateWidget(covariant _RoomTimeEditor oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.wakeTime != widget.wakeTime) {
      wake = _parse(widget.wakeTime);
    }
    if (oldWidget.sleepTime != widget.sleepTime) {
      sleep = _parse(widget.sleepTime);
    }
  }

  static int _parse(String value) {
    final parts = value.split(':');
    return int.parse(parts[0]) * 60 + int.parse(parts[1]);
  }

  String _display(int minute) {
    final hour = minute ~/ 60;
    final min = minute % 60;
    return '${hour.toString().padLeft(2, '0')}:${min.toString().padLeft(2, '0')}';
  }

  void _adjust({required bool wakeTime, required int delta}) {
    HapticFeedback.selectionClick();
    setState(() {
      if (wakeTime) {
        wake = (wake + delta) % 1440;
      } else {
        sleep = (sleep + delta) % 1440;
      }
    });
    widget.onChanged(_display(wake), _display(sleep), 'step_button');
  }

  void _commitFromClock(RhythmMode mode, double hour, TriggerAnchor? snapped) {
    final minutes = (hour * 60).round() % 1440;
    setState(() {
      if (mode == RhythmMode.day) {
        wake = minutes;
      } else {
        sleep = minutes;
      }
    });
    widget.onChanged(_display(wake), _display(sleep), 'dial_drag');
  }

  int _chipMinutes(RhythmMode mode) {
    final preview = _preview;
    if (preview != null && preview.mode == mode) {
      return (preview.hour * 60).round() % 1440;
    }
    return mode == RhythmMode.day ? wake : sleep;
  }

  String _chipValue(RhythmMode mode) {
    final minutes = _chipMinutes(mode);
    final data = _solarClockData;
    if (data != null) {
      for (final anchor in solarAnchorsForMode(mode, data)) {
        final anchorMinutes = (anchor.hour * 60).round() % 1440;
        if (anchorMinutes == minutes) return anchor.label;
      }
    }
    return _display(minutes);
  }

  RhythmCurveConfig? _activeProfile(ServerSyncProvider sync, RhythmMode mode) {
    String? id;
    for (final config in sync.modeConfigs) {
      if (config.mode == mode) {
        id = config.activeProfileId;
        break;
      }
    }
    if (id == null || id.isEmpty) return null;
    for (final profile in sync.profiles) {
      if (profile.id == id) return profile;
    }
    return null;
  }

  void _ensureVisuals(HomeProvider homeProvider, ServerSyncProvider sync) {
    // Profile colors apply even without a location — the time chips carry
    // them whether or not the clock can render.
    final profileColors = resolveProfileColors(sync.modeConfigs, sync.profiles);
    _dayColor = profileColors[RhythmMode.day] ?? fallbackDayColor;
    _sleepColor = profileColors[RhythmMode.sleep] ?? fallbackSleepColor;

    final solarClockDataOverride = widget.solarClockDataOverride;
    if (solarClockDataOverride != null) {
      _visualsKey = null;
      _solarClockData = solarClockDataOverride;
      return;
    }

    final home = homeProvider.currentHome;
    final loc = home?.location;
    if (loc == null) {
      _visualsKey = null;
      _solarClockData = null;
      return;
    }
    final tz =
        home?.timezone ?? SolarUtils.timezoneFromLongitude(loc.longitude);
    final dayProfile = _activeProfile(sync, RhythmMode.day);
    final sleepProfile = _activeProfile(sync, RhythmMode.sleep);
    final key = '${loc.latitude}:${loc.longitude}:$tz'
        '|${dayProfile?.id}:${dayProfile.hashCode}'
        '|${sleepProfile?.id}:${sleepProfile.hashCode}';
    if (key == _visualsKey) return;
    _visualsKey = key;
    _solarClockData = computeSolarClockData(
      latitude: loc.latitude,
      longitude: loc.longitude,
      timezone: tz,
    );
    if (_solarClockData == null) return;
    _dayVisual = buildProfileCurveVisual(
      profile: dayProfile,
      fallbackColor: _dayColor,
      latitude: loc.latitude,
      longitude: loc.longitude,
      timezone: tz,
    );
    _sleepVisual = buildProfileCurveVisual(
      profile: sleepProfile,
      fallbackColor: _sleepColor,
      latitude: loc.latitude,
      longitude: loc.longitude,
      timezone: tz,
    );
  }

  @override
  Widget build(BuildContext context) {
    final homeProvider = context.watch<HomeProvider>();
    final sync = context.watch<ServerSyncProvider>();
    _ensureVisuals(homeProvider, sync);
    final data = _solarClockData;

    return Opacity(
      opacity: widget.enabled ? 1 : 0.48,
      child: Column(
        children: [
          // Wake / Sleep summary chips — same shape as the Alarm Schedule
          // screen's Day/Sleep Start chips, with inline ±15 min steppers.
          // Times track the orb live while it is being dragged.
          IntrinsicHeight(
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Expanded(
                  child: _TimeChip(
                    label: 'Wake',
                    icon: Icons.wb_sunny_rounded,
                    accent: _dayColor,
                    value: _chipValue(RhythmMode.day),
                    valueKey: const ValueKey('room-schedule-wake-value'),
                    enabled: widget.enabled,
                    earlierKey: const ValueKey('room-schedule-wake-earlier'),
                    laterKey: const ValueKey('room-schedule-wake-later'),
                    earlierTooltip: 'Wake 15 minutes earlier',
                    laterTooltip: 'Wake 15 minutes later',
                    onEarlier: () => _adjust(wakeTime: true, delta: -15),
                    onLater: () => _adjust(wakeTime: true, delta: 15),
                  ),
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: _TimeChip(
                    label: 'Sleep',
                    icon: Icons.bedtime_rounded,
                    accent: _sleepColor,
                    value: _chipValue(RhythmMode.sleep),
                    valueKey: const ValueKey('room-schedule-sleep-value'),
                    enabled: widget.enabled,
                    earlierKey: const ValueKey('room-schedule-sleep-earlier'),
                    laterKey: const ValueKey('room-schedule-sleep-later'),
                    earlierTooltip: 'Sleep 15 minutes earlier',
                    laterTooltip: 'Sleep 15 minutes later',
                    onEarlier: () => _adjust(wakeTime: false, delta: -15),
                    onLater: () => _adjust(wakeTime: false, delta: 15),
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(height: 12),
          if (data == null)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 18, horizontal: 8),
              child: Text(
                'Set a home location to unlock the solar rhythm editor.',
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.7),
                  fontSize: 13,
                  fontWeight: FontWeight.w500,
                ),
              ),
            )
          else
            Semantics(
              label: 'Room schedule time dial',
              value: 'Wake ${_display(wake)}, Sleep ${_display(sleep)}',
              hint: 'Drag a marker or use the 15 minute adjustment buttons',
              child: AspectRatio(
                key: const ValueKey('room-schedule-time-dial'),
                aspectRatio: 1.0,
                child: RhythmScheduleClock(
                  data: data,
                  dayHour: wake / 60.0,
                  sleepHour: sleep / 60.0,
                  dayColor: _dayColor,
                  sleepColor: _sleepColor,
                  dayVisual: _dayVisual,
                  sleepVisual: _sleepVisual,
                  selectedMode: _clockMode,
                  onModeSelected: (mode) => setState(() => _clockMode = mode),
                  dayAnchors: solarAnchorsForMode(RhythmMode.day, data),
                  sleepAnchors: solarAnchorsForMode(RhythmMode.sleep, data),
                  onHourCommitted: _commitFromClock,
                  onDragPreview: (preview) =>
                      setState(() => _preview = preview),
                  enabled: widget.enabled,
                  // Room times are free-floating fixed times — unlike the
                  // alarm's transitions they may wrap past midnight.
                  enforceDayBeforeSleep: false,
                ),
              ),
            ),
        ],
      ),
    );
  }
}

/// Mode summary chip echoing the Alarm Schedule screen's Day/Sleep Start
/// chips: tinted fill + hairline in the mode accent, label row on top,
/// time + steppers below.
class _TimeChip extends StatelessWidget {
  const _TimeChip({
    required this.label,
    required this.icon,
    required this.accent,
    required this.value,
    required this.valueKey,
    required this.enabled,
    required this.earlierKey,
    required this.laterKey,
    required this.earlierTooltip,
    required this.laterTooltip,
    required this.onEarlier,
    required this.onLater,
  });

  final String label;
  final IconData icon;
  final Color accent;
  final String value;
  final Key valueKey;
  final bool enabled;
  final Key earlierKey;
  final Key laterKey;
  final String earlierTooltip;
  final String laterTooltip;
  final VoidCallback onEarlier;
  final VoidCallback onLater;

  @override
  Widget build(BuildContext context) {
    return ModeSummaryChip(
      icon: icon,
      label: label.toUpperCase(),
      accent: accent,
      centered: true,
      padding: const EdgeInsets.fromLTRB(10, 8, 10, 6),
      headerGap: 2,
      child: Row(
        children: [
          _stepButton(
            key: earlierKey,
            icon: Icons.remove_rounded,
            tooltip: earlierTooltip,
            onPressed: enabled ? onEarlier : null,
          ),
          Expanded(
            child: FittedBox(
              fit: BoxFit.scaleDown,
              child: Text(
                value,
                key: valueKey,
                maxLines: 1,
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: CelestialColors.textPrimary.withValues(alpha: 0.92),
                  fontSize: 18,
                  fontWeight: FontWeight.w600,
                  fontFeatures: const [FontFeature.tabularFigures()],
                  letterSpacing: 0.5,
                ),
              ),
            ),
          ),
          _stepButton(
            key: laterKey,
            icon: Icons.add_rounded,
            tooltip: laterTooltip,
            onPressed: enabled ? onLater : null,
          ),
        ],
      ),
    );
  }

  Widget _stepButton({
    required Key key,
    required IconData icon,
    required String tooltip,
    required VoidCallback? onPressed,
  }) {
    return IconButton(
      key: key,
      tooltip: tooltip,
      onPressed: onPressed,
      padding: EdgeInsets.zero,
      constraints: const BoxConstraints.tightFor(width: 30, height: 30),
      visualDensity: VisualDensity.compact,
      iconSize: 17,
      icon: Icon(
        icon,
        color: CelestialColors.textSecondary.withValues(alpha: 0.85),
      ),
    );
  }
}
