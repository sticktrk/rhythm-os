import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/hubs/room_device_add_flow.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

void main() {
  group('existingRoomDeviceCandidates', () {
    test('room bulb candidates put unassigned devices before assigned names',
        () {
      final candidates = existingRoomDeviceCandidates(
        canonicalDevices: const [
          {
            'id': 'assigned-alpha',
            'name': 'Alpha Lamp',
            'device_type': 'light',
            'room_id': 'room-1',
          },
          {
            'id': 'unassigned-zulu',
            'name': 'Zulu Lamp',
            'device_type': 'light',
          },
          {
            'id': 'assigned-delta',
            'name': 'Delta Lamp',
            'device_type': 'light',
            'parent_id': 'room-2',
          },
          {
            'id': 'unassigned-beta',
            'name': 'beta Lamp',
            'device_type': 'light',
          },
        ],
        topologyNodes: const [],
        roomNamesById: const {
          'room-1': 'Kitchen',
          'room-2': 'Hall',
        },
        deviceType: RhythmDeviceType.light,
      );

      expect(
        candidates.map((candidate) => candidate.device.id),
        [
          'unassigned-beta',
          'unassigned-zulu',
          'assigned-alpha',
          'assigned-delta',
        ],
      );
      expect(
        candidates.map((candidate) => candidate.parentLabel),
        ['Unassigned', 'Unassigned', 'Kitchen', 'Hall'],
      );
    });

    test('non-bulb candidates retain alphabetical ordering', () {
      final candidates = existingRoomDeviceCandidates(
        canonicalDevices: const [
          {
            'id': 'unassigned-zulu',
            'name': 'Zulu Motion',
            'device_type': 'motion',
          },
          {
            'id': 'assigned-alpha',
            'name': 'Alpha Motion',
            'device_type': 'motion',
            'room_id': 'room-1',
          },
        ],
        topologyNodes: const [],
        roomNamesById: const {'room-1': 'Kitchen'},
        deviceType: RhythmDeviceType.motion,
      );

      expect(
        candidates.map((candidate) => candidate.device.id),
        ['assigned-alpha', 'unassigned-zulu'],
      );
    });

    test('topology parent fallback keeps a bulb out of unassigned group', () {
      final candidates = existingRoomDeviceCandidates(
        canonicalDevices: const [
          {
            'id': 'topology-assigned-alpha',
            'name': 'Alpha Lamp',
            'device_type': 'light',
          },
          {
            'id': 'unassigned-zulu',
            'name': 'Zulu Lamp',
            'device_type': 'light',
          },
        ],
        topologyNodes: [
          RhythmTopologyNode.fromJson(const {
            'id': 'topology-assigned-alpha',
            'name': 'Alpha Lamp',
            'kind': 'light',
            'parent_id': 'room-1',
          }),
        ],
        roomNamesById: const {'room-1': 'Kitchen'},
        deviceType: RhythmDeviceType.light,
      );

      expect(
        candidates.map((candidate) => candidate.device.id),
        ['unassigned-zulu', 'topology-assigned-alpha'],
      );
      expect(candidates.last.parentLabel, 'Kitchen');
    });
  });
}
