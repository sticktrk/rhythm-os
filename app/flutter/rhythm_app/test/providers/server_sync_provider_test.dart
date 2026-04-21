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
import 'package:rhythm_app/widgets/room_settings_sheet.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class _TestHomeProvider extends HomeProvider {
  _TestHomeProvider(this._hubs);

  final List<Hub> _hubs;

  @override
  List<Hub> get currentHomeHubs => _hubs;

  @override
  Hub? getFirstHubOfType(HubType type) {
    for (final hub in _hubs) {
      if (hub.type == type) return hub;
    }
    return null;
  }
}

class _FakeRhythmServerApi extends RhythmServerApi {
  _FakeRhythmServerApi() : super(Dio());

  int hubCredentialsCalls = 0;
  String? lastHubType;
  String? lastAddress;
  Map<String, dynamic>? lastCredentials;
  List<RhythmTopologyNode> topologyNodes = const [];
  int assignDeviceParentCalls = 0;
  String? lastAssignedDeviceId;
  String? lastAssignedParentId;
  bool assignDeviceParentResult = true;
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

  @override
  bool get connected => isConnected;

  @override
  RhythmServerApi get api => fakeApi;

  @override
  Future<void> reconnect() async {
    reconnectCalls++;
  }
}

class _HelloRhythmConnection extends _FakeRhythmConnection {
  _HelloRhythmConnection(super.fakeApi);

  final _helloController = StreamController<RhythmHello>.broadcast();
  final _connectionStateController =
      StreamController<RhythmConnectionState>.broadcast();

  @override
  Stream<RhythmHello> get helloEvents => _helloController.stream;

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
      _connectionStateController.stream;

  void emitHello(RhythmHello hello) {
    _helloController.add(hello);
  }

  @override
  void dispose() {
    _helloController.close();
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

Future<void> _pumpRoomSettingsLauncher(
  WidgetTester tester, {
  required RoomProvider roomProvider,
  required ServerSyncProvider provider,
  required RoomDto room,
}) async {
  await tester.pumpWidget(
    _buildTestApp(
      roomProvider: roomProvider,
      provider: provider,
      child: Builder(
        builder: (context) {
          return TextButton(
            onPressed: () => Navigator.of(context).push(
              MaterialPageRoute<void>(
                builder: (_) => Scaffold(
                  body: TickerMode(
                    enabled: false,
                    child: RoomSettingsSheet(room: room),
                  ),
                ),
              ),
            ),
            child: const Text('Open'),
          );
        },
      ),
    ),
  );
  await tester.pump(const Duration(milliseconds: 10));
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

  testWidgets('Room card device flow offers Unassigned for Matter bulbs',
      (tester) async {
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
    await Future<void>.delayed(const Duration(milliseconds: 10));

    await _pumpRoomSettingsLauncher(
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

    await tester.tap(find.text('Open'));
    await tester.pump(const Duration(milliseconds: 400));
    await tester.tap(find.text('Devices'));
    await tester.pump(const Duration(milliseconds: 250));
    await tester.tap(find.text('Desk Lamp'));
    await tester.pump(const Duration(milliseconds: 400));
    await tester.tap(find.text('Move to Room...'));
    await tester.pump(const Duration(milliseconds: 400));

    expect(find.text('Unassigned'), findsOneWidget);

    await tester.tap(find.text('Unassigned'));
    await tester.pump(const Duration(milliseconds: 500));

    expect(api.assignDeviceParentCalls, 1);
    expect(api.lastAssignedDeviceId, 'light-1');
    expect(api.lastAssignedParentId, isNull);
    expect(connection.reconnectCalls, 1);
    expect(find.text('Removed Desk Lamp from its room'), findsOneWidget);
  });

  testWidgets('Delete Room is hidden when the room has Hue bulbs',
      (tester) async {
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
    await _pumpRoomSettingsLauncher(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.hue,
        deviceIds: ['light-1'],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: true,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );

    await tester.tap(find.text('Open'));
    await tester.pump(const Duration(milliseconds: 400));
    await tester.tap(find.text('Settings'));
    await tester.pump(const Duration(milliseconds: 250));

    expect(find.text('Delete Room'), findsNothing);
    expect(api.topologyDeleteRoomCalls, 0);
  });

  testWidgets('Delete Room is shown for matter rooms and calls the SDK method',
      (tester) async {
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
    await _pumpRoomSettingsLauncher(
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

    await tester.tap(find.text('Open'));
    await tester.pump(const Duration(milliseconds: 400));
    await tester.tap(find.text('Settings'));
    await tester.pump(const Duration(milliseconds: 250));

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
