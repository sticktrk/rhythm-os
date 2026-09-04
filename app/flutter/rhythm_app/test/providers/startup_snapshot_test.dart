import 'dart:async';
import 'package:flutter_test/flutter_test.dart';
import 'package:flutter/foundation.dart';
import 'package:rhythm_app/data/local_data_source.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/providers/room_provider.dart';
import 'package:rhythm_app/providers/server_sync_provider.dart';
import 'package:rhythm_app/services/settings_service.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class CountingSettingsStore implements LocalDataSource {
  AppSettings current = AppSettings.defaults();
  int writes = 0;
  Completer<void>? blocker;
  bool fail = false;
  @override
  bool get isInitialized => true;
  @override
  bool isMigrationComplete() => true;
  @override
  AppSettings getSettings() => current;
  @override
  Future<void> saveSettings(AppSettings settings) async {
    writes++;
    await blocker?.future;
    if (fail) throw StateError('simulated storage failure');
    current = settings;
  }

  @override
  Future<void> clearSettings() async {
    current = AppSettings.defaults();
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

class SnapshotConnection extends RhythmConnection {
  final hellos = StreamController<RhythmHello>.broadcast();
  @override
  Stream<RhythmHello> get helloEvents => hellos.stream;
  @override
  void dispose() {
    hellos.close();
    super.dispose();
  }
}

RhythmHello snapshot(int count, {int brightness = 70}) => RhythmHello.fromJson({
      'version': '0.6.632',
      'platform': 'rpiz',
      'context': 'server',
      'nodes': List.generate(
          count,
          (i) => {
                'id': 'node-$i',
                'name': 'Node $i',
                'kind': i == 0 ? 'room' : 'light_device',
                if (i > 0) 'parent_id': 'node-0',
                'hub_types': [i.isEven ? 'hue' : 'matter'],
                'rhythm_enabled': true,
                'lights_on': true,
                'state': 'active',
                'brightness': brightness,
                'kelvin': 4000,
              }),
    });
Future<void> drain() async {
  for (var i = 0; i < 6; i++) {
    await Future<void>.delayed(Duration.zero);
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final store = CountingSettingsStore();
  setUpAll(() => SettingsService.instance.initialize(localDataSource: store));
  for (final count in [10, 50, 100, 200, 500]) {
    test('hello with $count nodes publishes once and saves at most once',
        () async {
      await SettingsService.instance.clearRunnerState();
      final rooms = RoomProvider();
      await rooms.initialize();
      final connection = SnapshotConnection();
      final home = HomeProvider();
      final sync = ServerSyncProvider(
          connection: connection,
          roomProvider: rooms,
          homeProvider: home,
          activityCloudCanProvision: () => false,
          authStateChanges: const Stream.empty());
      addTearDown(() {
        sync.dispose();
        rooms.dispose();
        home.dispose();
      });
      int notifications = 0;
      rooms.addListener(() {
        notifications++;
      });
      store.writes = 0;
      final timer = Stopwatch()..start();
      connection.hellos.add(snapshot(count));
      await drain();
      timer.stop();
      expect(sync.hasBeenSynced, isTrue);
      expect(rooms.roomCount, count);
      expect(rooms.getBrightness('node-${count - 1}'), 70);
      expect(store.writes, lessThanOrEqualTo(1));
      expect(notifications, 1);
      debugPrint(
          'SNAPSHOT nodes=$count writes=${store.writes} notifications=$notifications elapsed_us=${timer.elapsedMicroseconds}');
      store.writes = 0;
      connection.hellos.add(snapshot(count, brightness: 80));
      await drain();
      expect(store.writes, 0,
          reason: 'runtime display changes are not runner cache changes');
      expect(rooms.getBrightness('node-${count - 1}'), 80);
      if (count == 10) {
        await rooms.setRoomLightsOnLocal('node-1', false);
        connection.hellos.add(snapshot(count));
        await drain();
        expect(rooms.isLightsOn('node-1'), isFalse,
            reason: 'bulk hello must preserve a newer optimistic command lock');
      }
      connection.hellos.add(snapshot(0));
      await drain();
      expect(rooms.roomCount, 0,
          reason: 'a successful empty hello clears membership');
    });
  }
  test(
      'cache writes serialize, survive failure and cannot restore cleared membership',
      () async {
    await SettingsService.instance.clearRunnerState();
    final rooms = RoomProvider();
    final connection = SnapshotConnection();
    final home = HomeProvider();
    final sync = ServerSyncProvider(
        connection: connection,
        roomProvider: rooms,
        homeProvider: home,
        activityCloudCanProvision: () => false,
        authStateChanges: const Stream.empty());
    addTearDown(() {
      sync.dispose();
      rooms.dispose();
      home.dispose();
    });
    store.fail = true;
    connection.hellos.add(snapshot(100));
    await drain();
    expect(rooms.roomCount, 100);
    expect(SettingsService.instance.runnerStateSaveFailed, isTrue);
    expect(store.current.runnerStateJson, isNull);
    store.fail = false;
    connection.hellos.add(snapshot(100));
    await drain();
    expect(SettingsService.instance.runnerStateSaveFailed, isFalse);
    expect(SettingsService.instance.getRunnerState()!.rooms.length, 100);

    final reopened = RoomProvider();
    await reopened.initialize();
    expect(reopened.roomCount, 100);
    final reopenedConnection = SnapshotConnection();
    final reopenedSync = ServerSyncProvider(
        connection: reopenedConnection,
        roomProvider: reopened,
        homeProvider: home,
        activityCloudCanProvision: () => false,
        authStateChanges: const Stream.empty());
    store.writes = 0;
    reopenedConnection.hellos.add(snapshot(100));
    await drain();
    expect(reopened.getBrightness('node-99'), 70);
    expect(store.writes, 0,
        reason: 'cached cold hello changes only runtime maps');
    reopenedSync.dispose();
    reopened.dispose();

    store.writes = 0;
    store.blocker = Completer<void>();
    connection.hellos.add(snapshot(200));
    await drain();
    expect(rooms.roomCount, 200,
        reason: 'storage cannot gate authoritative in-memory state');
    connection.hellos.add(snapshot(300));
    await drain();
    connection.hellos.add(snapshot(0));
    await drain();
    expect(rooms.roomCount, 0);
    final blocker = store.blocker!;
    store.blocker = null;
    blocker.complete();
    await drain();
    expect(SettingsService.instance.getRunnerState()!.rooms, isEmpty);
    expect(store.current.runnerStateJson, contains('"rooms":[]'));
    expect(store.writes, 2,
        reason: 'only the in-flight and latest queued snapshot are written');
  });

  test('clearRunnerState supersedes a snapshot before serialization', () async {
    final pending = SettingsService.instance
        .saveRunnerState(const RunnerStateDto(rooms: []));
    await SettingsService.instance.clearRunnerState();
    await pending;
    expect(SettingsService.instance.getRunnerState(), isNull);
    expect(store.current.runnerStateJson, isNull);
  });
}
