import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

RhythmActivityEvent event(
  String id,
  String action,
  int at, {
  String node = 'bedroom',
  String source = 'switch',
  Object? payload,
}) =>
    RhythmActivityEvent(
        id: id,
        nodeId: node,
        actionId: action,
        epochMs: at,
        source: RhythmActivitySource(
            rawSource: 'button', kind: source, marksTouched: true),
        payload: payload);

void main() {
  const hour = 60 * 60 * 1000;
  const bedtime = 1770073200000;
  List<RhythmSleepInterval> project(List<RhythmActivityEvent> events,
          {String node = 'bedroom'}) =>
      RhythmSleepInterval.fromActivity(events,
          nodeId: node, nowEpochMs: bedtime + 48 * hour);

  test(
      'pairs reversed retained history across midnight; repeated presses preserve first bedtime',
      () {
    final sleep = event('sleep', 'sleep_on', bedtime);
    final intervals = project([
      event('wake-again', 'sleep_off', bedtime + 9 * hour),
      event('wake', 'sleep_off', bedtime + 8 * hour),
      sleep,
      sleep,
      event('repeat', 'sleep_on', bedtime + hour),
    ]);
    expect(intervals, hasLength(1));
    expect(intervals.single.sleepEventId, 'sleep');
    expect(intervals.single.startEpochMs, bedtime);
    expect(intervals.single.endEpochMs, bedtime + 8 * hour);
  });

  test(
      'global mode and transition buttons pair independently from named schedules',
      () {
    final events = [
      event('s1', 'set_mode', bedtime, node: 'global', payload: {
        'mode': 'sleep',
        'automation_action': {
          'kind': 'light_schedule_mode_set',
          'schedule_id': 'personal'
        }
      }),
      event('global-wake', 'set_mode', bedtime + 6 * hour,
          node: 'global', payload: {'mode': 'day'}),
      event('s2', 'trigger_transition', bedtime + 8 * hour,
          node: 'global',
          payload: {
            'mode': 'day',
            'automation_action': {
              'kind': 'light_schedule_mode_cycle',
              'schedule_id': 'personal'
            }
          }),
    ];
    expect(project(events, node: 'global'), isEmpty);
    expect(project(events, node: 'schedule:other'), isEmpty);
    expect(project(events, node: 'schedule:personal').single.endEpochMs,
        bedtime + 8 * hour);
    expect(RhythmSleepInterval.historyTarget('schedule:personal'), 'global');
    expect(
        project([
          event('sleep', 'set_mode', bedtime,
              node: 'global', payload: {'mode': 'sleep'}),
          event('wake', 'trigger_transition', bedtime + 8 * hour,
              node: 'global', payload: {'mode': 'day'}),
        ], node: 'global'),
        hasLength(1));
  });

  test(
      'excludes app/schedule/light power intent, other rooms and incomplete intervals',
      () {
    expect(
        project([
          event('missing-wake', 'sleep_off', bedtime),
          event('app', 'sleep_on', bedtime, source: 'app'),
          event('power', 'turn_off', bedtime),
          event('auto', 'room_schedule_boundary', bedtime,
              payload: {'target_mode': 'sleep'}),
          event('legacy', 'set_mode', bedtime, payload: {}),
          event('failed', 'sleep_on', bedtime, payload: {'status': 'failed'}),
          event('other', 'sleep_on', bedtime, node: 'other'),
          event('wake', 'sleep_off', bedtime + 8 * hour),
          event('open', 'sleep_on', bedtime + 10 * hour),
        ]),
        isEmpty);
  });

  test('rejects future, zero, overlong and ambiguous intervals', () {
    expect(
        project([event('s', 'sleep_on', 0), event('w', 'sleep_off', bedtime)]),
        isEmpty);
    expect(
        project([
          event('s', 'sleep_on', bedtime),
          event('w', 'sleep_off', bedtime + 25 * hour)
        ]),
        isEmpty);
    expect(
        project([
          event('s', 'sleep_on', bedtime),
          event('w', 'sleep_off', bedtime + 50 * hour)
        ]),
        isEmpty);
    expect(
        project([
          event('s', 'sleep_on', bedtime),
          event('w', 'sleep_off', bedtime),
          event('w2', 'sleep_off', bedtime + hour)
        ]),
        isEmpty);
    expect(
        project([
          event('same', 'sleep_on', bedtime),
          event('same', 'sleep_on', bedtime + hour),
          event('w', 'sleep_off', bedtime + 8 * hour)
        ]),
        isEmpty);
  });
}
