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

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      unawaited(context.read<ServerSyncProvider>().loadLightSchedules());
    });
  }

  String _assignmentLabel(ServerSyncProvider sync) {
    final assignment =
        sync.nodeById(widget.nodeId)?.profileSettings?.lightSchedule;
    if (assignment == null) return 'Whole-home Alarm';
    if (assignment.isUnscheduled) return 'No automatic schedule';
    final schedule = sync.lightSchedules
        .where((value) => value.id == assignment.scheduleId)
        .firstOrNull;
    return schedule?.name ?? 'Named schedule';
  }

  Future<void> _assign(
    String? scheduleId, {
    bool legacy = false,
  }) async {
    final sync = context.read<ServerSyncProvider>();
    final journeyId = 'light-schedule-assignment-${_uuid.v4()}';
    final assignmentKind = legacy
        ? 'legacy'
        : scheduleId == null
            ? 'unscheduled'
            : 'named';
    unawaited(AnalyticsService().logLightScheduleAssignmentAttempted(
      journeyId: journeyId,
      assignmentKind: assignmentKind,
      overrideScope: 'none',
    ));
    final ok = await sync.setNodeLightScheduleAssignment(
      widget.nodeId,
      scheduleId,
      legacy: legacy,
    );
    unawaited(AnalyticsService().logLightScheduleAssignmentCompleted(
      journeyId: journeyId,
      assignmentKind: assignmentKind,
      overrideScope: 'none',
      outcome: ok ? 'succeeded' : 'failed',
      failureStage: ok ? null : 'appliance_ack',
    ));
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
      builder: (sheetContext) => SafeArea(
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
                leading: const Icon(Icons.home_outlined),
                title: const Text('Whole-home Alarm'),
                subtitle: const Text('Use the compatible legacy schedule'),
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
    );
  }

  Future<void> _customize(
    RhythmLightScheduleConfig schedule,
    RhythmModeTransitionConfig transition,
  ) async {
    final sync = context.read<ServerSyncProvider>();
    final current = sync
        .nodeById(widget.nodeId)
        ?.profileSettings
        ?.lightScheduleOverrides[schedule.id]
        ?.transitions[transition.id];
    var followAnchor = current?.trigger.event == null;
    var event = current?.trigger.event ??
        transition.trigger.event ??
        (transition.toMode == RhythmMode.day ? 'sunrise' : 'sunset');
    var offset = current?.trigger.offsetMinutes ?? 0;
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
                SwitchListTile.adaptive(
                  contentPadding: EdgeInsets.zero,
                  title: const Text('Follow schedule anchor'),
                  subtitle: const Text(
                    'Future anchor changes keep flowing to this room',
                  ),
                  value: followAnchor,
                  onChanged: solarAvailable
                      ? (value) => setSheetState(() => followAnchor = value)
                      : null,
                ),
                if (!solarAvailable)
                  const Padding(
                    padding: EdgeInsets.only(bottom: 12),
                    child: Text(
                      'Solar customization is unavailable until this home has a location and timezone.',
                      style: TextStyle(color: CelestialColors.sunWarm),
                    ),
                  ),
                if (!followAnchor)
                  DropdownButtonFormField<String>(
                    initialValue: event,
                    decoration:
                        const InputDecoration(labelText: 'Solar anchor'),
                    items: const [
                      DropdownMenuItem(
                          value: 'sunrise', child: Text('Sunrise')),
                      DropdownMenuItem(value: 'sunset', child: Text('Sunset')),
                      DropdownMenuItem(
                        value: 'civil_twilight',
                        child: Text('Civil twilight'),
                      ),
                      DropdownMenuItem(
                        value: 'nautical_twilight',
                        child: Text('Nautical twilight'),
                      ),
                      DropdownMenuItem(
                        value: 'astronomical_twilight',
                        child: Text('Astronomical twilight'),
                      ),
                    ],
                    onChanged: solarAvailable
                        ? (value) {
                            if (value != null) {
                              setSheetState(() => event = value);
                            }
                          }
                        : null,
                  ),
                if (sync.lightScheduleSolarOffsetsSupported) ...[
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
                    min: -180,
                    max: 180,
                    divisions: 72,
                    value: offset.clamp(-180, 180).toDouble(),
                    label: '$offset min',
                    onChanged: solarAvailable
                        ? (value) => setSheetState(() => offset = value.round())
                        : null,
                  ),
                ],
                const SizedBox(height: 8),
                FilledButton(
                  onPressed: solarAvailable
                      ? () => Navigator.of(context).pop(
                            RhythmModeTransitionOverride(
                              trigger: RhythmTransitionTriggerOverride(
                                event: followAnchor ? null : event,
                                offsetMinutes:
                                    sync.lightScheduleSolarOffsetsSupported
                                        ? offset
                                        : null,
                              ),
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
            ?.profileSettings
            ?.lightScheduleOverrides[schedule.id] ??
        const RhythmLightScheduleOverride();
    final transitions = Map<String, RhythmModeTransitionOverride>.from(
      previous.transitions,
    )..[transition.id] = updated;
    final journeyId = 'light-schedule-override-${_uuid.v4()}';
    unawaited(AnalyticsService().logLightScheduleAssignmentAttempted(
      journeyId: journeyId,
      assignmentKind: 'named',
      overrideScope: 'transition',
    ));
    final ok = await sync.setNodeLightScheduleOverride(
      widget.nodeId,
      schedule.id,
      RhythmLightScheduleOverride(transitions: transitions),
      journeyId: journeyId,
    );
    unawaited(AnalyticsService().logLightScheduleAssignmentCompleted(
      journeyId: journeyId,
      assignmentKind: 'named',
      overrideScope: 'transition',
      outcome: ok ? 'succeeded' : 'failed',
      failureStage: ok ? null : 'appliance_ack',
    ));
    if (mounted && !ok) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('Could not save the customization.')),
      );
    }
  }

  Future<void> _resetOverrides(String scheduleId) async {
    final sync = context.read<ServerSyncProvider>();
    final journeyId = 'light-schedule-reset-${_uuid.v4()}';
    unawaited(AnalyticsService().logLightScheduleAssignmentAttempted(
      journeyId: journeyId,
      assignmentKind: 'named',
      overrideScope: 'reset',
    ));
    final ok = await sync.setNodeLightScheduleOverride(
      widget.nodeId,
      scheduleId,
      null,
      journeyId: journeyId,
    );
    unawaited(AnalyticsService().logLightScheduleAssignmentCompleted(
      journeyId: journeyId,
      assignmentKind: 'named',
      overrideScope: 'reset',
      outcome: ok ? 'succeeded' : 'failed',
      failureStage: ok ? null : 'appliance_ack',
    ));
  }

  @override
  Widget build(BuildContext context) {
    final sync = context.watch<ServerSyncProvider>();
    if (!sync.lightSchedulesSupported) return const SizedBox.shrink();
    final settings = sync.nodeById(widget.nodeId)?.profileSettings;
    final scheduleId = settings?.lightScheduleId;
    final schedule = sync.lightSchedules
        .where((value) => value.id == scheduleId)
        .firstOrNull;
    final customized = scheduleId != null &&
        settings?.lightScheduleOverrides[scheduleId]?.isEmpty == false;
    final pending = sync.lightScheduleWritePendingForNode(widget.nodeId);

    return Container(
      key: ValueKey('named-light-schedule-${widget.nodeId}'),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundDark.withValues(alpha: 0.5),
        borderRadius: BorderRadius.circular(14),
        border: Border.all(
          color: CelestialColors.orbitRing.withValues(alpha: 0.3),
        ),
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
              if (transition.trigger.isSolar)
                ListTile(
                  key: ValueKey(
                    'light-schedule-customize-${transition.id}',
                  ),
                  leading: Icon(
                    transition.toMode == RhythmMode.day
                        ? Icons.wb_sunny_outlined
                        : Icons.bedtime_outlined,
                  ),
                  title: Text(transition.label),
                  subtitle: const Text('Customize anchor or offset'),
                  trailing: const Icon(Icons.tune_rounded),
                  onTap:
                      pending ? null : () => _customize(schedule, transition),
                ),
            if (customized)
              TextButton.icon(
                key: const ValueKey('light-schedule-overrides-reset'),
                onPressed: pending ? null : () => _resetOverrides(schedule.id),
                icon: const Icon(Icons.restart_alt_rounded),
                label: const Text('Follow schedule for every field'),
              ),
          ],
        ],
      ),
    );
  }
}
