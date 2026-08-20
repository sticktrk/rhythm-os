import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart' show SolarUtils;
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:uuid/uuid.dart';

import '../providers/home_provider.dart';
import '../providers/server_sync_provider.dart';
import '../services/analytics_service.dart';
import 'automation_section_header.dart';
import 'celestial_segmented_control.dart';
import 'mode_summary_chip.dart';
import 'rhythm_clock/rhythm_clock_visuals.dart';
import 'rhythm_clock/rhythm_schedule_clock.dart';
import 'room_schedule_behavior_control.dart';
import 'solar_clock/solar_clock_exports.dart';
import 'solar_orbit.dart';

// Mode accents shared with the whole-house Presets screen (Wake/Sleep rows,
// manual toggle) and the room-card behavior control.
const Color _wakeAccent = Color(0xFFF9A825);
const Color _sleepAccent = Color(0xFF7C83FF);
// Step accents mirroring the Presets screen's 1-2-3 sequence.
const Color _scheduleAccent = CelestialColors.accentBlue;
const Color _testAccent = Color(0xFF9C8CFF);

/// Capability-gated, room-scoped schedule editor.
///
/// Deliberately shares its visual grammar with the whole-house Presets screen
/// (numbered step headers, segmented toggles, mode summary chips) so the
/// per-room schedule reads as the same feature at room scope.
class RoomScheduleTab extends StatefulWidget {
  const RoomScheduleTab({super.key, required this.roomId});

  final String roomId;

  @override
  State<RoomScheduleTab> createState() => _RoomScheduleTabState();
}

/// Where a failure banner should surface — under the card whose action failed.
enum _FailureScope { save, test }

class _RoomScheduleTabState extends State<RoomScheduleTab> {
  final Uuid _uuid = const Uuid();
  String? _failure;
  _FailureScope _failureScope = _FailureScope.save;
  RhythmMode? _testing;
  String? _saveJourneyId;
  String? _failedSaveSignature;
  int _saveAttemptNumber = 0;
  String? _testJourneyId;
  RhythmMode? _failedTestMode;
  int _testAttemptNumber = 0;

  @override
  void initState() {
    super.initState();
    unawaited(
        AnalyticsService().logRoomScheduleOpened(source: 'room_settings'));
  }

  Future<void> _save(
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
      setState(() {
        _failure = 'Could not save. Try again.';
        _failureScope = _FailureScope.save;
      });
    }
  }

  Future<void> _test(RhythmMode mode) async {
    if (_failedTestMode == mode && _testJourneyId != null) {
      _testAttemptNumber += 1;
    } else {
      _testJourneyId = 'room-schedule-test-${_uuid.v4()}';
      _testAttemptNumber = 1;
    }
    final journeyId = _testJourneyId!;
    final attemptNumber = _testAttemptNumber;
    final source = context
        .read<ServerSyncProvider>()
        .scheduleForRoom(widget.roomId)
        .source
        .wireValue;
    setState(() {
      _testing = mode;
      _failure = null;
    });
    final ok = await context
        .read<ServerSyncProvider>()
        .testRoomSchedule(widget.roomId, mode, requestId: journeyId);
    unawaited(AnalyticsService().logRoomScheduleTestCompleted(
      journeyId: journeyId,
      attemptNumber: attemptNumber,
      inputMethod: 'button',
      source: source,
      action: mode == RhythmMode.day ? 'wake' : 'sleep',
      outcome: ok ? 'succeeded' : 'failed',
      failureStage: ok ? null : 'output_apply',
    ));
    if (ok) {
      _testJourneyId = null;
      _failedTestMode = null;
      _testAttemptNumber = 0;
    } else {
      _failedTestMode = mode;
    }
    if (!mounted) return;
    setState(() {
      _testing = null;
      _failure = ok ? null : 'Test failed. Check the room and try again.';
      if (!ok) _failureScope = _FailureScope.test;
    });
    if (ok) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
            content: Text(
                '${mode == RhythmMode.day ? 'Wake' : 'Sleep'} test applied to this room')),
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    final sync = context.watch<ServerSyncProvider>();
    if (!sync.roomScheduleSupportedForNode(widget.roomId)) {
      return ListView(
        key: const ValueKey('schedule'),
        padding: const EdgeInsets.symmetric(horizontal: 20),
        children: const [
          SizedBox(height: 30),
          _GroupCard(
            padding: EdgeInsets.symmetric(horizontal: 20, vertical: 24),
            child: Column(
              children: [
                Icon(Icons.system_update_alt,
                    color: CelestialColors.sunWarm, size: 32),
                SizedBox(height: 10),
                Text(
                  'Update required',
                  key: ValueKey('room-schedule-update-required'),
                  style: TextStyle(
                    color: CelestialColors.textPrimary,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                SizedBox(height: 6),
                Text(
                  'Update this Rhythm appliance to set a schedule for this room.',
                  textAlign: TextAlign.center,
                  style: TextStyle(color: CelestialColors.textSecondary),
                ),
              ],
            ),
          ),
        ],
      );
    }
    final schedule = sync.scheduleForRoom(widget.roomId);
    final followTime = schedule.source == RhythmRoomScheduleSource.followTime;
    final saving = sync.roomSchedulePendingForRoom(widget.roomId);
    final testing = sync.roomScheduleTestPendingForRoom(widget.roomId);
    final activeMode = sync.activeMode;

    return ListView(
      key: const ValueKey('schedule'),
      padding: const EdgeInsets.symmetric(horizontal: 20),
      children: [
        const AutomationSectionHeader(
          step: 1,
          title: 'Schedule',
          subtitle: 'Choose what sets this room’s Wake and Sleep times.',
          accent: _scheduleAccent,
        ),
        _GroupCard(
          padding: const EdgeInsets.all(12),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              _SourceToggle(
                followTime: followTime,
                enabled: !saving,
                onChanged: (useCustomTimes) => _save(
                  schedule.copyWith(
                    source: useCustomTimes
                        ? RhythmRoomScheduleSource.followTime
                        : RhythmRoomScheduleSource.wakeSleepPresets,
                  ),
                  changeKind: 'source',
                  inputMethod: 'choice_chip',
                ),
              ),
              const SizedBox(height: 10),
              Padding(
                padding: const EdgeInsets.symmetric(horizontal: 4),
                child: Text(
                  followTime
                      ? 'Only this room follows the times below. The rest of the home is unchanged.'
                      : 'This room wakes and sleeps with the whole-home Alarm schedule.',
                  style: TextStyle(
                    color:
                        CelestialColors.textSecondary.withValues(alpha: 0.85),
                    fontSize: 12.5,
                    height: 1.35,
                  ),
                ),
              ),
              AnimatedSize(
                duration: const Duration(milliseconds: 220),
                curve: Curves.easeOutCubic,
                alignment: Alignment.topCenter,
                child: followTime
                    ? Padding(
                        padding: const EdgeInsets.only(top: 14),
                        child: _RoomTimeEditor(
                          wakeTime: schedule.wakeTime,
                          sleepTime: schedule.sleepTime,
                          enabled: !saving,
                          onChanged: (wake, sleep, inputMethod) => _save(
                            schedule.copyWith(wakeTime: wake, sleepTime: sleep),
                            changeKind: 'times',
                            inputMethod: inputMethod,
                          ),
                        ),
                      )
                    : const SizedBox(width: double.infinity, height: 0),
              ),
              AnimatedSize(
                duration: const Duration(milliseconds: 180),
                alignment: Alignment.topCenter,
                child: saving
                    ? Padding(
                        key: const ValueKey('room-schedule-save-pending'),
                        padding: const EdgeInsets.only(top: 12),
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
              if (_failure != null && _failureScope == _FailureScope.save)
                Padding(
                  padding: const EdgeInsets.only(top: 12),
                  child: _FailureBanner(message: _failure!),
                ),
            ],
          ),
        ),
        const AutomationSectionHeader(
          step: 2,
          title: 'Wake / Sleep Presets',
          subtitle: 'Set what this room does once Wake or Sleep is triggered.',
          accent: CelestialColors.sunWarm,
        ),
        _GroupCard(
          padding: const EdgeInsets.all(12),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              RoomScheduleBehaviorSegments(
                roomId: widget.roomId,
                enabled: !followTime && !saving,
              ),
              if (followTime)
                Padding(
                  padding: const EdgeInsets.only(top: 10, left: 4, right: 4),
                  child: Text(
                    'Saved choices are not used while Custom times is selected.',
                    key: const ValueKey('room-schedule-presets-disabled'),
                    style: TextStyle(
                      color:
                          CelestialColors.textSecondary.withValues(alpha: 0.85),
                      fontSize: 12.5,
                      height: 1.35,
                    ),
                  ),
                ),
            ],
          ),
        ),
        const AutomationSectionHeader(
          step: 3,
          title: 'Test your presets',
          subtitle: 'Preview this room’s Wake or Sleep preset now.',
          accent: _testAccent,
        ),
        _GroupCard(
          padding: const EdgeInsets.all(12),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              if (followTime)
                Padding(
                  padding: const EdgeInsets.only(bottom: 10, left: 4, right: 4),
                  child: Text(
                    'Tests use your saved presets; the live schedule still follows your custom times.',
                    key: const ValueKey('room-schedule-test-follow-time-help'),
                    style: TextStyle(
                      color:
                          CelestialColors.textSecondary.withValues(alpha: 0.85),
                      fontSize: 12.5,
                      height: 1.35,
                    ),
                  ),
                ),
              // One exclusive toggle, sharing the Presets screen's manual
              // Wake/Sleep control. The current mode reads as selected; a
              // running test swaps the spinner into that side's icon slot.
              CelestialSegmentedControl<RhythmMode>(
                segments: const [
                  CelestialSegment(
                    value: RhythmMode.day,
                    label: 'Test Wake',
                    icon: Icons.wb_sunny_rounded,
                    accent: _wakeAccent,
                    key: ValueKey('room-schedule-test-wake'),
                  ),
                  CelestialSegment(
                    value: RhythmMode.sleep,
                    label: 'Test Sleep',
                    icon: Icons.bedtime_rounded,
                    accent: _sleepAccent,
                    key: ValueKey('room-schedule-test-sleep'),
                  ),
                ],
                selected: activeMode,
                busyValue: _testing,
                onTap: _test,
                enabled: !testing,
                allowReselect: true,
                tintUnselectedIcons: true,
                mediumHaptic: true,
              ),
              if (_failure != null && _failureScope == _FailureScope.test)
                Padding(
                  padding: const EdgeInsets.fromLTRB(2, 8, 2, 2),
                  child: _FailureBanner(message: _failure!),
                ),
            ],
          ),
        ),
        const SizedBox(height: 24),
      ],
    );
  }
}

/// Inline error surface rendered under the card whose action failed, so the
/// message appears next to the control the user just touched.
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

/// The sheet's shared card surface — same geometry as the Light/Motion/Buttons
/// tabs' settings groups so all four tabs read as one surface.
class _GroupCard extends StatelessWidget {
  const _GroupCard({required this.child, required this.padding});

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

/// Two-segment source selector styled after the Presets screen's manual
/// Wake/Sleep toggle: recessed track, tinted active segment.
class _SourceToggle extends StatelessWidget {
  const _SourceToggle({
    required this.followTime,
    required this.enabled,
    required this.onChanged,
  });

  final bool followTime;
  final bool enabled;
  final ValueChanged<bool> onChanged;

  @override
  Widget build(BuildContext context) {
    return CelestialSegmentedControl<bool>(
      segments: const [
        CelestialSegment(
          value: false,
          label: 'Auto schedule',
          icon: Icons.alarm_rounded,
          accent: _scheduleAccent,
          key: ValueKey('room-schedule-source-presets'),
        ),
        CelestialSegment(
          value: true,
          label: 'Custom times',
          icon: Icons.schedule_rounded,
          accent: _scheduleAccent,
          key: ValueKey('room-schedule-source-follow-time'),
        ),
      ],
      selected: followTime,
      onTap: onChanged,
      enabled: enabled,
      // Re-tapping the selected source re-saves — a natural retry after a
      // failed write.
      allowReselect: true,
    );
  }
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
    required this.onChanged,
  });
  final String wakeTime;
  final String sleepTime;
  final bool enabled;
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

  // Memoized solar data + per-mode curve visuals — the solar/curve math is
  // native work that must not rerun on every drag-frame rebuild.
  String? _visualsKey;
  SolarClockData? _solarClockData;
  ModeCurveVisual _dayVisual =
      const ModeCurveVisual(fallbackColor: _wakeAccent);
  ModeCurveVisual _sleepVisual =
      const ModeCurveVisual(fallbackColor: _sleepAccent);

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
      fallbackColor: _wakeAccent,
      latitude: loc.latitude,
      longitude: loc.longitude,
      timezone: tz,
    );
    _sleepVisual = buildProfileCurveVisual(
      profile: sleepProfile,
      fallbackColor: _sleepAccent,
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
                    accent: _wakeAccent,
                    time: _display(_chipMinutes(RhythmMode.day)),
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
                    accent: _sleepAccent,
                    time: _display(_chipMinutes(RhythmMode.sleep)),
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
                  dayColor: _wakeAccent,
                  sleepColor: _sleepAccent,
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
    required this.time,
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
  final String time;
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
            child: Text(
              time,
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
