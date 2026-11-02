import 'rhythm_runtime.dart';

/// A button-reported interval in bed, not a measured sleep stage.
class RhythmSleepInterval {
  final String sleepEventId;
  final int startEpochMs;
  final int endEpochMs;

  const RhythmSleepInterval({
    required this.sleepEventId,
    required this.startEpochMs,
    required this.endEpochMs,
  });

  /// Project only explicit physical Sleep/Wake intent for one selected target.
  /// All events must belong to one authenticated server identity. Repeated Sleep
  /// presses preserve the first bedtime; an unmatched Wake never invents one.
  static List<RhythmSleepInterval> fromActivity(
    Iterable<RhythmActivityEvent> activity, {
    required String nodeId,
    required int nowEpochMs,
  }) {
    final byId = <String, RhythmActivityEvent>{};
    final conflicting = <String>{};
    for (final event in activity) {
      if (activityScope(event) != nodeId ||
          event.source.kind != 'switch' ||
          event.id.isEmpty ||
          event.epochMs <= 0 ||
          event.epochMs > nowEpochMs ||
          _signal(event) == null) {
        continue;
      }
      final previous = byId[event.id];
      if (previous != null &&
          (previous.epochMs != event.epochMs ||
              _signal(previous) != _signal(event))) {
        conflicting.add(event.id);
      }
      byId[event.id] = event;
    }
    // A conflicting retained event makes the snapshot ambiguous. Do not infer
    // across a removed boundary and accidentally create a longer interval.
    if (conflicting.isNotEmpty) return const [];
    final events = byId.values.toList()
      ..sort((a, b) => a.epochMs.compareTo(b.epochMs));
    final result = <RhythmSleepInterval>[];
    RhythmActivityEvent? sleep;
    for (var index = 0; index < events.length;) {
      final epochMs = events[index].epochMs;
      final group = <RhythmActivityEvent>[];
      while (index < events.length && events[index].epochMs == epochMs) {
        group.add(events[index++]);
      }
      if (group.map(_signal).toSet().length != 1) {
        sleep = null; // Equal-time Sleep/Wake ordering cannot be established.
        continue;
      }
      group.sort((a, b) => a.id.compareTo(b.id));
      final event = group.first;
      if (_signal(event) == 'sleep') {
        sleep ??= event;
      } else {
        if (sleep != null &&
            event.epochMs > sleep.epochMs &&
            event.epochMs - sleep.epochMs <=
                const Duration(hours: 24).inMilliseconds) {
          result.add(RhythmSleepInterval(
              sleepEventId: sleep.id,
              startEpochMs: sleep.epochMs,
              endEpochMs: event.epochMs));
        }
        sleep = null;
      }
    }
    return result;
  }

  static String? _signal(RhythmActivityEvent event) {
    if (event.payload is Map && (event.payload as Map)['status'] == 'failed') {
      return null;
    }
    switch (event.actionId) {
      case 'sleep_on':
        return 'sleep';
      case 'sleep_off':
        return 'wake';
      case 'set_mode':
      case 'trigger_transition':
        final payload = event.payload;
        if (payload is! Map) return null;
        return switch (payload['mode']) {
          'sleep' => 'sleep',
          'day' => 'wake',
          _ => null,
        };
      default:
        return null;
    }
  }

  /// Schedule bindings share the server's global activity target, but their
  /// explicit schedule identity must never mix with home-wide Sleep/Wake.
  static String activityScope(RhythmActivityEvent event) {
    final payload = event.payload;
    if (payload is Map && payload['automation_action'] is Map) {
      final action = payload['automation_action'] as Map;
      if (action['kind'] == 'light_schedule_mode_set' ||
          action['kind'] == 'light_schedule_mode_cycle') {
        final id = action['schedule_id'];
        return id is String && id.isNotEmpty ? 'schedule:$id' : '';
      }
    }
    return event.nodeId;
  }

  static String historyTarget(String scope) =>
      scope.startsWith('schedule:') ? 'global' : scope;
}
