import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  group('RhythmInputEvent', () {
    test('parses button input events', () {
      final event = RhythmInputEvent.fromJson({
        'kind': 'button',
        'epoch_ms': 1778058932588,
        'route': 'node_control',
        'hub_type': 'hue',
        'address': '192.168.1.20:443',
        'source_node_id': 'button-1',
        'target_node_id': 'room-1',
        'source_room_id': 'hue-room-1',
        'native_device_id': 'native-button',
        'button_action': 'on_press',
      });

      expect(event, isA<RhythmButtonInputEvent>());
      final button = event as RhythmButtonInputEvent;
      expect(button.epochMs, 1778058932588);
      expect(button.route, RhythmInputEventRoute.nodeControl);
      expect(button.sourceNodeId, 'button-1');
      expect(button.targetNodeId, 'room-1');
      expect(button.nativeDeviceId, 'native-button');
      expect(button.buttonAction, RhythmButtonAction.onPress);
    });

    test('parses motion input events', () {
      final event = RhythmInputEvent.fromJson({
        'kind': 'motion',
        'epoch_ms': 1778058933000,
        'route': 'node_control',
        'source_node_id': 'motion-1',
        'target_node_id': 'room-1',
        'native_sensor_id': 'native-motion',
        'detected': true,
      });

      expect(event, isA<RhythmMotionInputEvent>());
      final motion = event as RhythmMotionInputEvent;
      expect(motion.sourceNodeId, 'motion-1');
      expect(motion.targetNodeId, 'room-1');
      expect(motion.nativeSensorId, 'native-motion');
      expect(motion.detected, isTrue);
    });

    test('parses contact input events', () {
      final event = RhythmInputEvent.fromJson({
        'kind': 'contact',
        'epoch_ms': 1778058934000,
        'route': 'node_control',
        'source_node_id': 'contact-1',
        'target_node_id': 'room-1',
        'native_sensor_id': 'binary_sensor.front_door',
        'open': true,
      });

      expect(event, isA<RhythmContactInputEvent>());
      expect(event.isContact, isTrue);
      final contact = event as RhythmContactInputEvent;
      expect(contact.sourceNodeId, 'contact-1');
      expect(contact.targetNodeId, 'room-1');
      expect(contact.nativeSensorId, 'binary_sensor.front_door');
      expect(contact.open, isTrue);
    });
  });
}
