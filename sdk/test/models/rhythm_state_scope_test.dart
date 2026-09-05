import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

Map<String, dynamic> node(String id, {String kind = 'room', String? parent}) =>
    {
      'id': id,
      'kind': kind,
      if (parent != null) 'parent_id': parent,
    };

RhythmHello selected(String scope, List<Map<String, dynamic>>? nodes) =>
    RhythmHello.fromJson({
      'state_scope': {
        'schema_version': 1,
        'included': [
          'base',
          if (scope == 'all') 'nodes' else if (scope == 'controls') 'controls'
        ],
        'nodes': scope
      },
      if (nodes != null) 'nodes': nodes,
    });

void main() {
  test('omission, complete empty controls, and complete empty all are distinct',
      () {
    final previous = [
      node('room'),
      node('bulb', kind: 'light_device', parent: 'room'),
      node('standalone', kind: 'light_device')
    ].map(RhythmRoom.fromJson).toList();
    expect(selected('none', null).mergeNodes(previous), previous);
    final fresh = selected('controls', [node('room')]).mergeNodes(previous);
    expect(fresh.map((n) => n.id), unorderedEquals(['room', 'bulb']));
    expect(selected('controls', []).mergeNodes(previous), isEmpty,
        reason: 'removed rooms also invalidate their cached children');
    expect(selected('all', []).mergeNodes(previous), isEmpty);
    expect(RhythmHello.fromJson({'nodes': []}).mergeNodes(previous), isEmpty);
  });

  test('incomplete, contradictory and unsupported receipts fail closed', () {
    expect(() => selected('controls', null), throwsFormatException);
    expect(
        () => selected(
            'controls', [node('child', kind: 'light_device', parent: 'room')]),
        throwsFormatException);
    expect(() => selected('all', [node(''), node('valid')]),
        throwsFormatException);
    expect(() => selected('all', [node('duplicate'), node('duplicate')]),
        throwsFormatException);
    expect(() => selected('none', []), throwsFormatException);
    expect(
        () => RhythmHello.fromJson({
              'state_scope': {
                'schema_version': 1,
                'included': ['base', 'nodes'],
                'nodes': 'all'
              },
              'nodes': [null]
            }),
        throwsFormatException);
    expect(
        () => RhythmHello.fromJson({
              'state_scope': {
                'schema_version': 1,
                'included': ['base', 'configuration'],
                'nodes': 'none'
              }
            }),
        throwsFormatException);
    for (final receipt in [
      {
        'schema_version': 2,
        'included': ['base'],
        'nodes': 'none'
      },
      {
        'schema_version': 1,
        'included': ['base', 'controls'],
        'nodes': 'all'
      },
      {
        'schema_version': 1,
        'included': ['base', 'nodes', 'controls'],
        'nodes': 'all'
      },
      {
        'schema_version': 1,
        'included': ['base', 'typo'],
        'nodes': 'none'
      },
    ]) {
      expect(() => RhythmHello.fromJson({'state_scope': receipt, 'nodes': []}),
          throwsFormatException);
    }
  });

  test('room summaries retain counts and motion without materializing devices',
      () {
    final room = RhythmRoom.fromJson({
      'id': 'room',
      'kind': 'room',
      'motion_active': false,
      'device_counts': {'light': 5, 'motion': 1, 'button': 2},
    });
    expect(room.devices, isEmpty);
    expect(room.lightCount, 5);
    expect(room.deviceCount, 8);
    expect(room.deviceSummary, '5 lights, 2 buttons, 1 sensor');
    expect(room.hasMotionSensor, isTrue);
  });
}
