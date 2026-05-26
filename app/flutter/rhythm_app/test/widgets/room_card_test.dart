import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/providers/room_provider.dart';
import 'package:rhythm_app/providers/server_sync_provider.dart';
import 'package:rhythm_app/widgets/room_card.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class _FakeHomeProvider extends HomeProvider {
  @override
  List<Hub> get currentHomeHubs => const [];

  @override
  Hub? getFirstHubOfType(HubType type) => null;

  @override
  Future<void> onUserSignIn() async {}
}

class _FakeRhythmServerApi extends RhythmServerApi {
  _FakeRhythmServerApi() : super(Dio());

  @override
  Future<Map<String, dynamic>?> getTriageCount() async => null;
}

class _TestRhythmConnection extends RhythmConnection {
  _TestRhythmConnection() : _api = _FakeRhythmServerApi();

  final RhythmServerApi _api;

  @override
  RhythmServerApi get api => _api;

  @override
  bool get connected => true;

  @override
  RhythmConnectionState get connectionState => RhythmConnectionState.connected;

  @override
  Future<void> pingOrReconnect() async {}

  @override
  Future<void> reconnect({bool authoritative = false}) async {}

  @override
  void disconnect() {}
}

void main() {
  testWidgets('shows and clears a spinner while the room is transitioning',
      (tester) async {
    final roomProvider = RoomProvider();
    await roomProvider.addRoom(
      const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.hue,
        kind: RoomNodeKind.room,
        deviceIds: ['light-1'],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: true,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );
    final homeProvider = _FakeHomeProvider();
    final connection = _TestRhythmConnection();
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<RoomProvider>.value(value: roomProvider),
          ChangeNotifierProvider<ServerSyncProvider>.value(value: serverSync),
        ],
        child: MaterialApp(
          home: Scaffold(
            body: RoomCard(
              roomId: 'room-1',
              globalConfig: defaultCurveConfig,
            ),
          ),
        ),
      ),
    );

    expect(find.byType(CircularProgressIndicator), findsNothing);
    expect(tester.widget<Slider>(find.byType(Slider)).onChanged, isNotNull);

    await roomProvider.applyServerNodeState(
      'room-1',
      rhythmEnabled: true,
      timeOffset: 0,
      brightnessOffset: 0,
      state: RoomModeState.active,
      transitioning: true,
      lightsOn: true,
    );
    await tester.pump();

    expect(find.byType(CircularProgressIndicator), findsOneWidget);
    expect(tester.widget<Slider>(find.byType(Slider)).onChanged, isNull);

    await tester.tap(find.text('Off'));
    await tester.pump();

    expect(roomProvider.getDisplayRoomState('room-1'), RoomModeState.active);
    expect(find.text('Settings'), findsNothing);

    await roomProvider.applyServerNodeState(
      'room-1',
      rhythmEnabled: true,
      timeOffset: 0,
      brightnessOffset: 0,
      state: RoomModeState.active,
      transitioning: false,
      lightsOn: true,
    );
    await tester.pumpAndSettle();

    expect(find.byType(CircularProgressIndicator), findsNothing);
    expect(tester.widget<Slider>(find.byType(Slider)).onChanged, isNotNull);
  });
}
