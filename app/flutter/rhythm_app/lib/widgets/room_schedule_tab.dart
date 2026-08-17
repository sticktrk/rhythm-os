import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../providers/server_sync_provider.dart';
import '../services/analytics_service.dart';
import 'room_schedule_behavior_control.dart';
import 'solar_orbit.dart';

/// Capability-gated, room-scoped schedule editor.
class RoomScheduleTab extends StatefulWidget {
  const RoomScheduleTab({super.key, required this.roomId});

  final String roomId;

  @override
  State<RoomScheduleTab> createState() => _RoomScheduleTabState();
}

class _RoomScheduleTabState extends State<RoomScheduleTab> {
  String? _failure;
  RhythmMode? _testing;

  @override
  void initState() {
    super.initState();
    unawaited(
        AnalyticsService().logRoomScheduleOpened(source: 'room_settings'));
  }

  Future<void> _save(
    RhythmRoomSchedule schedule, {
    required String changeKind,
  }) async {
    setState(() => _failure = null);
    unawaited(AnalyticsService().logRoomScheduleSaveAttempted(
      changeKind: changeKind,
      source: schedule.source.wireValue,
    ));
    final ok = await context.read<ServerSyncProvider>().setRoomSchedule(
          widget.roomId,
          schedule,
        );
    unawaited(AnalyticsService().logRoomScheduleSaveCompleted(
      changeKind: changeKind,
      source: schedule.source.wireValue,
      outcome: ok ? 'success' : 'failure',
      failureStage: ok ? null : 'appliance_ack',
    ));
    if (mounted && !ok) setState(() => _failure = 'Could not save. Try again.');
  }

  Future<void> _test(RhythmMode mode) async {
    setState(() {
      _testing = mode;
      _failure = null;
    });
    final ok = await context
        .read<ServerSyncProvider>()
        .testRoomSchedule(widget.roomId, mode);
    unawaited(AnalyticsService().logRoomScheduleTestCompleted(
      action: mode == RhythmMode.day ? 'wake' : 'sleep',
      outcome: ok ? 'success' : 'failure',
      failureStage: ok ? null : 'output_apply',
    ));
    if (!mounted) return;
    setState(() {
      _testing = null;
      _failure = ok ? null : 'Test failed. Check the room and try again.';
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
    final schedule = sync.scheduleForRoom(widget.roomId);
    final followTime = schedule.source == RhythmRoomScheduleSource.followTime;
    final saving = sync.roomSchedulePendingForRoom(widget.roomId);
    final testing = sync.roomScheduleTestPendingForRoom(widget.roomId);

    return ListView(
      key: const ValueKey('schedule'),
      padding: const EdgeInsets.symmetric(horizontal: 20),
      children: [
        _Section(
          title: 'Alarm / schedule source',
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Wrap(
                spacing: 8,
                children: [
                  ChoiceChip(
                    key: const ValueKey('room-schedule-source-presets'),
                    label: const Text('Wake/Sleep presets'),
                    selected: !followTime,
                    onSelected: saving
                        ? null
                        : (_) => _save(
                              schedule.copyWith(
                                source:
                                    RhythmRoomScheduleSource.wakeSleepPresets,
                              ),
                              changeKind: 'source',
                            ),
                  ),
                  ChoiceChip(
                    key: const ValueKey('room-schedule-source-follow-time'),
                    label: const Text('Follow time'),
                    selected: followTime,
                    onSelected: saving
                        ? null
                        : (_) => _save(
                              schedule.copyWith(
                                source: RhythmRoomScheduleSource.followTime,
                              ),
                              changeKind: 'source',
                            ),
                  ),
                ],
              ),
              const SizedBox(height: 10),
              Text(
                followTime
                    ? 'Only this room follows the times below. Home mode stays unchanged.'
                    : 'This room follows the home Wake and Sleep schedule.',
                style: const TextStyle(color: CelestialColors.textSecondary),
              ),
              const SizedBox(height: 14),
              _RoomTimeDial(
                wakeTime: schedule.wakeTime,
                sleepTime: schedule.sleepTime,
                enabled: followTime && !saving,
                onChanged: (wake, sleep) => _save(
                  schedule.copyWith(wakeTime: wake, sleepTime: sleep),
                  changeKind: 'times',
                ),
              ),
              if (saving)
                const Padding(
                  padding: EdgeInsets.only(top: 10),
                  child: LinearProgressIndicator(
                    key: ValueKey('room-schedule-save-pending'),
                  ),
                ),
            ],
          ),
        ),
        const SizedBox(height: 16),
        _Section(
          title: 'Wake / Sleep presets',
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              RoomScheduleBehaviorControl(
                roomId: widget.roomId,
                foregroundColor: CelestialColors.textPrimary,
                enabled: !followTime && !saving,
                showTopDivider: false,
                keyPrefix: 'room-schedule-presets',
                analyticsSource: 'room_schedule_tab',
              ),
              if (followTime)
                const Padding(
                  padding: EdgeInsets.only(top: 8),
                  child: Text(
                    'Saved choices are not used while Follow time is selected.',
                    key: ValueKey('room-schedule-presets-disabled'),
                    style: TextStyle(color: CelestialColors.textSecondary),
                  ),
                ),
            ],
          ),
        ),
        const SizedBox(height: 16),
        _Section(
          title: 'Test your presets',
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              if (followTime) ...[
                const Text(
                  'Tests use your saved presets; the live schedule still follows time.',
                  key: ValueKey('room-schedule-test-follow-time-help'),
                  style: TextStyle(color: CelestialColors.textSecondary),
                ),
                const SizedBox(height: 10),
              ],
              Row(
                children: [
                  Expanded(
                    child: OutlinedButton.icon(
                      key: const ValueKey('room-schedule-test-wake'),
                      onPressed: testing ? null : () => _test(RhythmMode.day),
                      icon: _testing == RhythmMode.day
                          ? const SizedBox.square(
                              dimension: 16,
                              child: CircularProgressIndicator(strokeWidth: 2),
                            )
                          : const Icon(Icons.wb_sunny_outlined),
                      label: const Text('Test Wake'),
                    ),
                  ),
                  const SizedBox(width: 10),
                  Expanded(
                    child: OutlinedButton.icon(
                      key: const ValueKey('room-schedule-test-sleep'),
                      onPressed: testing ? null : () => _test(RhythmMode.sleep),
                      icon: _testing == RhythmMode.sleep
                          ? const SizedBox.square(
                              dimension: 16,
                              child: CircularProgressIndicator(strokeWidth: 2),
                            )
                          : const Icon(Icons.bedtime_outlined),
                      label: const Text('Test Sleep'),
                    ),
                  ),
                ],
              ),
            ],
          ),
        ),
        if (_failure != null)
          Padding(
            padding: const EdgeInsets.only(top: 12),
            child: Text(
              _failure!,
              key: const ValueKey('room-schedule-failure'),
              style: const TextStyle(color: Colors.redAccent),
            ),
          ),
        const SizedBox(height: 16),
      ],
    );
  }
}

class _Section extends StatelessWidget {
  const _Section({required this.title, required this.child});
  final String title;
  final Widget child;

  @override
  Widget build(BuildContext context) => Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.only(left: 4, bottom: 8),
            child: Text(
              title.toUpperCase(),
              style: const TextStyle(
                color: CelestialColors.textSecondary,
                fontSize: 11,
                fontWeight: FontWeight.w600,
                letterSpacing: 1.4,
              ),
            ),
          ),
          Container(
            padding: const EdgeInsets.all(14),
            decoration: BoxDecoration(
              color: CelestialColors.backgroundDark.withValues(alpha: 0.5),
              borderRadius: BorderRadius.circular(14),
              border: Border.all(
                  color: CelestialColors.orbitRing.withValues(alpha: 0.3)),
            ),
            child: child,
          ),
        ],
      );
}

class _RoomTimeDial extends StatefulWidget {
  const _RoomTimeDial({
    required this.wakeTime,
    required this.sleepTime,
    required this.enabled,
    required this.onChanged,
  });
  final String wakeTime;
  final String sleepTime;
  final bool enabled;
  final void Function(String wakeTime, String sleepTime) onChanged;

  @override
  State<_RoomTimeDial> createState() => _RoomTimeDialState();
}

class _RoomTimeDialState extends State<_RoomTimeDial> {
  late int wake = _parse(widget.wakeTime);
  late int sleep = _parse(widget.sleepTime);
  bool draggingWake = true;

  static int _parse(String value) {
    final parts = value.split(':');
    return int.parse(parts[0]) * 60 + int.parse(parts[1]);
  }

  String _display(int minute) {
    final hour = minute ~/ 60;
    final min = minute % 60;
    return '${hour.toString().padLeft(2, '0')}:${min.toString().padLeft(2, '0')}';
  }

  int _minuteFor(Offset point, Size size) {
    final center = size.center(Offset.zero);
    final angle =
        math.atan2(point.dy - center.dy, point.dx - center.dx) + math.pi / 2;
    final normalized = angle < 0 ? angle + math.pi * 2 : angle;
    return (((normalized / (math.pi * 2) * 1440) / 15).round() * 15) % 1440;
  }

  int _distance(int a, int b) => math.min((a - b).abs(), 1440 - (a - b).abs());

  void _start(DragStartDetails details, Size size) {
    final minute = _minuteFor(details.localPosition, size);
    draggingWake = _distance(minute, wake) <= _distance(minute, sleep);
  }

  void _update(DragUpdateDetails details, Size size) {
    final minute = _minuteFor(details.localPosition, size);
    setState(() => draggingWake ? wake = minute : sleep = minute);
  }

  @override
  Widget build(BuildContext context) {
    const size = Size.square(184);
    return Opacity(
      opacity: widget.enabled ? 1 : 0.48,
      child: Column(
        children: [
          Center(
            child: GestureDetector(
              key: const ValueKey('room-schedule-time-dial'),
              onPanStart: widget.enabled ? (d) => _start(d, size) : null,
              onPanUpdate: widget.enabled ? (d) => _update(d, size) : null,
              onPanEnd: widget.enabled
                  ? (_) => widget.onChanged(_display(wake), _display(sleep))
                  : null,
              child: CustomPaint(
                size: size,
                painter: _DialPainter(wake: wake, sleep: sleep),
                child: SizedBox.fromSize(
                  size: size,
                  child: const Center(
                    child: Text('24 HOUR',
                        style: TextStyle(
                            color: CelestialColors.textSecondary,
                            fontSize: 11)),
                  ),
                ),
              ),
            ),
          ),
          Wrap(
            alignment: WrapAlignment.center,
            crossAxisAlignment: WrapCrossAlignment.center,
            spacing: 8,
            runSpacing: 4,
            children: [
              const Icon(Icons.wb_sunny_rounded,
                  color: Color(0xFFF9A825), size: 16),
              const SizedBox(width: 5),
              Text('Wake ${_display(wake)}'),
              const SizedBox(width: 6),
              const Icon(Icons.bedtime_rounded,
                  color: Color(0xFF7C83FF), size: 16),
              const SizedBox(width: 5),
              Text('Sleep ${_display(sleep)}'),
            ],
          ),
        ],
      ),
    );
  }
}

class _DialPainter extends CustomPainter {
  const _DialPainter({required this.wake, required this.sleep});
  final int wake;
  final int sleep;

  Offset _point(Size size, int minute) {
    final angle = minute / 1440 * math.pi * 2 - math.pi / 2;
    final radius = size.shortestSide / 2 - 13;
    return size.center(Offset.zero) +
        Offset(math.cos(angle), math.sin(angle)) * radius;
  }

  @override
  void paint(Canvas canvas, Size size) {
    final rect = Offset.zero & size;
    canvas.drawArc(
      rect.deflate(13),
      -math.pi / 2,
      math.pi * 2,
      false,
      Paint()
        ..color = CelestialColors.orbitRing.withValues(alpha: 0.35)
        ..style = PaintingStyle.stroke
        ..strokeWidth = 8,
    );
    canvas.drawCircle(
        _point(size, wake), 10, Paint()..color = const Color(0xFFF9A825));
    canvas.drawCircle(
        _point(size, sleep), 10, Paint()..color = const Color(0xFF7C83FF));
  }

  @override
  bool shouldRepaint(_DialPainter oldDelegate) =>
      oldDelegate.wake != wake || oldDelegate.sleep != sleep;
}
