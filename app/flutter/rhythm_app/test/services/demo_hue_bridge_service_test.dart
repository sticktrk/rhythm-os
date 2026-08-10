import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/demo_server_api.dart';
import 'package:rhythm_app/services/hue/demo_hue_bridge_service.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

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

  test('demo Hue authority review uses the production DTO contract', () async {
    final initial = await DemoServerApi.instance.getHueAuthority();
    final bridge = initial!.bridges.single;

    expect(bridge.rooms.length, greaterThanOrEqualTo(2));
    expect(
      bridge.rooms.map((room) => room.owner),
      everyElement(RhythmHueRoomAuthorityOwner.unreviewed),
    );

    final updated = await DemoServerApi.instance.updateHueAuthority(
      bridge: bridge,
      owners: {
        for (final room in bridge.rooms)
          room.roomId: RhythmHueRoomAuthorityOwner.rhythm,
      },
      correlationId: 'demo-hue-authority-test',
    );

    expect(updated!.bridges.single.bridgeTakeoverRequested, isTrue);
    expect(
      updated.bridges.single.rooms,
      everyElement(
        isA<RhythmHueRoomAuthority>()
            .having((room) => room.rhythmAutomationEnabled, 'enabled', isTrue),
      ),
    );
  });
}
