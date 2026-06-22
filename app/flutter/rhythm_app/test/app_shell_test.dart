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
import 'package:rhythm_app/screens/all_rooms_screen.dart';
import 'package:rhythm_app/screens/server_disconnected_screen.dart';
import 'package:rhythm_app/services/hue/hue_service_locator.dart';
import 'package:rhythm_app/widgets/main_bottom_nav.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'mocks/mock_rhythm_api.dart';

class _FakeHomeProvider extends HomeProvider {
  _FakeHomeProvider(List<Hub> hubs, {Home? currentHome})
      : _hubs = List<Hub>.from(hubs),
        _currentHome = currentHome;

  final List<Hub> _hubs;
  final Home? _currentHome;

  @override
  Home? get currentHome => _currentHome;

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
  int triggerTransitionCallCount = 0;
  String? lastTriggeredTransitionId;
  RhythmLightRuntime selectedLightRuntime = RhythmLightRuntime.rhythmAdaptive;
  List<RhythmModeTransitionConfig> transitions = const [];
  Completer<bool>? triggerTransitionCompleter;

  @override
  Future<Map<String, dynamic>?> getTriageCount() async => null;

  @override
  Future<RhythmModeResource?> getMode() async {
    getModeCallCount++;
    final lightRuntime = selectedLightRuntime;
    final dayProfileId = lightRuntime.defaultDayProfileId;
    return RhythmModeResource(
      active: RhythmMode.day,
      lightRuntime: lightRuntime,
      hasLightRuntime: true,
      configs: [
        RhythmModeConfig(
          mode: RhythmMode.day,
          activeProfileId: dayProfileId,
        ),
        const RhythmModeConfig(
          mode: RhythmMode.sleep,
          activeProfileId: 'sleep',
        ),
      ],
    );
  }

  @override
  Future<RhythmLightRuntimeState?> setLightRuntime(
    RhythmLightRuntime runtime, {
    int? transitionMs,
  }) async {
    selectedLightRuntime = runtime;
    return RhythmLightRuntimeState(
      runtime: runtime,
      availableRuntimes: RhythmLightRuntime.values,
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

  @override
  Future<bool> modeSet({
    RhythmMode? active,
    List<RhythmModeConfig>? configs,
  }) async {
    return true;
  }

  @override
  Future<bool> setTransitions(
    List<RhythmModeTransitionConfig> transitions,
  ) async {
    this.transitions = List<RhythmModeTransitionConfig>.unmodifiable(
      transitions,
    );
    return true;
  }

  @override
  Future<List<RhythmModeTransitionConfig>> getTransitions() async {
    return transitions;
  }

  @override
  Future<bool> triggerTransition(String id) {
    triggerTransitionCallCount++;
    lastTriggeredTransitionId = id;
    return triggerTransitionCompleter?.future ?? Future.value(true);
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
  int reconnectCallCount = 0;
  bool? lastReconnectAuthoritative;

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

  void emitHello(RhythmHello hello) {
    if (_disposed) return;
    _helloController.add(hello);
  }

  void setConnectionState(RhythmConnectionState state) {
    _connectionState = state;
    if (_disposed) return;
    _connectionStateController.add(state);
  }

  @override
  Future<void> pingOrReconnect() async {}

  @override
  Future<void> connect(
    String host, {
    int port = 80,
    bool useSsl = false,
    String? webBaseUrl,
    String? authToken,
  }) async {}

  @override
  Future<void> reconnect({bool authoritative = false}) async {
    reconnectCallCount++;
    lastReconnectAuthoritative = authoritative;
  }

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

Hub _serverHub({bool remote = false}) => Hub.create(
      id: 'server-1',
      homeId: 'home-1',
      type: HubType.server,
      name: 'RhythmServer',
      endpoint: const HubEndpoint(host: '127.0.0.1', port: 54448),
      remoteEndpoint: remote
          ? const HubEndpoint(
              host: 'remote.rhythm.test', port: 443, useSsl: true)
          : null,
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

void _emitSyncedHello(
  _TestRhythmConnection connection, {
  bool withKitchen = false,
  RhythmMode activeMode = RhythmMode.day,
}) {
  connection.emitHello(
    RhythmHello.fromJson({
      'nodes': withKitchen
          ? [
              {
                'id': 'room-1',
                'name': 'Kitchen',
                'kind': 'room',
                'hub_types': ['matter'],
                'state': 'active',
                'rhythm_enabled': true,
                'disabled': false,
                'time_offset': 0.0,
                'brightness_offset': 0.0,
                'lights_on': true,
              },
            ]
          : const <Map<String, dynamic>>[],
      'mode': {
        'active': activeMode == RhythmMode.sleep ? 'sleep' : 'day',
      },
      'location': const <String, dynamic>{},
    }),
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
  bool tickerModeEnabled = false,
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
      child: MaterialApp(
        home: TickerMode(
          enabled: tickerModeEnabled,
          child: const AppShell(),
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

    _emitSyncedHello(connection);
    await tester.pump(const Duration(milliseconds: 10));

    expect(find.text('Add Hubs'), findsOneWidget);
    expect(find.text('Setting up...'), findsNothing);
  });

  testWidgets('uses the Home name during home entry refresh', (tester) async {
    final roomProvider = RoomProvider();
    final home = Home.create(
      id: 'home-1',
      name: 'Kitchen',
      ownerId: 'user-1',
    );
    final homeProvider =
        _FakeHomeProvider([_serverHub(remote: true)], currentHome: home);
    final connection = _TestRhythmConnection(
      initialState: RhythmConnectionState.reconnecting,
    );
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    )..beginHomeEntryRefresh(homeName: home.name);
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
    expect(find.text('Connecting to Home'), findsNothing);
  });

  testWidgets(
      'home entry refresh overlays non-home tabs and hides shell chrome',
      (tester) async {
    final roomProvider = RoomProvider();
    final home = Home.create(
      id: 'home-1',
      name: 'Kitchen',
      ownerId: 'user-1',
    );
    final homeProvider =
        _FakeHomeProvider([_serverHub(remote: true)], currentHome: home);
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

    _emitSyncedHello(connection);
    await tester.pump(const Duration(milliseconds: 10));

    await tester.tap(find.text('Settings'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 10));
    expect(find.text('Settings'), findsWidgets);
    expect(find.byType(MainBottomNav), findsOneWidget);

    serverSync.beginHomeEntryRefresh(homeName: home.name);
    await tester.pump(const Duration(milliseconds: 10));

    expect(find.text('Kitchen'), findsOneWidget);
    expect(find.byType(MainBottomNav), findsNothing);

    serverSync.cancelHomeEntryRefresh();
    await tester.pump(const Duration(milliseconds: 10));
  });

  testWidgets(
      'keeps the disconnected screen visible across reconnect attempts until connected',
      (tester) async {
    final roomProvider = RoomProvider();
    final homeProvider = _FakeHomeProvider([_serverHub(remote: true)]);
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

    await tester.pump(const Duration(seconds: 5));

    expect(find.text('Server Unreachable'), findsOneWidget);
    expect(find.text('Waiting to Retry...'), findsOneWidget);
    expect(find.text('Trying local'), findsNothing);
    expect(find.text('Trying remote'), findsNothing);
    expect(find.text('Retry Now'), findsNothing);
    expect(find.text('Forget Server'), findsNothing);
    expect(find.text('Choose a different Home'), findsOneWidget);

    connection.setConnectionState(RhythmConnectionState.connecting);
    await tester.pump(const Duration(milliseconds: 10));
    expect(find.text('Server Unreachable'), findsOneWidget);

    connection.setConnectionState(RhythmConnectionState.connected);
    await tester.pump(const Duration(milliseconds: 10));
    expect(find.text('Server Unreachable'), findsNothing);
    _emitSyncedHello(connection);
    await tester.pump(const Duration(milliseconds: 10));
    expect(find.text('Add Hubs'), findsOneWidget);
  });

  testWidgets('server unreachable screen indicates connecting then waiting',
      (tester) async {
    final retryCompleter = Completer<void>();
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 844));
    await tester.pumpWidget(
      MaterialApp(
        home: ServerDisconnectedScreen(
          serverHub: _serverHub(remote: true),
          retryInterval: const Duration(minutes: 1),
          onRetry: () => retryCompleter.future,
          onChooseHome: () {},
        ),
      ),
    );

    await tester.pump();
    expect(find.text('Connecting...'), findsOneWidget);
    expect(find.text('Trying local'), findsNothing);
    expect(find.text('Trying remote'), findsNothing);

    retryCompleter.complete();
    await tester.pump();
    expect(find.text('Waiting to Retry...'), findsOneWidget);
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
    _emitSyncedHello(connection);
    await tester.pump(const Duration(milliseconds: 10));

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

  testWidgets(
      'returns to the LightBox onboarding gate when server hub is removed from another tab',
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

    await tester.tap(find.text('Settings'));
    await tester.pumpAndSettle();

    expect(find.text('Do you have\na LightBox?'), findsNothing);

    await homeProvider.deleteHub('server-1');
    await tester.pumpAndSettle();

    expect(find.text('Do you have\na LightBox?'), findsOneWidget);
    expect(find.byType(MainBottomNav), findsNothing);
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
    _emitSyncedHello(connection, withKitchen: true);
    await tester.pump(const Duration(milliseconds: 10));

    expect(find.text('Kitchen'), findsOneWidget);
  });

  testWidgets('app resume refresh keeps synced Home on the room grid',
      (tester) async {
    final roomProvider = RoomProvider();
    await _seedRoom(roomProvider);
    final home = Home.create(
      id: 'home-1',
      name: 'Kitchen',
      ownerId: 'user-1',
    );
    final hub = _serverHub(remote: true).copyWith(
      token: 'owner-token',
      homeId: home.id,
    );
    final homeProvider = _FakeHomeProvider([hub], currentHome: home);
    final connection = _TestRhythmConnection(
      initialState: RhythmConnectionState.connected,
    );
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
      endpointReachability: (_, __) async => false,
    );
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await serverSync.retryActiveServerConnection(
      assumeLanReachable: true,
      assumeSavedAuth: true,
    );

    await _pumpAppShell(
      tester,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
      serverSync: serverSync,
    );
    _emitSyncedHello(connection, withKitchen: true);
    await tester.pump(const Duration(milliseconds: 10));

    expect(find.text('Kitchen'), findsOneWidget);

    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 10));

    expect(serverSync.hasHomeEntryRefreshGate, isFalse);
    expect(connection.reconnectCallCount, 1);
    expect(connection.lastReconnectAuthoritative, isTrue);
    expect(find.text('Waiting to Retry...'), findsNothing);
    expect(find.byType(ServerDisconnectedScreen), findsNothing);
    expect(find.text('Kitchen'), findsOneWidget);
  });

  testWidgets(
      'shows setup loading while a server with cached rooms is still connecting',
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

    expect(find.text('Setting up...'), findsOneWidget);
    expect(find.text('Kitchen'), findsNothing);
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
    _emitSyncedHello(connection);
    await tester.pump(const Duration(milliseconds: 10));

    expect(api.getModeCallCount, 0);
    expect(api.getProfilesCallCount, 0);
    expect(find.text('Day'), findsNothing);

    // The Automations list reads already-synced provider data, so opening it
    // must not trigger the profile-loading server API.
    await tester.tap(find.text('Schedules'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 10));

    expect(find.text('Time Schedule'), findsOneWidget);
    expect(api.getModeCallCount, 0);
    expect(api.getProfilesCallCount, 0);

    // The Light tab stacks the Day + Sleep look profiles; each loads its own
    // config, so opening it triggers two profile loads.
    await tester.tap(find.text('Light'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 10));

    expect(find.text('Day'), findsOneWidget);
    expect(find.text('Sleep'), findsOneWidget);
    expect(api.getModeCallCount, 2);
    expect(api.getProfilesCallCount, 2);
  });

  testWidgets('refreshes default state when leaving expert mode',
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
      tickerModeEnabled: true,
    );
    _emitSyncedHello(connection, withKitchen: true);
    await tester.pump(const Duration(milliseconds: 10));

    await tester.tap(find.text('Settings').last);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 10));

    await tester.tap(find.text('Circadian Expert Mode'));
    await tester.pump();
    await tester.tap(find.text('Switch to Expert'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 700));
    await tester.pump(const Duration(milliseconds: 700));

    final reconnectsBeforeExit = connection.reconnectCallCount;
    await tester.tap(find.text('Settings').last);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 10));
    await tester.tap(find.text('Basic Mode'));
    await tester.pump();
    await tester.tap(find.text('Switch to Basic'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 700));
    await tester.pump(const Duration(milliseconds: 700));

    expect(connection.reconnectCallCount, reconnectsBeforeExit + 1);
    expect(connection.lastReconnectAuthoritative, isTrue);
    expect(find.text('Kitchen'), findsOneWidget);
  });

  testWidgets('keeps expert shell after reconnect hello omits runtime',
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
      tickerModeEnabled: true,
    );
    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'hub_types': ['matter'],
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': true,
          },
        ],
        'light_runtime': 'removed-circadian',
        'mode': {
          'active': 'day',
          'light_runtime': 'removed-circadian',
        },
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));
    await tester.pump(const Duration(milliseconds: 700));
    await tester.pump(const Duration(milliseconds: 700));

    expect(find.text('Rhythm'), findsOneWidget);
    expect(find.text('Light'), findsNothing);

    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': const <Map<String, dynamic>>[],
        'mode': {'active': 'day'},
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));
    await tester.pump(const Duration(milliseconds: 700));
    await tester.pump(const Duration(milliseconds: 700));

    expect(find.text('Rhythm'), findsOneWidget);
    expect(find.text('Light'), findsNothing);
  });

  testWidgets('mode toggle disables immediately while transition is pending',
      (tester) async {
    final roomProvider = RoomProvider();
    await _seedRoom(roomProvider);
    final homeProvider = _FakeHomeProvider([_serverHub()]);
    final api = _FakeRhythmServerApi();
    final transitionCompleter = Completer<bool>();
    api.triggerTransitionCompleter = transitionCompleter;
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
    _emitSyncedHello(connection, withKitchen: true);
    await tester.pump(const Duration(milliseconds: 10));

    await serverSync.dispatchSetTransitions(const [
      RhythmModeTransitionConfig(
        id: 'sleep_to_day',
        label: 'Sleep to Day',
        fromMode: RhythmMode.sleep,
        toMode: RhythmMode.day,
        trigger: RhythmTransitionTrigger.solar('sunrise'),
        duration: TransitionDuration.auto(),
        preserveHardOff: true,
      ),
      RhythmModeTransitionConfig(
        id: 'day_to_sleep',
        label: 'Day to Sleep',
        fromMode: RhythmMode.day,
        toMode: RhythmMode.sleep,
        trigger: RhythmTransitionTrigger.solar('sunset'),
        duration: TransitionDuration.auto(),
        preserveHardOff: true,
      ),
    ]);
    await serverSync.dispatchSetActiveMode(RhythmMode.day);
    await tester.pump(const Duration(milliseconds: 10));

    final roomGrid = find.byType(AllRoomsScreen);
    final sleepPill = find.descendant(
      of: roomGrid,
      matching: find.text('Sleep'),
    );
    final dayPill = find.descendant(
      of: roomGrid,
      matching: find.text('Day'),
    );

    await tester.tap(sleepPill);
    await tester.pump();

    expect(api.triggerTransitionCallCount, 1);
    expect(api.lastTriggeredTransitionId, 'day_to_sleep');
    expect(
      find.descendant(
        of: roomGrid,
        matching: find.byType(CircularProgressIndicator),
      ),
      findsOneWidget,
    );

    await tester.tap(dayPill, warnIfMissed: false);
    await tester.tap(sleepPill, warnIfMissed: false);
    await tester.pump();

    expect(api.triggerTransitionCallCount, 1);

    transitionCompleter.complete(true);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));

    expect(
      find.descendant(
        of: roomGrid,
        matching: find.byType(CircularProgressIndicator),
      ),
      findsOneWidget,
    );

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

    expect(
      find.descendant(
        of: roomGrid,
        matching: find.byType(CircularProgressIndicator),
      ),
      findsWidgets,
    );

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

    expect(
      find.descendant(
        of: roomGrid,
        matching: find.byType(CircularProgressIndicator),
      ),
      findsNothing,
    );
  });
}
