import 'dart:async';
import 'dart:io';
import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_app/data/local_data_source.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/providers/room_provider.dart';
import 'package:rhythm_app/providers/server_sync_provider.dart';
import 'package:rhythm_app/services/settings_service.dart';
import 'package:rhythm_app/widgets/device_details_loader.dart';
import 'package:rhythm_app/widgets/device_detail_sheet.dart';
import 'package:rhythm_app/widgets/header_close_button.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class _Store implements LocalDataSource {
  AppSettings current = AppSettings.defaults();
  @override
  bool get isInitialized => true;
  @override
  bool isMigrationComplete() => true;
  @override
  AppSettings getSettings() => current;
  @override
  Future<void> saveSettings(AppSettings settings) async {
    current = settings;
  }

  @override
  Future<void> clearSettings() async {
    current = AppSettings.defaults();
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

Map<String, dynamic> _node(String id,
        {String kind = 'room', String? parent, int brightness = 50}) =>
    {
      'id': id,
      'name': id,
      'kind': kind,
      if (parent != null) 'parent_id': parent,
      'rhythm_enabled': true,
      'lights_on': true,
      'state': 'active',
      'brightness': brightness,
      if (kind == 'room') 'device_counts': {'light': 1},
    };

RhythmHello _hello(
        {String? server = 'server-a',
        bool details = false,
        bool empty = false,
        List<Map<String, dynamic>>? nodes}) =>
    RhythmHello.fromJson({
      'active_profile': {},
      'location': {},
      'mode': {},
      'review': {},
      'transitions': [],
      'profiles': [],
      'scenes': [],
      'input_bindings': [],
      'server_instance_id': server,
      'version': '0.6.634-beta',
      'state_scope': {
        'schema_version': 1,
        'included': [
          'base',
          if (details) 'nodes' else ...['controls', 'configuration']
        ],
        'nodes': details ? 'all' : 'controls'
      },
      'nodes': nodes ??
          [
            if (!empty) _node('room'),
            if (details && !empty)
              _node('bulb',
                  kind: 'light_device', parent: 'room', brightness: 20)
          ],
    });

class _RuntimeApi extends RhythmRuntimeApi {
  _RuntimeApi() : super(Dio());
  int reads = 0;
  Future<RhythmHello> Function() reply = () async => _hello(details: true);
  @override
  Future<RhythmHello> getState(
      {Set<RhythmStateInclude> include = const {RhythmStateInclude.base},
      bool authoritative = false}) {
    expect(include, {RhythmStateInclude.nodes});
    reads++;
    return reply();
  }
}

class _ServerApi extends RhythmServerApi {
  _ServerApi() : super(Dio());
  int reads = 0;
  bool fail = false;
  bool empty = false;
  String? assignedDeviceId;
  String? assignedParentId;
  @override
  Future<RhythmDeviceRoomAssignmentResult?> assignDeviceParentResult(
      String deviceId, String? parentId) async {
    assignedDeviceId = deviceId;
    assignedParentId = parentId;
    return const RhythmDeviceRoomAssignmentResult.legacyCommitted();
  }

  @override
  Future<Map<String, dynamic>?> getTriageCount() async => null;
  @override
  Future<List<RhythmTopologyNode>> getTopologyNodesOrThrow() async {
    reads++;
    if (fail) throw StateError('simulated transport failure');
    return empty
        ? []
        : [
            _node('room'),
            _node('bulb', kind: 'light_device', parent: 'room'),
            if (assignedDeviceId != null)
              _node(assignedDeviceId!,
                  kind: 'light_device', parent: assignedParentId),
          ].map(RhythmTopologyNode.fromJson).toList();
  }

  @override
  Future<List<RhythmTopologyNode>> getTopologyNodes() =>
      getTopologyNodesOrThrow();
}

class _Connection extends RhythmConnection {
  final hellos = StreamController<RhythmHello>.broadcast();
  final connectionStates = StreamController<RhythmConnectionState>.broadcast();
  @override
  Stream<RhythmConnectionState> get connectionStateStream =>
      connectionStates.stream;
  final states = StreamController<RhythmRoomState>.broadcast();
  final motion = StreamController<RhythmMotionTimer>.broadcast();
  final _runtime = _RuntimeApi();
  final _server = _ServerApi();
  @override
  Future<void> reconnect({bool authoritative = false}) async {
    hellos.add(_hello());
  }

  @override
  bool get connected => true;
  @override
  Stream<RhythmHello> get helloEvents => hellos.stream;
  @override
  Stream<RhythmRoomState> get rhythmStateEvents => states.stream;
  @override
  Stream<RhythmMotionTimer> get motionTimerEvents => motion.stream;
  @override
  RhythmRuntimeApi get runtimeApi => _runtime;
  @override
  RhythmServerApi get api => _server;
  @override
  void dispose() {
    hellos.close();
    connectionStates.close();
    states.close();
    motion.close();
    super.dispose();
  }
}

Future<void> _drain() async {
  for (var i = 0; i < 6; i++) {
    await Future<void>.delayed(Duration.zero);
  }
}

void _registerWidgetCleanup(WidgetTester tester) {
  addTearDown(() async {
    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pump();
    // Finish queued snapshot writes before this widget test's clock is gone.
    final cleared = SettingsService.instance.clearRunnerState();
    await tester.pump();
    await cleared;
  });
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  late _Connection connection;
  late RoomProvider rooms;
  late HomeProvider home;
  late ServerSyncProvider sync;
  setUpAll(
      () => SettingsService.instance.initialize(localDataSource: _Store()));
  setUp(() async {
    await SettingsService.instance.clearRunnerState();
    rooms = RoomProvider();
    await rooms.initialize();
    home = HomeProvider();
    connection = _Connection();
    sync = ServerSyncProvider(
        connection: connection,
        roomProvider: rooms,
        homeProvider: home,
        activityCloudCanProvision: () => false,
        authStateChanges: const Stream.empty());
    connection.hellos.add(_hello());
    await _drain();
  });
  tearDown(() {
    sync.dispose();
    connection.dispose();
    rooms.dispose();
    home.dispose();
  });

  test(
      'startup only uses controls; explicit details load and omission preserves them',
      () async {
    expect(connection._server.reads, 0);
    expect(connection._runtime.reads, 0);
    expect(rooms.roomCount, 1);
    expect(sync.lightCountForRoom('room'), 1);
    expect(await sync.ensureDeviceDetails(), isTrue);
    expect(rooms.roomCount, 2);
    expect(sync.devicesForRoom('room').single.id, 'bulb');
    connection.hellos.add(_hello());
    await _drain();
    expect(rooms.getNode('bulb'), isNotNull);
    expect(connection._server.reads, 1,
        reason: 'resume does not silently fetch details');
    connection.hellos.add(RhythmHello.fromJson({
      'state_scope': {
        'schema_version': 1,
        'included': ['base'],
        'nodes': 'none'
      }
    }));
    await _drain();
    expect(rooms.roomCount, 2);
    connection.hellos.add(_hello(empty: true));
    await _drain();
    expect(rooms.roomCount, 0);
    expect(sync.helloRooms, isEmpty,
        reason: 'cached topology cannot resurrect a removed room');
  });

  test('detail requests coalesce and a newer live event wins over the reply',
      () async {
    final pending = Completer<RhythmHello>();
    connection._runtime.reply = () => pending.future;
    final first = sync.ensureDeviceDetails();
    final second = sync.ensureDeviceDetails();
    expect(identical(first, second), isTrue);
    connection.states.add(RhythmRoomState.fromJson({
      ..._node('bulb', kind: 'light_device', parent: 'room', brightness: 77),
      'node_id': 'bulb'
    }));
    connection.states.add(RhythmRoomState.fromJson(
        {'node_id': 'bulb', 'state': 'active', 'rhythm_enabled': true}));
    await _drain();
    pending.complete(_hello(details: true));
    expect(await first, isTrue);
    expect(rooms.getBrightness('bulb'), 77);
    expect(connection._runtime.reads, 1);
  });

  test('detail replay preserves the order between motion and state events',
      () async {
    final pending = Completer<RhythmHello>();
    connection._runtime.reply = () => pending.future;
    final request = sync.ensureDeviceDetails();
    connection.motion.add(const RhythmMotionTimer.node(
        nodeId: 'room',
        motionActive: true,
        motionOwned: true,
        timeoutSecs: 120,
        remainingSecs: 100));
    connection.states.add(RhythmRoomState.fromJson({
      ..._node('room'),
      'node_id': 'room',
      'motion_active': false,
      'motion_owned': false,
      'timeout_secs': 0
    }));
    await _drain();
    pending.complete(_hello(details: true));
    expect(await request, isTrue);
    expect(sync.nodeById('room')!.motionActive, isFalse);
  });

  test(
      'wrong identity in a detail response fails visibly and preserves controls',
      () async {
    connection._runtime.reply =
        () async => _hello(server: 'wrong-server', details: true);
    expect(await sync.ensureDeviceDetails(), isFalse);
    expect(sync.deviceDetailsFailed, isTrue);
    expect(rooms.roomCount, 1);
  });

  test('a reply from the previous hub cannot overwrite a newer hello',
      () async {
    final pending = Completer<RhythmHello>();
    connection._runtime.reply = () => pending.future;
    final first = sync.ensureDeviceDetails();
    connection.hellos.add(_hello(server: 'server-b'));
    await _drain();
    pending.complete(_hello(details: true));
    expect(await first, isFalse);
    expect(rooms.getNode('bulb'), isNull);
    expect(rooms.roomCount, 1);
  });

  for (final server in ['server-a', 'server-b', null]) {
    test('reconnect only retains devices for the same known server: $server',
        () async {
      expect(await sync.ensureDeviceDetails(), isTrue);
      expect(rooms.getNode('bulb'), isNotNull);
      connection.connectionStates.add(RhythmConnectionState.reconnecting);
      await _drain();
      connection.connectionStates.add(RhythmConnectionState.connected);
      connection.hellos.add(_hello(server: server));
      await _drain();
      final sameServer = server == 'server-a';
      expect(rooms.getNode('bulb'), sameServer ? isNotNull : isNull,
          reason: 'only the same known server can retain cached children');
      expect(sync.topologyNodes, sameServer ? isNotEmpty : isEmpty);
      expect(connection._runtime.reads, 1,
          reason: 'a reconnect does not trigger background device reads');
    });
  }

  test('failed topology preserves cache; a successful empty detail clears it',
      () async {
    connection._server.fail = true;
    expect(await sync.ensureDeviceDetails(), isFalse);
    expect(rooms.roomCount, 1);
    expect(sync.deviceDetailsFailed, isTrue);
    connection._server.fail = false;
    connection._server.empty = true;
    connection._runtime.reply = () async => _hello(details: true, empty: true);
    expect(await sync.ensureDeviceDetails(), isTrue);
    expect(rooms.roomCount, 0);
    expect(sync.topologyNodes, isEmpty);
  });

  testWidgets('standalone device route exposes readable loading failure',
      (tester) async {
    _registerWidgetCleanup(tester);
    connection._server.fail = true;
    await tester.pumpWidget(ChangeNotifierProvider.value(
      value: sync,
      child: MaterialApp(
        theme: ThemeData.dark(),
        home: const RepaintBoundary(
          key: ValueKey('detail-route-evidence'),
          child: DeviceDetailsLoader(
              child: Scaffold(body: Text('Device controls'))),
        ),
      ),
    ));
    await tester.pumpAndSettle();
    expect(find.text('Could not load devices.'), findsOneWidget);
    expect(find.text('Device controls'), findsNothing);
    final errorContext = tester.element(find.text('Could not load devices.'));
    final errorStyle = DefaultTextStyle.of(errorContext).style;
    expect(errorStyle.fontSize, lessThan(24));
    expect(errorStyle.decoration, isNot(TextDecoration.underline));
    final screenshotPath =
        Platform.environment['RHYTHM_DETAIL_ROUTE_SCREENSHOT'];
    if (screenshotPath != null) {
      await expectLater(find.byKey(const ValueKey('detail-route-evidence')),
          matchesGoldenFile(screenshotPath));
    }
    expect(tester.takeException(), isNull);
    connection._server.fail = false;
    await tester.tap(find.text('Try again'));
    await tester.pumpAndSettle();
    expect(find.text('Device controls'), findsOneWidget);
    expect(connection._runtime.reads, 2);
  });

  testWidgets('detail refresh preserves assignment completion context',
      (tester) async {
    _registerWidgetCleanup(tester);
    final result = Completer<bool>();
    await tester.pumpWidget(ChangeNotifierProvider.value(
      value: sync,
      child: MaterialApp(
        home: DeviceDetailsLoader(
          child: Scaffold(body: Builder(builder: (context) {
            return TextButton(
              onPressed: () async {
                result.complete(await showDeviceNodeAssignmentFlow(
                  context,
                  device: const RhythmDevice(
                      id: 'unassigned-bulb',
                      type: RhythmDeviceType.light,
                      name: 'New bulb'),
                  currentParentNodeId: '',
                ));
              },
              child: const Text('Assign device'),
            );
          })),
        ),
      ),
    ));
    await tester.pumpAndSettle();
    final pendingDetails = Completer<RhythmHello>();
    connection._runtime.reply = () => pendingDetails.future;
    await tester.tap(find.text('Assign device'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('room'));
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pump(const Duration(milliseconds: 300));
    pendingDetails.complete(_hello(details: true, nodes: [
      _node('room'),
      _node('bulb', kind: 'light_device', parent: 'room'),
      _node('unassigned-bulb', kind: 'light_device', parent: 'room'),
    ]));
    await tester.pumpAndSettle();
    expect(await result.future, isTrue,
        reason: 'a committed assignment must retain its completion context');
    expect(find.text('Assigned New bulb to room'), findsOneWidget);
    expect(sync.devicesForRoom('room').map((device) => device.id),
        contains('unassigned-bulb'));
  });

  testWidgets('same-server refresh preserves edits through failure and retry',
      (tester) async {
    _registerWidgetCleanup(tester);
    var actions = 0;
    await tester.pumpWidget(ChangeNotifierProvider.value(
      value: sync,
      child: MaterialApp(
        home: DeviceDetailsLoader(
          child: Scaffold(
            body: Column(children: [
              const TextField(),
              TextButton(
                onPressed: () => actions++,
                child: const Text('Device action'),
              ),
            ]),
          ),
        ),
      ),
    ));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'Unsaved device name');
    final editingState = tester.state(find.byType(TextField));
    final actionPosition = tester.getCenter(find.text('Device action'));
    final pendingDetails = Completer<RhythmHello>();
    connection._runtime.reply = () => pendingDetails.future;
    connection.connectionStates.add(RhythmConnectionState.reconnecting);
    await tester.pump();
    connection.connectionStates.add(RhythmConnectionState.connected);
    connection.hellos.add(_hello());
    await tester.pump();
    await tester.pump();
    expect(find.byType(CircularProgressIndicator), findsOneWidget);
    expect(tester.state(find.byType(TextField)), same(editingState));
    await tester.tapAt(actionPosition);
    expect(actions, 0, reason: 'stale device controls must be covered');
    pendingDetails.completeError(StateError('simulated detail read failure'));
    await tester.pumpAndSettle();
    expect(find.text('Could not load devices.'), findsOneWidget);
    expect(tester.state(find.byType(TextField)), same(editingState));
    connection._runtime.reply = () async => _hello(details: true);
    await tester.tap(find.text('Try again'));
    await tester.pumpAndSettle();
    expect(find.text('Unsaved device name'), findsOneWidget);
    expect(tester.state(find.byType(TextField)), same(editingState));
    await tester.tap(find.text('Device action'));
    expect(actions, 1);
  });

  for (final nextServer in ['server-b', null]) {
    testWidgets(
        'a new or unknown server discards the open device state: $nextServer',
        (tester) async {
      _registerWidgetCleanup(tester);
      await tester.pumpWidget(ChangeNotifierProvider.value(
        value: sync,
        child: const MaterialApp(
          home: DeviceDetailsLoader(child: Scaffold(body: TextField())),
        ),
      ));
      await tester.pumpAndSettle();
      await tester.enterText(find.byType(TextField), 'Old server draft');
      final pendingDetails = Completer<RhythmHello>();
      connection._runtime.reply = () => pendingDetails.future;
      connection.connectionStates.add(RhythmConnectionState.reconnecting);
      await tester.pump();
      connection.connectionStates.add(RhythmConnectionState.connected);
      connection.hellos.add(_hello(server: nextServer));
      await tester.pump();
      await tester.pump();
      expect(find.byType(TextField), findsNothing);
      pendingDetails.complete(_hello(server: nextServer, details: true));
      await tester.pumpAndSettle();
      expect(find.text('Old server draft'), findsNothing);
      if (nextServer != null) {
        expect(find.byType(TextField), findsOneWidget);
        expect(
            tester
                .widget<EditableText>(find.byType(EditableText))
                .controller
                .text,
            isEmpty);
      } else {
        expect(find.text('Could not load devices.'), findsOneWidget);
        expect(find.byType(TextField), findsNothing);
      }
    });
  }

  for (final fails in [false, true]) {
    testWidgets('hub route can close during ${fails ? 'failure' : 'loading'}',
        (tester) async {
      _registerWidgetCleanup(tester);
      final pendingDetails = Completer<RhythmHello>();
      connection._runtime.reply = () => pendingDetails.future;
      await tester.pumpWidget(ChangeNotifierProvider.value(
        value: sync,
        child: MaterialApp(
          theme: ThemeData.dark().copyWith(platform: TargetPlatform.iOS),
          home: Scaffold(body: Builder(builder: (context) {
            return TextButton(
              onPressed: () =>
                  Navigator.of(context).push(PageRouteBuilder<void>(
                opaque: false,
                barrierColor: Colors.black54,
                pageBuilder: (_, animation, secondaryAnimation) =>
                    const RepaintBoundary(
                  key: ValueKey('dismissible-detail-route'),
                  child: DeviceDetailsLoader(
                    showCloseButton: true,
                    child: Scaffold(body: Text('Hub devices')),
                  ),
                ),
              )),
              child: const Text('Open hub'),
            );
          })),
        ),
      ));
      await tester.tap(find.text('Open hub'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 300));
      expect(find.byType(CircularProgressIndicator), findsOneWidget);
      if (fails) {
        pendingDetails.completeError(StateError('simulated transport failure'));
        await tester.pumpAndSettle();
        expect(find.text('Could not load devices.'), findsOneWidget);
        final screenshotPath =
            Platform.environment['RHYTHM_DISMISSIBLE_DETAIL_SCREENSHOT'];
        if (screenshotPath != null) {
          await expectLater(
              find.byKey(const ValueKey('dismissible-detail-route')),
              matchesGoldenFile(screenshotPath));
        }
      }
      expect(find.text('Hub devices'), findsNothing);
      await tester.tap(find.byType(HeaderCloseButton));
      await tester.pumpAndSettle();
      expect(find.byType(DeviceDetailsLoader), findsNothing);
      expect(find.text('Open hub'), findsOneWidget);
      if (!fails) {
        pendingDetails.complete(_hello(details: true));
        await tester.pumpAndSettle();
      }
      expect(tester.takeException(), isNull);
    });
  }
}
