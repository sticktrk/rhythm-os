import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/providers/room_provider.dart';
import 'package:rhythm_app/providers/server_sync_provider.dart';
import 'package:rhythm_app/screens/triage_screen.dart';
import 'package:rhythm_app/services/hue_ble_auto_discovery_service.dart';
import 'package:rhythm_app/widgets/settings_row.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class _TestHomeProvider extends HomeProvider {
  @override
  List<Hub> get currentHomeHubs => const [];
}

class _FakeTriageServerApi extends RhythmServerApi {
  _FakeTriageServerApi({
    required this.triageEntries,
  }) : super(Dio());

  List<Map<String, dynamic>> triageEntries;
  int getTriageEntriesCalls = 0;
  int resolveTriageBindCalls = 0;
  int resolveTriageNewCalls = 0;
  int resolveTriageRoomCalls = 0;
  int resolveTriageDismissCalls = 0;
  int createTopologyRoomCalls = 0;

  @override
  Future<List<Map<String, dynamic>>?> getTriageEntries() async {
    getTriageEntriesCalls++;
    return triageEntries;
  }

  @override
  Future<Map<String, dynamic>?> getTriageCount() async {
    final roomCount = triageEntries
        .where((entry) =>
            entry['kind'] == 'room_binding' ||
            entry['kind'] == 'hub_configured')
        .length;
    return {
      'total': triageEntries.length,
      'devices': triageEntries.length - roomCount,
      'rooms': roomCount,
    };
  }

  @override
  Future<bool> resolveTriageBind(String entryId, {String? targetRoomId}) async {
    resolveTriageBindCalls++;
    triageEntries = [];
    return true;
  }

  @override
  Future<Map<String, dynamic>?> resolveTriageNewResult(String entryId) async {
    resolveTriageNewCalls++;
    triageEntries = [];
    return {'status': 'kept_separate'};
  }

  @override
  Future<bool> resolveTriageRoom(String entryId, String roomId) async {
    resolveTriageRoomCalls++;
    triageEntries = [];
    return true;
  }

  @override
  Future<bool> resolveTriageDismiss(String entryId) async {
    resolveTriageDismissCalls++;
    triageEntries = [];
    return true;
  }

  @override
  Future<Map<String, dynamic>?> createTopologyRoom(String name) async {
    createTopologyRoomCalls++;
    return {
      'id': 'room-created',
      'name': name,
    };
  }

  @override
  Future<List<Map<String, dynamic>>?> getCanonicalDevices() async => const [];
}

class _FakeRhythmConnection extends RhythmConnection {
  _FakeRhythmConnection(this.fakeApi);

  final _FakeTriageServerApi fakeApi;
  int reconnectCalls = 0;

  @override
  bool get connected => true;

  @override
  RhythmServerApi get api => fakeApi;

  @override
  Future<void> reconnect({bool authoritative = false}) async {
    reconnectCalls++;
  }
}

class _TestServerSyncProvider extends ServerSyncProvider {
  _TestServerSyncProvider({
    required super.connection,
    required super.roomProvider,
    required super.homeProvider,
  });

  bool matterPairingEnabled = false;
  bool hueBlePairingEnabled = false;
  bool hueAuthorityConsentSupportedForTest = false;
  RhythmHueAuthority? hueAuthorityForTest;
  int hueAuthorityFetches = 0;
  List<Map<String, dynamic>> hubs = const [];

  @override
  List<Map<String, dynamic>> get serverHubInfos => hubs;

  @override
  RhythmConnectionState get connectionState => RhythmConnectionState.connected;

  @override
  bool get canAddMatterDevice =>
      matterPairingEnabled || super.canAddMatterDevice;

  @override
  bool get canAddHueBleDevice =>
      hueBlePairingEnabled || super.canAddHueBleDevice;

  @override
  bool get hueRoomAuthorityConsentSupported =>
      hueAuthorityConsentSupportedForTest;

  @override
  Future<RhythmHueAuthority?> fetchHueAuthority() async {
    hueAuthorityFetches += 1;
    return hueAuthorityForTest;
  }
}

Widget _buildTestApp({
  required RoomProvider roomProvider,
  required RhythmConnection connection,
  required ServerSyncProvider serverSyncProvider,
  HueBleDiscoveryRequest? hueBleDiscoveryRequest,
}) {
  return MultiProvider(
    providers: [
      ChangeNotifierProvider<RoomProvider>.value(value: roomProvider),
      Provider<RhythmConnection>.value(value: connection),
      ChangeNotifierProvider<ServerSyncProvider>.value(
          value: serverSyncProvider),
    ],
    child: MaterialApp(
      home: TriageScreen(
        hueBleDiscoveryRequest: hueBleDiscoveryRequest,
      ),
    ),
  );
}

Future<void> _scrollTo(WidgetTester tester, Finder finder) async {
  if (finder.evaluate().isEmpty) {
    await tester.scrollUntilVisible(
      finder,
      240,
      scrollable: find.byType(Scrollable).first,
    );
  }
  await tester.ensureVisible(finder);
  await tester.pump();
}

Future<void> _tapVisible(WidgetTester tester, Finder finder) async {
  await _scrollTo(tester, finder);
  await tester.tap(finder);
  await tester.pumpAndSettle();
}

Map<String, dynamic> _roomBindingEntry() {
  return {
    'id': 'room-entry-1',
    'kind': 'room_binding',
    'room_binding': {
      'hub_room_name': 'Kitchen',
      'target_rhythm_room_name': 'Kitchen',
      'target_rhythm_room_id': 'room-kitchen',
      'candidate_rooms': const [],
      'light_device_ids': const ['light-1'],
    },
    'hub_key': {
      'hub_type': 'hue',
      'address': 'bridge.local',
    },
  };
}

Map<String, dynamic> _unassignedDeviceEntry() {
  return {
    'id': 'device-entry-1',
    'kind': 'unassigned_device',
    'unassigned_device': {
      'name': 'Matter Bulb',
      'device_type': 'light',
      'manufacturer': 'Acme',
      'model': 'A19',
    },
  };
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('TriageScreen actions', () {
    late RoomProvider roomProvider;
    late _TestHomeProvider homeProvider;
    late _FakeTriageServerApi api;
    late _FakeRhythmConnection connection;
    late _TestServerSyncProvider serverSyncProvider;

    setUp(() async {
      roomProvider = RoomProvider();
      await roomProvider.addRoom(const RoomDto(
        id: 'room-kitchen',
        name: 'Kitchen',
        source: RoomSourceDto.hue,
        deviceIds: [],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: false,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ));
      homeProvider = _TestHomeProvider();
      api = _FakeTriageServerApi(triageEntries: [_roomBindingEntry()]);
      connection = _FakeRhythmConnection(api);
      serverSyncProvider = _TestServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
      );
    });

    tearDown(() {
      serverSyncProvider.dispose();
      roomProvider.dispose();
      connection.dispose();
      homeProvider.dispose();
    });

    testWidgets('reconnects after merging a room binding', (tester) async {
      await tester.pumpWidget(
        _buildTestApp(
          roomProvider: roomProvider,
          connection: connection,
          serverSyncProvider: serverSyncProvider,
        ),
      );
      await tester.pumpAndSettle();

      await _scrollTo(tester, find.text('Merge Rooms'));
      expect(find.text('Merge Rooms'), findsOneWidget);

      await _tapVisible(tester, find.text('Merge Rooms'));

      expect(api.resolveTriageBindCalls, 1);
      expect(connection.reconnectCalls, 1);
      expect(
        find.text('No devices need your attention right now.'),
        findsOneWidget,
      );
    });

    testWidgets('makes device scanning primary and separates sync from rooms',
        (tester) async {
      serverSyncProvider.matterPairingEnabled = true;
      serverSyncProvider.hubs = const [
        {
          'type': 'hue',
          'address': '192.0.2.25',
          'connected': true,
        },
        {
          'type': 'hue_ble',
          'address': 'local',
          'connected': true,
        },
        {
          'type': 'local_ble',
          'address': 'default',
          'connected': true,
        },
        {
          'type': 'matter',
          'address': 'local',
          'connected': true,
        },
        {
          'type': 'homeassistant',
          'address': 'ha.local',
          'connected': true,
        },
      ];

      await tester.pumpWidget(
        _buildTestApp(
          roomProvider: roomProvider,
          connection: connection,
          serverSyncProvider: serverSyncProvider,
        ),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 50));

      expect(find.text('Add a Device'), findsOneWidget);
      expect(find.text('NEW HARDWARE'), findsOneWidget);
      expect(find.text('Add Bulb'), findsNothing);
      expect(find.text('HUBS'), findsOneWidget);
      expect(
        find.text(
          'Open a hub to manage its connection and devices.',
        ),
        findsOneWidget,
      );
      expect(find.text('Philips Hue'), findsOneWidget);
      expect(find.text('Hue Bluetooth'), findsOneWidget);
      expect(find.text('Local Bluetooth'), findsOneWidget);
      expect(find.text('Matter'), findsOneWidget);
      expect(find.text('Home Assistant'), findsOneWidget);
      expect(find.text('DEVICE ACTIONS'), findsOneWidget);
      expect(find.text('Add Matter Device'), findsOneWidget);
      expect(find.text('Sync All Hubs'), findsOneWidget);
      expect(find.text('No hardware'), findsNothing);
      expect(find.text('Scan a code or find nearby bulbs'), findsOneWidget);
      expect(find.text('Add nearby Hue Bluetooth bulbs'), findsNothing);

      final scanCardSize = tester.getSize(
        find.byKey(const ValueKey('add-review-scan-device')),
      );
      expect(scanCardSize.height, greaterThan(80));
      expect(
        tester
            .getTopLeft(
              find.byKey(const ValueKey('add-review-scan-device')),
            )
            .dy,
        lessThan(tester.getTopLeft(find.text('HUBS')).dy),
      );
      expect(
        tester.getTopLeft(find.text('HUBS')).dy,
        lessThan(tester.getTopLeft(find.text('DEVICE ACTIONS')).dy),
      );

      await _scrollTo(tester, find.text('CREATE A ROOM'));
      expect(find.text('CREATE A ROOM'), findsOneWidget);
      expect(
        find.text('Create a Rhythm room for organizing your devices.'),
        findsOneWidget,
      );
      expect(find.widgetWithText(SettingsRow, 'Add a Room'), findsOneWidget);
      expect(
        tester.getTopLeft(find.text('DEVICE ACTIONS')).dy,
        lessThan(tester.getTopLeft(find.text('CREATE A ROOM')).dy),
      );
    });

    testWidgets('offers new Hue bridge automation review in Add & Review',
        (tester) async {
      serverSyncProvider.hueAuthorityConsentSupportedForTest = true;
      serverSyncProvider.hueAuthorityForTest = const RhythmHueAuthority(
        schemaVersion: 1,
        bridges: [
          RhythmHueBridgeAuthority(
            address: '192.0.2.25:443',
            revision: '0123456789abcdef',
            takeoverScope: 'bridge',
            bridgeTakeoverRequested: false,
            rooms: [
              RhythmHueRoomAuthority(
                roomId: 'office',
                name: 'Office',
                owner: RhythmHueRoomAuthorityOwner.unreviewed,
                rhythmAutomationEnabled: false,
              ),
            ],
          ),
        ],
      );

      await tester.pumpWidget(
        _buildTestApp(
          roomProvider: roomProvider,
          connection: connection,
          serverSyncProvider: serverSyncProvider,
        ),
      );
      await tester.pumpAndSettle();

      expect(serverSyncProvider.hueAuthorityFetches, 1);
      await _scrollTo(tester, find.text('Review Hue room automation'));
      expect(find.text('AUTOMATION REVIEW'), findsOneWidget);
      expect(find.text('1 room needs review'), findsOneWidget);

      await _tapVisible(tester, find.text('Review Hue room automation'));

      expect(find.text('Hue room automation'), findsOneWidget);
      expect(find.text('Office'), findsOneWidget);
      expect(find.text('Save room choices'), findsOneWidget);
    });

    testWidgets('does not offer automation review for an approved Hue bridge',
        (tester) async {
      serverSyncProvider.hueAuthorityConsentSupportedForTest = true;
      serverSyncProvider.hueAuthorityForTest = const RhythmHueAuthority(
        schemaVersion: 1,
        bridges: [
          RhythmHueBridgeAuthority(
            address: '192.0.2.25:443',
            revision: '0123456789abcdef',
            takeoverScope: 'bridge',
            bridgeTakeoverRequested: true,
            rooms: [
              RhythmHueRoomAuthority(
                roomId: 'office',
                name: 'Office',
                owner: RhythmHueRoomAuthorityOwner.rhythm,
                rhythmAutomationEnabled: true,
              ),
            ],
          ),
        ],
      );

      await tester.pumpWidget(
        _buildTestApp(
          roomProvider: roomProvider,
          connection: connection,
          serverSyncProvider: serverSyncProvider,
        ),
      );
      await tester.pumpAndSettle();

      expect(serverSyncProvider.hueAuthorityFetches, 1);
      expect(find.text('AUTOMATION REVIEW'), findsNothing);
      expect(find.text('Review Hue room automation'), findsNothing);
    });

    testWidgets('offers a phone-discovered Hue bulb on Add & Review', (
      tester,
    ) async {
      serverSyncProvider.hueBlePairingEnabled = true;
      var discoveryCalls = 0;

      await tester.pumpWidget(
        _buildTestApp(
          roomProvider: roomProvider,
          connection: connection,
          serverSyncProvider: serverSyncProvider,
          hueBleDiscoveryRequest: ({required source}) async {
            discoveryCalls += 1;
            expect(source, 'add_review');
            return const HueBleDiscoveryResult(
              HueBleDiscoveryOutcome.found,
              deviceCount: 1,
            );
          },
        ),
      );
      await tester.pumpAndSettle();

      expect(discoveryCalls, 1);
      expect(
        find.byKey(const ValueKey('nearby-hue-ble-prompt')),
        findsOneWidget,
      );
      expect(find.text('How do you want to add it?'), findsNothing);
      expect(find.text('Add nearby Hue Bluetooth bulbs'), findsNothing);

      await tester.tap(find.byKey(const ValueKey('nearby-hue-ble-dismiss')));
      await tester.pumpAndSettle();

      expect(find.text('Add a Device'), findsOneWidget);
      expect(
        find.byKey(const ValueKey('nearby-hue-ble-prompt')),
        findsNothing,
      );
    });

    testWidgets('reconnects after keeping a room binding separate',
        (tester) async {
      await tester.pumpWidget(
        _buildTestApp(
          roomProvider: roomProvider,
          connection: connection,
          serverSyncProvider: serverSyncProvider,
        ),
      );
      await tester.pumpAndSettle();

      await _tapVisible(tester, find.text('Keep Separate'));

      expect(api.resolveTriageNewCalls, 1);
      expect(connection.reconnectCalls, 1);
      expect(
        find.text('No devices need your attention right now.'),
        findsOneWidget,
      );
    });

    testWidgets('assigns an unassigned device through triage room endpoint',
        (tester) async {
      api.triageEntries = [_unassignedDeviceEntry()];

      await tester.pumpWidget(
        _buildTestApp(
          roomProvider: roomProvider,
          connection: connection,
          serverSyncProvider: serverSyncProvider,
        ),
      );
      await tester.pumpAndSettle();

      await _scrollTo(tester, find.text('Assign Room'));
      expect(find.text('Assign Room'), findsOneWidget);

      await _tapVisible(tester, find.text('Assign Room'));

      await _tapVisible(tester, find.text('Kitchen'));

      expect(api.resolveTriageRoomCalls, 1);
      expect(connection.reconnectCalls, 1);
      expect(
        find.text('No devices need your attention right now.'),
        findsOneWidget,
      );
    });

    testWidgets('assign room picker only lists room nodes', (tester) async {
      api.triageEntries = [_unassignedDeviceEntry()];
      await roomProvider.addRoom(const RoomDto(
        id: 'node-desk-bulb',
        name: 'Desk Bulb Node',
        source: RoomSourceDto.matter,
        deviceIds: ['node-desk-bulb'],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: true,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
        kind: RoomNodeKind.lightDevice,
        parentId: 'room-kitchen',
      ));

      await tester.pumpWidget(
        _buildTestApp(
          roomProvider: roomProvider,
          connection: connection,
          serverSyncProvider: serverSyncProvider,
        ),
      );
      await tester.pumpAndSettle();

      await _tapVisible(tester, find.text('Assign Room'));

      expect(find.text('Kitchen'), findsOneWidget);
      expect(find.text('Desk Bulb Node'), findsNothing);

      await _tapVisible(tester, find.text('Kitchen'));

      expect(api.resolveTriageRoomCalls, 1);
      expect(connection.reconnectCalls, 1);
    });

    testWidgets('can explicitly keep an unassigned device standalone',
        (tester) async {
      api.triageEntries = [_unassignedDeviceEntry()];

      await tester.pumpWidget(
        _buildTestApp(
          roomProvider: roomProvider,
          connection: connection,
          serverSyncProvider: serverSyncProvider,
        ),
      );
      await tester.pumpAndSettle();

      await _scrollTo(tester, find.text('Use Standalone'));
      expect(find.text('Use Standalone'), findsOneWidget);
      await _tapVisible(tester, find.text('Use Standalone'));

      expect(api.resolveTriageNewCalls, 1);
      expect(connection.reconnectCalls, 1);
      expect(
        find.text('No devices need your attention right now.'),
        findsOneWidget,
      );
    });

    testWidgets('can ignore an unassigned device', (tester) async {
      api.triageEntries = [_unassignedDeviceEntry()];

      await tester.pumpWidget(
        _buildTestApp(
          roomProvider: roomProvider,
          connection: connection,
          serverSyncProvider: serverSyncProvider,
        ),
      );
      await tester.pumpAndSettle();

      await _scrollTo(tester, find.text('Ignore Device'));
      expect(find.text('Ignore Device'), findsOneWidget);
      await _tapVisible(tester, find.text('Ignore Device'));

      expect(api.resolveTriageDismissCalls, 1);
      expect(connection.reconnectCalls, 1);
      expect(
        find.text('No devices need your attention right now.'),
        findsOneWidget,
      );
    });

    testWidgets(
        'can create a room inline before assigning an unassigned device',
        (tester) async {
      api.triageEntries = [_unassignedDeviceEntry()];
      await roomProvider.clearAllRooms();

      await tester.pumpWidget(
        _buildTestApp(
          roomProvider: roomProvider,
          connection: connection,
          serverSyncProvider: serverSyncProvider,
        ),
      );
      await tester.pumpAndSettle();

      await _tapVisible(tester, find.text('Assign Room'));

      await _tapVisible(tester, find.text('Create New Room'));

      await tester.enterText(find.byType(TextField), 'Office');
      await tester.tap(find.text('Create'));
      await tester.pumpAndSettle();

      expect(api.createTopologyRoomCalls, 1);
      expect(api.resolveTriageRoomCalls, 1);
      expect(connection.reconnectCalls, 1);
      expect(
        find.text('No devices need your attention right now.'),
        findsOneWidget,
      );
    });
  });
}
