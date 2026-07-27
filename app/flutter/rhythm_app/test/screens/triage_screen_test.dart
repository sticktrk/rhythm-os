import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/providers/room_provider.dart';
import 'package:rhythm_app/providers/server_sync_provider.dart';
import 'package:rhythm_app/screens/triage_screen.dart';
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

  @override
  bool get canAddMatterDevice =>
      matterPairingEnabled || super.canAddMatterDevice;
}

Widget _buildTestApp({
  required RoomProvider roomProvider,
  required RhythmConnection connection,
  required ServerSyncProvider serverSyncProvider,
}) {
  return MultiProvider(
    providers: [
      ChangeNotifierProvider<RoomProvider>.value(value: roomProvider),
      Provider<RhythmConnection>.value(value: connection),
      ChangeNotifierProvider<ServerSyncProvider>.value(
          value: serverSyncProvider),
    ],
    child: const MaterialApp(
      home: TriageScreen(),
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

      await tester.pumpWidget(
        _buildTestApp(
          roomProvider: roomProvider,
          connection: connection,
          serverSyncProvider: serverSyncProvider,
        ),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 50));

      expect(find.text('Scan to Add Device'), findsOneWidget);
      expect(find.text('NEW HARDWARE'), findsOneWidget);
      expect(find.text('Add Bulb'), findsNothing);
      expect(find.text('SYNC FROM A HUB'), findsOneWidget);
      expect(
        find.text(
          'Bring in devices already paired with Home Assistant or Philips Hue.',
        ),
        findsOneWidget,
      );
      expect(find.text('Sync Devices'), findsOneWidget);
      expect(find.text('CREATE A ROOM'), findsOneWidget);
      expect(
        find.text('Create a Rhythm room for organizing your devices.'),
        findsOneWidget,
      );
      expect(find.text('No hardware'), findsNothing);
      expect(find.text('Scan any device QR code'), findsOneWidget);

      final scanCardSize = tester.getSize(
        find.byKey(const ValueKey('add-review-scan-device')),
      );
      final addRoomSize = tester.getSize(
        find.widgetWithText(SettingsRow, 'Add a Room'),
      );
      expect(scanCardSize.height, greaterThan(addRoomSize.height));
      expect(
        tester
            .getTopLeft(
              find.byKey(const ValueKey('add-review-scan-device')),
            )
            .dy,
        lessThan(tester.getTopLeft(find.text('SYNC FROM A HUB')).dy),
      );
      expect(
        tester.getTopLeft(find.text('SYNC FROM A HUB')).dy,
        lessThan(tester.getTopLeft(find.text('CREATE A ROOM')).dy),
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
