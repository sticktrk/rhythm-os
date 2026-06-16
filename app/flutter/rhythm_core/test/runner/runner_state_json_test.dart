import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_core/runner/runner_models.dart';
import 'package:rhythm_core/runner/runner_state_json.dart';

void main() {
  test('runner state JSON round-trips Matter rooms', () {
    final state = RunnerStateDto(
      rooms: const [
        RoomDto(
          id: 'matter-room',
          name: 'Matter Room',
          source: RoomSourceDto.matter,
          deviceIds: ['matter-light-1'],
          rhythmEnabled: true,
          disabled: false,
          lightsOn: true,
          timeOffsetMinutes: 15,
          brightnessOffset: 5,
        ),
      ],
    );

    final encoded = runnerStateToJson(state);
    final decoded = runnerStateFromJson(encoded);

    expect(decoded, isNotNull);
    expect(decoded!.rooms, hasLength(1));
    expect(decoded.rooms.single.source, RoomSourceDto.matter);
  });
}
