import 'dart:async';
import 'dart:io';

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

  final List<List<({String nodeId, String action})>> actionBatches = [];
  final List<List<({String nodeId, int brightness})>> brightnessBatches = [];
  final List<String> homeSceneApplies = [];
  final List<int?> homeSceneTransitionMs = [];
  Completer<RhythmDispatchResult>? actionBatchCompleter;
  int? nextDispatchCount;
  List<RhythmSceneDefinition> sceneCatalog = const [];
  RhythmHomeSceneActionResult? homeSceneResult;

  @override
  Future<Map<String, dynamic>?> getTriageCount() async => null;

  // Hello handling refreshes topology; keep it off the network so the widget
  // tests never leave a real Dio timer pending.
  @override
  Future<List<RhythmTopologyNode>> getTopologyNodes() async => const [];

  @override
  Future<RhythmSceneCatalogResult?> getSceneCatalog({String? targetId}) async {
    return RhythmSceneCatalogResult(scenes: sceneCatalog);
  }

  @override
  Future<RhythmHomeSceneActionResult?> applyHomeScene({
    required String sceneId,
    int? transitionMs,
    int? dispatchSpacingMs,
    String? correlationId,
    RhythmHomeSceneTargetMode? targetMode,
  }) async {
    homeSceneApplies.add(sceneId);
    homeSceneTransitionMs.add(transitionMs);
    return homeSceneResult;
  }

  @override
  Future<RhythmDispatchResult> nodeActionBatchResult(
    List<({String nodeId, String action})> actions, {
    int? dispatchSpacingMs,
    String? correlationId,
  }) async {
    actionBatches.add(List.of(actions));
    final completer = actionBatchCompleter;
    if (completer != null) return completer.future;
    return RhythmDispatchResult(
      metadata: RhythmDispatchMetadata(
        dispatchCount: nextDispatchCount ?? actions.length,
      ),
    );
  }

  @override
  Future<RhythmDispatchResult> nodeCurveBrightnessBatchResult(
    List<({String nodeId, int brightness})> items, {
    int? dispatchSpacingMs,
    String? correlationId,
  }) async {
    brightnessBatches.add(List.of(items));
    return RhythmDispatchResult(
      metadata: RhythmDispatchMetadata(
        dispatchCount: nextDispatchCount ?? items.length,
      ),
    );
  }
}

class _TestRhythmConnection extends RhythmConnection {
  _TestRhythmConnection({
    bool connected = true,
  })  : _connected = connected,
        _api = _FakeRhythmServerApi();

  final bool _connected;
  final _FakeRhythmServerApi _api;
  final StreamController<RhythmHello> _helloController =
      StreamController<RhythmHello>.broadcast();
  VoidCallback? onReconnect;

  void emitHello(RhythmHello hello) => _helloController.add(hello);

  @override
  _FakeRhythmServerApi get api => _api;

  @override
  bool get connected => _connected;

  @override
  RhythmConnectionState get connectionState => _connected
      ? RhythmConnectionState.connected
      : RhythmConnectionState.disconnected;

  @override
  Stream<RhythmHello> get helloEvents => _helloController.stream;

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
  Future<void> reconnect({bool authoritative = false}) async {
    onReconnect?.call();
  }

  @override
  void disconnect() {}

  @override
  void dispose() {
    _helloController.close();
    super.dispose();
  }
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

const _bedroom = RoomDto(
  id: 'bedroom',
  name: 'Bedroom',
  source: RoomSourceDto.hue,
  kind: RoomNodeKind.room,
  deviceIds: ['device-2'],
  rhythmEnabled: true,
  disabled: false,
  lightsOn: true,
  timeOffsetMinutes: 0,
  brightnessOffset: 0,
);

const _garage = RoomDto(
  id: 'garage',
  name: 'Garage',
  source: RoomSourceDto.hue,
  kind: RoomNodeKind.room,
  deviceIds: ['device-3'],
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
  const _AllRoomsHarness({
    required this.roomPageProvider,
    required this.roomProvider,
    required this.api,
    required this.connection,
    required this.pageController,
    required this.serverSync,
  });

  final RoomPageProvider roomPageProvider;
  final RoomProvider roomProvider;
  final _FakeRhythmServerApi api;
  final _TestRhythmConnection connection;
  final PageController pageController;
  final ServerSyncProvider serverSync;
}

RhythmHello _helloWithFeatures(List<String> features) => RhythmHello.fromJson({
      'version': '0.6.635-beta',
      'capabilities': {
        'api_schema_version': 2,
        'features': features,
        'hubs': const <dynamic>[],
      },
      'nodes': const <dynamic>[],
    });

RhythmSceneDefinition _paletteScene({
  required String id,
  required String name,
  List<(int, int, int)> colors = const [(255, 104, 0), (122, 0, 214)],
  RhythmSceneSource source = const RhythmSceneSource(
    kind: RhythmSceneSourceKind.user,
  ),
}) {
  return RhythmSceneDefinition(
    id: id,
    name: name,
    source: source,
    light: RhythmLightScene(
      defaultTransitionMs: 1200,
      palette: [
        for (final color in colors)
          RhythmLightSceneOutput.on(
            brightness: 80,
            color: RhythmLightColor.rgb(
              RhythmSceneRgbColor(r: color.$1, g: color.$2, b: color.$3),
            ),
          ),
      ],
    ),
  );
}

Future<_AllRoomsHarness> _pumpAllRooms(
  WidgetTester tester, {
  required List<RoomDto> rooms,
  VoidCallback? onHomeChooserTap,
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
            onHomeChooserTap: onHomeChooserTap,
          ),
        ),
      ),
    ),
  );
  await tester.pump();

  return _AllRoomsHarness(
    roomPageProvider: roomPageProvider,
    roomProvider: roomProvider,
    api: connection.api,
    connection: connection,
    pageController: pageController,
    serverSync: serverSync,
  );
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('home button opens the Home chooser', (tester) async {
    var opened = false;
    await _pumpAllRooms(
      tester,
      rooms: const [_room1],
      onHomeChooserTap: () => opened = true,
    );

    await tester.tap(find.byIcon(Icons.home_rounded));

    expect(opened, isTrue);
  });

  testWidgets('global action dock replaces search and expands exact slider',
      (tester) async {
    await _pumpAllRooms(
      tester,
      rooms: const [_room1, _bedroom, _garage],
    );

    expect(find.byKey(const ValueKey('room_quick_search_field')), findsNothing);
    expect(
      find.byKey(const ValueKey('global-room-action-dock')),
      findsOneWidget,
    );
    expect(find.text('Soften'), findsOneWidget);
    expect(find.text('Boost'), findsOneWidget);
    expect(
      find.descendant(
        of: find.byKey(const ValueKey('global-room-action-dock')),
        matching: find.text('Reset'),
      ),
      findsOneWidget,
    );
    expect(
      find.byKey(const ValueKey('global-room-slider-panel')),
      findsNothing,
    );

    await tester.tap(
      find.byKey(const ValueKey('global-room-action-expand')),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));

    expect(
      find.byKey(const ValueKey('global-room-slider-panel')),
      findsOneWidget,
    );
    expect(
      find.byKey(const ValueKey('global-room-brightness-slider')),
      findsOneWidget,
    );
  });

  Future<_AllRoomsHarness> pumpWithHomeScenes(
    WidgetTester tester, {
    bool supported = true,
    List<RhythmSceneDefinition>? scenes,
    RhythmHomeSceneActionResult? result,
  }) async {
    final harness = await _pumpAllRooms(
      tester,
      rooms: const [_room1, _bedroom],
    );
    harness.api.sceneCatalog = scenes ??
        [
          _paletteScene(id: 'halloween', name: 'Halloween'),
          _paletteScene(id: 'berry-pop', name: 'Berry Pop'),
        ];
    harness.api.homeSceneResult = result ??
        const RhythmHomeSceneActionResult(
          sceneId: 'halloween',
          targets: [
            RhythmHomeSceneTargetResult(
              targetId: 'room-1',
              affectedNodeIds: ['bulb-1'],
            ),
            RhythmHomeSceneTargetResult(
              targetId: 'bedroom',
              affectedNodeIds: ['bulb-2'],
            ),
          ],
          appliedTargetCount: 2,
        );
    harness.connection.emitHello(
      _helloWithFeatures(supported ? [RhythmFeature.homeSceneApply] : const []),
    );
    await tester.pump();
    return harness;
  }

  /// The whole-home scene chooser lives inside the tune panel.
  Future<void> openScenePanel(WidgetTester tester) async {
    await tester.tap(find.byKey(const ValueKey('global-room-action-expand')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));
  }

  testWidgets(
      'the scene chooser lives inside the tune panel and is hidden without '
      'the capability', (tester) async {
    final harness = await pumpWithHomeScenes(tester, supported: false);

    expect(
      find.byKey(const ValueKey('global-room-action-scene')),
      findsNothing,
      reason: 'the dock has no separate Scene button',
    );
    await openScenePanel(tester);
    expect(
      find.byKey(const ValueKey('global-room-slider-panel')),
      findsOneWidget,
    );
    expect(
      find.byKey(const ValueKey('global-room-scene-panel')),
      findsNothing,
      reason: 'a previous-floor appliance must not offer the whole-home apply',
    );

    harness.connection.emitHello(
      _helloWithFeatures(const [RhythmFeature.homeSceneApply]),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));

    expect(
      find.descendant(
        of: find.byKey(const ValueKey('global-room-slider-panel')),
        matching: find.byKey(const ValueKey('global-room-scene-panel')),
      ),
      findsOneWidget,
      reason:
          'the chooser sits beneath the brightness slider in the same panel',
    );
  });

  testWidgets(
      'the tune panel shows the slider and the scene chooser together and '
      'hides Hue-imported scenes', (tester) async {
    await pumpWithHomeScenes(
      tester,
      scenes: [
        _paletteScene(id: 'halloween', name: 'Halloween'),
        _paletteScene(
          id: 'native-hue-abc',
          name: 'Hue Savanna',
          source: const RhythmSceneSource.imported(
            provider: 'hue',
            externalId: 'abc',
          ),
        ),
      ],
    );

    await openScenePanel(tester);

    expect(
      find.byKey(const ValueKey('global-room-slider-panel')),
      findsOneWidget,
    );
    expect(
      find.byKey(const ValueKey('global-room-brightness-slider')),
      findsOneWidget,
      reason: 'the exact brightness slider stays available alongside scenes',
    );
    expect(
      find.byKey(const ValueKey('global-room-scene-panel')),
      findsOneWidget,
    );
    expect(find.text('Whole home scene'), findsOneWidget);
    expect(find.text('Applies to every room'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('global-room-scene-halloween')),
      findsOneWidget,
    );
    expect(
      find.byKey(const ValueKey('global-room-scene-native-hue-abc')),
      findsNothing,
      reason: 'Hue-imported scenes are room-bound and cannot cover the home',
    );

    await tester.tap(find.byKey(const ValueKey('global-room-action-expand')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));
    expect(
      find.byKey(const ValueKey('global-room-scene-panel')),
      findsNothing,
    );
    expect(
      find.byKey(const ValueKey('global-room-slider-panel')),
      findsNothing,
    );
  });

  testWidgets(
      'tapping a whole-home scene calls the server once and reports '
      'the applied rooms', (tester) async {
    final harness = await pumpWithHomeScenes(tester);
    await openScenePanel(tester);

    await tester.tap(find.byKey(const ValueKey('global-room-scene-halloween')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));

    expect(harness.api.homeSceneApplies, ['halloween']);
    expect(
      harness.api.homeSceneTransitionMs,
      [null],
      reason: 'a request-level transition would override each palette '
          "output's own transition_ms",
    );
    expect(find.text('Set Halloween in 2 of 2 rooms.'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('global-room-action-undo')),
      findsNothing,
      reason: 'the server owns this fan-out; there is nothing local to undo',
    );
    expect(
      find.byKey(const ValueKey('global-room-scene-panel')),
      findsOneWidget,
      reason: 'the panel stays open so another scene can be tried',
    );
  });

  testWidgets('an errored target is excluded from the applied room count',
      (tester) async {
    await pumpWithHomeScenes(
      tester,
      result: const RhythmHomeSceneActionResult(
        sceneId: 'halloween',
        targets: [
          RhythmHomeSceneTargetResult(
            targetId: 'room-1',
            affectedNodeIds: ['bulb-1'],
          ),
          RhythmHomeSceneTargetResult(
            targetId: 'bedroom',
            error: 'hub is offline',
          ),
        ],
        appliedTargetCount: 1,
        skippedTargetCount: 3,
      ),
    );
    await openScenePanel(tester);

    await tester.tap(find.byKey(const ValueKey('global-room-scene-halloween')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));

    expect(
      find.text('Set Halloween in 1 of 2 rooms.'),
      findsOneWidget,
      reason: 'skipped targets are excluded from the denominator',
    );
  });

  testWidgets('a rejected whole-home scene shows the failure snackbar',
      (tester) async {
    final harness = await pumpWithHomeScenes(tester);
    harness.api.homeSceneResult = null;
    await openScenePanel(tester);

    await tester.tap(find.byKey(const ValueKey('global-room-scene-halloween')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));

    expect(
      find.text("Couldn't set Halloween. Check the hub connection."),
      findsOneWidget,
    );
  });

  testWidgets('authoritative refresh preserves the visible room grouping',
      (tester) async {
    final harness = await _pumpAllRooms(
      tester,
      rooms: const [_room1, _bedroom],
    );
    harness.roomPageProvider.moveRoom(_bedroom.id, 1);
    await tester.pump();
    harness.pageController.jumpToPage(1);
    await tester.pump();

    expect(harness.pageController.page, 1);
    expect(find.text('Bedroom').hitTestable(), findsOneWidget);

    harness.connection.onReconnect = () {
      // Simulate the PageView detaching during the authoritative reconnect
      // and reattaching at its default first page.
      harness.pageController.jumpToPage(0);
    };
    final refresh = tester.widget<RefreshIndicator>(
      find.byType(RefreshIndicator).hitTestable(),
    );
    await refresh.onRefresh();
    await tester.pump();
    await tester.pump();

    expect(harness.pageController.page, 1);
    expect(find.text('Bedroom').hitTestable(), findsOneWidget);
  });

  testWidgets('the page view returns to the last page when pages disappear',
      (tester) async {
    final harness = await _pumpAllRooms(
      tester,
      rooms: const [_room1, _bedroom],
    );
    harness.roomPageProvider.reconcileRooms(const [_room1, _bedroom]);
    harness.roomPageProvider.moveRoom(_bedroom.id, 1);
    await tester.pump();
    harness.pageController.jumpToPage(1);
    await tester.pump();
    expect(harness.pageController.page, 1);
    expect(find.text('Bedroom').hitTestable(), findsOneWidget);

    // A refresh that momentarily omits the bedroom empties page 1.
    harness.roomPageProvider.reconcileRooms(const [_room1]);
    await tester.pump();
    await tester.pump();

    expect(harness.roomPageProvider.pageCount, 1);
    expect(harness.pageController.page, 0);
    expect(find.text('Kitchen').hitTestable(), findsOneWidget);

    // The bedroom comes back on its own page.
    harness.roomPageProvider.reconcileRooms(const [_room1, _bedroom]);
    await tester.pump();
    expect(harness.roomPageProvider.pageCount, 2);
    harness.pageController.jumpToPage(1);
    await tester.pump();
    expect(find.text('Bedroom').hitTestable(), findsOneWidget);
  });

  testWidgets('room cards stay alphabetical in normal page presentation',
      (tester) async {
    final harness = await _pumpAllRooms(
      tester,
      rooms: const [_room1, _bedroom, _garage],
    );
    harness.roomPageProvider.reconcileRooms(
      const [_room1, _bedroom, _garage],
    );
    harness.roomPageProvider.reorderInPage(_room1.id, 0, 0);
    await tester.pump();

    final bedroom = tester.getRect(_roomCardContainer(_bedroom.id));
    final garage = tester.getRect(_roomCardContainer(_garage.id));
    final kitchen = tester.getRect(_roomCardContainer(_room1.id));

    expect(bedroom.top, lessThan(garage.top));
    expect(garage.top, lessThan(kitchen.top));
  });

  testWidgets('global soften skips Low glow nodes and collapses child bulbs',
      (tester) async {
    final harness = await _pumpAllRooms(
      tester,
      rooms: const [_room1, _bulb1, _bedroom, _garage, _switch1],
    );
    harness.roomProvider.setRoomStateLocal(
      _bedroom.id,
      RoomModeState.mood,
    );
    harness.roomProvider.setRoomStateLocal(
      _garage.id,
      RoomModeState.standby,
    );
    await tester.pump();

    await tester.tap(
      find.byKey(const ValueKey('global-room-action-dim')),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 500));

    expect(harness.api.actionBatches, hasLength(1));
    expect(harness.api.actionBatches.single, [
      (nodeId: 'room-1', action: 'step_down'),
    ]);
    expect(find.text('Adjusted 1 of 1 rooms.'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('global-room-action-undo')),
      findsOneWidget,
    );
  });

  testWidgets('global boost skips legacy Low glow and locks duplicate taps',
      (tester) async {
    final harness = await _pumpAllRooms(
      tester,
      rooms: const [_room1, _bedroom],
    );
    harness.roomProvider.setRoomStateLocal(
      _bedroom.id,
      RoomModeState.idle,
    );
    await tester.pump();
    final pending = Completer<RhythmDispatchResult>();
    harness.api.actionBatchCompleter = pending;

    final boost = find.byKey(const ValueKey('global-room-action-bright'));
    await tester.tap(boost);
    await tester.tap(boost);
    await tester.pump();

    expect(harness.api.actionBatches, hasLength(1));
    expect(harness.api.actionBatches.single, [
      (nodeId: 'room-1', action: 'step_up'),
    ]);
    expect(
      find.byKey(const ValueKey('global-room-action-progress')),
      findsOneWidget,
    );

    pending.complete(
      const RhythmDispatchResult(
        metadata: RhythmDispatchMetadata(dispatchCount: 1),
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 500));

    expect(find.text('Adjusted 1 of 1 rooms.'), findsOneWidget);
  });

  testWidgets('global action result auto-dismisses while offering undo',
      (tester) async {
    await _pumpAllRooms(
      tester,
      rooms: const [_room1],
    );

    await tester.tap(
      find.byKey(const ValueKey('global-room-action-dim')),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 500));

    expect(find.text('Adjusted 1 of 1 rooms.'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('global-room-action-undo')),
      findsOneWidget,
    );
    expect(tester.widget<SnackBar>(find.byType(SnackBar)).persist, isFalse);

    await tester.pump(const Duration(seconds: 8));
    await tester.pump(const Duration(milliseconds: 500));

    expect(find.text('Adjusted 1 of 1 rooms.'), findsNothing);
    expect(
      find.byKey(const ValueKey('global-room-action-undo')),
      findsNothing,
    );
  });

  testWidgets('global reset uses reset action and exposes brightness undo',
      (tester) async {
    final harness = await _pumpAllRooms(
      tester,
      rooms: const [_room1, _bedroom],
    );

    await tester.tap(
      find.byKey(const ValueKey('global-room-action-reset')),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 500));

    expect(harness.api.actionBatches.single, [
      (nodeId: 'room-1', action: 'reset'),
      (nodeId: 'bedroom', action: 'reset'),
    ]);
    expect(find.text('Reset 2 of 2 rooms.'), findsOneWidget);

    await tester.tap(
      find.byKey(const ValueKey('global-room-action-undo')),
    );
    await tester.pump();
    await tester.pump(const Duration(seconds: 2));
    await tester.pump(const Duration(milliseconds: 500));
    await tester.pump();
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 500));

    expect(harness.api.brightnessBatches.single, [
      (nodeId: 'room-1', brightness: 50),
      (nodeId: 'bedroom', brightness: 50),
    ]);
  });

  testWidgets('global reset includes current and legacy Low glow rooms',
      (tester) async {
    final harness = await _pumpAllRooms(
      tester,
      rooms: const [_room1, _bedroom, _garage],
    );
    harness.roomProvider.setRoomStateLocal(
      _bedroom.id,
      RoomModeState.standby,
    );
    harness.roomProvider.setRoomStateLocal(
      _garage.id,
      RoomModeState.idle,
    );
    await tester.pump();

    await tester.tap(
      find.byKey(const ValueKey('global-room-action-reset')),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 500));

    expect(harness.api.actionBatches.single, [
      (nodeId: 'room-1', action: 'reset'),
      (nodeId: 'bedroom', action: 'reset'),
      (nodeId: 'garage', action: 'reset'),
    ]);
    expect(find.text('Reset 3 of 3 rooms.'), findsOneWidget);
  });

  testWidgets('global reset restores mode defaults on capable servers',
      (tester) async {
    final screenshotPath =
        Platform.environment['RHYTHM_GLOBAL_RESET_SCREENSHOT'];
    final harness = await _pumpAllRooms(
      tester,
      rooms: const [_room1, _bedroom],
    );
    harness.connection.emitHello(
      RhythmHello.fromJson({
        'capabilities': {
          'api_schema_version': 2,
          'features': [RhythmFeature.resetToModeDefault],
          'hubs': const <dynamic>[],
        },
        'mode': {
          'active': 'day',
          'configs': [
            {
              'mode': 'day',
              'active_profile_id': 'rhythm',
              'room_defaults': [
                {'room_id': 'room-1', 'state': 'active'},
                {'room_id': 'bedroom', 'state': 'standby'},
              ],
            },
          ],
        },
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'hub_types': ['hue'],
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'lights_on': true,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
          },
          {
            'id': 'bedroom',
            'name': 'Bedroom',
            'kind': 'room',
            'hub_types': ['hue'],
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'lights_on': true,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
          },
        ],
      }),
    );
    await tester.pump();

    await tester.tap(
      find.byKey(const ValueKey('global-room-action-reset')),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 500));

    expect(harness.api.actionBatches.single, [
      (nodeId: 'room-1', action: 'reset_to_mode_default'),
      (nodeId: 'bedroom', action: 'reset_to_mode_default'),
    ]);
    if (screenshotPath != null && screenshotPath.isNotEmpty) {
      await expectLater(
        find.byType(AllRoomsScreen),
        matchesGoldenFile(Uri.file(screenshotPath)),
      );
    }
  });

  testWidgets('global reset also targets rooms in Mood, Low glow and off',
      (tester) async {
    final harness = await _pumpAllRooms(
      tester,
      rooms: const [_room1, _bedroom, _garage],
    );
    harness.connection.emitHello(
      RhythmHello.fromJson({
        'capabilities': {
          'api_schema_version': 2,
          'features': [RhythmFeature.resetToModeDefault],
          'hubs': const <dynamic>[],
        },
        'mode': {
          'active': 'day',
          'configs': [
            {
              'mode': 'day',
              'active_profile_id': 'rhythm',
              'room_defaults': const <dynamic>[],
            },
          ],
        },
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'hub_types': ['hue'],
            // A whole-home scene left this room in Mood.
            'state': 'mood',
            'mood_active': true,
            'rhythm_enabled': true,
            'disabled': false,
            'lights_on': true,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'profile_settings': {'mood_scene_id': 'halloween'},
          },
          {
            'id': 'bedroom',
            'name': 'Bedroom',
            'kind': 'room',
            'hub_types': ['hue'],
            'state': 'standby',
            'rhythm_enabled': true,
            'disabled': false,
            'lights_on': true,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
          },
          {
            'id': 'garage',
            'name': 'Garage',
            'kind': 'room',
            'hub_types': ['hue'],
            'state': 'hard_off',
            'rhythm_enabled': true,
            'disabled': false,
            'lights_on': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
          },
        ],
      }),
    );
    await tester.pump();

    await tester.tap(
      find.byKey(const ValueKey('global-room-action-reset')),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 500));

    expect(
      harness.api.actionBatches.single,
      [
        (nodeId: 'room-1', action: 'reset_to_mode_default'),
        (nodeId: 'bedroom', action: 'reset_to_mode_default'),
        (nodeId: 'garage', action: 'reset_to_mode_default'),
      ],
      reason: 'Reset restores every enabled room to the mode default, '
          'whatever state it is in',
    );
  });

  testWidgets('expanded slider applies one exact batch to adaptive-on rooms',
      (tester) async {
    final harness = await _pumpAllRooms(
      tester,
      rooms: const [_room1, _bedroom],
    );

    await tester.tap(
      find.byKey(const ValueKey('global-room-action-expand')),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));
    final slider = find.byKey(const ValueKey('global-room-brightness-slider'));

    await tester.drag(slider, const Offset(90, 0));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 500));

    expect(harness.api.brightnessBatches, hasLength(1));
    final applied = harness.api.brightnessBatches.single;
    expect(applied.map((item) => item.nodeId), ['room-1', 'bedroom']);
    expect(applied.first.brightness, greaterThan(50));
    expect(
      applied.every((item) => item.brightness == applied.first.brightness),
      isTrue,
    );
    expect(find.text('Adjusted 2 of 2 rooms.'), findsOneWidget);
  });

  testWidgets('room cards show the shared title and settings affordance',
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
    expect(find.byIcon(Icons.settings_rounded), findsOneWidget);
    expect(
      find.byKey(const ValueKey('room-card-settings-room-1')),
      findsOneWidget,
    );
    expect(find.byIcon(Icons.meeting_room_rounded), findsNothing);
    expect(find.byIcon(Icons.lightbulb_outline_rounded), findsNothing);
  });

  testWidgets('rooms and individual bulbs both use full-width cards',
      (tester) async {
    await _pumpAllRooms(
      tester,
      rooms: const [_bulb1, _room1, _switch1],
    );

    final bulb = tester.getRect(_roomCardContainer('bulb-1'));
    final room = tester.getRect(_roomCardContainer('room-1'));
    final fullWidth = tester.getRect(_roomCardContainer('switch-1'));

    expect(room.top, greaterThan(bulb.top));
    expect(bulb.width, closeTo(room.width, 0.1));
    expect(fullWidth.width, closeTo(room.width, 0.1));
    expect(fullWidth.left, closeTo(bulb.left, 0.1));
  });

  testWidgets('bulb cards show mood for mood state', (tester) async {
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
    expect(find.text('Scenes'), findsOneWidget);
  });

  testWidgets('full-width room and bulb sizes persist when edit mode starts',
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
    expect(afterRoom1.top, greaterThan(afterBulb1.top));
  });

  testWidgets('edit-mode drag reorders full-width bulb cards', (tester) async {
    final harness = await _pumpAllRooms(
      tester,
      rooms: const [_bulb1, _bulb2],
    );

    await tester.longPress(find.text('Bulb 1'));
    await tester.pump();

    expect(find.text('Edit Rooms'), findsOneWidget);
    expect(
      harness.roomPageProvider
          .getRoomsForPage(0, const [_bulb1, _bulb2]).map((room) => room.id),
      ['bulb-1', 'bulb-2'],
    );

    final bulb = tester.getRect(_roomCardContainer('bulb-1'));
    final bulb2 = tester.getRect(_roomCardContainer('bulb-2'));
    final gesture = await tester.startGesture(bulb.center);
    await tester.pump(const Duration(milliseconds: 20));
    await gesture.moveBy(const Offset(0, 24));
    await tester.pump(const Duration(milliseconds: 20));
    await gesture.moveTo(Offset(bulb2.center.dx, bulb2.bottom - 2));
    await tester.pump(const Duration(milliseconds: 20));
    await gesture.up();
    await tester.pump(const Duration(milliseconds: 300));

    expect(
      harness.roomPageProvider
          .getRoomsForPage(0, const [_bulb1, _bulb2]).map((room) => room.id),
      ['bulb-2', 'bulb-1'],
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
}
