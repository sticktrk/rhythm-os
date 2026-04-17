import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/providers/room_provider.dart';
import 'package:rhythm_app/providers/server_sync_provider.dart';
import 'package:rhythm_app/screens/triage_screen.dart';
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

  @override
  Future<List<Map<String, dynamic>>?> getTriageEntries() async {
    getTriageEntriesCalls++;
    return triageEntries;
  }

  @override
  Future<Map<String, dynamic>?> getTriageCount() async {
    final roomCount =
        triageEntries.where((entry) => entry['kind'] == 'room_binding').length;
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
  Future<String?> resolveTriageNew(String entryId) async {
    resolveTriageNewCalls++;
    triageEntries = [];
    return 'new-room';
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
  Future<void> reconnect() async {
    reconnectCalls++;
  }
}

Widget _buildTestApp(ServerSyncProvider serverSyncProvider) {
  return MultiProvider(
    providers: [
      ChangeNotifierProvider<ServerSyncProvider>.value(
          value: serverSyncProvider),
    ],
    child: const MaterialApp(
      home: TriageScreen(),
    ),
  );
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

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('TriageScreen room binding refresh', () {
    late RoomProvider roomProvider;
    late _TestHomeProvider homeProvider;
    late _FakeTriageServerApi api;
    late _FakeRhythmConnection connection;
    late ServerSyncProvider serverSyncProvider;

    setUp(() {
      roomProvider = RoomProvider();
      homeProvider = _TestHomeProvider();
      api = _FakeTriageServerApi(triageEntries: [_roomBindingEntry()]);
      connection = _FakeRhythmConnection(api);
      serverSyncProvider = ServerSyncProvider(
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
      await tester.pumpWidget(_buildTestApp(serverSyncProvider));
      await tester.pumpAndSettle();

      expect(find.text('Merge Rooms'), findsOneWidget);

      await tester.tap(find.text('Merge Rooms'));
      await tester.pumpAndSettle();

      expect(api.resolveTriageBindCalls, 1);
      expect(connection.reconnectCalls, 1);
      expect(find.text('All clear'), findsOneWidget);
    });

    testWidgets('reconnects after keeping a room binding separate',
        (tester) async {
      await tester.pumpWidget(_buildTestApp(serverSyncProvider));
      await tester.pumpAndSettle();

      await tester.tap(find.text('Keep Separate'));
      await tester.pumpAndSettle();

      expect(api.resolveTriageNewCalls, 1);
      expect(connection.reconnectCalls, 1);
      expect(find.text('All clear'), findsOneWidget);
    });
  });
}
