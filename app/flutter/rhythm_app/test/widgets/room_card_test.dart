import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/providers/room_provider.dart';
import 'package:rhythm_app/providers/server_sync_provider.dart';
import 'package:rhythm_app/widgets/first_run_explainer.dart';
import 'package:rhythm_app/widgets/room_card.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class _FakeHomeProvider extends HomeProvider {
  @override
  List<Hub> get currentHomeHubs => const [];

  @override
  Hub? getFirstHubOfType(HubType type) => null;

  @override
  Future<void> onUserSignIn() async {}
}

class _FakeRhythmServerApi extends RhythmServerApi {
  _FakeRhythmServerApi() : super(Dio());

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
  final List<({String nodeId, int brightness})> nodeBrightnessCalls = [];
  final List<({String nodeId, int brightness})> nodeCurveBrightnessCalls = [];
  final List<
      ({
        String nodeId,
        int kelvin,
        bool preserveBrightness,
      })> nodeCurveColorTemperatureCalls = [];
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

  @override
  Future<Map<String, dynamic>?> getTriageCount() async => null;

  @override
  Future<void> nodeBrightness({
    required String nodeId,
    required int brightness,
  }) async {
    nodeBrightnessCalls.add((nodeId: nodeId, brightness: brightness));
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
  Future<void> nodeColor({
    required String nodeId,
    required int r,
    required int g,
    required int b,
    int? brightness,
    int? transitionMs,
    RhythmNodeColorScope? colorScope,
    String? scope,
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
}

class _TestRhythmConnection extends RhythmConnection {
  _TestRhythmConnection() : api = _FakeRhythmServerApi();

  @override
  final _FakeRhythmServerApi api;

  @override
  bool get connected => true;

  @override
  RhythmConnectionState get connectionState => RhythmConnectionState.connected;

  @override
  Future<void> pingOrReconnect() async {}

  @override
  Future<void> reconnect({bool authoritative = false}) async {}

  @override
  void disconnect() {}
}

class _StandbyServerSyncProvider extends ServerSyncProvider {
  _StandbyServerSyncProvider({
    required super.connection,
    required super.roomProvider,
    required super.homeProvider,
  });

  @override
  bool standbyEnabledForNode(String nodeId) => true;
}

CurveData _curveDataWithKelvins(List<int> kelvins) {
  return CurveData(
    hours: [
      for (var i = 0; i < kelvins.length; i++)
        kelvins.length == 1 ? 0 : i * (24 / (kelvins.length - 1)),
    ],
    brightness: List<int>.filled(kelvins.length, 50),
    kelvin: kelvins,
    solar: SolarInfo(
      solarNoon: 12,
      solarMidnight: 0,
      sunrise: 6,
      sunset: 18,
      dayLength: 12,
    ),
  );
}

double _timeOffsetMinutesForEffectiveHour(double targetHour) {
  final now = DateTime.now();
  final currentHour = now.hour + (now.minute / 60.0) + (now.second / 3600.0);
  var deltaHours = targetHour - currentHour;
  while (deltaHours > 12) {
    deltaHours -= 24;
  }
  while (deltaHours <= -12) {
    deltaHours += 24;
  }
  return deltaHours * 60;
}

Color _roomCardSurfaceColor(WidgetTester tester, String roomId) {
  final surface = tester.widget<AnimatedContainer>(
    find.byKey(ValueKey('room-card-surface-$roomId')),
  );
  final decoration = surface.decoration as BoxDecoration;
  return decoration.color!;
}

void main() {
  setUp(() {
    // Most tests exercise Mood mechanics directly; treat the one-time
    // explainer as already seen so it doesn't intercept the first tap. The
    // gating itself is covered by a dedicated test below.
    FirstRunExplainer.seenReader = (_) => true;
    FirstRunExplainer.seenWriter = (_) async {};
  });

  testWidgets('shows and clears a spinner while the room is transitioning',
      (tester) async {
    final roomProvider = RoomProvider();
    await roomProvider.addRoom(
      const RoomDto(
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
      ),
    );
    final homeProvider = _FakeHomeProvider();
    final connection = _TestRhythmConnection();
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<RoomProvider>.value(value: roomProvider),
          ChangeNotifierProvider<ServerSyncProvider>.value(value: serverSync),
        ],
        child: MaterialApp(
          home: Scaffold(
            body: RoomCard(
              roomId: 'room-1',
              globalConfig: defaultCurveConfig,
            ),
          ),
        ),
      ),
    );

    expect(find.byType(CircularProgressIndicator), findsNothing);
    expect(tester.widget<Slider>(find.byType(Slider)).onChanged, isNotNull);

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

    expect(find.byType(CircularProgressIndicator), findsOneWidget);
    expect(tester.widget<Slider>(find.byType(Slider)).onChanged, isNull);

    await tester.tap(find.text('Off'));
    await tester.pump();

    expect(roomProvider.getDisplayRoomState('room-1'), RoomModeState.active);
    expect(find.text('Settings'), findsNothing);

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

    expect(find.byType(CircularProgressIndicator), findsNothing);
    expect(tester.widget<Slider>(find.byType(Slider)).onChanged, isNotNull);
  });

  testWidgets('shows spinner for pending dispatch without disabling controls',
      (tester) async {
    final roomProvider = RoomProvider();
    await roomProvider.addRoom(
      const RoomDto(
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
    );
    final homeProvider = _FakeHomeProvider();
    final connection = _TestRhythmConnection();
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<RoomProvider>.value(value: roomProvider),
          ChangeNotifierProvider<ServerSyncProvider>.value(value: serverSync),
        ],
        child: MaterialApp(
          home: Scaffold(
            body: RoomCard(
              roomId: 'room-1',
              globalConfig: defaultCurveConfig,
            ),
          ),
        ),
      ),
    );

    expect(find.byType(CircularProgressIndicator), findsNothing);
    expect(tester.widget<Slider>(find.byType(Slider)).onChanged, isNotNull);

    await roomProvider.applyServerNodeState(
      'room-1',
      rhythmEnabled: true,
      timeOffset: 0,
      brightnessOffset: 0,
      state: RoomModeState.active,
      pendingDispatch: true,
      lightsOn: true,
    );
    await tester.pump();

    expect(find.byType(CircularProgressIndicator), findsOneWidget);
    expect(tester.widget<Slider>(find.byType(Slider)).onChanged, isNotNull);

    await roomProvider.applyServerNodeState(
      'room-1',
      rhythmEnabled: true,
      timeOffset: 0,
      brightnessOffset: 0,
      state: RoomModeState.active,
      pendingDispatch: false,
      lightsOn: true,
    );
    await tester.pumpAndSettle();

    expect(find.byType(CircularProgressIndicator), findsNothing);
  });

  testWidgets('pending dispatch uses a neutral surface instead of light color',
      (tester) async {
    final roomProvider = RoomProvider();
    await roomProvider.addRoom(
      const RoomDto(
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
    );
    final homeProvider = _FakeHomeProvider();
    final connection = _TestRhythmConnection();
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<RoomProvider>.value(value: roomProvider),
          ChangeNotifierProvider<ServerSyncProvider>.value(value: serverSync),
        ],
        child: MaterialApp(
          home: Scaffold(
            body: RoomCard(
              roomId: 'room-1',
              globalConfig: defaultCurveConfig,
            ),
          ),
        ),
      ),
    );

    await roomProvider.applyServerNodeState(
      'room-1',
      rhythmEnabled: true,
      timeOffset: 0,
      brightnessOffset: 0,
      state: RoomModeState.active,
      pendingDispatch: true,
      lightsOn: true,
      color: (255, 0, 0),
    );
    await tester.pump();

    expect(find.byType(CircularProgressIndicator), findsOneWidget);
    expect(_roomCardSurfaceColor(tester, 'room-1'), const Color(0xFF1B222C));
  });

  testWidgets('mood segment sends mood room state', (tester) async {
    final roomProvider = RoomProvider();
    await roomProvider.addRoom(
      const RoomDto(
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
      ),
    );
    final homeProvider = _FakeHomeProvider();
    final connection = _TestRhythmConnection();
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<RoomProvider>.value(value: roomProvider),
          ChangeNotifierProvider<ServerSyncProvider>.value(value: serverSync),
        ],
        child: MaterialApp(
          home: Scaffold(
            body: RoomCard(
              roomId: 'room-1',
              globalConfig: defaultCurveConfig,
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Mood'));
    await tester.pump();

    expect(roomProvider.getRoomState('room-1'), RoomModeState.mood);
    expect(roomProvider.getDisplayRoomState('room-1'), RoomModeState.mood);
    expect(connection.api.nodePreferenceCalls, hasLength(1));
    final call = connection.api.nodePreferenceCalls.single;
    expect(call.nodeId, 'room-1');
    expect(call.rhythmEnabled, isTrue);
    expect(call.state, RoomModeState.mood);
    expect(call.state?.wireValue, 'mood');
    expect(call.profileSettings, {'mood_enabled': true});
  });

  testWidgets('first Mood tap shows the explainer and defers the mode change',
      (tester) async {
    // Pretend the explainer has never been seen for this test only.
    var marked = false;
    FirstRunExplainer.seenReader = (_) => false;
    FirstRunExplainer.seenWriter = (_) async {
      marked = true;
    };

    final roomProvider = RoomProvider();
    await roomProvider.addRoom(
      const RoomDto(
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
      ),
    );
    final homeProvider = _FakeHomeProvider();
    final connection = _TestRhythmConnection();
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<RoomProvider>.value(value: roomProvider),
          ChangeNotifierProvider<ServerSyncProvider>.value(value: serverSync),
        ],
        child: MaterialApp(
          home: Scaffold(
            body: RoomCard(
              roomId: 'room-1',
              globalConfig: defaultCurveConfig,
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Mood'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    // The explainer is presented and marked seen…
    expect(find.text('Meet Mood'), findsOneWidget);
    expect(marked, isTrue);
    // …and the mode change is deferred until the user acts on it.
    expect(roomProvider.getRoomState('room-1'), isNot(RoomModeState.mood));
    expect(connection.api.nodePreferenceCalls, isEmpty);
  });

  testWidgets('standby uses on segment and disabled brightness slider',
      (tester) async {
    final roomProvider = RoomProvider();
    await roomProvider.addRoom(
      const RoomDto(
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
      ),
    );
    await roomProvider.applyServerNodeState(
      'room-1',
      rhythmEnabled: true,
      timeOffset: 0,
      brightnessOffset: 0,
      state: RoomModeState.standby,
      lightsOn: true,
      brightness: 1,
      kelvin: 2700,
    );

    final homeProvider = _FakeHomeProvider();
    final connection = _TestRhythmConnection();
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<RoomProvider>.value(value: roomProvider),
          ChangeNotifierProvider<ServerSyncProvider>.value(value: serverSync),
        ],
        child: MaterialApp(
          home: Scaffold(
            body: RoomCard(
              roomId: 'room-1',
              globalConfig: defaultCurveConfig,
            ),
          ),
        ),
      ),
    );

    expect(find.text('Standby'), findsOneWidget);
    expect(find.text('Off'), findsOneWidget);
    expect(find.text('On'), findsNothing);
    final slider = tester.widget<Slider>(find.byType(Slider));
    expect(slider.value, 1);
    expect(slider.onChanged, isNull);

    await tester.tap(find.byType(Slider));
    await tester.pumpAndSettle();

    expect(find.text('DONE'), findsNothing);
    expect(roomProvider.getRoomState('room-1'), RoomModeState.standby);
    expect(connection.api.nodePreferenceCalls, isEmpty);

    await tester.tap(find.text('Standby'));
    await tester.pump();

    expect(roomProvider.getRoomState('room-1'), RoomModeState.active);
    expect(connection.api.nodePreferenceCalls, hasLength(1));
    final call = connection.api.nodePreferenceCalls.single;
    expect(call.nodeId, 'room-1');
    expect(call.rhythmEnabled, isTrue);
    expect(call.state, RoomModeState.active);
  });

  testWidgets('selected on segment enters standby when enabled',
      (tester) async {
    final roomProvider = RoomProvider();
    await roomProvider.addRoom(
      const RoomDto(
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
      ),
    );
    await roomProvider.applyServerNodeState(
      'room-1',
      rhythmEnabled: true,
      timeOffset: 0,
      brightnessOffset: 0,
      state: RoomModeState.active,
      lightsOn: true,
      brightness: 42,
      kelvin: 2700,
    );

    final homeProvider = _FakeHomeProvider();
    final connection = _TestRhythmConnection();
    final serverSync = _StandbyServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<RoomProvider>.value(value: roomProvider),
          ChangeNotifierProvider<ServerSyncProvider>.value(value: serverSync),
        ],
        child: MaterialApp(
          home: Scaffold(
            body: RoomCard(
              roomId: 'room-1',
              globalConfig: defaultCurveConfig,
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.text('On'));
    await tester.pump();

    expect(roomProvider.getRoomState('room-1'), RoomModeState.standby);
    expect(connection.api.nodePreferenceCalls, hasLength(1));
    final call = connection.api.nodePreferenceCalls.single;
    expect(call.nodeId, 'room-1');
    expect(call.rhythmEnabled, isTrue);
    expect(call.state, RoomModeState.standby);
  });

  testWidgets('active brightness slider uses curve modifier endpoint',
      (tester) async {
    final roomProvider = RoomProvider();
    await roomProvider.addRoom(
      const RoomDto(
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
      ),
    );
    await roomProvider.applyServerNodeState(
      'room-1',
      rhythmEnabled: true,
      timeOffset: 0,
      brightnessOffset: 0,
      state: RoomModeState.active,
      lightsOn: true,
      brightness: 42,
      kelvin: 2700,
    );

    final homeProvider = _FakeHomeProvider();
    final connection = _TestRhythmConnection();
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<RoomProvider>.value(value: roomProvider),
          ChangeNotifierProvider<ServerSyncProvider>.value(value: serverSync),
        ],
        child: MaterialApp(
          home: Scaffold(
            body: RoomCard(
              roomId: 'room-1',
              globalConfig: defaultCurveConfig,
            ),
          ),
        ),
      ),
    );

    tester.widget<Slider>(find.byType(Slider)).onChanged!(67);
    await tester.pump();
    tester.widget<Slider>(find.byType(Slider)).onChangeEnd!(67);
    await tester.pump();

    expect(connection.api.nodeBrightnessCalls, isEmpty);
    expect(connection.api.nodeCurveBrightnessCalls, hasLength(1));
    final call = connection.api.nodeCurveBrightnessCalls.single;
    expect(call.nodeId, 'room-1');
    expect(call.brightness, 67);
  });

  testWidgets('active CCT slider uses curve color-temperature modifier',
      (tester) async {
    final roomProvider = RoomProvider();
    await roomProvider.addRoom(
      const RoomDto(
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
      ),
    );
    await roomProvider.applyServerNodeState(
      'room-1',
      rhythmEnabled: true,
      timeOffset: 0,
      brightnessOffset: 0,
      state: RoomModeState.active,
      lightsOn: true,
      brightness: 42,
      kelvin: 2700,
    );

    final homeProvider = _FakeHomeProvider();
    final connection = _TestRhythmConnection();
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<RoomProvider>.value(value: roomProvider),
          ChangeNotifierProvider<ServerSyncProvider>.value(value: serverSync),
        ],
        child: MaterialApp(
          home: Scaffold(
            body: RoomCard(
              roomId: 'room-1',
              globalConfig: defaultCurveConfig,
              curveData: _curveDataWithKelvins([3000, 3200, 3400]),
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.byIcon(Icons.wb_sunny_rounded).first);
    await tester.pump();
    tester.widget<Slider>(find.byType(Slider)).onChanged!(3200);
    await tester.pump();
    tester.widget<Slider>(find.byType(Slider)).onChangeEnd!(3200);
    await tester.pump();

    expect(connection.api.nodeCurveColorTemperatureCalls, hasLength(1));
    final call = connection.api.nodeCurveColorTemperatureCalls.single;
    expect(call.nodeId, 'room-1');
    expect(call.kelvin, 3200);
    expect(call.preserveBrightness, isTrue);
  });

  testWidgets('active CCT slider uses current curve side without wrapping',
      (tester) async {
    final roomProvider = RoomProvider();
    final timeOffsetMinutes = _timeOffsetMinutesForEffectiveHour(18);
    await roomProvider.addRoom(
      RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.hue,
        kind: RoomNodeKind.room,
        deviceIds: ['light-1'],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: true,
        timeOffsetMinutes: timeOffsetMinutes,
        brightnessOffset: 0,
      ),
    );
    await roomProvider.applyServerNodeState(
      'room-1',
      rhythmEnabled: true,
      timeOffset: timeOffsetMinutes,
      brightnessOffset: 0,
      state: RoomModeState.active,
      lightsOn: true,
      brightness: 42,
      kelvin: 3000,
    );

    final homeProvider = _FakeHomeProvider();
    final connection = _TestRhythmConnection();
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<RoomProvider>.value(value: roomProvider),
          ChangeNotifierProvider<ServerSyncProvider>.value(value: serverSync),
        ],
        child: MaterialApp(
          home: Scaffold(
            body: RoomCard(
              roomId: 'room-1',
              globalConfig: defaultCurveConfig,
              curveData: _curveDataWithKelvins([2400, 6500, 3000]),
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.byIcon(Icons.wb_sunny_rounded).first);
    await tester.pump(const Duration(milliseconds: 350));

    final cctSlider = tester.widget<Slider>(find.byType(Slider));
    expect(cctSlider.min, 3000);
    expect(cctSlider.max, 6500);
    cctSlider.onChanged!(3000);
    await tester.pump();
    tester.widget<Slider>(find.byType(Slider)).onChangeEnd!(3000);
    await tester.pump();

    expect(connection.api.nodeCurveColorTemperatureCalls, hasLength(1));
    expect(connection.api.nodeCurveColorTemperatureCalls.single.kelvin, 3000);
  });

  testWidgets('mood color picker keeps current mood brightness',
      (tester) async {
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.binding.setSurfaceSize(const Size(390, 900));

    final roomProvider = RoomProvider();
    await roomProvider.addRoom(
      const RoomDto(
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
      ),
    );
    await roomProvider.applyServerNodeState(
      'room-1',
      rhythmEnabled: true,
      timeOffset: 0,
      brightnessOffset: 0,
      state: RoomModeState.mood,
      lightsOn: true,
      brightness: 6,
    );

    final homeProvider = _FakeHomeProvider();
    final connection = _TestRhythmConnection();
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<RoomProvider>.value(value: roomProvider),
          ChangeNotifierProvider<ServerSyncProvider>.value(value: serverSync),
        ],
        child: MaterialApp(
          home: Scaffold(
            body: RoomCard(
              roomId: 'room-1',
              globalConfig: defaultCurveConfig,
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Mood'));
    await tester.pump(const Duration(milliseconds: 300));

    tester
        .widget<GestureDetector>(
          find.byKey(const Key('mood_color_preset_red')),
        )
        .onTap!();
    await tester.pump();

    expect(connection.api.nodeBrightnessCalls, isEmpty);
    expect(connection.api.nodeColorCalls, hasLength(1));
    final call = connection.api.nodeColorCalls.single;
    expect(call.nodeId, 'room-1');
    expect(call.scope, 'mood');
    expect(call.brightness, 6);
    expect(roomProvider.getRoomState('room-1'), RoomModeState.mood);
  });

  testWidgets('mood brightness slider uses mood-scoped color endpoint',
      (tester) async {
    final roomProvider = RoomProvider();
    await roomProvider.addRoom(
      const RoomDto(
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
      ),
    );
    roomProvider.setRoomStateLocal('room-1', RoomModeState.mood);
    roomProvider.setRoomColorLocal(
      'room-1',
      20,
      80,
      240,
      rememberAsMood: true,
    );

    final homeProvider = _FakeHomeProvider();
    final connection = _TestRhythmConnection();
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<RoomProvider>.value(value: roomProvider),
          ChangeNotifierProvider<ServerSyncProvider>.value(value: serverSync),
        ],
        child: MaterialApp(
          home: Scaffold(
            body: RoomCard(
              roomId: 'room-1',
              globalConfig: defaultCurveConfig,
            ),
          ),
        ),
      ),
    );

    tester.widget<Slider>(find.byType(Slider)).onChanged!(35);
    await tester.pump();
    tester.widget<Slider>(find.byType(Slider)).onChangeEnd!(35);
    await tester.pump();

    expect(connection.api.nodeBrightnessCalls, isEmpty);
    expect(connection.api.nodeColorCalls, hasLength(1));
    final call = connection.api.nodeColorCalls.single;
    expect(call.nodeId, 'room-1');
    expect(call.brightness, 35);
    expect(call.scope, 'mood');
    expect(call.r, 20);
    expect(call.g, 80);
    expect(call.b, 240);
    expect(roomProvider.getRoomState('room-1'), RoomModeState.mood);
  });

  testWidgets('mood color picker uses slider brightness over active cache',
      (tester) async {
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.binding.setSurfaceSize(const Size(390, 900));

    final roomProvider = RoomProvider();
    await roomProvider.addRoom(
      const RoomDto(
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
      ),
    );
    await roomProvider.applyServerNodeState(
      'room-1',
      rhythmEnabled: true,
      timeOffset: 0,
      brightnessOffset: 0,
      state: RoomModeState.active,
      lightsOn: true,
      brightness: 44,
    );
    roomProvider.setRoomStateLocal('room-1', RoomModeState.mood);
    roomProvider.setRoomColorLocal(
      'room-1',
      20,
      80,
      240,
      rememberAsMood: true,
    );

    final homeProvider = _FakeHomeProvider();
    final connection = _TestRhythmConnection();
    final serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
    );
    addTearDown(roomProvider.dispose);
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<RoomProvider>.value(value: roomProvider),
          ChangeNotifierProvider<ServerSyncProvider>.value(value: serverSync),
        ],
        child: MaterialApp(
          home: Scaffold(
            body: RoomCard(
              roomId: 'room-1',
              globalConfig: defaultCurveConfig,
            ),
          ),
        ),
      ),
    );

    tester.widget<Slider>(find.byType(Slider)).onChanged!(13);
    await tester.pump();
    tester.widget<Slider>(find.byType(Slider)).onChangeEnd!(13);
    await tester.pump();

    await tester.tap(find.text('Mood'));
    await tester.pump(const Duration(milliseconds: 300));
    tester
        .widget<GestureDetector>(
          find.byKey(const Key('mood_color_preset_amber')),
        )
        .onTap!();
    await tester.pump();

    expect(connection.api.nodeColorCalls, hasLength(2));
    expect(connection.api.nodeColorCalls.first.brightness, 13);
    expect(connection.api.nodeColorCalls.last.brightness, 13);
  });
}
