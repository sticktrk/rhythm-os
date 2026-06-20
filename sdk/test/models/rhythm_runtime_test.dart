import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  group('RhythmRuntime models', () {
    test('parses unknown activity source enum payloads defensively', () {
      final history = RhythmHistory.fromJson({
        'activities': [
          {
            'id': 'evt-1',
            'node_id': 'room-1',
            'action_id': 'custom',
            'source': {
              'raw': 'vendor_custom',
              'kind': {'unknown': 'vendor_custom'},
              'marks_touched': false,
              'control_id': 'button-1',
            },
            'epoch_ms': 1778058932588,
            'change': {
              'axis': 'brightness',
              'before': 20,
              'after': 40,
            },
            'trace': {
              'entries': [
                {
                  'stage_id': 'stage',
                  'phase': 'render',
                  'value': 'brightness',
                  'input': 0.2,
                  'output': 0.4,
                },
              ],
            },
          },
        ],
      });

      final activity = history.activities.single;
      expect(activity.source.kind, 'vendor_custom');
      expect(activity.source.controlId, 'button-1');
      expect(activity.change!.axis, 'brightness');
      expect(activity.trace!.entries.single.output, 0.4);
    });

    test('filter preset document round-trips normalized document fields', () {
      final document = RhythmFilterPresetDocument.fromJson({
        'schema_version': 1,
        'presets': [
          {
            'id': 'desk',
            'filter': {
              'device_ids': ['lamp-a'],
            },
          },
        ],
        'extra': {'note': 'kept'},
      });

      expect(document.toJson(), {
        'schema_version': 1,
        'presets': [
          {
            'id': 'desk',
            'filter': {
              'device_ids': ['lamp-a'],
            },
          },
        ],
        'extra': {'note': 'kept'},
      });
    });
  });
}
