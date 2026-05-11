import 'dart:async';

import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/providers/room_provider.dart';
import 'package:rhythm_app/providers/server_sync_provider.dart';
import 'package:rhythm_app/services/demo_server_api.dart';
import 'package:rhythm_app/services/hue/hue_service_locator.dart';
import 'package:rhythm_app/widgets/device_detail_sheet.dart';
import 'package:rhythm_app/widgets/hub_picker_screen.dart';
import 'package:rhythm_app/widgets/room_settings_sheet.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class _TestHomeProvider extends HomeProvider {
  _TestHomeProvider(this._hubs, {Home? currentHome})
      : _currentHome = currentHome;

  final List<Hub> _hubs;
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
  Future<List<RhythmTopologyNode>> getTopologyNodes() async => topologyNodes;

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

  @override
  bool get connected => isConnected;

  @override
  RhythmServerApi get api => fakeApi;

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
  Stream<void> get newNodesDetected => const Stream<void>.empty();

  @override
  Stream<Map<String, dynamic>> get triageChangedEvents =>
      const Stream<Map<String, dynamic>>.empty();

  @override
  Stream<RhythmConnectionState> get connectionStateStream =>
      _connectionStateController.stream;

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

  @override
  void dispose() {
    _helloController.close();
    _rhythmStateController.close();
    _hubEventController.close();
    _motionTimerController.close();
    _modeChangedController.close();
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
          'state': 'idle',
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
      expect(room.state, RoomModeState.idle);
      expect(room.transitioning, isTrue);
      expect(room.rhythmEnabled, isFalse);
      expect(room.lightsOn, isFalse);
      expect(room.brightness, 9);
      expect(room.kelvin, 2100);
      expect(room.profileSettings?.profileId, 'sleep');
      expect(room.profileSettings?.fadeMs, 1500);
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

    await _selectRoomSettingsTab(tester, 'Settings');

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
