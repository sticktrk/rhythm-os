import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  test('parses an exact canonical target node when advertised', () {
    final failure = RhythmDispatchFailure.fromJson({
      'hub_type': 'matter',
      'hub_key': 'matter@local',
      'node_id': 'room-1',
      'target_node_id': 'bulb-1',
      'target': 'matter-113',
      'kind': 'matter_controller_command',
      'status': 'timed_out',
    });

    expect(failure.targetNodeId, 'bulb-1');
    expect(failure.target, 'matter-113');
  });

  test('keeps previous-appliance events compatible', () {
    final failure = RhythmDispatchFailure.fromJson({
      'hub_type': 'hue',
      'hub_key': 'hue@bridge.local',
      'node_id': 'room-1',
      'target': 'grouped_light/abc',
      'kind': 'turn_on',
      'status': 'failed',
    });

    expect(failure.targetNodeId, isNull);
  });
}
