import 'dart:async';

import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_app/app_shell.dart';
import 'package:rhythm_app/config/platform_capabilities.dart';
import 'package:rhythm_app/models/config_model.dart';
import 'package:rhythm_app/models/plan_tier.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/providers/hub_connection_provider.dart';
import 'package:rhythm_app/providers/room_page_provider.dart';
import 'package:rhythm_app/providers/room_provider.dart';
import 'package:rhythm_app/providers/server_sync_provider.dart';
import 'package:rhythm_app/providers/subscription_provider.dart';
import 'package:rhythm_app/services/hue/hue_service_locator.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'mocks/mock_rhythm_api.dart';

class _FakeHomeProvider extends HomeProvider {
  _FakeHomeProvider(List<Hub> hubs) : _hubs = List<Hub>.from(hubs);

  final List<Hub> _hubs;

  @override
  List<Hub> get currentHomeHubs => List<Hub>.unmodifiable(_hubs);

  @override
  Hub? getFirstHubOfType(HubType type) {
    for (final hub in _hubs) {
      if (hub.type == type) return hub;
    }
    return null;
  }

  @override
  Future<bool> deleteHub(String hubId) async {
    _hubs.removeWhere((hub) => hub.id == hubId);
    notifyListeners();
    return true;
  }

  @override
  Future<void> onUserSignIn() async {}
}

class _FakeSubscriptionProvider extends ChangeNotifier
    implements SubscriptionProvider {
  @override
  PlanTier get tier => PlanTier.basic;

  @override
  bool get isPro => tier == PlanTier.pro;

  @override
  bool has(Entitlement e) => tier.grants(e) && !e.isComingSoon;

  @override
  bool isEligibleFor(Entitlement e) => tier.grants(e);

  @override
  bool get isDemoOverrideActive => false;

  @override
  PlanTier? get demoOverride => null;

  @override
  Future<void> setDemoOverride(PlanTier? tier) async {}

  @override
  Future<void> changePlan(PlanTier tier) async {}

  @override
  Future<void> refresh() async {}
}

class _FakeRhythmServerApi extends RhythmServerApi {
  _FakeRhythmServerApi() : super(Dio());

  int getModeCallCount = 0;
  int getProfilesCallCount = 0;
  int getConfigCallCount = 0;
  int getCurveDataCallCount = 0;

  @override
  Future<Map<String, dynamic>?> getTriageCount() async => null;

  @override
  Future<RhythmModeResource?> getMode() async {
    getModeCallCount++;
    return const RhythmModeResource(
      active: RhythmMode.day,
      configs: [
        RhythmModeConfig(
          mode: RhythmMode.day,
          activeProfileId: 'rhythm',
        ),
        RhythmModeConfig(
          mode: RhythmMode.sleep,
          activeProfileId: 'sleep',
        ),
      ],
    );
  }

  @override
  Future<List<RhythmCurveConfig>> getProfiles() async {
    getProfilesCallCount++;
    return const [
      RhythmCurveConfig(id: 'rhythm', name: 'Day Profile'),
      RhythmCurveConfig(id: 'sleep', name: 'Sleep Profile'),
    ];
  }

  @override
  Future<RhythmCurveConfig?> getConfig({required String id}) async {
    getConfigCallCount++;
    return RhythmCurveConfig(id: id, name: id);
  }

  @override
  Future<RhythmCurveData?> getCurveData({
    required String id,
    RhythmCurveConfig? overrides,
    DateTime? date,
    int samplesPerHour = 4,
    double startHour = 12,
    int? maxSteps,
  }) async {
    getCurveDataCallCount++;
    return RhythmCurveData(
      hours: List.generate(24, (i) => i.toDouble()),
      brightness: List.filled(24, 50),
      kelvin: List.filled(24, 3000),
      solar: RhythmSolarInfo(
        sunrise: 6,
        sunset: 20,
        solarNoon: 12,
        solarMidnight: 0,
        dayLength: 14,
      ),
    );
  }
}

class _TestRhythmConnection extends RhythmConnection {
  _TestRhythmConnection({
    RhythmConnectionState initialState = RhythmConnectionState.disconnected,
    RhythmServerApi? api,
  })  : _connectionState = initialState,
        _api = api ?? _FakeRhythmServerApi();

  final RhythmServerApi _api;
  RhythmConnectionState _connectionState;
  bool _disposed = false;

  final _helloController = StreamController<RhythmHello>.broadcast();
  final _connectionStateController =
      StreamController<RhythmConnectionState>.broadcast();

  @override
  RhythmServerApi get api => _api;

  @override
  bool get connected => _connectionState == RhythmConnectionState.connected;

  @override
  RhythmConnectionState get connectionState => _connectionState;

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
      _connectionStateController.stream;

  void setConnectionState(RhythmConnectionState state) {
    _connectionState = state;
    if (_disposed) return;
    _connectionStateController.add(state);
  }

  @override
  Future<void> pingOrReconnect() async {}

  @override
  Future<void> reconnect({bool authoritative = false}) async {}

  @override
  void disconnect() {
    _connectionState = RhythmConnectionState.disconnected;
    if (_disposed) return;
    _connectionStateController.add(_connectionState);
  }

  @override
  void dispose() {
    _disposed = true;
    super.dispose();
    _helloController.close();
    _connectionStateController.close();
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

Hub _serverHub() => Hub.create(
      id: 'server-1',
      homeId: 'home-1',
      type: HubType.server,
      name: 'RhythmServer',
      endpoint: const HubEndpoint(host: '127.0.0.1', port: 54448),
    );

Future<void> _seedRoom(RoomProvider roomProvider) {
  return roomProvider.addRoomsFromSource(
    RoomSourceDto.matter,
    const [
      RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.matter,
        kind: RoomNodeKind.room,
        deviceIds: ['light-1'],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: true,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    ],
  );
}

Future<void> _pumpAppShell(
  WidgetTester tester, {
  required RoomProvider roomProvider,
  required HomeProvider homeProvider,
  required ServerSyncProvider serverSync,
  RoomPageProvider? roomPageProvider,
  MockRhythmApi? api,
  Size surfaceSize = const Size(390, 844),
}) async {
  await tester.binding.setSurfaceSize(surfaceSize);

  await tester.pumpWidget(
    MultiProvider(
      providers: [
        ChangeNotifierProvider<ConfigModel>(
          create: (_) => ConfigModel(),
        ),
        Provider<PlatformCapabilities>.value(
          value: const PlatformCapabilities(
            hasAccounts: true,
            hasHubPairing: true,
            hasLocationSetup: true,
            autoImportsRooms: false,
            hasCloudBackend: true,
          ),
        ),
        ChangeNotifierProvider<RoomProvider>.value(value: roomProvider),
        ChangeNotifierProvider<HomeProvider>.value(value: homeProvider),
        ChangeNotifierProvider<ServerSyncProvider>.value(value: serverSync),
        ChangeNotifierProvider<SubscriptionProvider>(
          create: (_) => _FakeSubscriptionProvider(),
        ),
        ChangeNotifierProvider<RoomPageProvider>.value(
          value: roomPageProvider ??
              RoomPageProvider(
                layoutStore: _MemoryRoomPageLayoutStore(),
              ),
        ),
        ChangeNotifierProvider<HubConnectionProvider>(
          create: (_) => HubConnectionProvider(),
        ),
        Provider<RhythmApi>.value(value: api ?? MockRhythmApi()),
        Provider<RhythmConnection>.value(value: serverSync.connection),
      ],
      child: const MaterialApp(
        home: TickerMode(
          enabled: false,
          child: AppShell(),
        ),
      ),
    ),
  );

  await tester.pump();
  await tester.pump(const Duration(milliseconds: 10));
  await tester.pump(const Duration(milliseconds: 10));
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUp(() {
    SharedPreferences.setMockInitialValues({});
    HueServiceLocator.setDemoMode(false);
  });

  tearDown(() {
    HueServiceLocator.setDemoMode(false);
  });

  testWidgets('shows server setup when no server hub or rooms exist',
      (tester) async {
    final roomProvider = RoomProvider();
    final homeProvider = _FakeHomeProvider(const []);
    final connection = _TestRhythmConnection();
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await _pumpAppShell(
      tester,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
      serverSync: serverSync,
    );

    expect(find.text('Do you have\na LightBox?'), findsOneWidget);
  });

  testWidgets(
      'shows Setting up during the first server connection with no rooms',
      (tester) async {
    final roomProvider = RoomProvider();
    final homeProvider = _FakeHomeProvider([_serverHub()]);
    final connection = _TestRhythmConnection(
      initialState: RhythmConnectionState.connecting,
    );
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await _pumpAppShell(
      tester,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
      serverSync: serverSync,
    );

    expect(find.text('Setting up...'), findsOneWidget);
  });

  testWidgets(
      'shows Add Hubs instead of Setting up when the server is connected with no rooms',
      (tester) async {
    final roomProvider = RoomProvider();
    final homeProvider = _FakeHomeProvider([_serverHub()]);
    final connection = _TestRhythmConnection(
      initialState: RhythmConnectionState.connected,
    );
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await _pumpAppShell(
      tester,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
      serverSync: serverSync,
    );

    expect(find.text('Add Hubs'), findsOneWidget);
    expect(find.text('Setting up...'), findsNothing);
  });

  testWidgets(
      'keeps the disconnected screen visible across reconnect attempts until connected',
      (tester) async {
    final roomProvider = RoomProvider();
    final homeProvider = _FakeHomeProvider([_serverHub()]);
    final connection = _TestRhythmConnection(
      initialState: RhythmConnectionState.reconnecting,
    );
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await _pumpAppShell(
      tester,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
      serverSync: serverSync,
    );

    expect(find.text('Server Unreachable'), findsOneWidget);

    connection.setConnectionState(RhythmConnectionState.connecting);
    await tester.pump(const Duration(milliseconds: 10));
    expect(find.text('Server Unreachable'), findsOneWidget);

    connection.setConnectionState(RhythmConnectionState.connected);
    await tester.pump(const Duration(milliseconds: 10));
    expect(find.text('Server Unreachable'), findsNothing);
    expect(find.text('Add Hubs'), findsOneWidget);
  });

  testWidgets('pops pushed routes when the paired server hub is removed',
      (tester) async {
    final roomProvider = RoomProvider();
    final homeProvider = _FakeHomeProvider([_serverHub()]);
    final connection = _TestRhythmConnection(
      initialState: RhythmConnectionState.connected,
    );
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await _pumpAppShell(
      tester,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
      serverSync: serverSync,
    );

    final nestedNavigatorContext = tester.element(find.text('Add Hubs'));
    unawaited(
      Navigator.of(nestedNavigatorContext).push(
        MaterialPageRoute<void>(
          builder: (_) => const Scaffold(body: Text('Pushed route')),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Pushed route'), findsOneWidget);

    await homeProvider.deleteHub('server-1');
    await tester.pumpAndSettle();

    expect(find.text('Pushed route'), findsNothing);
    expect(find.text('Do you have\na LightBox?'), findsOneWidget);
  });

  testWidgets('shows the room grid when the connected server has rooms',
      (tester) async {
    final roomProvider = RoomProvider();
    await _seedRoom(roomProvider);
    final homeProvider = _FakeHomeProvider([_serverHub()]);
    final connection = _TestRhythmConnection(
      initialState: RhythmConnectionState.connected,
    );
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await _pumpAppShell(
      tester,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
      serverSync: serverSync,
    );

    expect(find.text('Kitchen'), findsOneWidget);
  });

  testWidgets(
      'shows the room grid while a server with cached rooms is still connecting',
      (tester) async {
    final roomProvider = RoomProvider();
    await _seedRoom(roomProvider);
    final homeProvider = _FakeHomeProvider([_serverHub()]);
    final connection = _TestRhythmConnection(
      initialState: RhythmConnectionState.connecting,
    );
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await _pumpAppShell(
      tester,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
      serverSync: serverSync,
    );

    expect(find.text('Kitchen'), findsOneWidget);
  });

  testWidgets('shows the room grid when rooms exist without a server hub',
      (tester) async {
    final roomProvider = RoomProvider();
    await _seedRoom(roomProvider);
    final homeProvider = _FakeHomeProvider(const []);
    final connection = _TestRhythmConnection();
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await _pumpAppShell(
      tester,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
      serverSync: serverSync,
    );

    expect(find.text('Kitchen'), findsOneWidget);
  });

  testWidgets('lazily mounts bottom-nav editor tabs', (tester) async {
    final roomProvider = RoomProvider();
    final homeProvider = _FakeHomeProvider([_serverHub()]);
    final api = _FakeRhythmServerApi();
    final connection = _TestRhythmConnection(
      initialState: RhythmConnectionState.connected,
      api: api,
    );
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await _pumpAppShell(
      tester,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
      serverSync: serverSync,
    );

    expect(api.getModeCallCount, 0);
    expect(api.getProfilesCallCount, 0);
    expect(find.text('Day Profile'), findsNothing);

    await tester.tap(find.text('Transition'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 10));

    expect(find.text('How Day and Sleep change hands'), findsOneWidget);
    expect(api.getModeCallCount, 0);
    expect(api.getProfilesCallCount, 0);

    await tester.tap(find.text('Day'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 10));

    expect(api.getModeCallCount, 1);
    expect(api.getProfilesCallCount, 1);
    expect(api.getCurveDataCallCount, 1);
    expect(find.text('Day Profile'), findsOneWidget);

    await tester.tap(find.text('Sleep'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 10));

    expect(api.getModeCallCount, 2);
    expect(api.getProfilesCallCount, 2);
    expect(find.text('Sleep Profile'), findsOneWidget);
  });
}
