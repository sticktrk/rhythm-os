import 'dart:async';

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
  final List<({String nodeId, bool enabled, String requestId})>
      motionActivationCalls = [];
  Completer<RhythmRoomState?>? motionActivationCompleter;
  bool motionActivationSucceeds = true;
  final List<({String nodeId, int brightness})> nodeCurveBrightnessCalls = [];
  final List<({String nodeId, String action})> nodeActionCalls = [];
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
  Future<List<RhythmTopologyNode>> getTopologyNodes() async => const [];

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

  @override
  Future<RhythmRoomState?> nodeMotionActivationSet({
    required String nodeId,
    required bool enabled,
    required String requestId,
  }) async {
    motionActivationCalls.add((
      nodeId: nodeId,
      enabled: enabled,
      requestId: requestId,
    ));
    final pending = motionActivationCompleter;
    if (pending != null) return pending.future;
    if (!motionActivationSucceeds) return null;
    return RhythmRoomState.fromJson({
      'node_id': nodeId,
      'rhythm_enabled': true,
      'time_offset': 0.0,
      'brightness_offset': 0.0,
      'state': 'active',
      'profile_settings': {'motion_activation_enabled': enabled},
    });
  }

  @override
  Future<RhythmRoomState?> nodeAction({
    required String nodeId,
    required String action,
  }) async {
    nodeActionCalls.add((nodeId: nodeId, action: action));
    return null;
  }
}

class _TestRhythmConnection extends RhythmConnection {
  _TestRhythmConnection() : api = _FakeRhythmServerApi();

  @override
  final _FakeRhythmServerApi api;

  final _helloController = StreamController<RhythmHello>.broadcast();
  final _dispatchFailures = StreamController<RhythmDispatchFailure>.broadcast();

  @override
  Stream<RhythmHello> get helloEvents => _helloController.stream;

  @override
  Stream<RhythmDispatchFailure> get dispatchFailureEvents =>
      _dispatchFailures.stream;

  void emitHello(RhythmHello hello) {
    _helloController.add(hello);
  }

  void emitDispatchFailure(RhythmDispatchFailure failure) {
    _dispatchFailures.add(failure);
  }

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

  @override
  void dispose() {
    _helloController.close();
    _dispatchFailures.close();
    super.dispose();
  }
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

Finder _roomSegment(String label, {String roomId = 'room-1'}) =>
    find.byKey(ValueKey('room-card-segment-$roomId-${label.toLowerCase()}'));

Finder _brightnessSlider({String roomId = 'room-1'}) => find.byKey(
      ValueKey('room-card-brightness-slider-$roomId'),
    );

Finder _cctSlider({String roomId = 'room-1'}) => find.byKey(
      ValueKey('room-card-cct-slider-$roomId'),
    );

Future<void> _tapRoomSegment(
  WidgetTester tester,
  String label, {
  String roomId = 'room-1',
}) async {
  tester
      .widget<GestureDetector>(
        _roomSegment(label, roomId: roomId),
      )
      .onTap!();
  await tester.pump();
}

void main() {
  setUp(() {
    // Most tests exercise Mood mechanics directly; treat the one-time
    // explainer as already seen so it doesn't intercept the first tap. The
    // gating itself is covered by a dedicated test below.
    FirstRunExplainer.seenReader = (_) => true;
    FirstRunExplainer.seenWriter = (_) async {};
  });

  testWidgets('header docks motion right and neutral tap disables motion only',
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
    roomProvider.markRoomHasSensor('room-1');
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

    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'hub_types': ['hue'],
            'device_ids': ['light-1', 'motion-1'],
            'devices': [
              {'id': 'motion-1', 'type': 'motion'},
            ],
            'rhythm_enabled': true,
            'disabled': false,
            'lights_on': true,
            'time_offset': 0,
            'brightness_offset': 0,
            'state': 'active',
            'profile_settings': {'motion_activation_enabled': true},
          },
        ],
      }),
    );

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
      transitioning: true,
      lightsOn: true,
    );
    await tester.pump();

    final title = find.byKey(const ValueKey('room-card-title-room-1'));
    final activity = find.byKey(const ValueKey('room-card-activity-room-1'));
    final motion = find.byKey(const ValueKey('room-card-motion-room-1'));
    expect(
      tester.getCenter(title).dx,
      lessThan(tester.getCenter(activity).dx),
    );
    expect(
      tester.getCenter(activity).dx,
      lessThan(tester.getCenter(motion).dx),
    );
    expect(tester.getSize(motion), const Size(28, 28));

    await roomProvider.applyServerNodeState(
      'room-1',
      rhythmEnabled: true,
      timeOffset: 0,
      brightnessOffset: 0,
      state: RoomModeState.active,
      transitioning: false,
      lightsOn: true,
    );
    await tester.pump();
    final pendingResponse = Completer<RhythmRoomState?>();
    connection.api.motionActivationCompleter = pendingResponse;
    await tester.tap(motion);
    await tester.pump();

    final pending = find.byKey(
      const ValueKey('room-card-motion-pending-room-1'),
    );
    expect(pending, findsOneWidget);
    expect(find.text('Settings'), findsNothing);
    await tester.tap(pending);
    await tester.pump();
    expect(connection.api.motionActivationCalls, hasLength(1));

    pendingResponse.complete(
      RhythmRoomState.fromJson({
        'node_id': 'room-1',
        'rhythm_enabled': true,
        'time_offset': 0.0,
        'brightness_offset': 0.0,
        'state': 'active',
        'profile_settings': {'motion_activation_enabled': false},
      }),
    );
    connection.api.motionActivationCompleter = null;
    await tester.pump();

    expect(roomProvider.getDisplayRoomState('room-1'), RoomModeState.active);
    expect(roomProvider.isLightsOn('room-1'), isTrue);
    expect(serverSync.motionActivationEnabledForNode('room-1'), isFalse);
    expect(find.byIcon(Icons.sensors_off_rounded), findsOneWidget);
    expect(connection.api.motionActivationCalls, hasLength(1));
    expect(connection.api.motionActivationCalls.single.enabled, isFalse);
    expect(connection.api.motionActivationCalls.single.requestId, isNotEmpty);

    await tester.tap(motion);
    await tester.pump();

    expect(serverSync.motionActivationEnabledForNode('room-1'), isTrue);
    expect(roomProvider.getDisplayRoomState('room-1'), RoomModeState.active);
    expect(roomProvider.isLightsOn('room-1'), isTrue);
    expect(connection.api.motionActivationCalls, hasLength(2));
    expect(connection.api.motionActivationCalls.last.enabled, isTrue);

    connection.emitHello(
      RhythmHello.fromJson({
        'version': '0.6.510-beta',
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'hub_types': ['hue'],
            'device_ids': ['light-1', 'motion-1'],
            'devices': [
              {'id': 'motion-1', 'type': 'motion'},
            ],
            'rhythm_enabled': true,
            'disabled': false,
            'lights_on': true,
            'time_offset': 0,
            'brightness_offset': 0,
            'state': 'active',
            'profile_settings': const <String, dynamic>{},
          },
        ],
      }),
    );
    await tester.pump();

    expect(serverSync.motionActivationSupportedForNode('room-1'), isFalse);
    await tester.tap(motion);
    await tester.pump();

    expect(connection.api.motionActivationCalls, hasLength(2));
    expect(
      find.text(
        'Update the Rhythm appliance (0.6.510-beta) to control motion for Kitchen.',
      ),
      findsOneWidget,
    );
    expect(find.text('Settings'), findsNothing);
  });

  testWidgets('motion countdown tap does nothing', (tester) async {
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
    roomProvider.markRoomHasSensor('room-1');
    roomProvider.updateMotionTimer(
      'room-1',
      MotionTimerInfo(
        motionActive: false,
        motionOwned: true,
        remainingSecs: 30,
        timeoutSecs: 60,
        receivedAt: DateTime.now(),
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

    await tester.tap(
      find.byKey(const ValueKey('room-card-motion-room-1')),
    );
    await tester.pump();

    expect(connection.api.nodePreferenceCalls, isEmpty);
    expect(
      roomProvider.getDisplayRoomState('room-1'),
      isNot(RoomModeState.hardOff),
    );
    expect(find.text('Settings'), findsNothing);
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
        brightnessOffset: 1,
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
    expect(tester.widget<Slider>(_brightnessSlider()).onChanged, isNotNull);
    expect(tester.widget<Slider>(_cctSlider()).onChanged, isNotNull);

    await roomProvider.applyServerNodeState(
      'room-1',
      rhythmEnabled: true,
      timeOffset: 0,
      brightnessOffset: 1,
      state: RoomModeState.active,
      transitioning: true,
      lightsOn: true,
    );
    await tester.pump();

    expect(find.byType(CircularProgressIndicator), findsOneWidget);
    expect(tester.widget<Slider>(_brightnessSlider()).onChanged, isNull);
    expect(tester.widget<Slider>(_cctSlider()).onChanged, isNull);

    await tester.tap(_roomSegment('Off'));
    await tester.pump();

    expect(roomProvider.getDisplayRoomState('room-1'), RoomModeState.active);
    expect(find.text('Settings'), findsNothing);

    await roomProvider.applyServerNodeState(
      'room-1',
      rhythmEnabled: true,
      timeOffset: 0,
      brightnessOffset: 1,
      state: RoomModeState.active,
      transitioning: false,
      lightsOn: true,
    );
    await tester.pumpAndSettle();

    expect(find.byType(CircularProgressIndicator), findsNothing);
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
        brightnessOffset: 1,
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
    expect(tester.widget<Slider>(_brightnessSlider()).onChanged, isNotNull);
    expect(tester.widget<Slider>(_cctSlider()).onChanged, isNotNull);

    await roomProvider.applyServerNodeState(
      'room-1',
      rhythmEnabled: true,
      timeOffset: 0,
      brightnessOffset: 1,
      state: RoomModeState.active,
      pendingDispatch: true,
      lightsOn: true,
    );
    await tester.pump();

    expect(find.byType(CircularProgressIndicator), findsOneWidget);
    expect(tester.widget<Slider>(_brightnessSlider()).onChanged, isNotNull);
    expect(tester.widget<Slider>(_cctSlider()).onChanged, isNotNull);

    await roomProvider.applyServerNodeState(
      'room-1',
      rhythmEnabled: true,
      timeOffset: 0,
      brightnessOffset: 1,
      state: RoomModeState.active,
      pendingDispatch: false,
      lightsOn: true,
    );
    await tester.pump();
    await tester.pump(const Duration(seconds: 1));

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
        brightnessOffset: 1,
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

    // A missed server clear must not spin forever: the fallback timeout
    // clears the flag on its own.
    await tester.pump(const Duration(seconds: 26));
    await tester.pump(); // rebuild after the fallback notifies
    await tester.pump(const Duration(milliseconds: 200)); // switcher exit
    expect(find.byType(CircularProgressIndicator), findsNothing);
  });

  testWidgets('dispatch failure shows red badge with tappable popover',
      (tester) async {
    final roomProvider = RoomProvider();
    // On-curve room (no offsets): the off-curve reset clasp overlays the
    // card's top-right corner and would sit above the failure badge.
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

    connection.emitDispatchFailure(const RhythmDispatchFailure(
      hubType: 'hue',
      hubKey: 'hue@192.168.5.1',
      nodeId: 'room-1',
      target: 'grouped_light/abc',
      kind: 'turn_on',
      status: 'timed_out',
      detail: 'exceeded 4500ms',
      dispatchMs: 4500,
    ));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));

    expect(find.byType(CircularProgressIndicator), findsNothing);
    final badge = find.byKey(const ValueKey('room_dispatch_failure_badge'));
    expect(badge, findsOneWidget);

    // Tapping reveals the failure popover.
    await tester.tap(badge);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 200));
    expect(find.text('Failed to turn on Kitchen'), findsOneWidget);

    // Tapping outside dismisses it.
    await tester.tapAt(const Offset(5, 500));
    await tester.pump();
    expect(find.text('Failed to turn on Kitchen'), findsNothing);

    // The badge expires with the failure window.
    await tester.pump(const Duration(seconds: 31));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 200));
    expect(badge, findsNothing);
  });

  testWidgets('on from off sends one reset action without active preference',
      (tester) async {
    final roomProvider = RoomProvider();
    await roomProvider.addRoom(
      const RoomDto(
        id: 'room-1',
        name: 'Matter',
        source: RoomSourceDto.matter,
        kind: RoomNodeKind.room,
        deviceIds: ['matter-104', 'matter-106', 'matter-107'],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: false,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );
    await roomProvider.applyServerNodeState(
      'room-1',
      rhythmEnabled: true,
      timeOffset: 0,
      brightnessOffset: 0,
      state: RoomModeState.hardOff,
      lightsOn: false,
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

    expect(_brightnessSlider(), findsNothing);
    expect(_cctSlider(), findsNothing);

    await _tapRoomSegment(tester, 'On');
    await tester.pump();

    expect(roomProvider.getDisplayRoomState('room-1'), RoomModeState.active);
    expect(_brightnessSlider(), findsOneWidget);
    expect(_cctSlider(), findsOneWidget);
    expect(connection.api.nodePreferenceCalls, isEmpty);
    expect(
      connection.api.nodeActionCalls,
      [(nodeId: 'room-1', action: 'reset')],
    );
    final activitySpinner = find.byKey(
      const ValueKey('room_transition_spinner'),
    );
    expect(activitySpinner, findsOneWidget);

    await tester.pump(const Duration(milliseconds: 399));
    expect(activitySpinner, findsOneWidget);

    await tester.pump(const Duration(milliseconds: 1));
    await tester.pump(const Duration(milliseconds: 200));
    expect(activitySpinner, findsNothing);
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
        brightnessOffset: 1,
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

    await _tapRoomSegment(tester, 'Scenes');

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

  testWidgets('off segment sends standby when room standby is enabled',
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
        brightnessOffset: 1,
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

    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'hub_types': ['hue'],
            'device_ids': ['light-1'],
            'rhythm_enabled': true,
            'disabled': false,
            'lights_on': true,
            'time_offset': 0,
            'brightness_offset': 1,
            'state': 'active',
            'standby_enabled': true,
          },
        ],
      }),
    );

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
    await tester.pump();

    expect(serverSync.standbyEnabledForNode('room-1'), isTrue);

    await _tapRoomSegment(tester, 'Off');

    expect(roomProvider.getRoomState('room-1'), RoomModeState.standby);
    expect(roomProvider.getDisplayRoomState('room-1'), RoomModeState.standby);
    expect(connection.api.nodePreferenceCalls, hasLength(1));
    final call = connection.api.nodePreferenceCalls.single;
    expect(call.nodeId, 'room-1');
    expect(call.rhythmEnabled, isTrue);
    expect(call.state, RoomModeState.standby);
    expect(call.state?.wireValue, 'standby');
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
        brightnessOffset: 1,
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

    await _tapRoomSegment(tester, 'Scenes');
    await tester.pump(const Duration(milliseconds: 50));

    // The explainer is presented and marked seen…
    expect(find.text('Meet Mood'), findsOneWidget);
    expect(marked, isTrue);
    // …and the mode change is deferred until the user acts on it.
    expect(roomProvider.getRoomState('room-1'), isNot(RoomModeState.mood));
    expect(connection.api.nodePreferenceCalls, isEmpty);
  });

  testWidgets('standby uses off segment and hides room sliders',
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

    expect(find.text('Off'), findsOneWidget);
    expect(find.text('On'), findsOneWidget);
    expect(find.text('Dim'), findsNothing);
    expect(find.text('Bright'), findsNothing);
    expect(find.byType(Slider), findsNothing);
    expect(_brightnessSlider(), findsNothing);
    expect(_cctSlider(), findsNothing);

    await tester.tap(_roomSegment('Off'));
    await tester.pump();

    expect(roomProvider.getRoomState('room-1'), RoomModeState.standby);
    expect(connection.api.nodePreferenceCalls, isEmpty);
  });

  testWidgets('on room shows full-width brightness then CCT sliders',
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
        brightnessOffset: 1,
      ),
    );
    await roomProvider.applyServerNodeState(
      'room-1',
      rhythmEnabled: true,
      timeOffset: 0,
      brightnessOffset: 1,
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

    expect(find.text('Dim'), findsNothing);
    expect(find.text('Bright'), findsNothing);
    expect(_brightnessSlider(), findsOneWidget);
    expect(_cctSlider(), findsOneWidget);
    expect(
      tester.getCenter(_cctSlider()).dy,
      greaterThan(tester.getCenter(_brightnessSlider()).dy),
    );
    final cardWidth = tester
        .getSize(
          find.byKey(const ValueKey('room-card-surface-room-1')),
        )
        .width;
    expect(
      tester.getSize(_brightnessSlider()).width,
      closeTo(cardWidth - 16, 0.1),
    );
    expect(
      tester.getSize(_cctSlider()).width,
      closeTo(cardWidth - 16, 0.1),
    );
    expect(connection.api.nodePreferenceCalls, isEmpty);
    expect(connection.api.nodeCurveBrightnessCalls, isEmpty);
    expect(connection.api.nodeCurveColorTemperatureCalls, isEmpty);
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
        brightnessOffset: 1,
      ),
    );
    await roomProvider.applyServerNodeState(
      'room-1',
      rhythmEnabled: true,
      timeOffset: 0,
      brightnessOffset: 1,
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

    tester.widget<Slider>(_brightnessSlider()).onChanged!(67);
    await tester.pump();
    tester.widget<Slider>(_brightnessSlider()).onChangeEnd!(67);
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
        brightnessOffset: 1,
      ),
    );
    await roomProvider.applyServerNodeState(
      'room-1',
      rhythmEnabled: true,
      timeOffset: 0,
      brightnessOffset: 1,
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

    tester.widget<Slider>(_cctSlider()).onChanged!(3200);
    await tester.pump();
    tester.widget<Slider>(_cctSlider()).onChangeEnd!(3200);
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
        brightnessOffset: 1,
      ),
    );
    await roomProvider.applyServerNodeState(
      'room-1',
      rhythmEnabled: true,
      timeOffset: timeOffsetMinutes,
      brightnessOffset: 1,
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

    final cctSlider = tester.widget<Slider>(_cctSlider());
    expect(cctSlider.min, 3000);
    expect(cctSlider.max, 6500);
    cctSlider.onChanged!(3000);
    await tester.pump();
    tester.widget<Slider>(_cctSlider()).onChangeEnd!(3000);
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

    await _tapRoomSegment(tester, 'Scenes');
    await tester.pump(const Duration(milliseconds: 300));

    final redWheelPoint =
        tester.getCenter(find.byKey(const Key('mood_color_wheel'))) +
            const Offset(40, 0);
    await tester.tapAt(redWheelPoint);
    await tester.pump();

    expect(connection.api.nodeBrightnessCalls, isEmpty);
    expect(connection.api.nodeColorCalls, hasLength(1));
    final call = connection.api.nodeColorCalls.single;
    expect(call.nodeId, 'room-1');
    expect(call.scope, 'mood');
    expect(call.brightness, 6);
    expect(roomProvider.getRoomState('room-1'), RoomModeState.mood);
  });

  testWidgets('mood hides brightness and CCT sliders', (tester) async {
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

    expect(find.byType(Slider), findsNothing);
    expect(_brightnessSlider(), findsNothing);
    expect(_cctSlider(), findsNothing);
    expect(connection.api.nodeBrightnessCalls, isEmpty);
    expect(connection.api.nodeCurveBrightnessCalls, isEmpty);
    expect(connection.api.nodeCurveColorTemperatureCalls, isEmpty);
    expect(connection.api.nodeColorCalls, isEmpty);
    expect(roomProvider.getRoomState('room-1'), RoomModeState.mood);
  });
}
