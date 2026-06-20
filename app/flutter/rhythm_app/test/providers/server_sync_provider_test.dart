import 'dart:async';

import 'package:connectivity_plus/connectivity_plus.dart';
import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/providers/room_provider.dart';
import 'package:rhythm_app/providers/server_sync_provider.dart';
import 'package:rhythm_app/services/account_cloud_sync_service.dart';
import 'package:rhythm_app/services/demo_server_api.dart';
import 'package:rhythm_app/services/hue/hue_service_locator.dart';
import 'package:rhythm_app/widgets/device_detail_sheet.dart';
import 'package:rhythm_app/widgets/hub_picker_screen.dart';
import 'package:rhythm_app/widgets/room_settings_sheet.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class _TestHomeProvider extends HomeProvider {
  _TestHomeProvider(
    List<Hub> hubs, {
    Home? currentHome,
    List<AccountHomeServerHubs> accountHomes = const [],
  })  : _hubs = List<Hub>.of(hubs),
        _currentHome = currentHome,
        _accountHomes = accountHomes;

  final List<Hub> _hubs;
  final List<AccountHomeServerHubs> _accountHomes;
  Home? _currentHome;

  @override
  Home? get currentHome => _currentHome;

  @override
  List<Hub> get currentHomeHubs => _hubs;

  @override
  Hub? getFirstHubOfType(HubType type) {
    for (final hub in _hubs) {
      if (hub.type == type) return hub;
    }
    return null;
  }

  @override
  Future<bool> updateCurrentHome(Home home) async {
    _currentHome = home;
    notifyListeners();
    return true;
  }

  @override
  Future<List<AccountHomeServerHubs>> loadAccountHomes() async {
    return _accountHomes;
  }

  @override
  Future<bool> updateHub(Hub hub) async {
    final index = _hubs.indexWhere((saved) => saved.id == hub.id);
    if (index == -1) {
      _hubs.add(hub);
    } else {
      _hubs[index] = hub;
    }
    notifyListeners();
    return true;
  }
}

class _FakeRhythmServerApi extends RhythmServerApi {
  _FakeRhythmServerApi() : super(Dio());

  int hubCredentialsCalls = 0;
  int hubRetryCalls = 0;
  String? lastHubType;
  String? lastAddress;
  Map<String, dynamic>? lastCredentials;
  List<RhythmTopologyNode> topologyNodes = const [];
  int assignDeviceParentCalls = 0;
  String? lastAssignedDeviceId;
  String? lastAssignedParentId;
  bool assignDeviceParentResult = true;
  int createTopologyRoomCalls = 0;
  String? lastCreatedRoomName;
  int topologyDeleteRoomCalls = 0;
  String? lastDeletedRoomId;
  bool topologyDeleteRoomResult = true;
  final Map<String, Map<String, dynamic>?> canonicalDevices = {};
  List<RhythmInputBinding> inputBindings = const [];
  int createDaySleepToggleInputBindingCalls = 0;
  int setInputBindingCalls = 0;
  int deleteInputBindingCalls = 0;
  bool createReplacesPresetBindings = false;
  String? lastInputBindingSourceNodeId;
  RhythmButtonAction? lastInputBindingButtonAction;
  RhythmInputBinding? lastSetInputBinding;
  String? lastDeletedInputBindingId;
  List<RhythmModeTransitionConfig> transitions = const [];
  List<RhythmModeTransitionConfig>? lastSetTransitions;
  int setTransitionsCalls = 0;
  bool setTransitionsResult = true;
  bool lightBreakerEnabled = true;
  bool setLightBreakerResult = true;
  int setLightBreakerCalls = 0;
  bool? lastLightBreakerEnabled;
  final List<
      ({
        String nodeId,
        bool? rhythmEnabled,
        bool? disabled,
        bool? standbyEnabled,
        RoomModeState? state,
        bool? softOff,
        Map<String, dynamic>? profileSettings,
      })> nodePreferenceCalls = [];
  final List<
      ({
        String nodeId,
        Map<String, dynamic>? profileOverrides,
      })> nodeProfileOverrideCalls = [];
  final List<
      ({
        String nodeId,
        int r,
        int g,
        int b,
        int? brightness,
        int? transitionMs,
        String? scope,
      })> nodeColorCalls = [];
  List<RhythmSceneDefinition> scenes = const [];
  final List<({String sceneId, String targetId, int? transitionMs})>
      applySceneCalls = [];
  final List<({String nodeId, int brightness})> nodeCurveBrightnessCalls = [];
  final List<
      ({
        String nodeId,
        int kelvin,
        bool preserveBrightness,
      })> nodeCurveColorTemperatureCalls = [];

  @override
  Future<void> hubCredentials({
    required String hubType,
    required String address,
    required Map<String, dynamic> credentials,
  }) async {
    hubCredentialsCalls++;
    lastHubType = hubType;
    lastAddress = address;
    lastCredentials = credentials;
  }

  @override
  Future<bool> hubRetry({
    required String hubType,
    required String address,
  }) async {
    hubRetryCalls++;
    lastHubType = hubType;
    lastAddress = address;
    return true;
  }

  @override
  Future<Map<String, dynamic>?> getTriageCount() async => null;

  @override
  Future<void> nodePreferencesSet({
    required String nodeId,
    bool? rhythmEnabled,
    bool? disabled,
    bool? standbyEnabled,
    RoomModeState? state,
    bool? softOff,
    Map<String, dynamic>? profileSettings,
  }) async {
    nodePreferenceCalls.add((
      nodeId: nodeId,
      rhythmEnabled: rhythmEnabled,
      disabled: disabled,
      standbyEnabled: standbyEnabled,
      state: state,
      softOff: softOff,
      profileSettings: profileSettings,
    ));
  }

  @override
  Future<void> nodeProfileOverridesSet({
    required String nodeId,
    required Map<String, dynamic>? profileOverrides,
  }) async {
    nodeProfileOverrideCalls.add((
      nodeId: nodeId,
      profileOverrides: profileOverrides,
    ));
  }

  @override
  Future<List<RhythmTopologyNode>> getTopologyNodes() async => topologyNodes;

  @override
  Future<List<RhythmInputBinding>> getInputBindings() async => inputBindings;

  @override
  Future<List<RhythmModeTransitionConfig>> getTransitions() async =>
      List<RhythmModeTransitionConfig>.unmodifiable(transitions);

  @override
  Future<RhythmLightBreaker?> getLightBreaker() async =>
      RhythmLightBreaker(enabled: lightBreakerEnabled);

  @override
  Future<bool> setLightBreaker(bool enabled) async {
    setLightBreakerCalls++;
    lastLightBreakerEnabled = enabled;
    if (!setLightBreakerResult) return false;
    lightBreakerEnabled = enabled;
    return true;
  }

  @override
  Future<void> nodeColor({
    required String nodeId,
    required int r,
    required int g,
    required int b,
    int? brightness,
    int? transitionMs,
    String? scope,
    RhythmNodeColorScope? colorScope,
  }) async {
    nodeColorCalls.add((
      nodeId: nodeId,
      r: r,
      g: g,
      b: b,
      brightness: brightness,
      transitionMs: transitionMs,
      scope: scope,
    ));
  }

  @override
  Future<List<RhythmSceneDefinition>> getScenes() async => scenes;

  @override
  Future<RhythmSceneActionResult?> applyScene({
    required String sceneId,
    required String targetId,
    int? transitionMs,
  }) async {
    applySceneCalls.add((
      sceneId: sceneId,
      targetId: targetId,
      transitionMs: transitionMs,
    ));
    return RhythmSceneActionResult(
      sceneId: sceneId,
      targetId: targetId,
      affectedNodeIds: [targetId],
    );
  }

  @override
  Future<RhythmRoomState?> nodeCurveBrightness({
    required String nodeId,
    required int brightness,
  }) async {
    nodeCurveBrightnessCalls.add((nodeId: nodeId, brightness: brightness));
    return null;
  }

  @override
  Future<RhythmRoomState?> nodeCurveColorTemperature({
    required String nodeId,
    required int kelvin,
    bool preserveBrightness = true,
  }) async {
    nodeCurveColorTemperatureCalls.add((
      nodeId: nodeId,
      kelvin: kelvin,
      preserveBrightness: preserveBrightness,
    ));
    return null;
  }

  @override
  Future<bool> setTransitions(
    List<RhythmModeTransitionConfig> transitions,
  ) async {
    setTransitionsCalls++;
    lastSetTransitions = List<RhythmModeTransitionConfig>.from(transitions);
    if (!setTransitionsResult) return false;
    this.transitions = List<RhythmModeTransitionConfig>.from(transitions);
    return true;
  }

  @override
  Future<List<RhythmInputBinding>> createDaySleepToggleInputBinding({
    required String sourceNodeId,
    RhythmButtonAction? buttonAction = RhythmButtonAction.onPress,
    bool enabled = true,
  }) async {
    createDaySleepToggleInputBindingCalls++;
    lastInputBindingSourceNodeId = sourceNodeId;
    lastInputBindingButtonAction = buttonAction;
    final binding = RhythmInputBinding(
      id: 'day_sleep_toggle:$sourceNodeId:${buttonAction?.wireValue ?? 'any'}',
      preset: RhythmInputBindingPreset.daySleepToggle,
      sourceNodeId: sourceNodeId,
      trigger: RhythmInputBindingTrigger.button(buttonAction: buttonAction),
      action: const RhythmModeCycleAction(
        modes: [RhythmMode.day, RhythmMode.sleep],
      ),
      enabled: enabled,
    );
    inputBindings = [
      ...inputBindings.where((existing) {
        if (existing.id == binding.id) return false;
        return !createReplacesPresetBindings ||
            existing.preset != RhythmInputBindingPreset.daySleepToggle;
      }),
      binding,
    ];
    return inputBindings;
  }

  @override
  Future<List<RhythmInputBinding>> setInputBinding(
    RhythmInputBinding binding,
  ) async {
    setInputBindingCalls++;
    lastSetInputBinding = binding;
    inputBindings = [
      for (final existing in inputBindings)
        if (existing.id == binding.id) binding else existing,
      if (!inputBindings.any((existing) => existing.id == binding.id)) binding,
    ];
    return inputBindings;
  }

  @override
  Future<List<RhythmInputBinding>> deleteInputBinding(String id) async {
    deleteInputBindingCalls++;
    lastDeletedInputBindingId = id;
    inputBindings = [
      for (final binding in inputBindings)
        if (binding.id != id) binding,
    ];
    return inputBindings;
  }

  @override
  Future<bool> assignDeviceParent(String deviceId, String? parentId) async {
    assignDeviceParentCalls++;
    lastAssignedDeviceId = deviceId;
    lastAssignedParentId = parentId;
    return assignDeviceParentResult;
  }

  @override
  Future<Map<String, dynamic>?> createTopologyRoom(String name) async {
    createTopologyRoomCalls++;
    lastCreatedRoomName = name;
    topologyNodes = [
      ...topologyNodes,
      RhythmTopologyNode.fromJson({
        'id': 'room-created',
        'name': name,
        'kind': 'room',
      }),
    ];
    return {
      'id': 'room-created',
      'name': name,
    };
  }

  @override
  Future<bool> topologyDeleteRoom(String roomId) async {
    topologyDeleteRoomCalls++;
    lastDeletedRoomId = roomId;
    return topologyDeleteRoomResult;
  }

  @override
  Future<Map<String, dynamic>?> getCanonicalDevice(String id) async {
    return canonicalDevices[id];
  }
}

class _FakeRhythmConnection extends RhythmConnection {
  _FakeRhythmConnection(this.fakeApi);

  final _FakeRhythmServerApi fakeApi;
  bool isConnected = true;
  int reconnectCalls = 0;
  bool? lastReconnectAuthoritative;
  final List<
      ({
        String host,
        int port,
        bool useSsl,
        String? authToken,
      })> connectCalls = [];

  @override
  bool get connected => isConnected;

  @override
  RhythmServerApi get api => fakeApi;

  @override
  Future<void> connect(
    String host, {
    int port = 80,
    bool useSsl = false,
    String? webBaseUrl,
    String? authToken,
  }) async {
    connectCalls.add((
      host: host,
      port: port,
      useSsl: useSsl,
      authToken: authToken,
    ));
  }

  @override
  Future<void> reconnect({bool authoritative = false}) async {
    reconnectCalls++;
    lastReconnectAuthoritative = authoritative;
  }
}

class _HelloRhythmConnection extends _FakeRhythmConnection {
  _HelloRhythmConnection(super.fakeApi);

  final _helloController = StreamController<RhythmHello>.broadcast();
  final _rhythmStateController = StreamController<RhythmRoomState>.broadcast();
  final _hubEventController = StreamController<
      ({String event, String? hubType, String? address})>.broadcast();
  final _motionTimerController =
      StreamController<RhythmMotionTimer>.broadcast();
  final _modeChangedController =
      StreamController<RhythmModeResource>.broadcast();
  final _settingsChangedController =
      StreamController<RhythmSettings>.broadcast();
  final _lightBreakerChangedController =
      StreamController<RhythmLightBreaker>.broadcast();
  final _connectionStateController =
      StreamController<RhythmConnectionState>.broadcast();

  @override
  Stream<RhythmHello> get helloEvents => _helloController.stream;

  @override
  Stream<RhythmRoomState> get rhythmStateEvents =>
      _rhythmStateController.stream;

  @override
  Stream<({String event, String? hubType, String? address})> get hubEvents =>
      _hubEventController.stream;

  @override
  Stream<RhythmMotionTimer> get motionTimerEvents =>
      _motionTimerController.stream;

  @override
  Stream<RhythmModeResource> get modeChangedEvents =>
      _modeChangedController.stream;

  @override
  Stream<RhythmSettings> get settingsChangedEvents =>
      _settingsChangedController.stream;

  @override
  Stream<RhythmLightBreaker> get lightBreakerChangedEvents =>
      _lightBreakerChangedController.stream;

  @override
  Stream<void> get newNodesDetected => const Stream<void>.empty();

  @override
  Stream<Map<String, dynamic>> get triageChangedEvents =>
      const Stream<Map<String, dynamic>>.empty();

  @override
  Stream<RhythmConnectionState> get connectionStateStream =>
      _connectionStateController.stream;

  void emitConnectionState(RhythmConnectionState state) {
    _connectionStateController.add(state);
  }

  void emitHello(RhythmHello hello) {
    _helloController.add(hello);
  }

  void emitRhythmState(RhythmRoomState state) {
    _rhythmStateController.add(state);
  }

  void emitHubEvent({
    required String event,
    String? hubType,
    String? address,
  }) {
    _hubEventController.add((
      event: event,
      hubType: hubType,
      address: address,
    ));
  }

  void emitMotionTimer(RhythmMotionTimer timer) {
    _motionTimerController.add(timer);
  }

  void emitModeChanged(RhythmModeResource mode) {
    _modeChangedController.add(mode);
  }

  void emitSettingsChanged(RhythmSettings settings) {
    _settingsChangedController.add(settings);
  }

  void emitLightBreakerChanged(RhythmLightBreaker lightBreaker) {
    _lightBreakerChangedController.add(lightBreaker);
  }

  @override
  void dispose() {
    _helloController.close();
    _rhythmStateController.close();
    _hubEventController.close();
    _motionTimerController.close();
    _modeChangedController.close();
    _settingsChangedController.close();
    _lightBreakerChangedController.close();
    _connectionStateController.close();
    super.dispose();
  }
}

Widget _buildTestApp({
  required RoomProvider roomProvider,
  required ServerSyncProvider provider,
  required Widget child,
}) {
  return MultiProvider(
    providers: [
      ChangeNotifierProvider<RoomProvider>.value(value: roomProvider),
      ChangeNotifierProvider<ServerSyncProvider>.value(value: provider),
    ],
    child: MaterialApp(
      home: Scaffold(body: child),
    ),
  );
}

Future<void> _pumpRoomSettingsSheet(
  WidgetTester tester, {
  required RoomProvider roomProvider,
  required ServerSyncProvider provider,
  required RoomDto room,
}) async {
  await tester.pumpWidget(
    _buildTestApp(
      roomProvider: roomProvider,
      provider: provider,
      child: TickerMode(
        enabled: false,
        child: RoomSettingsSheet(
          room: room,
          enableLivePreview: false,
        ),
      ),
    ),
  );
  await tester.pump(const Duration(milliseconds: 10));
}

Future<void> _selectRoomSettingsTab(
  WidgetTester tester,
  String label,
) async {
  final labelFinder = find.text(label).last;
  await tester.ensureVisible(labelFinder);
  final tapTarget = tester.getTopLeft(labelFinder) + const Offset(4, 4);
  await tester.tapAt(tapTarget);
  await tester.pump(const Duration(milliseconds: 250));
}

void _registerWidgetCleanup(WidgetTester tester) {
  addTearDown(() async {
    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pump();
  });
}

RhythmSceneDefinition _testScene(String id) => RhythmSceneDefinition(
      id: id,
      name: 'Test Scene',
      light: const RhythmLightScene(
        defaultOutput: RhythmLightSceneOutput.on(
          brightness: 55,
          color:
              RhythmLightColor.rgb(RhythmSceneRgbColor(r: 240, g: 80, b: 24)),
        ),
      ),
    );

RhythmSceneDefinition _testPaletteScene(String id) => RhythmSceneDefinition(
      id: id,
      name: 'Palette Scene',
      light: const RhythmLightScene(
        palette: [
          RhythmLightSceneOutput.on(
            brightness: 42,
            color:
                RhythmLightColor.rgb(RhythmSceneRgbColor(r: 40, g: 90, b: 210)),
          ),
          RhythmLightSceneOutput.on(
            brightness: 80,
            color:
                RhythmLightColor.rgb(RhythmSceneRgbColor(r: 250, g: 80, b: 40)),
          ),
        ],
      ),
    );

RhythmSceneDefinition _testOffScene(String id) => RhythmSceneDefinition(
      id: id,
      name: 'Off Scene',
      light: const RhythmLightScene(
        defaultOutput: RhythmLightSceneOutput.off(),
      ),
    );

RhythmSceneDefinition _testPresetOnlyScene(String id) =>
    RhythmSceneDefinition.fromJson({
      'id': id,
      'name': 'Reset Scene',
      'light': {
        'entries': [
          {
            'target': {
              'kind': 'node',
              'node_id': 'room-1',
            },
            'value': {
              'kind': 'preset',
              'preset': 'reset',
            },
          },
        ],
      },
    });

void main() {
  group('ServerSyncProvider.pushHubCredentials', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _FakeRhythmConnection connection;

    setUp(() {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _FakeRhythmConnection(api);
    });

    tearDown(() {
      roomProvider.dispose();
      connection.dispose();
    });

    test('pushes Hue credentials and reconnects immediately', () async {
      final homeProvider = _TestHomeProvider([
        Hub.hue(
          id: 'hue-1',
          homeId: 'home-1',
          name: 'Philips Hue',
          bridgeIp: '192.168.1.20',
          appKey: 'hue-user',
        ),
      ]);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
      );
      addTearDown(provider.dispose);

      await provider.pushHubCredentials(RoomSourceDto.hue);

      expect(api.hubCredentialsCalls, 1);
      expect(api.lastHubType, 'hue');
      expect(api.lastAddress, '192.168.1.20:443');
      expect(api.lastCredentials, {'username': 'hue-user'});
      expect(connection.reconnectCalls, 1);
    });

    test('pushes Home Assistant credentials with token payload', () async {
      final homeProvider = _TestHomeProvider([
        Hub.homeAssistant(
          id: 'ha-1',
          homeId: 'home-1',
          name: 'Home Assistant',
          host: 'ha.local',
          port: 8123,
          useSsl: false,
          token: 'ha-token',
        ),
      ]);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
      );
      addTearDown(provider.dispose);

      await provider.pushHubCredentials(RoomSourceDto.homeAssistant);

      expect(api.hubCredentialsCalls, 1);
      expect(api.lastHubType, 'homeassistant');
      expect(api.lastAddress, 'ha.local:8123');
      expect(api.lastCredentials, {'token': 'ha-token'});
      expect(connection.reconnectCalls, 1);
    });

    test('does nothing when the server is disconnected', () async {
      connection.isConnected = false;
      final homeProvider = _TestHomeProvider([
        Hub.hue(
          id: 'hue-1',
          homeId: 'home-1',
          name: 'Philips Hue',
          bridgeIp: '192.168.1.20',
          appKey: 'hue-user',
        ),
      ]);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
      );
      addTearDown(provider.dispose);

      await provider.pushHubCredentials(RoomSourceDto.hue);

      expect(api.hubCredentialsCalls, 0);
      expect(connection.reconnectCalls, 0);
    });
  });

  group('ServerSyncProvider.retryHub', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _FakeRhythmConnection connection;

    setUp(() {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _FakeRhythmConnection(api);
    });

    tearDown(() {
      roomProvider.dispose();
      connection.dispose();
    });

    test('calls /api/hub/retry and refreshes state once', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      final ok = await provider.retryHub('hue', '192.168.1.20:443');

      expect(ok, isTrue);
      expect(api.hubRetryCalls, 1);
      expect(api.lastHubType, 'hue');
      expect(api.lastAddress, '192.168.1.20:443');
      expect(connection.reconnectCalls, 1);
    });

    test('retries multiple hubs and reconnects once', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      final accepted = await provider.retryHubs([
        {
          'type': 'hue',
          'address': '192.168.1.20:443',
          'startup_retry': {'status': 'manual_retry_required'},
        },
        {
          'type': 'homeassistant',
          'address': 'ha.local:8123',
          'startup_retry': {'status': 'manual_retry_required'},
        },
      ]);

      expect(accepted, 2);
      expect(api.hubRetryCalls, 2);
      expect(connection.reconnectCalls, 1);
    });
  });

  group('ServerSyncProvider Matter capabilities', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _HelloRhythmConnection connection;

    setUp(() {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _HelloRhythmConnection(api);
    });

    tearDown(() {
      roomProvider.dispose();
      connection.dispose();
    });

    test('uses device_onboarding_methods from the hello capabilities payload',
        () async {
      final homeProvider = _TestHomeProvider(const []);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'rooms': const <Map<String, dynamic>>[],
          'location': const <String, dynamic>{},
          'capabilities': {
            'hubs': [
              {
                'type': 'matter',
                'configurable': true,
                'device_onboarding_methods': [
                  'matter_on_network_setup_code',
                ],
                'supports_unpairing': true,
                'supports_roomless_devices': true,
              },
            ],
          },
        }),
      );

      await Future<void>.delayed(Duration.zero);

      expect(provider.canAddMatterDevice, isTrue);
      expect(provider.canAddMatterOnNetworkDevice, isTrue);
      expect(provider.canCommissionMatterBleWifi, isFalse);
      expect(provider.canUnpairMatterDevices, isTrue);
      expect(provider.supportsMatterRoomlessDevices, isTrue);
    });

    test(
        'treats explicit hub capabilities as authoritative when Matter is absent',
        () async {
      final homeProvider = _TestHomeProvider(const []);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'rooms': const <Map<String, dynamic>>[],
          'location': const <String, dynamic>{},
          'capabilities': {
            'hubs': [
              {
                'type': 'hue',
                'configurable': true,
                'device_onboarding_methods': const <String>[],
                'supports_unpairing': false,
                'supports_roomless_devices': false,
              },
            ],
          },
        }),
      );

      await Future<void>.delayed(Duration.zero);

      expect(provider.canConfigureHub('hue'), isTrue);
      expect(provider.canConfigureHub('homeassistant'), isFalse);
      expect(provider.canAddMatterDevice, isFalse);
      expect(provider.canAddMatterOnNetworkDevice, isFalse);
      expect(provider.canCommissionMatterBleWifi, isFalse);
    });
  });

  group('ServerSyncProvider location reconciliation', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _HelloRhythmConnection connection;

    setUp(() {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _HelloRhythmConnection(api);
    });

    tearDown(() {
      roomProvider.dispose();
      connection.dispose();
    });

    test('accepts server location when the local home has none', () async {
      final home = Home.create(
        id: 'home-1',
        name: 'My Home',
        ownerId: 'user-1',
      );
      final homeProvider = _TestHomeProvider(
        const [],
        currentHome: home,
      );
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'rooms': const <Map<String, dynamic>>[],
          'location': {
            'latitude': 40.7128,
            'longitude': -74.0060,
            'timezone': 'America/New_York',
          },
        }),
      );

      await Future<void>.delayed(Duration.zero);

      final adoptedHome = homeProvider.currentHome!;
      expect(adoptedHome.location?.latitude, 40.7128);
      expect(adoptedHome.location?.longitude, -74.0060);
      expect(adoptedHome.timezone, 'America/New_York');
    });
  });

  group('ServerSyncProvider topology wiring', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _HelloRhythmConnection connection;

    setUp(() {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _HelloRhythmConnection(api);
    });

    tearDown(() {
      roomProvider.dispose();
      connection.dispose();
    });

    test('marks motion-target nodes from topology controls', () async {
      api.topologyNodes = [
        RhythmTopologyNode.fromJson({
          'id': 'room-1',
          'name': 'Kitchen',
          'kind': 'room',
        }),
        RhythmTopologyNode.fromJson({
          'id': 'sensor-1',
          'name': 'Kitchen Motion',
          'kind': 'motion_sensor',
          'controls': [
            {
              'kind': 'motion',
              'target_id': 'room-1',
              'inherited': false,
            },
          ],
        }),
      ];

      final homeProvider = _TestHomeProvider(const []);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': [
            {
              'id': 'room-1',
              'name': 'Kitchen',
              'kind': 'room',
              'hub_types': ['hue'],
              'state': 'active',
              'rhythm_enabled': true,
              'disabled': false,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
              'lights_on': true,
            },
          ],
          'location': const <String, dynamic>{},
        }),
      );

      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(roomProvider.hasMotionSensor('room-1'), isTrue);
      expect(provider.nodeHasMotionControlTarget('room-1'), isTrue);
      expect(
        provider.controlTargetNodeId(
          sourceNodeId: 'sensor-1',
          controlKind: 'motion',
        ),
        'room-1',
      );
    });

    test('keeps roomless light-device nodes in the room provider', () async {
      final homeProvider = _TestHomeProvider(const []);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
      );
      addTearDown(provider.dispose);

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
            {
              'id': 'light-1',
              'name': 'Desk Lamp',
              'kind': 'light_device',
              'hub_types': ['matter'],
              'state': 'active',
              'rhythm_enabled': true,
              'disabled': false,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
              'lights_on': true,
            },
          ],
          'location': const <String, dynamic>{},
        }),
      );

      await Future<void>.delayed(const Duration(milliseconds: 10));

      final node = roomProvider.getNode('light-1');
      expect(node, isNotNull);
      expect(node!.kind, RoomNodeKind.lightDevice);
      expect(node.parentId, isNull);
      expect(node.deviceIds, ['light-1']);
      expect(
        roomProvider.enabledRooms.map((room) => room.id),
        contains('light-1'),
      );
    });

    test('uses observed_power lights_on for room visuals', () async {
      final homeProvider = _TestHomeProvider(const []);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': [
            {
              'id': 'room-1',
              'name': 'Kitchen',
              'kind': 'room',
              'hub_types': ['hue'],
              'state': 'active',
              'rhythm_enabled': true,
              'disabled': false,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
              'lights_on': true,
              'observed_power': {'lights_on': false},
            },
          ],
          'location': const <String, dynamic>{},
        }),
      );

      await Future<void>.delayed(const Duration(milliseconds: 10));

      final room = roomProvider.getRoom('room-1');
      expect(room, isNotNull);
      expect(room!.lightsOn, isFalse);

      final helloRoom =
          provider.helloRooms.singleWhere((entry) => entry.id == 'room-1');
      expect(helloRoom.lightsOn, isFalse);
    });
  });

  group('ServerSyncProvider input bindings', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _HelloRhythmConnection connection;

    setUp(() {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _HelloRhythmConnection(api);
    });

    tearDown(() {
      roomProvider.dispose();
      connection.dispose();
    });

    test('reads day/sleep toggle bindings from hello', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'input_bindings': [
            {
              'id': 'day_sleep_toggle:button-1:on_press',
              'preset': 'day_sleep_toggle',
              'source_node_id': 'button-1',
              'trigger': {
                'kind': 'button',
                'button_action': 'on_press',
              },
              'action': {
                'kind': 'mode_cycle',
                'modes': ['day', 'sleep'],
                'transition': {'kind': 'auto'},
              },
              'enabled': true,
            },
          ],
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.daySleepToggleInputBinding?.sourceNodeId, 'button-1');
      expect(provider.daySleepToggleInputBinding?.enabled, isTrue);
    });

    test('binds one selected button as the day/sleep toggle', () async {
      api.inputBindings = [
        RhythmInputBinding(
          id: 'day_sleep_toggle:old:on_press',
          preset: RhythmInputBindingPreset.daySleepToggle,
          sourceNodeId: 'old',
          trigger: const RhythmInputBindingTrigger.button(
            buttonAction: RhythmButtonAction.onPress,
          ),
          action: const RhythmModeCycleAction(
            modes: [RhythmMode.day, RhythmMode.sleep],
          ),
        ),
      ];
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'input_bindings': api.inputBindings
              .map((binding) => binding.toJson())
              .toList(growable: false),
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      final ok = await provider.bindDaySleepToggleButton('button-2');

      expect(ok, isTrue);
      expect(api.createDaySleepToggleInputBindingCalls, 1);
      expect(api.lastInputBindingSourceNodeId, 'button-2');
      expect(api.deleteInputBindingCalls, 1);
      expect(api.lastDeletedInputBindingId, 'day_sleep_toggle:old:on_press');
      expect(provider.daySleepToggleInputBinding?.sourceNodeId, 'button-2');
    });

    test('preserves the selected button action when binding', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(RhythmHello.fromJson({}));
      await Future<void>.delayed(const Duration(milliseconds: 10));

      final ok = await provider.bindDaySleepToggleButton(
        'button-2',
        buttonAction: RhythmButtonAction.offPress,
      );

      expect(ok, isTrue);
      expect(api.lastInputBindingSourceNodeId, 'button-2');
      expect(api.lastInputBindingButtonAction, RhythmButtonAction.offPress);
      expect(
        provider.daySleepToggleInputBinding?.trigger.buttonAction,
        RhythmButtonAction.offPress,
      );
    });

    test('does not delete stale binding when create already replaced it',
        () async {
      api.createReplacesPresetBindings = true;
      api.inputBindings = [
        RhythmInputBinding(
          id: 'day_sleep_toggle:old:on_press',
          preset: RhythmInputBindingPreset.daySleepToggle,
          sourceNodeId: 'old',
          trigger: const RhythmInputBindingTrigger.button(
            buttonAction: RhythmButtonAction.onPress,
          ),
          action: const RhythmModeCycleAction(
            modes: [RhythmMode.day, RhythmMode.sleep],
          ),
        ),
      ];
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'input_bindings': api.inputBindings
              .map((binding) => binding.toJson())
              .toList(growable: false),
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      final ok = await provider.bindDaySleepToggleButton('button-2');

      expect(ok, isTrue);
      expect(api.deleteInputBindingCalls, 0);
      expect(provider.daySleepToggleInputBinding?.sourceNodeId, 'button-2');
    });

    test('toggles the persisted day/sleep binding enabled flag', () async {
      final binding = RhythmInputBinding(
        id: 'day_sleep_toggle:button-1:on_press',
        preset: RhythmInputBindingPreset.daySleepToggle,
        sourceNodeId: 'button-1',
        trigger: const RhythmInputBindingTrigger.button(
          buttonAction: RhythmButtonAction.onPress,
        ),
        action: const RhythmModeCycleAction(
          modes: [RhythmMode.day, RhythmMode.sleep],
        ),
      );
      api.inputBindings = [binding];
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'input_bindings': api.inputBindings
              .map((entry) => entry.toJson())
              .toList(growable: false),
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      final ok = await provider.setDaySleepToggleButtonEnabled(false);

      expect(ok, isTrue);
      expect(api.setInputBindingCalls, 1);
      expect(api.lastSetInputBinding?.enabled, isFalse);
      expect(provider.daySleepToggleInputBinding?.enabled, isFalse);
    });

    test('treats missing binding enable toggles as successful no-ops',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(RhythmHello.fromJson({}));
      await Future<void>.delayed(const Duration(milliseconds: 10));

      final ok = await provider.setDaySleepToggleButtonEnabled(false);

      expect(ok, isTrue);
      expect(api.setInputBindingCalls, 0);
    });
  });

  group('ServerSyncProvider SSE sync', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _HelloRhythmConnection connection;

    setUp(() {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _HelloRhythmConnection(api);
    });

    tearDown(() {
      roomProvider.dispose();
      connection.dispose();
    });

    test('bootstraps motion timer state from hello nodes', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': [
            {
              'id': 'room-1',
              'name': 'Kitchen',
              'kind': 'room',
              'hub_types': ['hue'],
              'state': 'active',
              'rhythm_enabled': true,
              'disabled': false,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
              'lights_on': true,
              'motion_active': false,
              'motion_owned': true,
              'remaining_secs': 42,
              'timeout_secs': 1200,
              'warning_active': true,
              'devices': [
                {
                  'id': 'sensor-1',
                  'type': 'motion',
                  'name': 'Kitchen Motion',
                },
              ],
            },
          ],
          'location': const <String, dynamic>{},
        }),
      );

      await Future<void>.delayed(const Duration(milliseconds: 10));

      final timer = roomProvider.getMotionTimer('room-1');
      expect(timer, isNotNull);
      expect(timer!.remainingSecs, 42);
      expect(timer.motionOwned, isTrue);
      expect(timer.warningActive, isTrue);
      expect(roomProvider.hasMotionSensor('room-1'), isTrue);
      expect(
        provider.helloRooms
            .singleWhere((room) => room.id == 'room-1')
            .warningActive,
        isTrue,
      );
    });

    test('applies node_state updates to hello room summaries', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': [
            {
              'id': 'room-1',
              'name': 'Kitchen',
              'kind': 'room',
              'hub_types': ['hue'],
              'state': 'active',
              'rhythm_enabled': true,
              'disabled': false,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
              'lights_on': true,
            },
          ],
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      connection.emitRhythmState(
        RhythmRoomState.fromJson({
          'id': 'room-1',
          'name': 'Kitchen Evening',
          'kind': 'room',
          'hub_types': ['hue'],
          'state': 'hard_off',
          'transitioning': true,
          'rhythm_enabled': false,
          'time_offset': 12.0,
          'brightness_offset': -8.0,
          'lights_on': false,
          'brightness': 9,
          'kelvin': 2100,
          'profile_settings': {
            'profile_id': 'sleep',
            'fade_ms': {'mode': 'fixed', 'value': 1500},
          },
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      final room =
          provider.helloRooms.singleWhere((entry) => entry.id == 'room-1');
      expect(room.name, 'Kitchen Evening');
      expect(room.state, RoomModeState.hardOff);
      expect(room.transitioning, isTrue);
      expect(room.rhythmEnabled, isFalse);
      expect(room.lightsOn, isFalse);
      expect(room.brightness, 9);
      expect(room.kelvin, 2100);
      expect(room.profileSettings?.profileId, 'sleep');
      expect(room.profileSettings?.fadeMs, 1500);
    });

    test('resolves persisted mood color from server mood profile', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': [
            {
              'id': 'room-1',
              'name': 'Kitchen',
              'kind': 'room',
              'hub_types': ['hue'],
              'state': 'active',
              'rhythm_enabled': true,
              'disabled': false,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
              'lights_on': true,
              'profile_settings': {
                'mood_enabled': true,
                'mood_profile_id': 'node_mood_room-1',
              },
            },
          ],
          'profiles': [
            {
              'id': 'node_mood_room-1',
              'name': 'Kitchen Mood',
              'curve': {
                'type': 'constant',
                'brightness': 1,
                'color_temp': 0,
                'direct_color': {
                  'rgb': {'r': 20, 'g': 80, 'b': 240},
                  'xy': {'x': 0.16, 'y': 0.08},
                },
              },
            },
          ],
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.moodColorForNode('room-1'), (20, 80, 240));
      expect(roomProvider.getMoodColor('room-1'), (20, 80, 240));
    });

    test('applies mode_changed updates to active mode for main pills',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': const <Map<String, dynamic>>[],
          'mode': {'active': 'day'},
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.activeMode, RhythmMode.day);

      connection.emitModeChanged(
        RhythmModeResource.fromJson({
          'active': 'sleep',
          'cause': 'manual',
          'transition_id': 'day_to_sleep',
          'epoch_ms': 1778058932588,
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.activeMode, RhythmMode.sleep);
    });

    test('preserves active mode across transient reconnect after hello',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': const <Map<String, dynamic>>[],
          'mode': {'active': 'day'},
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.activeMode, RhythmMode.day);
      expect(provider.hasBeenSynced, isTrue);

      connection.emitConnectionState(RhythmConnectionState.reconnecting);
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.activeMode, RhythmMode.day);
      expect(provider.hasBeenSynced, isTrue);
    });

    test('settings_changed without power_save preserves legacy cache',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': const <Map<String, dynamic>>[],
          'settings': {
            'power_save': false,
            'auto_update': true,
          },
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.powerSave, isFalse);
      expect(provider.autoUpdate, isTrue);

      connection.emitSettingsChanged(
        RhythmSettings.fromJson({'auto_update': false}),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.powerSave, isFalse);
      expect(provider.autoUpdate, isFalse);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': const <Map<String, dynamic>>[],
          'settings': {
            'auto_update': true,
          },
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.powerSave, isFalse);
      expect(provider.autoUpdate, isTrue);
    });

    test('applies light breaker state from hello and SSE', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': const <Map<String, dynamic>>[],
          'light_breaker': {'enabled': false},
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.lightBreakerEnabled, isFalse);

      connection.emitLightBreakerChanged(
        const RhythmLightBreaker(enabled: true),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.lightBreakerEnabled, isTrue);
    });

    test('sets light breaker optimistically and rolls back on failure',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': const <Map<String, dynamic>>[],
          'light_breaker': {'enabled': true},
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      final success = await provider.setLightBreakerEnabled(false);

      expect(success, isTrue);
      expect(api.setLightBreakerCalls, 1);
      expect(api.lastLightBreakerEnabled, isFalse);
      expect(provider.lightBreakerEnabled, isFalse);

      api.setLightBreakerResult = false;
      final failed = await provider.setLightBreakerEnabled(true);

      expect(failed, isFalse);
      expect(provider.lightBreakerEnabled, isFalse);
    });

    test('applies motion timer SSE updates to room provider and hello rooms',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': [
            {
              'id': 'room-1',
              'name': 'Kitchen',
              'kind': 'room',
              'hub_types': ['hue'],
              'state': 'active',
              'rhythm_enabled': true,
              'disabled': false,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
              'lights_on': true,
            },
          ],
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      connection.emitMotionTimer(
        const RhythmMotionTimer.node(
          nodeId: 'room-1',
          motionActive: false,
          motionOwned: true,
          remainingSecs: 18,
          timeoutSecs: 1200,
          warningActive: true,
        ),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      final timer = roomProvider.getMotionTimer('room-1');
      expect(timer, isNotNull);
      expect(timer!.remainingSecs, 18);
      expect(timer.warningActive, isTrue);

      final room =
          provider.helloRooms.singleWhere((entry) => entry.id == 'room-1');
      expect(room.motionOwned, isTrue);
      expect(room.remainingSecs, 18);
      expect(room.timeoutSecs, 1200);
      expect(room.warningActive, isTrue);
    });

    test('updates only the addressed hub when multiple hubs share a type',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': const <Map<String, dynamic>>[],
          'location': const <String, dynamic>{},
          'hubs': [
            {
              'type': 'hue',
              'address': '192.168.1.10:443',
              'connected': true,
            },
            {
              'type': 'hue',
              'address': '192.168.1.11:443',
              'connected': true,
            },
          ],
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      connection.emitHubEvent(
        event: 'disconnected',
        hubType: 'hue',
        address: '192.168.1.11:443',
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      final hubsByAddress = {
        for (final hub in provider.serverHubInfos)
          hub['address'] as String: hub['connected'] as bool? ?? false,
      };
      expect(hubsByAddress['192.168.1.10:443'], isTrue);
      expect(hubsByAddress['192.168.1.11:443'], isFalse);
    });
  });

  group('ServerSyncProvider curve modifiers', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _FakeRhythmConnection connection;
    late ServerSyncProvider provider;

    setUp(() async {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _FakeRhythmConnection(api);
      provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      await roomProvider.addRoom(const RoomDto(
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
      ));
    });

    tearDown(() {
      provider.dispose();
      roomProvider.dispose();
      connection.dispose();
    });

    test('dispatches brightness as a curve modifier', () {
      final dispatched = provider.dispatchNodeCurveBrightness('room-1', 61);

      expect(dispatched, isTrue);
      expect(api.nodeCurveBrightnessCalls, hasLength(1));
      final call = api.nodeCurveBrightnessCalls.single;
      expect(call.nodeId, 'room-1');
      expect(call.brightness, 61);
    });

    test('dispatches color temperature as a curve modifier', () {
      final dispatched = provider.dispatchNodeCurveColorTemperature(
        'room-1',
        3200,
      );

      expect(dispatched, isTrue);
      expect(api.nodeCurveColorTemperatureCalls, hasLength(1));
      final call = api.nodeCurveColorTemperatureCalls.single;
      expect(call.nodeId, 'room-1');
      expect(call.kelvin, 3200);
      expect(call.preserveBrightness, isTrue);
    });
  });

  group('ServerSyncProvider mood scenes', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _FakeRhythmConnection connection;
    late ServerSyncProvider provider;

    setUp(() {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _FakeRhythmConnection(api);
      provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
    });

    tearDown(() {
      provider.dispose();
      roomProvider.dispose();
      connection.dispose();
    });

    test('hides generated and non-lit scenes from picker data', () async {
      api.scenes = [
        _testScene('evening-glow'),
        _testScene('node-mood-scene-room-1'),
        _testOffScene('all-off'),
        _testPresetOnlyScene('reset-room'),
      ];

      final fetched = await provider.fetchScenes();

      expect(fetched.map((scene) => scene.id), ['evening-glow']);
      expect(provider.scenes.map((scene) => scene.id), ['evening-glow']);
    });

    test('applies a mood scene without issuing a duplicate preferences write',
        () async {
      api.scenes = [_testScene('evening-glow')];
      await provider.fetchScenes();

      final dispatched = provider.applyMoodScene(
        'room-1',
        'evening-glow',
        color: (240, 80, 24),
        transitionMs: 450,
      );

      expect(dispatched, isTrue);
      expect(api.applySceneCalls, hasLength(1));
      expect(api.applySceneCalls.single.sceneId, 'evening-glow');
      expect(api.applySceneCalls.single.targetId, 'room-1');
      expect(api.applySceneCalls.single.transitionMs, 450);
      expect(api.nodePreferenceCalls, isEmpty);
      expect(provider.moodSceneIdForRoom('room-1'), 'evening-glow');
      expect(roomProvider.getMoodColor('room-1'), (240, 80, 24));
      expect(roomProvider.getMoodBrightness('room-1'), 55);
    });

    test('uses palette scenes for representative mood brightness', () async {
      api.scenes = [_testPaletteScene('color-carnival')];
      await provider.fetchScenes();

      final dispatched = provider.applyMoodScene(
        'room-1',
        'color-carnival',
      );

      expect(dispatched, isTrue);
      expect(provider.moodSceneIdForRoom('room-1'), 'color-carnival');
      expect(roomProvider.getMoodBrightness('room-1'), 42);
    });
  });

  group('ServerSyncProvider generated mood scene state', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _HelloRhythmConnection connection;
    late ServerSyncProvider provider;

    setUp(() {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _HelloRhythmConnection(api);
      provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
    });

    tearDown(() {
      provider.dispose();
      roomProvider.dispose();
      connection.dispose();
    });

    test('treats generated mood scene ids as custom color state', () async {
      connection.emitHello(RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'state': 'mood',
            'profile_settings': {
              'mood_scene_id': 'node-mood-scene-room-1',
            },
          },
        ],
      }));

      await Future<void>.delayed(Duration.zero);

      expect(provider.moodSceneIdForRoom('room-1'), isNull);
    });

    test('keeps public mood scene ids selectable', () async {
      connection.emitHello(RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'state': 'mood',
            'profile_settings': {
              'mood_scene_id': 'evening-glow',
            },
          },
        ],
      }));

      await Future<void>.delayed(Duration.zero);

      expect(provider.moodSceneIdForRoom('room-1'), 'evening-glow');
    });

    test('clears public scene selection after direct mood color', () async {
      connection.emitHello(RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'state': 'mood',
            'profile_settings': {
              'mood_scene_id': 'evening-glow',
            },
          },
        ],
      }));

      await Future<void>.delayed(Duration.zero);
      expect(provider.moodSceneIdForRoom('room-1'), 'evening-glow');

      final dispatched = provider.dispatchNodeColor(
        'room-1',
        20,
        80,
        240,
        scope: 'mood',
      );

      expect(dispatched, isTrue);
      expect(provider.moodSceneIdForRoom('room-1'), isNull);
    });

    test('shows selected scene before server state catches up', () async {
      api.scenes = [_testScene('evening-glow')];
      await provider.fetchScenes();
      connection.emitHello(RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'state': 'mood',
            'profile_settings': {
              'mood_scene_id': 'node-mood-scene-room-1',
            },
          },
        ],
      }));

      await Future<void>.delayed(Duration.zero);
      expect(provider.moodSceneIdForRoom('room-1'), isNull);

      final dispatched = provider.applyMoodScene(
        'room-1',
        'evening-glow',
        color: (240, 80, 24),
      );

      expect(dispatched, isTrue);
      expect(provider.moodSceneIdForRoom('room-1'), 'evening-glow');
    });
  });

  group('ServerSyncProvider.dispatchNodeColor', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _FakeRhythmConnection connection;
    late ServerSyncProvider provider;

    setUp(() async {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _FakeRhythmConnection(api);
      provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      await roomProvider.addRoom(const RoomDto(
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
      ));
    });

    tearDown(() {
      provider.dispose();
      roomProvider.dispose();
      connection.dispose();
    });

    test('preserves current brightness for mood-scoped color writes', () async {
      await roomProvider.applyServerNodeState(
        'room-1',
        rhythmEnabled: true,
        timeOffset: 0,
        brightnessOffset: 0,
        state: RoomModeState.mood,
        lightsOn: true,
        brightness: 7,
      );

      final dispatched = provider.dispatchNodeColor(
        'room-1',
        20,
        80,
        240,
        scope: 'mood',
      );

      expect(dispatched, isTrue);
      expect(api.nodeColorCalls, hasLength(1));
      final call = api.nodeColorCalls.single;
      expect(call.scope, 'mood');
      expect(call.brightness, 7);
      expect(call.r, 20);
      expect(call.g, 80);
      expect(call.b, 240);
      expect(roomProvider.getMoodColor('room-1'), (20, 80, 240));
    });

    test('uses mood default brightness when no server brightness is known', () {
      final dispatched = provider.dispatchNodeColor(
        'room-1',
        255,
        149,
        0,
        scope: 'mood',
      );

      expect(dispatched, isTrue);
      expect(api.nodeColorCalls.single.brightness, 1);
    });

    test('does not reuse active brightness for mood-scoped color writes',
        () async {
      await roomProvider.applyServerNodeState(
        'room-1',
        rhythmEnabled: true,
        timeOffset: 0,
        brightnessOffset: 0,
        state: RoomModeState.active,
        lightsOn: true,
        brightness: 44,
      );

      final dispatched = provider.dispatchNodeColor(
        'room-1',
        255,
        149,
        0,
        scope: 'mood',
      );

      expect(dispatched, isTrue);
      expect(api.nodeColorCalls.single.brightness, 1);
    });

    test('does not inject brightness for non-mood color writes', () {
      final dispatched = provider.dispatchNodeColor(
        'room-1',
        255,
        149,
        0,
      );

      expect(dispatched, isTrue);
      expect(api.nodeColorCalls.single.brightness, isNull);
    });
  });

  group('ServerSyncProvider fullRefresh', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _FakeRhythmConnection connection;

    setUp(() {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _FakeRhythmConnection(api);
    });

    tearDown(() {
      roomProvider.dispose();
      connection.dispose();
    });

    test('requests authoritative state on reconnect', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      await provider.fullRefresh();

      expect(connection.reconnectCalls, 1);
      expect(connection.lastReconnectAuthoritative, isTrue);
    });

    test('home entry refresh gates until authoritative hello arrives',
        () async {
      final helloConnection = _HelloRhythmConnection(api);
      addTearDown(helloConnection.dispose);
      final provider = ServerSyncProvider(
        connection: helloConnection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider([
          Hub.server(
            id: 'server-1',
            homeId: 'home-1',
            name: 'Kitchen Server',
            host: '127.0.0.1',
            port: 54448,
            token: 'owner-token',
          ),
        ]),
      );
      addTearDown(provider.dispose);

      final refresh = provider.refreshForHomeEntry(
        homeName: 'Kitchen',
        timeout: const Duration(seconds: 1),
        allowWifiFastPath: false,
      );

      expect(provider.hasHomeEntryRefreshGate, isTrue);
      expect(provider.isHomeEntryRefreshPending, isTrue);

      await Future<void>.delayed(const Duration(milliseconds: 100));

      expect(helloConnection.reconnectCalls, 1);
      expect(helloConnection.lastReconnectAuthoritative, isTrue);
      expect(provider.hasHomeEntryRefreshGate, isTrue);

      helloConnection.emitHello(RhythmHello.fromJson({}));

      expect(await refresh, isTrue);
      expect(provider.hasHomeEntryRefreshGate, isFalse);
      expect(provider.homeEntryRefreshError, isNull);
    });

    test('wifi fast path refreshes an already synced Home without gate',
        () async {
      final helloConnection = _HelloRhythmConnection(api);
      addTearDown(helloConnection.dispose);
      final hub = Hub.server(
        id: 'server-1',
        homeId: 'home-1',
        name: 'Kitchen Server',
        host: '127.0.0.1',
        port: 54448,
        token: 'owner-token',
        remoteEndpoint: const HubEndpoint(
          host: 'server.rhythm.lighting',
          port: 443,
          useSsl: true,
        ),
      );
      final provider = ServerSyncProvider(
        connection: helloConnection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider([hub]),
        connectivityCheck: () async => const [ConnectivityResult.wifi],
      );
      addTearDown(provider.dispose);

      await provider.retryActiveServerConnection(
        assumeLanReachable: true,
        assumeSavedAuth: true,
      );
      helloConnection.emitHello(RhythmHello.fromJson({}));
      await Future<void>.delayed(const Duration(milliseconds: 10));

      final priorConnectCalls = helloConnection.connectCalls.length;
      final refresh = provider.refreshForHomeEntry(
        homeName: 'Kitchen',
        wifiFastPathTimeout: const Duration(milliseconds: 50),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.hasHomeEntryRefreshGate, isFalse);
      expect(helloConnection.connectCalls.length, priorConnectCalls + 1);
      expect(helloConnection.connectCalls.last.host, '127.0.0.1');

      helloConnection.emitHello(RhythmHello.fromJson({}));

      expect(await refresh, isTrue);
      expect(provider.hasHomeEntryRefreshGate, isFalse);
      expect(provider.homeEntryRefreshError, isNull);
    });

    test('wifi fast path miss falls back to visible Home entry gate', () async {
      final helloConnection = _HelloRhythmConnection(api);
      addTearDown(helloConnection.dispose);
      final hub = Hub.server(
        id: 'server-1',
        homeId: 'home-1',
        name: 'Kitchen Server',
        host: '127.0.0.1',
        port: 54448,
        token: 'owner-token',
        remoteEndpoint: const HubEndpoint(
          host: 'server.rhythm.lighting',
          port: 443,
          useSsl: true,
        ),
      );
      final provider = ServerSyncProvider(
        connection: helloConnection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider([hub]),
        connectivityCheck: () async => const [ConnectivityResult.wifi],
      );
      addTearDown(provider.dispose);

      await provider.retryActiveServerConnection(
        assumeLanReachable: true,
        assumeSavedAuth: true,
      );
      helloConnection.emitHello(RhythmHello.fromJson({}));
      await Future<void>.delayed(const Duration(milliseconds: 10));

      final refresh = provider.refreshForHomeEntry(
        homeName: 'Kitchen',
        timeout: const Duration(seconds: 1),
        wifiFastPathTimeout: const Duration(milliseconds: 20),
      );
      await Future<void>.delayed(const Duration(milliseconds: 80));

      expect(provider.hasHomeEntryRefreshGate, isTrue);
      expect(provider.isHomeEntryRefreshPending, isTrue);

      helloConnection.emitHello(RhythmHello.fromJson({}));

      expect(await refresh, isTrue);
      expect(provider.hasHomeEntryRefreshGate, isFalse);
    });

    test('fails over from LAN to remote endpoint when LAN reconnects',
        () async {
      final api = _FakeRhythmServerApi();
      final connection = _HelloRhythmConnection(api);
      addTearDown(connection.dispose);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider([
          Hub.server(
            id: 'server-1',
            homeId: 'home-1',
            name: 'Kitchen Server',
            host: '127.0.0.1',
            port: 54448,
            token: 'owner-token',
            remoteEndpoint: const HubEndpoint(
              host: 'server.rhythm.lighting',
              port: 443,
              useSsl: true,
            ),
          ),
        ]),
        endpointReachability: (endpoint, authToken) async {
          expect(authToken, 'owner-token');
          return endpoint.host == '127.0.0.1';
        },
      );
      addTearDown(provider.dispose);

      provider.connectIfAvailable();
      await Future<void>.delayed(const Duration(milliseconds: 100));

      expect(connection.connectCalls, hasLength(1));
      expect(connection.connectCalls.single.host, '127.0.0.1');
      expect(connection.connectCalls.single.authToken, 'owner-token');

      connection.emitConnectionState(RhythmConnectionState.reconnecting);
      await Future<void>.delayed(const Duration(milliseconds: 20));

      expect(connection.connectCalls, hasLength(2));
      expect(connection.connectCalls.last.host, 'server.rhythm.lighting');
      expect(connection.connectCalls.last.port, 443);
      expect(connection.connectCalls.last.useSsl, isTrue);
      expect(connection.connectCalls.last.authToken, 'owner-token');
    });

    test('retry reselects remote endpoint when LAN is unreachable', () async {
      final api = _FakeRhythmServerApi();
      final connection = _HelloRhythmConnection(api);
      addTearDown(connection.dispose);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider([
          Hub.server(
            id: 'server-1',
            homeId: 'home-1',
            name: 'Kitchen Server',
            host: '127.0.0.1',
            port: 54448,
            token: 'owner-token',
            remoteEndpoint: const HubEndpoint(
              host: 'server.rhythm.lighting',
              port: 443,
              useSsl: true,
            ),
          ),
        ]),
        endpointReachability: (endpoint, authToken) async {
          expect(authToken, 'owner-token');
          return endpoint.host != '127.0.0.1';
        },
      );
      addTearDown(provider.dispose);

      await provider.retryActiveServerConnection();

      expect(connection.connectCalls, hasLength(1));
      expect(connection.connectCalls.single.host, 'server.rhythm.lighting');
      expect(connection.connectCalls.single.port, 443);
      expect(connection.connectCalls.single.useSsl, isTrue);
      expect(connection.connectCalls.single.authToken, 'owner-token');
      expect(provider.activeConnectionEndpoint?.host, 'server.rhythm.lighting');
    });

    test('retry refreshes changed tunnel endpoint before reconnecting',
        () async {
      final api = _FakeRhythmServerApi();
      final connection = _HelloRhythmConnection(api);
      addTearDown(connection.dispose);
      final home = Home.create(
        id: 'home-1',
        name: 'Kitchen',
        ownerId: 'user-1',
      );
      final localHub = Hub.server(
        id: 'server-1',
        homeId: home.id,
        name: 'Kitchen Server',
        host: '127.0.0.1',
        port: 54448,
        token: 'owner-token',
        remoteEndpoint: const HubEndpoint(
          host: 'old-server.rhythm.lighting',
          port: 443,
          useSsl: true,
        ),
      );
      final homeProvider = _TestHomeProvider(
        [localHub],
        currentHome: home,
        accountHomes: [
          AccountHomeServerHubs(
            home: home,
            serverHubs: [
              Hub.server(
                id: localHub.id,
                homeId: home.id,
                name: localHub.name,
                host: localHub.endpoint.host,
                port: localHub.endpoint.port,
                remoteEndpoint: const HubEndpoint(
                  host: 'new-server.rhythm.lighting',
                  port: 443,
                  useSsl: true,
                ),
              ),
            ],
          ),
        ],
      );
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
        endpointReachability: (endpoint, authToken) async {
          expect(authToken, 'owner-token');
          return endpoint.host != '127.0.0.1';
        },
      );
      addTearDown(provider.dispose);

      await provider.retryActiveServerConnection();

      expect(connection.connectCalls, hasLength(1));
      expect(connection.connectCalls.single.host, 'new-server.rhythm.lighting');
      expect(connection.connectCalls.single.port, 443);
      expect(connection.connectCalls.single.useSsl, isTrue);
      expect(connection.connectCalls.single.authToken, 'owner-token');
      expect(homeProvider.currentHomeHubs.single.remoteEndpoint?.host,
          'new-server.rhythm.lighting');
      expect(provider.activeConnectionEndpoint?.host,
          'new-server.rhythm.lighting');
    });

    test('retry refreshes tunnel endpoint from the selected Home', () async {
      final api = _FakeRhythmServerApi();
      final connection = _HelloRhythmConnection(api);
      addTearDown(connection.dispose);
      final otherHome = Home.create(
        id: 'home-a',
        name: 'Kitchen',
        ownerId: 'user-1',
      );
      final selectedHome = Home.create(
        id: 'home-b',
        name: 'Cabin',
        ownerId: 'user-1',
      );
      final localHub = Hub.server(
        id: 'server-1',
        homeId: selectedHome.id,
        name: 'Cabin Server',
        host: '127.0.0.1',
        port: 54448,
        token: 'owner-token',
        remoteEndpoint: const HubEndpoint(
          host: 'old-cabin.rhythm.lighting',
          port: 443,
          useSsl: true,
        ),
      );
      final homeProvider = _TestHomeProvider(
        [localHub],
        currentHome: selectedHome,
        accountHomes: [
          AccountHomeServerHubs(
            home: otherHome,
            serverHubs: [
              Hub.server(
                id: localHub.id,
                homeId: otherHome.id,
                name: 'Kitchen Server',
                host: '192.168.5.10',
                remoteEndpoint: const HubEndpoint(
                  host: 'stale-kitchen.rhythm.lighting',
                  port: 443,
                  useSsl: true,
                ),
              ),
            ],
          ),
          AccountHomeServerHubs(
            home: selectedHome,
            serverHubs: [
              Hub.server(
                id: localHub.id,
                homeId: selectedHome.id,
                name: localHub.name,
                host: localHub.endpoint.host,
                port: localHub.endpoint.port,
                remoteEndpoint: const HubEndpoint(
                  host: 'fresh-cabin.rhythm.lighting',
                  port: 443,
                  useSsl: true,
                ),
              ),
            ],
          ),
        ],
      );
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
        endpointReachability: (endpoint, authToken) async {
          expect(authToken, 'owner-token');
          return endpoint.host != '127.0.0.1';
        },
      );
      addTearDown(provider.dispose);

      await provider.retryActiveServerConnection();

      expect(connection.connectCalls, hasLength(1));
      expect(
          connection.connectCalls.single.host, 'fresh-cabin.rhythm.lighting');
      expect(homeProvider.currentHomeHubs.single.remoteEndpoint?.host,
          'fresh-cabin.rhythm.lighting');
      expect(provider.activeConnectionEndpoint?.host,
          'fresh-cabin.rhythm.lighting');
    });

    test('retry refreshes changed local endpoint before reconnecting',
        () async {
      final api = _FakeRhythmServerApi();
      final connection = _HelloRhythmConnection(api);
      addTearDown(connection.dispose);
      final home = Home.create(
        id: 'home-1',
        name: 'Kitchen',
        ownerId: 'user-1',
      );
      final localHub = Hub.server(
        id: 'server-1',
        homeId: home.id,
        name: 'Kitchen Server',
        host: '100.64.0.12',
        port: 54448,
        token: 'owner-token',
      ).copyWith(
        updatedAt: DateTime.utc(2026, 6, 1),
        pendingSync: false,
      );
      final homeProvider = _TestHomeProvider(
        [localHub],
        currentHome: home,
        accountHomes: [
          AccountHomeServerHubs(
            home: home,
            serverHubs: [
              Hub.server(
                id: localHub.id,
                homeId: home.id,
                name: localHub.name,
                host: '192.168.5.123',
                port: localHub.endpoint.port,
              ).copyWith(updatedAt: DateTime.utc(2026, 6, 2)),
            ],
          ),
        ],
      );
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
        endpointReachability: (endpoint, authToken) async {
          expect(authToken, 'owner-token');
          return endpoint.host == '192.168.5.123';
        },
      );
      addTearDown(provider.dispose);

      await provider.retryActiveServerConnection();

      expect(connection.connectCalls, hasLength(1));
      expect(connection.connectCalls.single.host, '192.168.5.123');
      expect(connection.connectCalls.single.port, 54448);
      expect(connection.connectCalls.single.authToken, 'owner-token');
      expect(
        homeProvider.currentHomeHubs.single.endpoint.host,
        '192.168.5.123',
      );
      expect(provider.activeConnectionEndpoint?.host, '192.168.5.123');
    });

    test('refreshes authoritative state when preview tick is missing',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      await roomProvider.addRoom(const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.hue,
        deviceIds: ['light-1'],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: true,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ));

      await provider.ensureRoomPreviewStateFresh('room-1');

      expect(connection.reconnectCalls, 1);
      expect(connection.lastReconnectAuthoritative, isTrue);
    });

    test('skips preview refresh when tick is recent', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      await roomProvider.addRoom(const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.hue,
        deviceIds: ['light-1'],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: true,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ));
      roomProvider.setLastTickTime('room-1', DateTime.now());

      await provider.ensureRoomPreviewStateFresh('room-1');

      expect(connection.reconnectCalls, 0);
    });
  });

  group('ServerSyncProvider transition saves', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _HelloRhythmConnection connection;

    setUp(() {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _HelloRhythmConnection(api);
    });

    tearDown(() {
      roomProvider.dispose();
      connection.dispose();
    });

    List<RhythmModeTransitionConfig> seedTransitions() {
      return const [
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
      ];
    }

    test('stores trigger-enabled edits in the server and local cache',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      final initial = seedTransitions();
      api.transitions = initial;
      connection.emitHello(RhythmHello.fromJson({
        'rooms': const <Map<String, dynamic>>[],
        'location': const <String, dynamic>{},
        'transitions':
            initial.map((transition) => transition.toJson()).toList(),
      }));
      await Future<void>.delayed(Duration.zero);

      final disabled = [
        for (final transition in provider.modeTransitions)
          transition.copyWith(triggerEnabled: false),
      ];

      final success = await provider.dispatchSetTransitions(disabled);

      expect(success, isTrue);
      expect(api.setTransitionsCalls, 1);
      expect(
        api.lastSetTransitions!
            .every((transition) => !transition.triggerEnabled),
        isTrue,
      );
      expect(
        provider.modeTransitions
            .every((transition) => !transition.triggerEnabled),
        isTrue,
      );
      expect(connection.reconnectCalls, 0);
    });

    test('rolls back optimistic transition cache on server failure', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      final initial = seedTransitions();
      api.transitions = initial;
      connection.emitHello(RhythmHello.fromJson({
        'rooms': const <Map<String, dynamic>>[],
        'location': const <String, dynamic>{},
        'transitions':
            initial.map((transition) => transition.toJson()).toList(),
      }));
      await Future<void>.delayed(Duration.zero);

      api.setTransitionsResult = false;

      final success = await provider.dispatchSetTransitions([
        for (final transition in provider.modeTransitions)
          transition.copyWith(triggerEnabled: false),
      ]);

      expect(success, isFalse);
      expect(
        provider.modeTransitions
            .every((transition) => transition.triggerEnabled),
        isTrue,
      );
    });
  });

  testWidgets('room settings live preview refreshes a missing tick on open',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _FakeRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);

    const room = RoomDto(
      id: 'room-1',
      name: 'Kitchen',
      source: RoomSourceDto.hue,
      deviceIds: ['light-1'],
      rhythmEnabled: true,
      disabled: false,
      lightsOn: true,
      timeOffsetMinutes: 0,
      brightnessOffset: 0,
    );
    await roomProvider.addRoom(room);

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const TickerMode(
          enabled: false,
          child: RoomSettingsSheet(room: room),
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 10));

    expect(connection.reconnectCalls, 1);
    expect(connection.lastReconnectAuthoritative, isTrue);
  });

  testWidgets(
      'Hub picker always shows Matter and marks connected Hue and HA hubs',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);

    connection.emitHello(
      RhythmHello.fromJson({
        'rooms': const <Map<String, dynamic>>[],
        'location': const <String, dynamic>{},
        'hubs': [
          {
            'type': 'hue',
            'connected': true,
          },
          {
            'type': 'homeassistant',
            'connected': true,
          },
        ],
        'capabilities': {
          'hubs': [
            {
              'type': 'hue',
              'configurable': true,
              'device_onboarding_methods': const <String>[],
              'supports_unpairing': false,
              'supports_roomless_devices': false,
            },
          ],
        },
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));

    expect(provider.canAddMatterDevice, isFalse);

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const HubPickerScreen(),
      ),
    );
    await tester.pump(const Duration(milliseconds: 10));

    expect(find.text('Add Hubs'), findsOneWidget);
    expect(find.text('Home Assistant'), findsOneWidget);
    expect(find.text('Philips Hue'), findsOneWidget);
    expect(find.text('Matter'), findsOneWidget);
    expect(find.text('BETA'), findsNWidgets(2));
    expect(find.byIcon(Icons.check_circle_rounded), findsNWidgets(2));
  });

  group('ServerSyncProvider demo mode', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _HelloRhythmConnection connection;

    setUp(() {
      DemoServerApi.instance.reset();
      HueServiceLocator.setDemoMode(true);
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _HelloRhythmConnection(api);
    });

    tearDown(() {
      HueServiceLocator.setDemoMode(false);
      roomProvider.dispose();
      connection.dispose();
    });

    test('populates topology and triage data from the shared demo state',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider([
          Hub.create(
            id: 'demo-server',
            homeId: 'home-1',
            type: HubType.server,
            name: 'Demo Server',
            endpoint: const HubEndpoint(host: '127.0.0.1', port: 54448),
          ),
        ]),
      );
      addTearDown(provider.dispose);

      provider.connectIfAvailable();
      await Future<void>.delayed(const Duration(milliseconds: 20));

      expect(provider.synced, isTrue);
      expect(provider.helloRooms.map((room) => room.name),
          contains('Living Room'));
      expect(provider.devicesForRoom('hue_demo_1').length, 4);
      expect(provider.deviceSummaryForRoom('hue_demo_1'),
          '2 lights, 1 button, 1 sensor');
      expect(provider.triagePendingCount, 2);
      expect(provider.triagePendingDevices, 1);
      expect(provider.triagePendingRooms, 1);
      expect(provider.hasReviewAttention, isTrue);
      expect(provider.hubConfiguredConflicts, hasLength(1));
      expect(provider.reviewHistory, isNotEmpty);
      expect(provider.reviewHistory.first.isPending, isFalse);
    });

    test('refreshes demo topology after assigning an unassigned device',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider([
          Hub.create(
            id: 'demo-server',
            homeId: 'home-1',
            type: HubType.server,
            name: 'Demo Server',
            endpoint: const HubEndpoint(host: '127.0.0.1', port: 54448),
          ),
        ]),
      );
      addTearDown(provider.dispose);

      provider.connectIfAvailable();
      await Future<void>.delayed(const Duration(milliseconds: 20));

      final success = await provider.api
          .assignDeviceParent('demo_floor_lamp', 'hue_demo_2');
      await Future<void>.delayed(const Duration(milliseconds: 20));

      expect(success, isTrue);
      expect(provider.deviceSummaryForRoom('hue_demo_2'), '3 lights');
      expect(provider.triagePendingDevices, 0);
      expect(provider.triagePendingCount, 1);
    });

    test('stores demo transition trigger enabled updates', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider([
          Hub.create(
            id: 'demo-server',
            homeId: 'home-1',
            type: HubType.server,
            name: 'Demo Server',
            endpoint: const HubEndpoint(host: '127.0.0.1', port: 54448),
          ),
        ]),
      );
      addTearDown(provider.dispose);

      provider.connectIfAvailable();
      await Future<void>.delayed(const Duration(milliseconds: 20));

      expect(provider.modeTransitions, hasLength(2));
      expect(
        provider.modeTransitions
            .every((transition) => transition.triggerEnabled),
        isTrue,
      );

      final success = await provider.api.setTransitions([
        for (final transition in provider.modeTransitions)
          transition.copyWith(triggerEnabled: false),
      ]);
      await Future<void>.delayed(const Duration(milliseconds: 20));

      expect(success, isTrue);
      expect(
        provider.modeTransitions
            .every((transition) => transition.triggerEnabled),
        isFalse,
      );
    });
  });

  testWidgets('Unassigned keeps a new matter bulb unassigned', (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final assignmentResult = Completer<bool>();
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);

    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'hub_types': ['matter'],
            'state': 'active',
            'rhythm_enabled': false,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': false,
          },
        ],
        'location': const <String, dynamic>{},
      }),
    );

    await tester.pumpWidget(
      ChangeNotifierProvider<ServerSyncProvider>.value(
        value: provider,
        child: MaterialApp(
          home: Scaffold(
            body: Builder(
              builder: (context) {
                return TextButton(
                  onPressed: () async {
                    assignmentResult.complete(
                      await showDeviceNodeAssignmentFlow(
                        context,
                        device: const RhythmDevice(
                          id: 'light-1',
                          type: RhythmDeviceType.light,
                          name: 'Desk Lamp',
                        ),
                        currentParentNodeId: '',
                        allowNoRoom: true,
                      ),
                    );
                  },
                  child: const Text('Open'),
                );
              },
            ),
          ),
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();

    expect(find.text('Unassigned'), findsOneWidget);
    expect(find.text('Kitchen'), findsOneWidget);

    await tester.tap(find.text('Unassigned'));
    await tester.pumpAndSettle();

    expect(await assignmentResult.future, isTrue);
    expect(api.assignDeviceParentCalls, 0);
    expect(connection.reconnectCalls, 0);
    expect(find.text('Desk Lamp has no room assignment'), findsOneWidget);
  });

  testWidgets('Move to Room sheet scrolls when many rooms are available',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final assignmentResult = Completer<bool>();
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 700));

    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': [
          for (int i = 1; i <= 20; i++)
            {
              'id': 'room-$i',
              'name': 'Room ${i.toString().padLeft(2, '0')}',
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
        'location': const <String, dynamic>{},
      }),
    );

    await tester.pumpWidget(
      ChangeNotifierProvider<ServerSyncProvider>.value(
        value: provider,
        child: MaterialApp(
          home: Scaffold(
            body: Builder(
              builder: (context) {
                return TextButton(
                  onPressed: () async {
                    assignmentResult.complete(
                      await showDeviceNodeAssignmentFlow(
                        context,
                        device: const RhythmDevice(
                          id: 'light-1',
                          type: RhythmDeviceType.light,
                          name: 'Desk Lamp',
                        ),
                        currentParentNodeId: 'room-1',
                      ),
                    );
                  },
                  child: const Text('Open'),
                );
              },
            ),
          ),
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();

    final scrollable = find.byType(SingleChildScrollView);
    await tester.dragUntilVisible(
      find.text('Room 20'),
      scrollable,
      const Offset(0, -250),
    );
    await tester.tap(find.text('Room 20'));
    await tester.pumpAndSettle();

    expect(await assignmentResult.future, isTrue);
    expect(api.assignDeviceParentCalls, 1);
    expect(api.lastAssignedDeviceId, 'light-1');
    expect(api.lastAssignedParentId, 'room-20');
    expect(connection.reconnectCalls, 1);
    expect(find.text('Moved Desk Lamp to Room 20'), findsOneWidget);
  });

  testWidgets('device assignment can create a room inline', (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final assignmentResult = Completer<bool>();
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);

    await tester.pumpWidget(
      ChangeNotifierProvider<ServerSyncProvider>.value(
        value: provider,
        child: MaterialApp(
          home: Scaffold(
            body: Builder(
              builder: (context) {
                return TextButton(
                  onPressed: () async {
                    assignmentResult.complete(
                      await showDeviceNodeAssignmentFlow(
                        context,
                        device: const RhythmDevice(
                          id: 'light-1',
                          type: RhythmDeviceType.light,
                          name: 'Desk Lamp',
                        ),
                        currentParentNodeId: '',
                      ),
                    );
                  },
                  child: const Text('Open'),
                );
              },
            ),
          ),
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();

    expect(find.text('Create New Room'), findsOneWidget);

    await tester.tap(find.text('Create New Room'));
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField), 'Office');
    await tester.tap(find.text('Create'));
    await tester.pumpAndSettle();

    expect(await assignmentResult.future, isTrue);
    expect(api.createTopologyRoomCalls, 1);
    expect(api.lastCreatedRoomName, 'Office');
    expect(api.assignDeviceParentCalls, 1);
    expect(api.lastAssignedDeviceId, 'light-1');
    expect(api.lastAssignedParentId, 'room-created');
    expect(connection.reconnectCalls, 1);
    expect(find.text('Assigned Desk Lamp to Office'), findsOneWidget);
  });

  testWidgets('device detail header centers long device names', (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);

    const deviceName = 'GE Lighting, a Savant company Cync Full Color A19';

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const DeviceDetailSheet(
          device: RhythmDevice(
            id: 'light-1',
            type: RhythmDeviceType.light,
            name: deviceName,
          ),
          roomId: '',
        ),
      ),
    );
    await tester.pumpAndSettle();

    final headerText = tester.widget<Text>(find.text(deviceName));
    expect(headerText.textAlign, TextAlign.center);
  });

  testWidgets('Room card device flow offers Remove from Room for Matter bulbs',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 900));

    api.topologyNodes = [
      RhythmTopologyNode.fromJson({
        'id': 'room-1',
        'name': 'Kitchen',
        'kind': 'room',
      }),
      RhythmTopologyNode.fromJson({
        'id': 'light-1',
        'name': 'Desk Lamp',
        'kind': 'light_device',
        'parent_id': 'room-1',
      }),
    ];
    api.canonicalDevices['light-1'] = {
      'endpoints': [
        {
          'hub_key': {'hub_type': 'matter'},
          'native_id': 'matter-light-1',
          'preferred': true,
        },
      ],
    };

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
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await _pumpRoomSettingsSheet(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.matter,
        deviceIds: ['light-1'],
        rhythmEnabled: false,
        disabled: false,
        lightsOn: false,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );

    await _selectRoomSettingsTab(tester, 'Devices');
    await tester.tap(find.text('Desk Lamp').last);
    await tester.pumpAndSettle();

    expect(find.text('Move or Remove...'), findsOneWidget);

    await tester.tap(find.text('Move or Remove...'));
    await tester.pumpAndSettle();

    expect(find.text('Remove from Room'), findsOneWidget);

    await tester.tap(find.text('Remove from Room'));
    await tester.pumpAndSettle();

    expect(api.assignDeviceParentCalls, 1);
    expect(api.lastAssignedDeviceId, 'light-1');
    expect(api.lastAssignedParentId, isNull);
    expect(connection.reconnectCalls, 1);
    expect(find.text('Removed Desk Lamp from its room'), findsOneWidget);
  });

  testWidgets('Room device flow removes motion sensors from a room',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 900));

    api.canonicalDevices['sensor-1'] = {
      'endpoints': [
        {
          'hub_key': {'hub_type': 'hue'},
          'native_id': 'hue-sensor-1',
          'preferred': true,
        },
      ],
    };

    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'hub_types': ['hue'],
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': true,
            'devices': [
              {
                'id': 'sensor-1',
                'type': 'motion',
                'name': 'Kitchen Motion',
              },
            ],
          },
        ],
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await _pumpRoomSettingsSheet(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.hue,
        deviceIds: ['sensor-1'],
        rhythmEnabled: false,
        disabled: false,
        lightsOn: false,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );

    await _selectRoomSettingsTab(tester, 'Devices');
    await tester.tap(find.text('Kitchen Motion').last);
    await tester.pumpAndSettle();

    expect(find.text('Move or Remove...'), findsOneWidget);

    await tester.tap(find.text('Move or Remove...'));
    await tester.pumpAndSettle();

    expect(find.text('Remove from Room'), findsOneWidget);

    await tester.tap(find.text('Remove from Room'));
    await tester.pumpAndSettle();

    expect(api.assignDeviceParentCalls, 1);
    expect(api.lastAssignedDeviceId, 'sensor-1');
    expect(api.lastAssignedParentId, isNull);
    expect(connection.reconnectCalls, 1);
    expect(find.text('Removed Kitchen Motion from its room'), findsOneWidget);
  });

  testWidgets('Delete Room is hidden when the room has Hue bulbs',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 1400));

    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'hub_types': ['hue'],
            'device_ids': ['light-1'],
            'state': 'active',
            'rhythm_enabled': false,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': false,
          },
        ],
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));
    await _pumpRoomSettingsSheet(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.hue,
        deviceIds: ['light-1'],
        rhythmEnabled: false,
        disabled: false,
        lightsOn: false,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );

    await _selectRoomSettingsTab(tester, 'Settings');

    expect(find.text('Delete Room'), findsNothing);
    expect(api.topologyDeleteRoomCalls, 0);
  });

  testWidgets('room settings standby switch pushes node preference',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 1200));

    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'hub_types': ['matter'],
            'device_ids': ['light-1'],
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'standby_enabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': true,
          },
        ],
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));
    await _pumpRoomSettingsSheet(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.matter,
        deviceIds: ['light-1'],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: true,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );

    await _selectRoomSettingsTab(tester, 'Settings');
    await tester.tap(find.text('Standby'));
    await tester.pump();

    expect(api.nodePreferenceCalls, hasLength(1));
    final call = api.nodePreferenceCalls.single;
    expect(call.nodeId, 'room-1');
    expect(call.standbyEnabled, isTrue);
    expect(provider.standbyEnabledForNode('room-1'), isTrue);
  });

  testWidgets('room settings rhythm tab pushes day and sleep motion timeouts',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 1200));

    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'hub_types': ['matter'],
            'device_ids': ['light-1'],
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': true,
            'profile_settings': {
              'profile_overrides': {
                'sleep': {
                  'motion_timeout_secs': {'mode': 'fixed', 'value': 900},
                },
              },
            },
          },
        ],
        'mode': {
          'active': 'day',
          'configs': [
            {'mode': 'day', 'active_profile_id': 'rhythm'},
            {'mode': 'sleep', 'active_profile_id': 'sleep'},
          ],
        },
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));
    roomProvider.updateNodeMotionTimer(
      'room-1',
      MotionTimerInfo(
        motionActive: false,
        motionOwned: true,
        remainingSecs: 850,
        timeoutSecs: 900,
        receivedAt: DateTime.now(),
      ),
    );
    await _pumpRoomSettingsSheet(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.matter,
        deviceIds: ['light-1'],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: true,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );

    await _selectRoomSettingsTab(tester, 'Settings');
    expect(find.text('Motion Timeout'), findsNWidgets(2));

    await tester.tap(find.text('10m').first);
    await tester.pump();

    expect(api.nodePreferenceCalls, isEmpty);
    expect(api.nodeProfileOverrideCalls, isEmpty);
    expect(find.byType(Slider), findsNWidgets(2));
    expect(
      provider
          .nodeById('room-1')
          ?.profileSettings
          ?.profileOverrides['rhythm']
          ?.motionTimeoutSetting,
      isNull,
    );
    expect(roomProvider.getMotionTimer('room-1')?.timeoutSecs, 900);
    expect(roomProvider.getMotionTimer('room-1')?.remainingSecs, 850);

    final daySlider = tester.widget<Slider>(find.byType(Slider).first);
    expect(daySlider.value, 600);
    expect(daySlider.onChanged, isNotNull);
    expect(daySlider.onChangeEnd, isNotNull);
    daySlider.onChanged!(720);
    await tester.pump();

    final draggedDaySlider = tester.widget<Slider>(find.byType(Slider).first);
    expect(draggedDaySlider.value, 720);
    expect(api.nodeProfileOverrideCalls, isEmpty);
    expect(
      provider
          .nodeById('room-1')
          ?.profileSettings
          ?.profileOverrides['rhythm']
          ?.motionTimeoutSetting,
      isNull,
    );

    draggedDaySlider.onChangeEnd!(720);
    await tester.pump();

    expect(
      provider
          .nodeById('room-1')
          ?.profileSettings
          ?.profileOverrides['rhythm']
          ?.motionTimeoutSetting
          ?.fixedValue,
      720,
    );
    expect(roomProvider.getMotionTimer('room-1')?.timeoutSecs, 720);
    expect(roomProvider.getMotionTimer('room-1')?.remainingSecs, 720);

    expect(api.nodeProfileOverrideCalls, hasLength(1));
    final dayCall = api.nodeProfileOverrideCalls.single;
    expect(dayCall.nodeId, 'room-1');
    expect(
      dayCall.profileOverrides,
      {
        'rhythm': {
          'motion_timeout_secs': {'mode': 'fixed', 'value': 720},
        },
      },
    );

    await tester.tap(find.text('Auto').last);
    await tester.pump();

    expect(api.nodeProfileOverrideCalls, hasLength(2));
    expect(roomProvider.getMotionTimer('room-1')?.timeoutSecs, 720);
    final sleepCall = api.nodeProfileOverrideCalls.last;
    expect(
      sleepCall.profileOverrides,
      {'sleep': null},
    );
    expect(
      provider
          .nodeById('room-1')
          ?.profileSettings
          ?.profileOverrides['sleep']
          ?.motionTimeoutSetting,
      isNull,
    );
  });

  testWidgets('Delete Room is shown for matter rooms and calls the SDK method',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 1400));

    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'hub_types': ['matter'],
            'device_ids': ['light-1'],
            'state': 'active',
            'rhythm_enabled': false,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': false,
          },
        ],
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));
    await _pumpRoomSettingsSheet(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.matter,
        deviceIds: ['light-1'],
        rhythmEnabled: false,
        disabled: false,
        lightsOn: false,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );

    await _selectRoomSettingsTab(tester, 'Info');

    expect(find.text('Delete Room'), findsOneWidget);

    await tester.tap(find.text('Delete Room'));
    await tester.pump(const Duration(milliseconds: 250));
    await tester.tap(find.widgetWithText(TextButton, 'Delete'));
    await tester.pump(const Duration(milliseconds: 500));

    expect(api.topologyDeleteRoomCalls, 1);
    expect(api.lastDeletedRoomId, 'room-1');
    expect(connection.reconnectCalls, 1);
    expect(find.text('Deleted Kitchen'), findsOneWidget);
  });
}
