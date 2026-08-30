import 'dart:async';

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:uuid/uuid.dart';

import '../../providers/server_sync_provider.dart';
import '../../services/analytics_service.dart';
import '../../widgets/settings_row.dart';
import '../../widgets/solar_orbit.dart' show CelestialColors;

class LightSchedulesScreen extends StatefulWidget {
  const LightSchedulesScreen({super.key});

  @override
  State<LightSchedulesScreen> createState() => _LightSchedulesScreenState();
}

class _LightSchedulesScreenState extends State<LightSchedulesScreen> {
  @override
  void initState() {
    super.initState();
    unawaited(AnalyticsService().logLightSchedulesOpened(source: 'presets'));
    WidgetsBinding.instance.addPostFrameCallback((_) {
      unawaited(context.read<ServerSyncProvider>().loadLightSchedules());
    });
  }

  Future<void> _edit(RhythmLightScheduleConfig? schedule) async {
    final updated = await Navigator.of(context).push<RhythmLightScheduleConfig>(
      MaterialPageRoute(
        builder: (_) => _LightScheduleEditor(schedule: schedule),
      ),
    );
    if (updated == null || !mounted) return;
    final sync = context.read<ServerSyncProvider>();
    final schedules = List<RhythmLightScheduleConfig>.from(sync.lightSchedules);
    final index = schedules.indexWhere((value) => value.id == updated.id);
    if (index == -1) {
      schedules.add(updated);
    } else {
      schedules[index] = updated;
    }
    final journeyId = 'light-schedule-save-${const Uuid().v4()}';
    unawaited(AnalyticsService().logLightScheduleMutationAttempted(
      journeyId: journeyId,
      mutation: index == -1 ? 'create' : 'update',
      inputMethod: 'editor',
    ));
    final ok = await sync.saveLightSchedules(schedules);
    final trigger = updated.transitions.firstOrNull?.trigger ??
        const RhythmTransitionTrigger.manual();
    unawaited(AnalyticsService().logLightScheduleMutationCompleted(
      journeyId: journeyId,
      mutation: index == -1 ? 'create' : 'update',
      inputMethod: 'editor',
      triggerKind: trigger.kind,
      solarEvent: trigger.event ?? 'none',
      offsetDirection: _offsetDirection(trigger.offsetMinutes),
      outcome: ok ? 'succeeded' : 'failed',
      failureStage: ok ? null : 'appliance_ack',
    ));
    if (!mounted || ok) return;
    ScaffoldMessenger.of(context).showSnackBar(
      const SnackBar(content: Text('Could not save schedules. Try again.')),
    );
  }

  Future<void> _duplicate(RhythmLightScheduleConfig schedule) async {
    final copy = RhythmLightScheduleConfig(
      id: '${schedule.id}-copy',
      name: '${schedule.name} Copy',
      enabled: schedule.enabled,
      activeMode: schedule.activeMode,
      transitions: schedule.transitions,
    );
    await _edit(copy);
  }

  Future<void> _delete(RhythmLightScheduleConfig schedule) async {
    final sync = context.read<ServerSyncProvider>();
    final next = sync.lightSchedules
        .where((value) => value.id != schedule.id)
        .toList(growable: false);
    final journeyId = 'light-schedule-delete-${const Uuid().v4()}';
    unawaited(AnalyticsService().logLightScheduleMutationAttempted(
      journeyId: journeyId,
      mutation: 'delete',
      inputMethod: 'menu',
    ));
    final ok = await sync.saveLightSchedules(next);
    unawaited(AnalyticsService().logLightScheduleMutationCompleted(
      journeyId: journeyId,
      mutation: 'delete',
      inputMethod: 'menu',
      triggerKind: 'none',
      solarEvent: 'none',
      offsetDirection: 'exact',
      outcome: ok ? 'succeeded' : 'failed',
      failureStage: ok ? null : 'appliance_ack',
    ));
    if (!mounted || ok) return;
    ScaffoldMessenger.of(context).showSnackBar(
      const SnackBar(
        content: Text(
          'This schedule is still assigned or customized. Reset those rooms first.',
        ),
      ),
    );
  }

  static String _offsetDirection(int offset) => switch (offset.compareTo(0)) {
        < 0 => 'before',
        > 0 => 'after',
        _ => 'exact',
      };

  @override
  Widget build(BuildContext context) {
    final sync = context.watch<ServerSyncProvider>();
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      appBar: AppBar(
        backgroundColor: CelestialColors.backgroundDark,
        foregroundColor: CelestialColors.textPrimary,
        title: const Text('Schedules'),
      ),
      floatingActionButton: FloatingActionButton.extended(
        key: const ValueKey('light-schedules-create'),
        onPressed: sync.lightSchedulesSavePending ? null : () => _edit(null),
        icon: const Icon(Icons.add_rounded),
        label: const Text('New schedule'),
      ),
      body: sync.lightSchedulesLoading && sync.lightSchedules.isEmpty
          ? const Center(child: CircularProgressIndicator())
          : ListView(
              padding: const EdgeInsets.fromLTRB(20, 16, 20, 104),
              children: [
                Text(
                  'Reuse one Day and Sleep policy across rooms. Room customizations keep following every field they do not override.',
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: .9),
                    height: 1.4,
                  ),
                ),
                const SizedBox(height: 18),
                if (sync.lightSchedules.isEmpty)
                  const _EmptySchedules()
                else
                  SettingsGroup(
                    children: [
                      for (final schedule in sync.lightSchedules)
                        SettingsRow(
                          key: ValueKey('light-schedule-${schedule.id}'),
                          icon: schedule.enabled
                              ? Icons.schedule_rounded
                              : Icons.schedule_outlined,
                          iconColor: schedule.enabled
                              ? CelestialColors.accentBlue
                              : CelestialColors.textSecondary,
                          label: schedule.name,
                          value: '${schedule.enabled ? _summary(schedule) : 'Dormant'} · '
                              '${_usageSummary(sync, schedule)}',
                          onTap: () => _edit(schedule),
                          trailing: PopupMenuButton<String>(
                            tooltip: 'Schedule actions',
                            onSelected: (value) {
                              if (value == 'duplicate') {
                                unawaited(_duplicate(schedule));
                              } else if (value == 'delete') {
                                unawaited(_delete(schedule));
                              }
                            },
                            itemBuilder: (_) => const [
                              PopupMenuItem(
                                value: 'duplicate',
                                child: Text('Duplicate'),
                              ),
                              PopupMenuItem(
                                value: 'delete',
                                child: Text('Delete'),
                              ),
                            ],
                          ),
                        ),
                    ],
                  ),
              ],
            ),
    );
  }

  static String _summary(RhythmLightScheduleConfig schedule) {
    final day = schedule.transitions
        .where((value) => value.toMode == RhythmMode.day)
        .firstOrNull;
    final sleep = schedule.transitions
        .where((value) => value.toMode == RhythmMode.sleep)
        .firstOrNull;
    return '${_triggerLabel(day?.trigger)} · ${_triggerLabel(sleep?.trigger)}';
  }

  static String _usageSummary(
    ServerSyncProvider sync,
    RhythmLightScheduleConfig schedule,
  ) {
    final targets = sync.helloNodes.where(
      (node) =>
          node.kind.isRoom ||
          (node.kind.isLightDevice && node.parentId == null),
    );
    final assigned = targets
        .where(
          (node) => node.profileSettings?.lightScheduleId == schedule.id,
        )
        .length;
    final customized = targets
        .where(
          (node) =>
              node.profileSettings
                  ?.lightScheduleOverrides[schedule.id]
                  ?.isEmpty ==
              false,
        )
        .length;
    return '$assigned assigned · $customized customized';
  }

  static String _triggerLabel(RhythmTransitionTrigger? trigger) {
    if (trigger == null || trigger.isManual) return 'Manual';
    if (trigger.isScheduled) return trigger.time ?? 'Fixed time';
    final event = (trigger.event ?? 'Solar').replaceAll('_', ' ');
    final offset = trigger.offsetMinutes;
    if (offset == 0) return event;
    return '${offset.abs()}m ${offset < 0 ? 'before' : 'after'} $event';
  }
}

class _EmptySchedules extends StatelessWidget {
  const _EmptySchedules();

  @override
  Widget build(BuildContext context) => Container(
        padding: const EdgeInsets.all(24),
        decoration: BoxDecoration(
          color: Colors.white.withValues(alpha: .04),
          borderRadius: BorderRadius.circular(16),
        ),
        child: const Column(
          children: [
            Icon(Icons.schedule_rounded, color: CelestialColors.textSecondary),
            SizedBox(height: 10),
            Text(
              'No named schedules yet',
              style: TextStyle(
                color: CelestialColors.textPrimary,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
        ),
      );
}

class _LightScheduleEditor extends StatefulWidget {
  const _LightScheduleEditor({this.schedule});

  final RhythmLightScheduleConfig? schedule;

  @override
  State<_LightScheduleEditor> createState() => _LightScheduleEditorState();
}

class _LightScheduleEditorState extends State<_LightScheduleEditor> {
  late final TextEditingController _name;
  late bool _enabled;
  late RhythmModeTransitionConfig _day;
  late RhythmModeTransitionConfig _sleep;

  @override
  void initState() {
    super.initState();
    final schedule = widget.schedule;
    _name = TextEditingController(text: schedule?.name ?? '');
    _enabled = schedule?.enabled ?? true;
    _day = _transitionFor(schedule, RhythmMode.day);
    _sleep = _transitionFor(schedule, RhythmMode.sleep);
  }

  static RhythmModeTransitionConfig _transitionFor(
    RhythmLightScheduleConfig? schedule,
    RhythmMode target,
  ) {
    final preferredId = target == RhythmMode.day ? 'day_start' : 'sleep_start';
    final candidates = schedule?.transitions
            .where((value) =>
                value.toMode == target && !value.trigger.isManual)
            .toList(growable: false) ??
        const <RhythmModeTransitionConfig>[];
    final preferred = candidates
        .where((value) => value.id == preferredId)
        .firstOrNull;
    final existing =
        preferred ?? (candidates.length == 1 ? candidates.single : null);
    if (existing != null) return existing;
    final isDay = target == RhythmMode.day;
    return RhythmModeTransitionConfig(
      id: isDay ? 'day_start' : 'sleep_start',
      label: isDay ? 'Day Start' : 'Sleep Start',
      fromMode: isDay ? RhythmMode.sleep : RhythmMode.day,
      toMode: target,
      trigger: RhythmTransitionTrigger.solar(isDay ? 'sunrise' : 'sunset'),
      triggerEnabled: true,
      duration: const TransitionDuration.auto(),
      preserveHardOff: true,
    );
  }

  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  void _save() {
    final name = _name.text.trim();
    if (name.isEmpty) return;
    final id = widget.schedule?.id ?? _slug(name);
    final editedIds = {_day.id, _sleep.id};
    Navigator.of(context).pop(
      RhythmLightScheduleConfig(
        id: id,
        name: name,
        enabled: _enabled,
        activeMode: widget.schedule?.activeMode ?? RhythmMode.day,
        transitions: [
          for (final transition in widget.schedule?.transitions ??
              const <RhythmModeTransitionConfig>[])
            if (!editedIds.contains(transition.id)) transition,
          _day,
          _sleep,
        ],
      ),
    );
  }

  static String _slug(String value) {
    final slug = value
        .toLowerCase()
        .replaceAll(RegExp('[^a-z0-9]+'), '-')
        .replaceAll(RegExp(r'^-|-$'), '');
    return slug.isEmpty
        ? 'schedule-${const Uuid().v4().substring(0, 8)}'
        : slug;
  }

  RhythmModeTransitionConfig? _savedTransition(
    RhythmModeTransitionConfig transition,
  ) => widget.schedule?.transitions
      .where((saved) => saved.id == transition.id)
      .firstOrNull;

  bool _triggerMatchesSaved(RhythmModeTransitionConfig transition) {
    final saved = _savedTransition(transition);
    if (saved == null) return false;
    return saved.trigger.kind == transition.trigger.kind &&
        saved.trigger.event == transition.trigger.event &&
        saved.trigger.time == transition.trigger.time &&
        saved.trigger.offsetMinutes == transition.trigger.offsetMinutes;
  }

  String? _resolvedTime(
    ServerSyncProvider sync,
    RhythmModeTransitionConfig transition,
  ) {
    final schedule = widget.schedule;
    if (schedule == null || !_triggerMatchesSaved(transition)) return null;
    return sync.resolvedBaseLightScheduleTransitionLocalTime(
      schedule,
      transition,
    );
  }

  @override
  Widget build(BuildContext context) {
    final sync = context.watch<ServerSyncProvider>();
    final solarAvailable = sync.solarScheduleAnchorsAvailable;
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      appBar: AppBar(
        backgroundColor: CelestialColors.backgroundDark,
        foregroundColor: CelestialColors.textPrimary,
        title: Text(widget.schedule == null ? 'New schedule' : 'Edit schedule'),
        actions: [
          TextButton(onPressed: _save, child: const Text('Save')),
        ],
      ),
      body: ListView(
        padding: const EdgeInsets.all(20),
        children: [
          TextField(
            key: const ValueKey('light-schedule-name'),
            controller: _name,
            style: const TextStyle(color: CelestialColors.textPrimary),
            decoration: const InputDecoration(
              labelText: 'Name',
              labelStyle: TextStyle(color: CelestialColors.textSecondary),
            ),
          ),
          SwitchListTile.adaptive(
            title: const Text(
              'Automatic transitions',
              style: TextStyle(color: CelestialColors.textPrimary),
            ),
            value: _enabled,
            onChanged: (value) => setState(() => _enabled = value),
          ),
          const SizedBox(height: 16),
          _TransitionEditor(
            title: 'Day Start',
            transition: _day,
            dayBoundary: true,
            solarAvailable: solarAvailable,
            solarOffsetsSupported: sync.lightScheduleSolarOffsetsSupported,
            resolvedLocalTime: _resolvedTime(sync, _day),
            resolutionPending: !_triggerMatchesSaved(_day),
            onChanged: (value) => setState(() => _day = value),
          ),
          const SizedBox(height: 16),
          _TransitionEditor(
            title: 'Sleep Start',
            transition: _sleep,
            dayBoundary: false,
            solarAvailable: solarAvailable,
            solarOffsetsSupported: sync.lightScheduleSolarOffsetsSupported,
            resolvedLocalTime: _resolvedTime(sync, _sleep),
            resolutionPending: !_triggerMatchesSaved(_sleep),
            onChanged: (value) => setState(() => _sleep = value),
          ),
        ],
      ),
    );
  }
}

class _TransitionEditor extends StatelessWidget {
  const _TransitionEditor({
    required this.title,
    required this.transition,
    required this.dayBoundary,
    required this.solarAvailable,
    required this.solarOffsetsSupported,
    required this.resolvedLocalTime,
    required this.resolutionPending,
    required this.onChanged,
  });

  final String title;
  final RhythmModeTransitionConfig transition;
  final bool dayBoundary;
  final bool solarAvailable;
  final bool solarOffsetsSupported;
  final String? resolvedLocalTime;
  final bool resolutionPending;
  final ValueChanged<RhythmModeTransitionConfig> onChanged;

  static const _events = [
    'sunrise',
    'sunset',
    'civil_twilight',
    'nautical_twilight',
    'astronomical_twilight',
  ];

  @override
  Widget build(BuildContext context) {
    final trigger = transition.trigger;
    final type = trigger.isScheduled ? 'scheduled' : 'solar';
    final dropdownTextStyle = Theme.of(context)
        .textTheme
        .titleMedium
        ?.copyWith(color: CelestialColors.textPrimary);
    return Material(
      color: Colors.white.withValues(alpha: .04),
      borderRadius: BorderRadius.circular(16),
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
          Text(
            title,
            style: const TextStyle(
              color: CelestialColors.textPrimary,
              fontWeight: FontWeight.w700,
            ),
          ),
          SwitchListTile.adaptive(
            contentPadding: EdgeInsets.zero,
            title: const Text(
              'Enabled',
              style: TextStyle(color: CelestialColors.textPrimary),
            ),
            value: transition.triggerEnabled,
            onChanged: (value) => onChanged(
              transition.copyWith(triggerEnabled: value),
            ),
          ),
          DropdownButtonFormField<String>(
            initialValue: type,
            dropdownColor: CelestialColors.backgroundDark,
            style: dropdownTextStyle,
            decoration: const InputDecoration(
              labelText: 'Trigger',
              labelStyle: TextStyle(color: CelestialColors.textSecondary),
            ),
            items: const [
              DropdownMenuItem(value: 'solar', child: Text('Solar event')),
              DropdownMenuItem(value: 'scheduled', child: Text('Fixed time')),
            ],
            onChanged: (value) {
              final next = value == 'scheduled'
                  ? const RhythmTransitionTrigger.scheduled('07:00')
                  : RhythmTransitionTrigger.solar(
                      dayBoundary ? 'sunrise' : 'sunset',
                    );
              onChanged(transition.copyWith(trigger: next));
            },
          ),
          const SizedBox(height: 12),
          if (trigger.isSolar) ...[
            if (!solarAvailable)
              const Padding(
                padding: EdgeInsets.only(bottom: 8),
                child: Text(
                  'Solar anchors are unavailable until this home has a location and timezone. This rule remains saved but will not run.',
                  style: TextStyle(color: CelestialColors.sunWarm),
                ),
              ),
            DropdownButtonFormField<String>(
              isExpanded: true,
              initialValue: _events.contains(trigger.event)
                  ? trigger.event
                  : (dayBoundary ? 'sunrise' : 'sunset'),
              dropdownColor: CelestialColors.backgroundDark,
              style: dropdownTextStyle,
              decoration: const InputDecoration(
                labelText: 'Solar anchor',
                labelStyle: TextStyle(color: CelestialColors.textSecondary),
              ),
              items: [
                for (final event in _events)
                  DropdownMenuItem(
                    value: event,
                    child: Text(event.replaceAll('_', ' ')),
                  ),
              ],
              onChanged: solarAvailable
                  ? (event) {
                      if (event == null) return;
                      onChanged(transition.copyWith(
                        trigger: RhythmTransitionTrigger.solar(
                          event,
                          offsetMinutes: trigger.offsetMinutes,
                        ),
                      ));
                    }
                  : null,
            ),
            if (solarOffsetsSupported) ...[
              const SizedBox(height: 8),
              Text(
                trigger.offsetMinutes == 0
                    ? 'At the solar event'
                    : '${trigger.offsetMinutes.abs()} minutes ${trigger.offsetMinutes < 0 ? 'before' : 'after'}',
                style: const TextStyle(color: CelestialColors.textSecondary),
              ),
            ] else
              const Padding(
                padding: EdgeInsets.only(top: 8),
                child: Text(
                  'Solar offset editing requires updated Rhythm Box software.',
                  key: ValueKey('schedule-offset-unsupported'),
                  style: TextStyle(color: CelestialColors.textSecondary),
                ),
              ),
            const SizedBox(height: 4),
            Text(
              resolutionPending
                  ? 'Save to resolve on the appliance'
                  : resolvedLocalTime == null
                      ? 'Temporarily unavailable today'
                  : 'Today · $resolvedLocalTime local',
              key: ValueKey('schedule-resolved-time-$title'),
              style: TextStyle(
                color: resolvedLocalTime == null
                    ? CelestialColors.sunWarm
                    : CelestialColors.textSecondary,
              ),
            ),
            if (solarOffsetsSupported)
              Slider(
                key: ValueKey('schedule-offset-$title'),
                min: -maxSolarScheduleOffsetMinutes.toDouble(),
                max: maxSolarScheduleOffsetMinutes.toDouble(),
                divisions: maxSolarScheduleOffsetMinutes * 2,
                value: trigger.offsetMinutes
                    .clamp(
                      -maxSolarScheduleOffsetMinutes,
                      maxSolarScheduleOffsetMinutes,
                    )
                    .toDouble(),
                label: '${trigger.offsetMinutes} min',
                onChanged: solarAvailable
                    ? (value) => onChanged(
                          transition.copyWith(
                            trigger: RhythmTransitionTrigger.solar(
                              trigger.event ??
                                  (dayBoundary ? 'sunrise' : 'sunset'),
                              offsetMinutes: value.round(),
                            ),
                          ),
                        )
                    : null,
              ),
          ] else
            TextFormField(
              key: ValueKey('schedule-fixed-time-$title'),
              initialValue: trigger.time ?? '07:00',
              style: const TextStyle(color: CelestialColors.textPrimary),
              decoration: const InputDecoration(
                labelText: 'Local time (HH:MM)',
                labelStyle: TextStyle(color: CelestialColors.textSecondary),
              ),
              onChanged: (value) => onChanged(
                transition.copyWith(
                  trigger: RhythmTransitionTrigger.scheduled(value),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
