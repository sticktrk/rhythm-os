import 'dart:async';

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:uuid/uuid.dart';

import '../providers/server_sync_provider.dart';
import '../services/analytics_service.dart';
import 'solar_orbit.dart' show CelestialColors;

/// Capability-gated named-schedule authority and sparse override controls for
/// one room or standalone light. Legacy authority, explicit opt-out, and a
/// named assignment stay separate so rollback never has to guess intent.
class LightScheduleAssignmentCard extends StatefulWidget {
  const LightScheduleAssignmentCard({
    super.key,
    required this.nodeId,
    required this.targetLabel,
  });

  final String nodeId;
  final String targetLabel;

  @override
  State<LightScheduleAssignmentCard> createState() =>
      _LightScheduleAssignmentCardState();
}

class _LightScheduleAssignmentCardState
    extends State<LightScheduleAssignmentCard> {
  static const _uuid = Uuid();
  String? _retryScheduleId;
  RhythmLightScheduleOverride? _retryOverride;
  String? _retryJourneyId;
  String? _retryOverrideScope;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      unawaited(context.read<ServerSyncProvider>().loadLightSchedules());
    });
  }

  String _assignmentLabel(ServerSyncProvider sync) {
    final node = sync.nodeById(widget.nodeId);
    final assignment = node?.profileSettings?.lightSchedule;
    final localAssignment = node?.localProfileSettings?.lightSchedule;
    if (assignment == null) return 'Legacy whole-home Alarm';
    if (assignment.isUnscheduled) return 'No automatic schedule';
    final schedule = sync.lightSchedules
        .where((value) => value.id == assignment.scheduleId)
        .firstOrNull;
    final name = schedule?.name ?? 'Named schedule';
    if (localAssignment == null && node?.parentId != null) {
      return '$name · Inherited from parent';
    }
    return schedule?.enabled == false ? '$name · Dormant' : name;
  }

  Future<void> _assign(String? scheduleId, {bool legacy = false}) async {
    final sync = context.read<ServerSyncProvider>();
    final journeyId = 'light-schedule-assignment-${_uuid.v4()}';
    final assignmentKind = legacy
        ? 'legacy'
        : scheduleId == null
            ? 'unscheduled'
            : 'named';
    unawaited(
      AnalyticsService().logLightScheduleAssignmentAttempted(
        journeyId: journeyId,
        assignmentKind: assignmentKind,
        overrideScope: 'none',
      ),
    );
    final ok = await sync.setNodeLightScheduleAssignment(
      widget.nodeId,
      scheduleId,
      legacy: legacy,
    );
    unawaited(
      AnalyticsService().logLightScheduleAssignmentCompleted(
        journeyId: journeyId,
        assignmentKind: assignmentKind,
        overrideScope: 'none',
        outcome: ok ? 'succeeded' : 'failed',
        failureStage: ok ? null : 'appliance_ack',
      ),
    );
    if (!mounted) return;
    if (ok) {
      Navigator.of(context).pop();
    } else {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('Could not change the schedule.')),
      );
    }
  }

  Future<void> _chooseAssignment() async {
    final sync = context.read<ServerSyncProvider>();
    await showModalBottomSheet<void>(
      context: context,
      backgroundColor: CelestialColors.backgroundDark,
      showDragHandle: true,
      builder: (sheetContext) => ListTileTheme(
        data: const ListTileThemeData(
          textColor: CelestialColors.textPrimary,
          iconColor: CelestialColors.textSecondary,
        ),
        child: SafeArea(
          child: Padding(
          padding: const EdgeInsets.fromLTRB(16, 0, 16, 20),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Text(
                '${widget.targetLabel} schedule',
                style: Theme.of(context).textTheme.titleLarge,
              ),
              const SizedBox(height: 8),
              ListTile(
                key: const ValueKey('light-schedule-assignment-legacy'),
                leading: const Icon(Icons.account_tree_outlined),
                title: const Text('Follow inherited schedule'),
                subtitle: const Text(
                  'Use the nearest parent, or the legacy whole-home Alarm',
                ),
                onTap: () => _assign(null, legacy: true),
              ),
              ListTile(
                key: const ValueKey('light-schedule-assignment-none'),
                leading: const Icon(Icons.notifications_off_outlined),
                title: const Text('No automatic schedule'),
                subtitle: const Text('Keep manual Wake and Sleep control'),
                onTap: () => _assign(null),
              ),
              for (final schedule in sync.lightSchedules)
                ListTile(
                  key: ValueKey('light-schedule-assignment-${schedule.id}'),
                  leading: const Icon(Icons.schedule_rounded),
                  title: Text(schedule.name),
                  subtitle: Text(schedule.enabled ? 'Automatic' : 'Off'),
                  onTap: () => _assign(schedule.id),
                ),
            ],
            ),
          ),
        ),
      ),
    );
  }

  Future<void> _customize(
    RhythmLightScheduleConfig schedule,
    RhythmModeTransitionConfig transition,
  ) async {
    final sync = context.read<ServerSyncProvider>();
    final node = sync.nodeById(widget.nodeId);
    final current = node?.localProfileSettings
        ?.lightScheduleOverrides[schedule.id]?.transitions[transition.id];
    final inherited = sync.inheritedLightScheduleTransitionOverride(
      widget.nodeId,
      schedule.id,
      transition.id,
    );
    final inheritedKind = inherited?.trigger.kind ?? transition.trigger.kind;
    var kindChoice = current?.trigger.kind ?? 'inherit';
    var eventChoice = current?.trigger.event ?? 'inherit';
    var timeOverride = current?.trigger.time != null;
    var time = current?.trigger.time ??
        inherited?.trigger.time ??
        transition.trigger.time ??
        '07:00';
    var offsetOverride = current?.trigger.offsetMinutes != null;
    var offset = current?.trigger.offsetMinutes ??
        inherited?.trigger.offsetMinutes ??
        transition.trigger.offsetMinutes;
    var enabledChoice = current?.triggerEnabled == null
        ? 'inherit'
        : current!.triggerEnabled!
            ? 'enabled'
            : 'disabled';
    final solarAvailable = sync.solarScheduleAnchorsAvailable;
    final updated = await showModalBottomSheet<RhythmModeTransitionOverride>(
      context: context,
      backgroundColor: CelestialColors.backgroundDark,
      showDragHandle: true,
      isScrollControlled: true,
      builder: (context) => StatefulBuilder(
        builder: (context, setSheetState) => SafeArea(
          child: Padding(
            padding: EdgeInsets.fromLTRB(
              20,
              0,
              20,
              20 + MediaQuery.viewInsetsOf(context).bottom,
            ),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Text(
                  '${transition.label} customization',
                  style: Theme.of(context).textTheme.titleLarge,
                ),
                const SizedBox(height: 12),
                DropdownButtonFormField<String>(
                  initialValue: kindChoice,
                  decoration: const InputDecoration(labelText: 'Trigger type'),
                  items: const [
                    DropdownMenuItem(
                      value: 'inherit',
                      child: Text('Follow schedule'),
                    ),
                    DropdownMenuItem(
                      value: 'solar',
                      child: Text('Solar event'),
                    ),
                    DropdownMenuItem(
                      value: 'scheduled',
                      child: Text('Fixed local time'),
                    ),
                  ],
                  onChanged: (value) {
                    if (value == null) return;
                    setSheetState(() {
                      kindChoice = value;
                      final selected =
                          value == 'inherit' ? inheritedKind : value;
                      if (selected == 'solar') {
                        timeOverride = false;
                        if (value != 'inherit' &&
                            inheritedKind != 'solar' &&
                            eventChoice == 'inherit') {
                          eventChoice = transition.toMode == RhythmMode.day
                              ? 'sunrise'
                              : 'sunset';
                        }
                      } else if (selected == 'scheduled') {
                        eventChoice = 'inherit';
                        offsetOverride = false;
                        if (value != 'inherit' &&
                            inheritedKind != 'scheduled') {
                          timeOverride = true;
                        }
                      }
                    });
                  },
                ),
                const SizedBox(height: 12),
                DropdownButtonFormField<String>(
                  initialValue: enabledChoice,
                  decoration: const InputDecoration(labelText: 'Enabled'),
                  items: const [
                    DropdownMenuItem(
                      value: 'inherit',
                      child: Text('Follow schedule'),
                    ),
                    DropdownMenuItem(
                      value: 'enabled',
                      child: Text('Enabled here'),
                    ),
                    DropdownMenuItem(
                      value: 'disabled',
                      child: Text('Disabled here'),
                    ),
                  ],
                  onChanged: (value) {
                    if (value != null) {
                      setSheetState(() => enabledChoice = value);
                    }
                  },
                ),
                if ((kindChoice == 'inherit' ? inheritedKind : kindChoice) ==
                    'solar') ...[
                  if (!solarAvailable)
                    const Padding(
                      padding: EdgeInsets.only(top: 12),
                      child: Text(
                        'Solar customization is unavailable until this home has a location and timezone.',
                        style: TextStyle(color: CelestialColors.sunWarm),
                      ),
                    ),
                  const SizedBox(height: 12),
                  DropdownButtonFormField<String>(
                    initialValue: eventChoice,
                    decoration: const InputDecoration(
                      labelText: 'Solar anchor',
                    ),
                    items: [
                      if (kindChoice == 'inherit' || inheritedKind == 'solar')
                        const DropdownMenuItem(
                          value: 'inherit',
                          child: Text('Follow schedule'),
                        ),
                      const DropdownMenuItem(
                        value: 'sunrise',
                        child: Text('Sunrise'),
                      ),
                      const DropdownMenuItem(
                        value: 'sunset',
                        child: Text('Sunset'),
                      ),
                      const DropdownMenuItem(
                        value: 'civil_twilight',
                        child: Text('Civil twilight'),
                      ),
                      const DropdownMenuItem(
                        value: 'nautical_twilight',
                        child: Text('Nautical twilight'),
                      ),
                      const DropdownMenuItem(
                        value: 'astronomical_twilight',
                        child: Text('Astronomical twilight'),
                      ),
                    ],
                    onChanged: solarAvailable
                        ? (value) {
                            if (value != null) {
                              setSheetState(() => eventChoice = value);
                            }
                          }
                        : null,
                  ),
                  if (sync.lightScheduleSolarOffsetsSupported) ...[
                    SwitchListTile.adaptive(
                      contentPadding: EdgeInsets.zero,
                      title: const Text('Override offset'),
                      subtitle: const Text(
                        'Turn off to follow the schedule offset',
                      ),
                      value: offsetOverride,
                      onChanged: solarAvailable
                          ? (value) =>
                              setSheetState(() => offsetOverride = value)
                          : null,
                    ),
                    if (offsetOverride) ...[
                      const SizedBox(height: 12),
                      Text(
                        offset == 0
                            ? 'At the anchor'
                            : '${offset.abs()} minutes ${offset < 0 ? 'before' : 'after'}',
                        textAlign: TextAlign.center,
                      ),
                      Slider(
                        key: ValueKey(
                          'light-schedule-override-offset-${transition.id}',
                        ),
                        min: -maxSolarScheduleOffsetMinutes.toDouble(),
                        max: maxSolarScheduleOffsetMinutes.toDouble(),
                        divisions: maxSolarScheduleOffsetMinutes * 2,
                        value: offset
                            .clamp(
                              -maxSolarScheduleOffsetMinutes,
                              maxSolarScheduleOffsetMinutes,
                            )
                            .toDouble(),
                        label: '$offset min',
                        onChanged: solarAvailable
                            ? (value) =>
                                setSheetState(() => offset = value.round())
                            : null,
                      ),
                    ],
                  ],
                ] else ...[
                  SwitchListTile.adaptive(
                    contentPadding: EdgeInsets.zero,
                    title: const Text('Override local time'),
                    subtitle: const Text(
                      'Turn off to follow the schedule time',
                    ),
                    value: timeOverride,
                    onChanged: kindChoice == 'scheduled' &&
                            inheritedKind != 'scheduled'
                        ? null
                        : (value) => setSheetState(() => timeOverride = value),
                  ),
                  if (timeOverride)
                    TextFormField(
                      key: ValueKey(
                        'light-schedule-override-time-${transition.id}',
                      ),
                      initialValue: time,
                      decoration: const InputDecoration(
                        labelText: 'Local time (HH:MM)',
                      ),
                      onChanged: (value) => time = value,
                    ),
                ],
                const SizedBox(height: 8),
                FilledButton(
                  onPressed: ((kindChoice == 'inherit'
                              ? inheritedKind
                              : kindChoice) ==
                          'scheduled' ||
                          solarAvailable)
                      ? () => Navigator.of(context).pop(
                            RhythmModeTransitionOverride(
                              trigger: RhythmTransitionTriggerOverride(
                                kind:
                                    kindChoice == 'inherit' ? null : kindChoice,
                                event: eventChoice == 'inherit'
                                    ? null
                                    : eventChoice,
                                time: timeOverride ? time : null,
                                offsetMinutes: offsetOverride &&
                                        sync.lightScheduleSolarOffsetsSupported
                                    ? offset
                                    : null,
                              ),
                              triggerEnabled: enabledChoice == 'inherit'
                                  ? null
                                  : enabledChoice == 'enabled',
                            ),
                          )
                      : null,
                  child: const Text('Save customization'),
                ),
              ],
            ),
          ),
        ),
      ),
    );
    if (updated == null || !mounted) return;
    final previous = sync
            .nodeById(widget.nodeId)
            ?.localProfileSettings
            ?.lightScheduleOverrides[schedule.id] ??
        const RhythmLightScheduleOverride();
    final transitions = Map<String, RhythmModeTransitionOverride>.from(
      previous.transitions,
    );
    if (updated.isEmpty) {
      transitions.remove(transition.id);
    } else {
      transitions[transition.id] = updated;
    }
    final nextOverride = transitions.isEmpty
        ? null
        : RhythmLightScheduleOverride(transitions: transitions);
    final journeyId = 'light-schedule-override-${_uuid.v4()}';
    unawaited(
      AnalyticsService().logLightScheduleAssignmentAttempted(
        journeyId: journeyId,
        assignmentKind: 'named',
        overrideScope: 'transition',
      ),
    );
    final ok = await sync.setNodeLightScheduleOverride(
      widget.nodeId,
      schedule.id,
      nextOverride,
      journeyId: journeyId,
    );
    unawaited(
      AnalyticsService().logLightScheduleAssignmentCompleted(
        journeyId: journeyId,
        assignmentKind: 'named',
        overrideScope: 'transition',
        outcome: ok ? 'succeeded' : 'failed',
        failureStage: ok ? null : 'appliance_ack',
      ),
    );
    if (!mounted) return;
    setState(() {
      _retryScheduleId = ok ? null : schedule.id;
      _retryOverride = ok ? null : nextOverride;
      _retryJourneyId = ok ? null : journeyId;
      _retryOverrideScope = ok ? null : 'transition';
    });
    if (!ok) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('Could not save the customization.')),
      );
    }
  }

  Future<void> _retryLastOverride() async {
    final scheduleId = _retryScheduleId;
    final journeyId = _retryJourneyId;
    final overrideScope = _retryOverrideScope;
    if (scheduleId == null || journeyId == null || overrideScope == null) return;
    unawaited(AnalyticsService().logLightScheduleAssignmentAttempted(
      journeyId: journeyId,
      assignmentKind: 'named',
      overrideScope: overrideScope,
    ));
    final ok =
        await context.read<ServerSyncProvider>().setNodeLightScheduleOverride(
              widget.nodeId,
              scheduleId,
              _retryOverride,
              journeyId: journeyId,
            );
    unawaited(AnalyticsService().logLightScheduleAssignmentCompleted(
      journeyId: journeyId,
      assignmentKind: 'named',
      overrideScope: overrideScope,
      outcome: ok ? 'succeeded' : 'failed',
      failureStage: ok ? null : 'appliance_ack',
    ));
    if (mounted && ok) {
      setState(() {
        _retryScheduleId = null;
        _retryOverride = null;
        _retryJourneyId = null;
        _retryOverrideScope = null;
      });
    }
  }

  Future<void> _resetOverrides(String scheduleId) async {
    final sync = context.read<ServerSyncProvider>();
    final journeyId = 'light-schedule-reset-${_uuid.v4()}';
    unawaited(
      AnalyticsService().logLightScheduleAssignmentAttempted(
        journeyId: journeyId,
        assignmentKind: 'named',
        overrideScope: 'reset',
      ),
    );
    final ok = await sync.setNodeLightScheduleOverride(
      widget.nodeId,
      scheduleId,
      null,
      journeyId: journeyId,
    );
    unawaited(
      AnalyticsService().logLightScheduleAssignmentCompleted(
        journeyId: journeyId,
        assignmentKind: 'named',
        overrideScope: 'reset',
        outcome: ok ? 'succeeded' : 'failed',
        failureStage: ok ? null : 'appliance_ack',
      ),
    );
    if (mounted) {
      setState(() {
        _retryScheduleId = ok ? null : scheduleId;
        _retryOverride = null;
        _retryJourneyId = ok ? null : journeyId;
        _retryOverrideScope = ok ? null : 'reset';
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final sync = context.watch<ServerSyncProvider>();
    if (!sync.lightSchedulesSupported) return const SizedBox.shrink();
    final node = sync.nodeById(widget.nodeId);
    final settings = node?.profileSettings;
    final localSettings = node?.localProfileSettings;
    final scheduleId = settings?.lightScheduleId;
    final schedule = sync.lightSchedules
        .where((value) => value.id == scheduleId)
        .firstOrNull;
    final customized = scheduleId != null &&
        localSettings?.lightScheduleOverrides[scheduleId]?.isEmpty == false;
    final pending = sync.lightScheduleWritePendingForNode(widget.nodeId);
    final rejected = sync.lightScheduleWriteRejectedForNode(widget.nodeId);

    return Container(
      key: ValueKey('named-light-schedule-${widget.nodeId}'),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundDark.withValues(alpha: 0.5),
        borderRadius: BorderRadius.circular(14),
        border: Border.all(
          color: CelestialColors.orbitRing.withValues(alpha: 0.3),
        ),
      ),
      child: Material(
        color: Colors.transparent,
        borderRadius: BorderRadius.circular(14),
        child: ListTileTheme(
          data: const ListTileThemeData(
            textColor: CelestialColors.textPrimary,
            iconColor: CelestialColors.textSecondary,
          ),
          child: Column(
            children: [
            ListTile(
              leading: pending
                  ? const SizedBox.square(
                      dimension: 20,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    )
                  : const Icon(
                      Icons.schedule_rounded,
                      color: CelestialColors.accentBlue,
                    ),
              title: const Text('Named schedule'),
              subtitle: Text(_assignmentLabel(sync)),
              trailing: const Icon(Icons.chevron_right_rounded),
              onTap: pending ? null : _chooseAssignment,
            ),
            if (schedule != null && sync.lightScheduleOverridesSupported) ...[
              Divider(
                height: 1,
                indent: 56,
                color: CelestialColors.orbitRing.withValues(alpha: 0.2),
              ),
              for (final transition in schedule.transitions)
                ListTile(
                  key: ValueKey('light-schedule-customize-${transition.id}'),
                  leading: Icon(
                    transition.toMode == RhythmMode.day
                        ? Icons.wb_sunny_outlined
                        : Icons.bedtime_outlined,
                  ),
                  title: Text(transition.label),
                  subtitle: Builder(
                    builder: (context) {
                      final local = localSettings
                          ?.lightScheduleOverrides[schedule.id]
                          ?.transitions[transition.id];
                      final inherited = settings
                          ?.lightScheduleOverrides[schedule.id]
                          ?.transitions[transition.id];
                      final source = local?.isEmpty == false
                          ? 'Customized here'
                          : inherited?.isEmpty == false
                              ? 'Inherited customization'
                              : 'Following schedule';
                      final time =
                          sync.resolvedLightScheduleTransitionLocalTime(
                        widget.nodeId,
                        schedule,
                        transition,
                      );
                      return Text(
                        time == null ? source : '$source · $time local',
                      );
                    },
                  ),
                  trailing: const Icon(Icons.tune_rounded),
                  onTap:
                      pending ? null : () => _customize(schedule, transition),
                ),
              if (customized)
                TextButton.icon(
                  key: const ValueKey('light-schedule-overrides-reset'),
                  onPressed:
                      pending ? null : () => _resetOverrides(schedule.id),
                  icon: const Icon(Icons.restart_alt_rounded),
                  label: const Text('Follow schedule for every field'),
                ),
              if (rejected)
                Padding(
                  padding: const EdgeInsets.fromLTRB(16, 0, 16, 10),
                  child: Row(
                    children: [
                      const Expanded(
                        child: Text(
                          'Save rejected by the appliance.',
                          style: TextStyle(color: CelestialColors.sunWarm),
                        ),
                      ),
                      TextButton(
                        key: const ValueKey('light-schedule-override-retry'),
                        onPressed: pending || _retryScheduleId == null
                            ? null
                            : _retryLastOverride,
                        child: const Text('Retry'),
                      ),
                    ],
                  ),
                ),
            ],
            ],
          ),
        ),
      ),
    );
  }
}
