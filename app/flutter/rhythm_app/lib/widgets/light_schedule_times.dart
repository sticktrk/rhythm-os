import 'dart:async';

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:uuid/uuid.dart';

import '../providers/server_sync_provider.dart';
import '../services/analytics_service.dart';
import 'solar_orbit.dart' show CelestialColors;

const _uuid = Uuid();

/// The Wake and Sleep boundaries a schedule gives a room: its automatic
/// transition into each mode, preferring the canonical `day_start` /
/// `sleep_start` ids.
Iterable<RhythmModeTransitionConfig> lightScheduleBoundaryTransitions(
  RhythmLightScheduleConfig schedule,
) sync* {
  for (final target in [RhythmMode.day, RhythmMode.sleep]) {
    final preferredId = target == RhythmMode.day ? 'day_start' : 'sleep_start';
    final candidates = schedule.transitions
        .where((transition) =>
            transition.toMode == target && !transition.trigger.isManual)
        .toList(growable: false);
    final preferred = candidates
        .where((transition) => transition.id == preferredId)
        .firstOrNull;
    final boundary =
        preferred ?? (candidates.length == 1 ? candidates.single : null);
    if (boundary != null) yield boundary;
  }
}

/// Plain-language name for what fires a boundary: "Sunrise",
/// "15 min before Sunset", or the fixed local time.
String lightScheduleTriggerLabel(
  RhythmModeTransitionConfig transition, [
  RhythmModeTransitionOverride? effectiveOverride,
]) {
  final trigger = transition.trigger;
  final override = effectiveOverride?.trigger;
  final kind = override?.kind ?? trigger.kind;
  switch (kind) {
    case 'solar':
      final event = override?.event ?? trigger.event;
      if (event == null || event.isEmpty) return 'Sun position';
      final anchor = lightScheduleSolarEventLabel(event, transition.toMode);
      final offset = override?.offsetMinutes ?? trigger.offsetMinutes;
      if (offset == 0) return anchor;
      return '${offset.abs()} min ${offset < 0 ? 'before' : 'after'} $anchor';
    case 'scheduled':
      return override?.time ?? trigger.time ?? 'Fixed time';
    default:
      return 'Manual';
  }
}

/// One-line description of a named schedule for a picker row, for example
/// "Switches at Sunrise and Sunset".
String lightScheduleSummary(RhythmLightScheduleConfig schedule) {
  if (!schedule.enabled) return 'Paused';
  final triggers = [
    for (final transition in lightScheduleBoundaryTransitions(schedule))
      if (transition.triggerEnabled) lightScheduleTriggerLabel(transition),
  ];
  return triggers.isEmpty
      ? 'No automatic times'
      : 'Switches at ${triggers.join(' and ')}';
}

/// The trigger that fires a node's Wake ([RhythmMode.day]) or Sleep preset
/// under the named schedule it follows, or null when that boundary is
/// missing or turned off.
String? lightScheduleTriggerForMode(
  RhythmRoom? node,
  RhythmLightScheduleConfig schedule,
  RhythmMode mode,
) {
  final transition = lightScheduleBoundaryTransitions(schedule)
      .where((value) => value.toMode == mode)
      .firstOrNull;
  if (transition == null) return null;
  final effective = node?.profileSettings?.lightScheduleOverrides[schedule.id]
      ?.transitions[transition.id];
  if (!(effective?.triggerEnabled ?? transition.triggerEnabled)) return null;
  return lightScheduleTriggerLabel(transition, effective);
}

/// Hands one room or standalone light to a schedule authority: a named
/// schedule ([scheduleId]), no automatic schedule (null), or back to the
/// inherited/whole-home schedule ([legacy]). Legacy authority, explicit
/// opt-out, and a named assignment stay separate so rollback never has to
/// guess intent.
///
/// A named schedule runs the same in every room, so choosing one also drops
/// any adjustments this node still holds for it. Rooms that need their own
/// times use Custom times instead.
Future<bool> assignLightSchedule(
  ServerSyncProvider sync,
  String nodeId,
  String? scheduleId, {
  bool legacy = false,
}) async {
  final journeyId = 'light-schedule-assignment-${_uuid.v4()}';
  final clearsOwnTimes = !legacy &&
      scheduleId != null &&
      sync.lightScheduleOverridesSupported &&
      sync
              .nodeById(nodeId)
              ?.localProfileSettings
              ?.lightScheduleOverrides[scheduleId]
              ?.isEmpty ==
          false;
  final assignmentKind = legacy
      ? 'legacy'
      : scheduleId == null
          ? 'unscheduled'
          : 'named';
  final overrideScope = clearsOwnTimes ? 'reset' : 'none';
  unawaited(
    AnalyticsService().logLightScheduleAssignmentAttempted(
      journeyId: journeyId,
      assignmentKind: assignmentKind,
      overrideScope: overrideScope,
    ),
  );
  var failureStage = 'assignment_ack';
  var ok = true;
  if (clearsOwnTimes) {
    ok = await sync.setNodeLightScheduleOverride(
      nodeId,
      scheduleId,
      null,
      journeyId: journeyId,
    );
    failureStage = 'override_ack';
  }
  if (ok) {
    failureStage = 'assignment_ack';
    ok = await sync.setNodeLightScheduleAssignment(
      nodeId,
      scheduleId,
      legacy: legacy,
      journeyId: journeyId,
    );
  }
  unawaited(
    AnalyticsService().logLightScheduleAssignmentCompleted(
      journeyId: journeyId,
      assignmentKind: assignmentKind,
      overrideScope: overrideScope,
      outcome: ok ? 'succeeded' : 'failed',
      failureStage: ok ? null : failureStage,
    ),
  );
  return ok;
}

/// Read-only Wake and Sleep times for the named schedule a room or
/// standalone light follows. Named schedules are edited once, for the whole
/// home, on the Schedules screen; a room only ever shows when they fire today.
class LightScheduleTimes extends StatelessWidget {
  const LightScheduleTimes({super.key, required this.nodeId});

  final String nodeId;

  /// The trigger's name and, when it is not itself a clock time, the local
  /// time it resolves to today.
  (String, String) _triggerAndTime(
    ServerSyncProvider sync,
    RhythmLightScheduleConfig schedule,
    RhythmModeTransitionConfig transition,
  ) {
    final effective = sync
        .nodeById(nodeId)
        ?.profileSettings
        ?.lightScheduleOverrides[schedule.id]
        ?.transitions[transition.id];
    final trigger = lightScheduleTriggerLabel(transition, effective);
    if (!(effective?.triggerEnabled ?? transition.triggerEnabled)) {
      return (trigger, 'Turned off');
    }
    final time = sync.resolvedLightScheduleTransitionLocalTime(
      nodeId,
      schedule,
      transition,
    );
    return (trigger, time == null || time == trigger ? '' : time);
  }

  @override
  Widget build(BuildContext context) {
    final sync = context.watch<ServerSyncProvider>();
    if (!sync.lightScheduleTargetSupportedForNode(nodeId)) {
      return const SizedBox.shrink();
    }
    final scheduleId = sync.nodeById(nodeId)?.profileSettings?.lightScheduleId;
    final schedule = sync.lightSchedules
        .where((value) => value.id == scheduleId)
        .firstOrNull;
    if (schedule == null) return const SizedBox.shrink();

    return Padding(
      key: ValueKey('light-schedule-times-$nodeId'),
      padding: const EdgeInsets.fromLTRB(10, 6, 10, 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (final transition in lightScheduleBoundaryTransitions(schedule))
            Padding(
              key: ValueKey('light-schedule-time-${transition.id}'),
              padding: const EdgeInsets.symmetric(vertical: 6),
              child: Row(
                children: [
                  Icon(
                    transition.toMode == RhythmMode.day
                        ? Icons.wb_sunny_rounded
                        : Icons.bedtime_rounded,
                    size: 16,
                    color: transition.toMode == RhythmMode.day
                        ? const Color(0xFFF9A825)
                        : const Color(0xFF7C83FF),
                  ),
                  const SizedBox(width: 12),
                  Text(
                    _triggerAndTime(sync, schedule, transition).$1,
                    style: const TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 13.5,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Text(
                      _triggerAndTime(sync, schedule, transition).$2,
                      textAlign: TextAlign.end,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.9),
                        fontSize: 13,
                        fontFeatures: const [FontFeature.tabularFigures()],
                      ),
                    ),
                  ),
                ],
              ),
            ),
        ],
      ),
    );
  }
}
