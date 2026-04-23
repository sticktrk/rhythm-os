import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/providers/room_page_provider.dart';
import 'package:rhythm_app/providers/room_provider.dart';
import 'package:rhythm_app/providers/server_sync_provider.dart';
import 'package:rhythm_app/screens/all_rooms_screen.dart';
import 'package:rhythm_app/widgets/editable_room_card.dart';
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
  bool get connected => false;

  @override
  RhythmConnectionState get connectionState =>
      RhythmConnectionState.disconnected;

  @override
  Stream<RhythmHello> get helloEvents => const Stream<RhythmHello>.empty();

  @override
  Stream<RhythmRoomState> get rhythmStateEvents =>
      const Stream<RhythmRoomState>.empty();

  @override
  Stream<({String event, String? hubType})> get hubEvents =>
      const Stream<({String event, String? hubType})>.empty();

  @override
  Stream<RhythmMotionTimer> get motionTimerEvents =>
      const Stream<RhythmMotionTimer>.empty();

  @override
  Stream<void> get newNodesDetected => const Stream<void>.empty();

  @override
  Stream<Map<String, dynamic>> get triageChangedEvents =>
      const Stream<Map<String, dynamic>>.empty();

  @override
  Stream<RhythmConnectionState> get connectionStateStream =>
      const Stream<RhythmConnectionState>.empty();

  @override
  Future<void> pingOrReconnect() async {}

  @override
  Future<void> reconnect() async {}

  @override
  void disconnect() {}
}

class _MemoryRoomPageLayoutStore implements RoomPageLayoutStore {
  List<List<String>>? _pages;

  @override
  Future<void> clearLayout({String? scopeKey}) async {
    _pages = null;
  }

  @override
  List<List<String>>? loadLayout({String? scopeKey}) => _pages;

  @override
  List<List<String>>? loadLegacyLayout() => null;

  @override
  Future<void> migrateLegacyLayoutToScope(String scopeKey) async {}

  @override
  Future<void> saveLayout(List<List<String>> pages, {String? scopeKey}) async {
    _pages = pages.map((page) => List<String>.from(page)).toList();
  }
}

Finder _roomCardContainer(String roomId) {
  return find.byWidgetPredicate((widget) {
    if (widget is! Container) return false;
    final child = widget.child;
    return child is EditableRoomCard && child.roomId == roomId;
  });
}

const _bulb1 = RoomDto(
  id: 'bulb-1',
  name: 'Bulb 1',
  source: RoomSourceDto.hue,
  kind: RoomNodeKind.lightDevice,
  parentId: 'room-1',
  deviceIds: ['device-1'],
  rhythmEnabled: true,
  disabled: false,
  lightsOn: true,
  timeOffsetMinutes: 0,
  brightnessOffset: 0,
);

const _bulb2 = RoomDto(
  id: 'bulb-2',
  name: 'Bulb 2',
  source: RoomSourceDto.hue,
  kind: RoomNodeKind.lightDevice,
  parentId: 'room-1',
  deviceIds: ['device-2'],
  rhythmEnabled: true,
  disabled: false,
  lightsOn: true,
  timeOffsetMinutes: 0,
  brightnessOffset: 0,
);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets(
      'compact bulb cards stay compact when long-press enters edit mode',
      (tester) async {
    final roomProvider = RoomProvider();
    final homeProvider = _FakeHomeProvider();
    final connection = _TestRhythmConnection();
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    final roomPageProvider = RoomPageProvider(
      layoutStore: _MemoryRoomPageLayoutStore(),
    );

    addTearDown(roomProvider.dispose);
    addTearDown(homeProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);
    addTearDown(roomPageProvider.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await roomProvider.addRoom(_bulb1);
    await roomProvider.addRoom(_bulb2);
    await tester.binding.setSurfaceSize(const Size(390, 844));

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<RoomProvider>.value(value: roomProvider),
          ChangeNotifierProvider<HomeProvider>.value(value: homeProvider),
          ChangeNotifierProvider<ServerSyncProvider>.value(value: serverSync),
          ChangeNotifierProvider<RoomPageProvider>.value(
              value: roomPageProvider),
        ],
        child: MaterialApp(
          home: Scaffold(
            body: AllRoomsScreen(
              rooms: const [_bulb1, _bulb2],
              globalConfig: defaultCurveConfig,
              pageController: PageController(),
              onPageChanged: (_) {},
            ),
          ),
        ),
      ),
    );
    await tester.pump();

    final beforeBulb1 = tester.getRect(_roomCardContainer('bulb-1'));
    final beforeBulb2 = tester.getRect(_roomCardContainer('bulb-2'));

    expect(beforeBulb1.width, closeTo(beforeBulb2.width, 0.1));

    await tester.longPress(find.text('Bulb 1'));
    await tester.pump();

    expect(find.text('Edit Rooms'), findsOneWidget);

    final afterBulb1 = tester.getRect(_roomCardContainer('bulb-1'));
    final afterBulb2 = tester.getRect(_roomCardContainer('bulb-2'));

    expect(afterBulb1.width, closeTo(beforeBulb1.width, 0.1));
    expect(afterBulb2.width, closeTo(beforeBulb2.width, 0.1));
    expect(afterBulb1.top, closeTo(afterBulb2.top, 0.1));
  });
}
