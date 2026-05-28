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
  Stream<({String event, String? hubType, String? address})> get hubEvents =>
      const Stream<({String event, String? hubType, String? address})>.empty();

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
  Future<void> reconnect({bool authoritative = false}) async {}

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

const _room1 = RoomDto(
  id: 'room-1',
  name: 'Kitchen',
  source: RoomSourceDto.hue,
  kind: RoomNodeKind.room,
  deviceIds: ['device-1'],
  rhythmEnabled: true,
  disabled: false,
  lightsOn: true,
  timeOffsetMinutes: 0,
  brightnessOffset: 0,
);

const _switch1 = RoomDto(
  id: 'switch-1',
  name: 'Wall Switch',
  source: RoomSourceDto.matter,
  kind: RoomNodeKind.switchDevice,
  deviceIds: ['switch-device-1'],
  rhythmEnabled: false,
  disabled: false,
  lightsOn: false,
  timeOffsetMinutes: 0,
  brightnessOffset: 0,
);

class _AllRoomsHarness {
  const _AllRoomsHarness({required this.roomPageProvider});

  final RoomPageProvider roomPageProvider;
}

Future<_AllRoomsHarness> _pumpAllRooms(
  WidgetTester tester, {
  required List<RoomDto> rooms,
}) async {
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
  final pageController = PageController();

  addTearDown(roomProvider.dispose);
  addTearDown(homeProvider.dispose);
  addTearDown(serverSync.dispose);
  addTearDown(connection.dispose);
  addTearDown(roomPageProvider.dispose);
  addTearDown(pageController.dispose);
  addTearDown(() => tester.binding.setSurfaceSize(null));

  for (final room in rooms) {
    await roomProvider.addRoom(room);
  }
  await tester.binding.setSurfaceSize(const Size(390, 844));

  await tester.pumpWidget(
    MultiProvider(
      providers: [
        ChangeNotifierProvider<RoomProvider>.value(value: roomProvider),
        ChangeNotifierProvider<HomeProvider>.value(value: homeProvider),
        ChangeNotifierProvider<ServerSyncProvider>.value(value: serverSync),
        ChangeNotifierProvider<RoomPageProvider>.value(
          value: roomPageProvider,
        ),
      ],
      child: MaterialApp(
        home: Scaffold(
          body: AllRoomsScreen(
            rooms: rooms,
            globalConfig: defaultCurveConfig,
            pageController: pageController,
            onPageChanged: (_) {},
          ),
        ),
      ),
    ),
  );
  await tester.pump();

  return _AllRoomsHarness(roomPageProvider: roomPageProvider);
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('room cards show the default room icon', (tester) async {
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

    await roomProvider.addRoom(_room1);
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
              rooms: const [_room1],
              globalConfig: defaultCurveConfig,
              pageController: PageController(),
              onPageChanged: (_) {},
            ),
          ),
        ),
      ),
    );
    await tester.pump();

    expect(find.text('Kitchen'), findsOneWidget);
    expect(find.byIcon(Icons.meeting_room_rounded), findsOneWidget);
    expect(find.byIcon(Icons.lightbulb_outline_rounded), findsNothing);
  });

  testWidgets('rooms and bulbs share the half-width grid', (tester) async {
    await _pumpAllRooms(
      tester,
      rooms: const [_bulb1, _room1, _switch1],
    );

    final bulb = tester.getRect(_roomCardContainer('bulb-1'));
    final room = tester.getRect(_roomCardContainer('room-1'));
    final fullWidth = tester.getRect(_roomCardContainer('switch-1'));

    expect(bulb.top, closeTo(room.top, 0.1));
    expect(bulb.width, closeTo(room.width, 0.1));
    expect(fullWidth.width, greaterThan(bulb.width * 1.8));
    expect(fullWidth.left, closeTo(bulb.left, 0.1));
  });

  testWidgets('compact bulb cards show mood for mood state', (tester) async {
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
    roomProvider.setRoomStateLocal(_bulb1.id, RoomModeState.mood);
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
              rooms: const [_bulb1],
              globalConfig: defaultCurveConfig,
              pageController: PageController(),
              onPageChanged: (_) {},
            ),
          ),
        ),
      ),
    );
    await tester.pump();

    expect(find.text('Bulb 1'), findsOneWidget);
    expect(find.text('Mood'), findsOneWidget);
  });

  testWidgets(
      'half-width room and bulb cards stay compact when long-press enters edit mode',
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
    await roomProvider.addRoom(_room1);
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
              rooms: const [_bulb1, _room1],
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
    final beforeRoom1 = tester.getRect(_roomCardContainer('room-1'));

    expect(beforeBulb1.width, closeTo(beforeRoom1.width, 0.1));

    await tester.longPress(find.text('Bulb 1'));
    await tester.pump();

    expect(find.text('Edit Rooms'), findsOneWidget);
    expect(find.byIcon(Icons.drag_indicator_rounded), findsNothing);

    final afterBulb1 = tester.getRect(_roomCardContainer('bulb-1'));
    final afterRoom1 = tester.getRect(_roomCardContainer('room-1'));

    expect(afterBulb1.width, closeTo(beforeBulb1.width, 0.1));
    expect(afterRoom1.width, closeTo(beforeRoom1.width, 0.1));
    expect(afterBulb1.top, closeTo(afterRoom1.top, 0.1));
  });

  testWidgets('edit-mode drag reorders half-width tiles', (tester) async {
    final harness = await _pumpAllRooms(
      tester,
      rooms: const [_bulb1, _room1],
    );

    await tester.longPress(find.text('Bulb 1'));
    await tester.pump();

    expect(find.text('Edit Rooms'), findsOneWidget);
    expect(
      harness.roomPageProvider
          .getRoomsForPage(0, const [_bulb1, _room1]).map((room) => room.id),
      ['bulb-1', 'room-1'],
    );

    final bulb = tester.getRect(_roomCardContainer('bulb-1'));
    final room = tester.getRect(_roomCardContainer('room-1'));
    final gesture = await tester.startGesture(bulb.center);
    await tester.pump(const Duration(milliseconds: 20));
    await gesture.moveBy(const Offset(24, 0));
    await tester.pump(const Duration(milliseconds: 20));
    await gesture.moveTo(Offset(room.right - 2, room.center.dy));
    await tester.pump(const Duration(milliseconds: 20));
    await gesture.up();
    await tester.pump(const Duration(milliseconds: 300));

    expect(
      harness.roomPageProvider
          .getRoomsForPage(0, const [_bulb1, _room1]).map((room) => room.id),
      ['room-1', 'bulb-1'],
    );
  });

  testWidgets('double tapping outside tiles exits edit mode', (tester) async {
    final harness = await _pumpAllRooms(
      tester,
      rooms: const [_bulb1, _room1],
    );

    await tester.longPress(find.text('Bulb 1'));
    await tester.pump();

    expect(find.text('Edit Rooms'), findsOneWidget);
    expect(harness.roomPageProvider.editMode, isTrue);

    const emptySpace = Offset(195, 760);
    await tester.tapAt(emptySpace);
    await tester.pump(const Duration(milliseconds: 40));
    await tester.tapAt(emptySpace);
    await tester.pump(const Duration(milliseconds: 400));

    expect(find.text('Edit Rooms'), findsNothing);
    expect(harness.roomPageProvider.editMode, isFalse);
  });

  testWidgets('double tapping the active mode toggle requests a reapply',
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
    await tester.binding.setSurfaceSize(const Size(390, 844));

    RhythmMode? selectedMode;
    RhythmMode? reappliedMode;

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<RoomProvider>.value(value: roomProvider),
          ChangeNotifierProvider<HomeProvider>.value(value: homeProvider),
          ChangeNotifierProvider<ServerSyncProvider>.value(value: serverSync),
          ChangeNotifierProvider<RoomPageProvider>.value(
            value: roomPageProvider,
          ),
        ],
        child: MaterialApp(
          home: Scaffold(
            body: AllRoomsScreen(
              rooms: const [_bulb1],
              globalConfig: defaultCurveConfig,
              pageController: PageController(),
              onPageChanged: (_) {},
              activeMode: RhythmMode.day,
              onModeSelected: (mode) => selectedMode = mode,
              onActiveModeDoubleTap: (mode) => reappliedMode = mode,
            ),
          ),
        ),
      ),
    );
    await tester.pump();

    final dayToggle = find.text('Day');
    await tester.tap(dayToggle);
    await tester.pump(const Duration(milliseconds: 40));
    await tester.tap(dayToggle);
    await tester.pump(const Duration(milliseconds: 400));

    expect(reappliedMode, RhythmMode.day);
    expect(selectedMode, isNull);

    await tester.tap(find.text('Sleep'));
    await tester.pump();

    expect(selectedMode, RhythmMode.sleep);
  });
}
