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
import 'package:rhythm_app/screens/server_disconnected_screen.dart';
import 'package:rhythm_app/screens/settings/automations_screen.dart';
import 'package:rhythm_app/screens/settings/light_screen.dart';
import 'package:rhythm_app/screens/settings/settings_screen.dart';
import 'package:rhythm_app/services/hue/hue_service_locator.dart';
import 'package:rhythm_app/widgets/hub_connection_loading_screen.dart';
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
  bool lightBreakerEnabled = true,
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
      'light_breaker': {'enabled': lightBreakerEnabled},
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

Future<void> _openNavigationFan(WidgetTester tester) async {
  await tester.tap(find.byIcon(Icons.settings_rounded).last);
  await tester.pump();
}

Future<void> _selectNavigationFanItem(
  WidgetTester tester,
  String label,
) async {
  await _openNavigationFan(tester);
  await tester.tap(find.text(label).last);
  await tester.pump();
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

  testWidgets('startup curve load does not issue a config-state request',
      (tester) async {
    final roomProvider = RoomProvider();
    final homeProvider = _FakeHomeProvider(const []);
    final connection = _TestRhythmConnection();
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    final api = MockRhythmApi();
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await _pumpAppShell(
      tester,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
      serverSync: serverSync,
      api: api,
    );

    expect(api.getConfigStateCallCount, 0);
    expect(api.getCurveDataCallCount, 1);
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
    expect(find.byTooltip('Choose Home'), findsOneWidget);

    await tester.tap(find.byTooltip('Choose Home'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.byIcon(Icons.close), findsOneWidget);
    expect(find.text('Welcome to Rhythm'), findsOneWidget);
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
    expect(find.byTooltip('Choose Home'), findsOneWidget);

    await tester.tap(find.byTooltip('Choose Home'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.byIcon(Icons.close), findsOneWidget);
    expect(find.text('Welcome to Rhythm'), findsOneWidget);
  });

  testWidgets('scheduled BLE startup keeps existing hello rooms available',
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

    final scheduledBleHello = RhythmHello.fromJson({
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
      'hubs': [
        {
          'type': 'hue_ble',
          'address': 'local',
          'connected': false,
          'startup_retry': {
            'status': 'scheduled',
            'attempt_count': 1,
          },
        },
      ],
      'capabilities': {
        'hubs': [
          {
            'type': 'hue_ble',
            'configurable': false,
            'blocks_room_readiness': true,
          },
        ],
      },
      'mode': {'active': 'day'},
      'light_breaker': {'enabled': true},
      'location': const <String, dynamic>{},
    });
    connection.emitHello(scheduledBleHello);
    await tester.pump(const Duration(milliseconds: 10));

    expect(find.text('Setting up...'), findsNothing);
    expect(find.text('Kitchen'), findsOneWidget);
    expect(find.byKey(const Key('all_rooms_read_only')), findsNothing);

    connection.emitHello(scheduledBleHello);
    await tester.pump(const Duration(milliseconds: 10));

    expect(find.text('Setting up...'), findsNothing);
    expect(find.text('Kitchen'), findsOneWidget);
  });

  testWidgets('uses the shared connecting screen during home entry refresh',
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

    expect(find.byType(HubConnectionLoadingScreen), findsWidgets);
    expect(find.text('Setting up...'), findsWidgets);
    expect(find.text('Connecting to your lights'), findsWidgets);
    expect(find.text('Kitchen'), findsNothing);
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

    await _selectNavigationFanItem(tester, 'Settings');
    expect(find.text('Settings'), findsWidgets);
    expect(find.byType(SettingsScreen), findsOneWidget);

    serverSync.beginHomeEntryRefresh(homeName: home.name);
    await tester.pump(const Duration(milliseconds: 10));

    expect(find.byType(HubConnectionLoadingScreen), findsWidgets);
    expect(find.text('Setting up...'), findsWidgets);
    expect(find.byType(ServerDisconnectedScreen), findsNothing);
    expect(find.byType(SettingsScreen), findsOneWidget);

    serverSync.cancelHomeEntryRefresh();
    await tester.pump(const Duration(milliseconds: 10));
  });

  testWidgets(
      'keeps the shared connecting screen visible across slow reconnect attempts',
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

    expect(find.byType(HubConnectionLoadingScreen), findsOneWidget);
    expect(find.text('Setting up...'), findsOneWidget);
    expect(find.text('Connecting to your lights'), findsOneWidget);
    expect(find.text('Connecting to Your Home'), findsNothing);
    expect(find.text('Connecting...'), findsNothing);
    expect(find.text('Trying local'), findsNothing);
    expect(find.text('Trying remote'), findsNothing);
    expect(find.text('Retry Now'), findsNothing);
    expect(find.text('Forget Server'), findsNothing);
    expect(find.text('Choose a different Home'), findsNothing);
    expect(find.byTooltip('Choose Home'), findsOneWidget);

    connection.setConnectionState(RhythmConnectionState.connecting);
    await tester.pump(const Duration(milliseconds: 10));
    expect(find.text('Setting up...'), findsOneWidget);

    connection.setConnectionState(RhythmConnectionState.connected);
    await tester.pump(const Duration(milliseconds: 10));
    expect(find.text('Setting up...'), findsOneWidget);
    _emitSyncedHello(connection);
    await tester.pump(const Duration(milliseconds: 10));
    expect(find.text('Setting up...'), findsNothing);
    expect(find.text('Add Hubs'), findsOneWidget);
  });

  testWidgets('remote server reconnect screen keeps generic retry state',
      (tester) async {
    final retryCompleter = Completer<void>();
    var chooseHomeCalls = 0;
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 844));
    await tester.pumpWidget(
      MaterialApp(
        home: ServerDisconnectedScreen(
          serverHub: _serverHub(remote: true),
          retryInterval: const Duration(minutes: 1),
          onRetry: () => retryCompleter.future,
          onChooseHome: () => chooseHomeCalls += 1,
        ),
      ),
    );

    await tester.pump();
    expect(find.text('Connecting to Your Home'), findsOneWidget);
    expect(find.text('Connecting...'), findsOneWidget);
    expect(find.text('Trying local'), findsNothing);
    expect(find.text('Trying remote'), findsNothing);

    retryCompleter.complete();
    await tester.pump();
    expect(find.text('Retrying soon...'), findsOneWidget);
    expect(find.textContaining('tunnel', findRichText: true), findsNothing);
    expect(find.byTooltip('Choose Home'), findsOneWidget);
    await tester.tap(find.byTooltip('Choose Home'));
    await tester.pump();
    expect(chooseHomeCalls, 1);
  });

  testWidgets('LAN server unreachable screen keeps generic retry state',
      (tester) async {
    final retryCompleter = Completer<void>();
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 844));
    await tester.pumpWidget(
      MaterialApp(
        home: ServerDisconnectedScreen(
          serverHub: _serverHub(),
          retryInterval: const Duration(minutes: 1),
          onRetry: () => retryCompleter.future,
          onChooseHome: () {},
        ),
      ),
    );

    await tester.pump();
    expect(find.text('Server Unreachable'), findsOneWidget);
    expect(find.text('Connecting...'), findsOneWidget);

    retryCompleter.complete();
    await tester.pump();
    expect(find.text('Retrying soon...'), findsOneWidget);
  });

  testWidgets('server unreachable screen follows provider connecting state',
      (tester) async {
    final roomProvider = RoomProvider();
    final homeProvider = _FakeHomeProvider([_serverHub(remote: true)]);
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

    await tester.binding.setSurfaceSize(const Size(390, 844));
    await tester.pumpWidget(
      ChangeNotifierProvider<ServerSyncProvider>.value(
        value: serverSync,
        child: MaterialApp(
          home: ServerDisconnectedScreen(
            serverHub: _serverHub(remote: true),
            autoRetry: false,
            retryInterval: const Duration(minutes: 1),
            onChooseHome: () {},
          ),
        ),
      ),
    );

    await tester.pump();

    expect(find.text('Connecting...'), findsOneWidget);
    expect(find.text('Retrying soon...'), findsNothing);
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

    await _selectNavigationFanItem(tester, 'Settings');

    expect(find.text('Do you have\na LightBox?'), findsNothing);

    await homeProvider.deleteHub('server-1');
    await tester.pumpAndSettle();

    expect(find.text('Do you have\na LightBox?'), findsOneWidget);
    expect(find.byIcon(Icons.settings_rounded), findsNothing);
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

  testWidgets('navigation fan owns Lighting and Settings no longer lists it',
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

    await _selectNavigationFanItem(tester, 'Lighting');

    final lightingScreen = find.byType(LightScreen);
    expect(lightingScreen, findsOneWidget);
    expect(
      find.descendant(
        of: lightingScreen,
        matching: find.text('Lighting'),
      ),
      findsOneWidget,
    );
    expect(find.byType(SettingsScreen), findsNothing);

    await tester.tap(
      find.descendant(
        of: lightingScreen,
        matching: find.byIcon(Icons.close_rounded),
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 10));

    expect(find.text('Kitchen'), findsOneWidget);

    await _selectNavigationFanItem(tester, 'Settings');

    final settingsScreen = find.byType(SettingsScreen);
    expect(settingsScreen, findsOneWidget);
    expect(
      find.descendant(
        of: settingsScreen,
        matching: find.text('Your Light'),
      ),
      findsNothing,
    );
    expect(
      find.descendant(
        of: settingsScreen,
        matching: find.text('Light'),
      ),
      findsNothing,
    );
  });

  testWidgets('settings fan opens report bug flow from the room grid',
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

    await _openNavigationFan(tester);
    expect(find.text('Report Bug'), findsOneWidget);
    await tester.pump(const Duration(milliseconds: 300));
    final reportBugTop = tester.getTopLeft(find.text('Report Bug')).dy;
    final scheduleTop = tester.getTopLeft(find.text('Schedule')).dy;
    final lightingTop = tester.getTopLeft(find.text('Lighting')).dy;
    final settingsTop = tester.getTopLeft(find.text('Settings')).dy;
    expect(reportBugTop, lessThan(scheduleTop));
    expect(scheduleTop, lessThan(lightingTop));
    expect(lightingTop, lessThan(settingsTop));

    await tester.tap(find.text('Report Bug'));
    await tester.pump();

    expect(
      find.text(
        'This sends a snapshot of recent logs and redacted state to Rhythm support.',
      ),
      findsOneWidget,
    );
    expect(find.text('What went wrong? (optional)'), findsOneWidget);
  });

  testWidgets('app resume shows cached rooms read-only until fresh hello',
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
    _emitSyncedHello(
      connection,
      withKitchen: true,
      lightBreakerEnabled: false,
    );
    await tester.pump(const Duration(milliseconds: 10));

    expect(find.text('Kitchen'), findsOneWidget);
    expect(find.text('AUTOMATIC LIGHTING OFF'), findsOneWidget);

    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(serverSync.hasHomeEntryRefreshGate, isFalse);
    expect(connection.reconnectCallCount, 1);
    expect(connection.lastReconnectAuthoritative, isTrue);
    expect(find.text('Waiting to Retry...'), findsNothing);
    expect(find.byType(ServerDisconnectedScreen), findsNothing);
    expect(find.text('Setting up...'), findsNothing);
    expect(find.text('AUTOMATIC LIGHTING OFF'), findsNothing);
    expect(find.text('Kitchen'), findsOneWidget);
    expect(find.byKey(const Key('all_rooms_read_only')), findsOneWidget);
    expect(
      find.byKey(const ValueKey('all-rooms-page-read-only-0')),
      findsOneWidget,
    );
    expect(
        find.byKey(const Key('all_rooms_connecting_banner')), findsOneWidget);

    _emitSyncedHello(
      connection,
      withKitchen: true,
      lightBreakerEnabled: false,
    );
    await tester.pump(const Duration(milliseconds: 10));

    expect(find.text('Setting up...'), findsNothing);
    expect(find.text('AUTOMATIC LIGHTING OFF'), findsOneWidget);
    expect(find.text('Kitchen'), findsOneWidget);
    expect(find.byKey(const Key('all_rooms_read_only')), findsNothing);
  });

  testWidgets(
      'shows cached rooms read-only while the server is still connecting',
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

    expect(find.text('Setting up...'), findsNothing);
    expect(find.text('Kitchen'), findsOneWidget);
    expect(find.byKey(const Key('all_rooms_read_only')), findsOneWidget);
    expect(
      find.byKey(const ValueKey('all-rooms-page-read-only-0')),
      findsOneWidget,
    );
    expect(
        find.byKey(const Key('all_rooms_connecting_banner')), findsOneWidget);
  });

  testWidgets('cached room pages remain horizontally swipeable',
      (tester) async {
    final roomProvider = RoomProvider();
    await roomProvider.addRoomsFromSource(
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
        RoomDto(
          id: 'room-2',
          name: 'Bedroom',
          source: RoomSourceDto.matter,
          kind: RoomNodeKind.room,
          deviceIds: ['light-2'],
          rhythmEnabled: true,
          disabled: false,
          lightsOn: true,
          timeOffsetMinutes: 0,
          brightnessOffset: 0,
        ),
      ],
    );
    final roomPageProvider = RoomPageProvider(
      layoutStore: _MemoryRoomPageLayoutStore(),
    )..initialize();
    roomPageProvider.reconcileRooms(roomProvider.enabledRooms);
    roomPageProvider.moveRoom('room-2', 1);
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
    addTearDown(roomPageProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await _pumpAppShell(
      tester,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
      serverSync: serverSync,
      roomPageProvider: roomPageProvider,
    );

    expect(find.byKey(const Key('all_rooms_read_only')), findsOneWidget);
    expect(find.text('Kitchen'), findsOneWidget);
    expect(find.text('Bedroom'), findsNothing);

    await tester.longPress(find.text('Kitchen'), warnIfMissed: false);
    await tester.pump();
    expect(find.text('Edit Rooms'), findsNothing);

    await tester.flingFrom(
      tester.getCenter(find.byType(PageView)),
      const Offset(-320, 0),
      1000,
    );
    await tester.pumpAndSettle();

    expect(find.text('Bedroom'), findsOneWidget);
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

  testWidgets('preset editor uses already-synced provider data',
      (tester) async {
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
    await _selectNavigationFanItem(tester, 'Schedule');

    expect(find.text('Presets'), findsOneWidget);
    expect(api.getModeCallCount, 0);
    expect(api.getProfilesCallCount, 0);

    // The preset detail also reads already-synced provider state.
    await tester.tap(find.text('Wake Presets'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 10));

    expect(find.text('Wake Presets'), findsWidgets);
    expect(api.getModeCallCount, 0);
    expect(api.getProfilesCallCount, 0);
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

    await _selectNavigationFanItem(tester, 'Schedule');

    final presetsScreen = find.byType(AutomationsScreen);
    final sleepSegment = find.descendant(
      of: presetsScreen,
      matching: find.text('Sleep'),
    );
    final wakeSegment = find.descendant(
      of: presetsScreen,
      matching: find.text('Wake'),
    );

    await tester.tap(sleepSegment);
    await tester.pump();

    expect(api.triggerTransitionCallCount, 1);
    expect(api.lastTriggeredTransitionId, 'day_to_sleep');

    await tester.tap(wakeSegment, warnIfMissed: false);
    await tester.tap(sleepSegment, warnIfMissed: false);
    await tester.pump();

    expect(api.triggerTransitionCallCount, 1);

    transitionCompleter.complete(true);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));

    expect(serverSync.activeMode, RhythmMode.sleep);
  });
}
