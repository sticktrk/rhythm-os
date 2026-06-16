import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/demo_server_api.dart';
import 'package:rhythm_app/services/hue/demo_hue_bridge_service.dart';

void main() {
  setUp(() {
    DemoServerApi.instance.reset();
    DemoHueBridgeService.instance.reset();
  });

  test('demo rooms start in their seeded on-state', () async {
    final rooms = await DemoHueBridgeService.instance.fetchRooms();

    expect(rooms, isNotEmpty);
    expect(await DemoHueBridgeService.instance.isRoomOn('hue_demo_1'), isTrue);
    expect(
        await DemoHueBridgeService.instance.fetchAllRoomStates(), isNotEmpty);
    expect(
      (await DemoHueBridgeService.instance.fetchAllRoomStates()).values,
      everyElement(isTrue),
    );
  });
}
