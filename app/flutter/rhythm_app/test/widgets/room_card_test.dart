import 'dart:async';
import 'dart:math' as math;
import 'dart:ui' show SemanticsAction, Tristate;

import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/providers/room_provider.dart';
import 'package:rhythm_app/providers/server_sync_provider.dart';
import 'package:rhythm_app/services/analytics_service.dart';
import 'package:rhythm_app/widgets/first_run_explainer.dart';
import 'package:rhythm_app/widgets/mood_sheet.dart';
import 'package:rhythm_app/widgets/room_card.dart';
import 'package:rhythm_app/widgets/room_settings_sheet.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../helpers/capturing_analytics_backend.dart';

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

Finder _powerToggle({String roomId = 'room-1'}) =>
    find.byKey(ValueKey('room-card-power-toggle-$roomId'));

Finder _powerSwitch({String roomId = 'room-1'}) =>
    find.byKey(ValueKey('room-card-power-switch-$roomId'));

Finder _powerThumb({String roomId = 'room-1'}) =>
    find.byKey(ValueKey('room-card-power-thumb-$roomId'));

Finder _powerToggleIcon({String roomId = 'room-1'}) =>
    find.byKey(ValueKey('room-card-power-toggle-icon-$roomId'));

Finder _powerToggleLabel({String roomId = 'room-1'}) =>
    find.byKey(ValueKey('room-card-power-toggle-label-$roomId'));

Finder _scenesControl({String roomId = 'room-1'}) =>
    find.byKey(ValueKey('room-card-control-pill-$roomId-scenes'));

Finder _brightnessControl({String roomId = 'room-1'}) =>
    find.byKey(ValueKey('room-card-control-pill-$roomId-brightness'));

Finder _colorControl({String roomId = 'room-1'}) =>
    find.byKey(ValueKey('room-card-control-pill-$roomId-color'));

Finder _jewelLabel(String control, {String roomId = 'room-1'}) =>
    find.byKey(ValueKey('room-card-control-label-$roomId-$control'));

Finder _brightnessSlider({String roomId = 'room-1'}) => find.byKey(
      ValueKey('room-card-brightness-slider-$roomId'),
    );

Finder _cctSlider({String roomId = 'room-1'}) => find.byKey(
      ValueKey('room-card-cct-slider-$roomId'),
    );

Finder _sceneBrightnessSlider({String roomId = 'room-1'}) => find.byKey(
      ValueKey('room-card-scene-brightness-slider-$roomId'),
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

Future<void> _tapPowerAt(
  WidgetTester tester,
  double position, {
  String roomId = 'room-1',
}) async {
  expect(
    _powerSwitch(roomId: roomId).hitTestable(),
    findsOneWidget,
    reason: 'The visible power track must receive pointer input.',
  );
  final track = tester.getRect(_powerSwitch(roomId: roomId));
  await tester.tapAt(
    Offset(
      track.left + track.width * position.clamp(0.05, 0.95),
      track.center.dy,
    ),
  );
  // Let the parent card's double-tap recognizer release its short-lived timer.
  // The switch itself updates from raw pointer-up before this timeout.
  await tester.pump(const Duration(milliseconds: 350));
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
    final titleText = tester.widget<Text>(title);
    expect(titleText.style?.fontSize, 20);
    expect(titleText.style?.fontWeight, FontWeight.w700);
    expect(titleText.style?.letterSpacing, -0.35);
    expect(titleText.maxLines, 2);
    expect(find.byIcon(Icons.meeting_room_rounded), findsOneWidget);
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
    expect(find.byType(Slider), findsNothing);
    await _tapRoomSegment(tester, 'Brightness');
    expect(tester.widget<Slider>(_brightnessSlider()).onChanged, isNotNull);

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
    expect(
      tester.widget<GestureDetector>(_roomSegment('Brightness')).onTap,
      isNull,
    );
    expect(tester.widget<GestureDetector>(_roomSegment('Color')).onTap, isNull);
    expect(tester.widget<GestureDetector>(_powerSwitch()).onTap, isNull);
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
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 200));

    expect(roomProvider.isRoomTransitioning('room-1'), isFalse);
    expect(roomProvider.isNodeDispatchPending('room-1'), isFalse);
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
    expect(find.byType(Slider), findsNothing);
    await _tapRoomSegment(tester, 'Brightness');
    expect(tester.widget<Slider>(_brightnessSlider()).onChanged, isNotNull);

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
    expect(
      tester.widget<GestureDetector>(_roomSegment('Color')).onTap,
      isNotNull,
    );

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

    await _tapPowerAt(tester, 0.95);

    expect(roomProvider.getDisplayRoomState('room-1'), RoomModeState.active);
    expect(find.byType(Slider), findsNothing);
    expect(
      tester.widget<GestureDetector>(_roomSegment('Brightness')).onTap,
      isNotNull,
    );
    expect(
      tester.widget<GestureDetector>(_roomSegment('Color')).onTap,
      isNotNull,
    );
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

    await _tapPowerAt(tester, 0.05);

    expect(roomProvider.getDisplayRoomState('room-1'), RoomModeState.hardOff);
    expect(connection.api.nodePreferenceCalls, hasLength(1));
    expect(
      connection.api.nodePreferenceCalls.single.state,
      RoomModeState.hardOff,
    );
  });

  testWidgets(
      'inactive Scenes opens a current-CCT picker without changing lights',
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
      kelvin: 3200,
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

    await _tapRoomSegment(tester, 'Brightness');
    expect(_brightnessSlider(), findsOneWidget);

    await _tapRoomSegment(tester, 'Scenes');
    await tester.pump(const Duration(milliseconds: 300));

    expect(_brightnessSlider(), findsNothing);
    expect(_cctSlider(), findsNothing);
    expect(_sceneBrightnessSlider(), findsNothing);
    expect(
      find.byKey(const Key('mood_brightness_slider')),
      findsOneWidget,
    );
    expect(roomProvider.getRoomState('room-1'), RoomModeState.active);
    expect(roomProvider.getDisplayRoomState('room-1'), RoomModeState.active);
    expect(find.byKey(const Key('mood_color_wheel')), findsOneWidget);
    expect(find.byKey(const Key('mood_tab_hue')), findsOneWidget);
    final picker = tester.widget<MoodSheet>(find.byType(MoodSheet));
    expect(picker.initialTab, MoodTab.color);
    expect(picker.initialSceneId, isNull);
    expect(picker.initialBrightness, 42);
    expect(picker.initialColor, ColorUtils.cctToColor(3200));
    expect(connection.api.nodePreferenceCalls, isEmpty);
    expect(connection.api.nodeColorCalls, isEmpty);

    final colorPoint =
        tester.getCenter(find.byKey(const Key('mood_color_wheel'))) +
            const Offset(40, 0);
    await tester.tapAt(colorPoint);
    await tester.pump();

    expect(roomProvider.getRoomState('room-1'), RoomModeState.mood);
    expect(connection.api.nodeColorCalls, hasLength(1));
    expect(connection.api.nodeColorCalls.single.scope, 'mood');
    expect(connection.api.nodeColorCalls.single.brightness, 42);
    expect(_sceneBrightnessSlider(), findsOneWidget);
  });

  testWidgets('power switch taps cycle and drags select exact states',
      (tester) async {
    final semantics = tester.ensureSemantics();
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
    expect(_roomSegment('Off'), findsNothing);
    expect(_roomSegment('On'), findsNothing);
    expect(_roomSegment('Scenes'), findsOneWidget);
    expect(_powerToggle(), findsOneWidget);
    expect(
      find.byKey(const ValueKey('room-card-control-pill-room-1-off')),
      findsOneWidget,
    );
    expect(
      find.byKey(const ValueKey('room-card-control-pill-room-1-scenes')),
      findsOneWidget,
    );
    expect(find.text('Off'), findsNothing);
    expect(find.text('On'), findsOneWidget);
    expect(find.text('Scenes'), findsOneWidget);
    expect(_roomSegment('Brightness'), findsOneWidget);
    expect(_roomSegment('Color'), findsOneWidget);
    expect(find.byType(Slider), findsNothing);
    expect(roomProvider.getDisplayRoomState('room-1'), RoomModeState.active);
    expect(connection.api.nodePreferenceCalls, isEmpty);
    expect(
      tester.widget<AnimatedAlign>(_powerThumb()).alignment,
      Alignment.centerRight,
    );
    expect(
      find.descendant(
        of: _powerToggle(),
        matching: find.byType(Icon),
      ),
      findsOneWidget,
    );
    expect(tester.widget<Text>(_powerToggleLabel()).data, 'On');
    expect(find.text('Full off'), findsNothing);
    var powerSemantics = tester.getSemantics(_powerToggle());
    expect(powerSemantics.label, 'Room power');
    expect(powerSemantics.value, 'On');
    expect(
      powerSemantics.hint,
      'Tap to cycle; swipe to choose Off, Low glow, or On',
    );
    expect(
      powerSemantics.getSemanticsData().hasAction(SemanticsAction.decrease),
      isTrue,
    );

    // A vertical swipe that starts over the switch belongs to scrolling, not
    // power. It must not be treated as a tap merely because the pointer ended.
    final initialPowerTrack = tester.getRect(_powerSwitch());
    await tester.dragFrom(
      initialPowerTrack.center,
      const Offset(0, -48),
    );
    await tester.pump(const Duration(milliseconds: 350));
    expect(roomProvider.getRoomState('room-1'), RoomModeState.active);
    expect(connection.api.nodePreferenceCalls, isEmpty);
    expect(connection.api.nodeActionCalls, isEmpty);

    // Every point on the switch advances the state, including the active
    // thumb. Start by tapping the selected On position.
    await _tapPowerAt(tester, 0.95);

    expect(roomProvider.getRoom('room-1')?.lightsOn, isFalse);
    expect(roomProvider.getRoomState('room-1'), RoomModeState.hardOff);
    expect(
      tester.widget<AnimatedAlign>(_powerThumb()).alignment,
      Alignment.centerLeft,
    );
    expect(tester.widget<Text>(_powerToggleLabel()).data, 'Off');
    expect(_brightnessSlider(), findsNothing);
    expect(_cctSlider(), findsNothing);
    expect(
      tester.widget<GestureDetector>(_roomSegment('Brightness')).onTap,
      isNull,
    );
    expect(tester.widget<GestureDetector>(_roomSegment('Color')).onTap, isNull);
    expect(connection.api.nodePreferenceCalls, hasLength(1));
    expect(
      connection.api.nodePreferenceCalls.single.state,
      RoomModeState.hardOff,
    );

    // Tapping the selected Off position advances to Low glow.
    await _tapPowerAt(tester, 0.05);

    expect(roomProvider.getRoomState('room-1'), RoomModeState.standby);
    expect(roomProvider.getRoom('room-1')?.lightsOn, isTrue);
    expect(
      tester.widget<AnimatedAlign>(_powerThumb()).alignment,
      Alignment.center,
    );
    expect(tester.widget<Text>(_powerToggleLabel()).data, 'Low glow');
    expect(connection.api.nodePreferenceCalls, hasLength(2));
    expect(
      connection.api.nodePreferenceCalls.last.state,
      RoomModeState.standby,
    );

    expect(find.text('Full off'), findsNothing);
    final lowGlowBrightness = roomProvider.getBrightness('room-1') ?? 50;
    final lowGlowKelvin = roomProvider.getKelvin('room-1') ?? 3000;
    final lowGlowCct = ColorUtils.cctToColor(lowGlowKelvin);
    final lowGlowTint =
        (0.20 + math.sqrt(lowGlowBrightness.clamp(1, 100) / 100.0) * 0.30)
            .clamp(0.20, 0.50)
            .toDouble();
    final expectedLowGlowSurface = Color.lerp(
      const Color(0xFF141210),
      lowGlowCct,
      lowGlowTint,
    );
    expect(
      _roomCardSurfaceColor(tester, 'room-1'),
      expectedLowGlowSurface,
    );
    expect(
      find.byKey(const ValueKey('room-card-low-glow-halo-room-1')),
      findsOneWidget,
    );
    powerSemantics = tester.getSemantics(_powerToggle());
    expect(powerSemantics.label, 'Room power');
    expect(powerSemantics.value, 'Low glow');
    expect(
      powerSemantics.hint,
      'Tap to cycle; swipe to choose Off, Low glow, or On',
    );
    expect(
      powerSemantics.getSemanticsData().hasAction(SemanticsAction.increase),
      isTrue,
    );
    expect(
      powerSemantics.getSemanticsData().hasAction(SemanticsAction.decrease),
      isTrue,
    );
    expect(
      tester.widget<Icon>(_powerToggleIcon()).icon,
      Icons.brightness_low_rounded,
    );
    expect(find.byType(Slider), findsNothing);

    // Tapping the selected Low glow position completes the three-state cycle.
    await _tapPowerAt(tester, 0.5);

    expect(roomProvider.getRoom('room-1')?.lightsOn, isTrue);
    expect(roomProvider.getRoomState('room-1'), RoomModeState.active);
    expect(
      tester.widget<AnimatedAlign>(_powerThumb()).alignment,
      Alignment.centerRight,
    );
    expect(tester.widget<Text>(_powerToggleLabel()).data, 'On');
    expect(
      connection.api.nodeActionCalls,
      [(nodeId: 'room-1', action: 'reset')],
    );

    // Dragging remains positional and can jump directly from On to Off.
    final powerTrack = tester.getRect(_powerSwitch());
    await tester.dragFrom(
      powerTrack.center,
      Offset(-powerTrack.width * 0.45, 0),
    );
    await tester.pump(const Duration(milliseconds: 350));

    expect(roomProvider.getRoom('room-1')?.lightsOn, isFalse);
    expect(roomProvider.getRoomState('room-1'), RoomModeState.hardOff);
    expect(
      tester.widget<AnimatedAlign>(_powerThumb()).alignment,
      Alignment.centerLeft,
    );
    expect(tester.widget<Text>(_powerToggleLabel()).data, 'Off');
    expect(_brightnessSlider(), findsNothing);
    expect(_cctSlider(), findsNothing);
    expect(
      tester.widget<GestureDetector>(_roomSegment('Brightness')).onTap,
      isNull,
    );
    expect(tester.widget<GestureDetector>(_roomSegment('Color')).onTap, isNull);
    expect(connection.api.nodePreferenceCalls, hasLength(3));
    expect(
      connection.api.nodePreferenceCalls.last.state,
      RoomModeState.hardOff,
    );

    await tester.dragFrom(
      powerTrack.center,
      Offset(powerTrack.width * 0.45, 0),
    );
    await tester.pump(const Duration(milliseconds: 350));

    expect(roomProvider.getRoom('room-1')?.lightsOn, isTrue);
    expect(roomProvider.getRoomState('room-1'), RoomModeState.active);
    expect(
      tester.widget<AnimatedAlign>(_powerThumb()).alignment,
      Alignment.centerRight,
    );
    expect(tester.widget<Text>(_powerToggleLabel()).data, 'On');
    expect(find.byType(Slider), findsNothing);
    expect(
      connection.api.nodeActionCalls,
      [
        (nodeId: 'room-1', action: 'reset'),
        (nodeId: 'room-1', action: 'reset'),
      ],
    );

    // Re-selecting the current state by dragging remains a no-op.
    await tester.dragFrom(
      powerTrack.center,
      Offset(powerTrack.width * 0.45, 0),
    );
    await tester.pump(const Duration(milliseconds: 350));
    expect(connection.api.nodeActionCalls, hasLength(2));
    semantics.dispose();
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

  testWidgets(
      'Low glow exposes either detail and slider movement returns the room to On',
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

    expect(_powerToggle(), findsOneWidget);
    expect(find.text('Off'), findsNothing);
    expect(find.text('On'), findsNothing);
    expect(_roomSegment('Scenes'), findsOneWidget);
    expect(_roomSegment('Brightness'), findsOneWidget);
    expect(_roomSegment('Color'), findsOneWidget);
    expect(_roomSegment('Low glow'), findsNothing);
    expect(_roomSegment('Bright'), findsNothing);
    expect(find.byType(Slider), findsNothing);
    expect(tester.widget<Text>(_powerToggleLabel()).data, 'Low glow');
    expect(find.text('Full off'), findsNothing);
    expect(
      tester.widget<AnimatedAlign>(_powerThumb()).alignment,
      Alignment.center,
    );
    expect(roomProvider.getRoomState('room-1'), RoomModeState.standby);

    await _tapRoomSegment(tester, 'Brightness');
    expect(_brightnessSlider(), findsOneWidget);
    expect(_cctSlider(), findsNothing);

    tester.widget<Slider>(_brightnessSlider()).onChanged!(35);
    await tester.pump();

    expect(roomProvider.getRoomState('room-1'), RoomModeState.active);
    expect(tester.widget<Text>(_powerToggleLabel()).data, 'On');
    expect(tester.widget<Slider>(_brightnessSlider()).value, 35);
    expect(connection.api.nodePreferenceCalls, isEmpty);

    tester.widget<Slider>(_brightnessSlider()).onChangeEnd!(35);
    await tester.pump();

    expect(connection.api.nodeCurveBrightnessCalls, hasLength(1));
    expect(
      connection.api.nodeCurveBrightnessCalls.single,
      (nodeId: 'room-1', brightness: 35),
    );

    roomProvider.setRoomLightsOnLocal('room-1', true);
    roomProvider.setRoomStateLocal('room-1', RoomModeState.standby);
    await tester.pump();

    expect(tester.widget<Text>(_powerToggleLabel()).data, 'Low glow');
    expect(find.byType(Slider), findsNothing);

    await _tapRoomSegment(tester, 'Color');
    expect(_brightnessSlider(), findsNothing);
    expect(_cctSlider(), findsOneWidget);

    tester.widget<Slider>(_cctSlider()).onChanged!(3200);
    await tester.pump();

    expect(roomProvider.getRoomState('room-1'), RoomModeState.active);
    expect(tester.widget<Text>(_powerToggleLabel()).data, 'On');
    expect(connection.api.nodePreferenceCalls, isEmpty);

    tester.widget<Slider>(_cctSlider()).onChangeEnd!(3200);
    await tester.pump();

    expect(connection.api.nodeCurveColorTemperatureCalls, hasLength(1));
    final cctCall = connection.api.nodeCurveColorTemperatureCalls.single;
    expect(cctCall.nodeId, 'room-1');
    expect(cctCall.kelvin, 3200);
    expect(cctCall.preserveBrightness, isTrue);
  });

  testWidgets('on room keeps four even controls with pronounced Power',
      (tester) async {
    await tester.binding.setSurfaceSize(const Size(320, 700));
    addTearDown(() => tester.binding.setSurfaceSize(null));

    final analyticsBackend = CapturingAnalyticsBackend();
    await analyticsBackend.initialize();
    BackendProvider.setInstanceForTesting(
      auth: OfflineAuthBackend(),
      analytics: analyticsBackend,
    );
    final analytics = AnalyticsService();
    analytics.resetForTesting();
    await analytics.initialize();
    addTearDown(() {
      analytics.resetForTesting();
      BackendProvider.resetForTesting();
    });

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

    expect(_roomSegment('Low glow'), findsNothing);
    expect(_roomSegment('Bright'), findsNothing);
    expect(_powerToggle(), findsOneWidget);
    expect(_roomSegment('Off'), findsNothing);
    expect(_roomSegment('On'), findsNothing);
    expect(_roomSegment('Scenes'), findsOneWidget);
    expect(_roomSegment('Brightness'), findsOneWidget);
    expect(_roomSegment('Color'), findsOneWidget);
    expect(find.byType(Slider), findsNothing);

    final powerRect = tester.getRect(_powerToggle());
    final scenesRect = tester.getRect(_scenesControl());
    final brightnessRect = tester.getRect(_brightnessControl());
    final colorRect = tester.getRect(_colorControl());
    final scenesLabelRect = tester.getRect(_jewelLabel('scenes'));
    final brightnessLabelRect = tester.getRect(_jewelLabel('brightness'));
    final colorLabelRect = tester.getRect(_jewelLabel('color'));
    expect(
      tester.widget<AnimatedAlign>(_powerThumb()).alignment,
      Alignment.centerRight,
    );
    final binaryPowerSemantics = tester.getSemantics(_powerToggle());
    expect(
      binaryPowerSemantics.getSemanticsData().flagsCollection.isToggled,
      Tristate.isTrue,
    );
    final scenesDecoration = tester
        .widget<AnimatedContainer>(_scenesControl())
        .decoration as BoxDecoration;
    final brightnessDecoration = tester
        .widget<AnimatedContainer>(_brightnessControl())
        .decoration as BoxDecoration;
    final colorDecoration = tester
        .widget<AnimatedContainer>(_colorControl())
        .decoration as BoxDecoration;
    final brightnessGradient = brightnessDecoration.gradient! as RadialGradient;
    final colorGradient = colorDecoration.gradient! as LinearGradient;
    expect(scenesDecoration.shape, BoxShape.circle);
    expect(
      brightnessGradient.colors.every(
        (color) => color.computeLuminance() > 0.35,
      ),
      isTrue,
      reason: 'Brightness must remain luminous while unselected.',
    );
    expect(brightnessGradient.stops, const [0, 0.48, 1]);
    expect(
      colorGradient.colors.every(
        (color) => color.computeLuminance() > 0.25,
      ),
      isTrue,
      reason: 'Color must show its warm/cool endpoints instead of going dark.',
    );
    expect(colorGradient.begin, Alignment.centerLeft);
    expect(colorGradient.end, Alignment.centerRight);
    expect(colorGradient.stops, const [0, 0.46, 0.54, 1]);
    expect(colorGradient.colors.first, isNot(colorGradient.colors.last));
    expect(brightnessRect.width, closeTo(scenesRect.width, 0.1));
    expect(brightnessRect.height, closeTo(scenesRect.height, 0.1));
    expect(colorRect.width, closeTo(scenesRect.width, 0.1));
    expect(colorRect.height, closeTo(scenesRect.height, 0.1));
    expect(scenesLabelRect.top, greaterThan(scenesRect.bottom));
    expect(brightnessLabelRect.top, greaterThan(brightnessRect.bottom));
    expect(colorLabelRect.top, greaterThan(colorRect.bottom));
    expect(scenesLabelRect.center.dx, closeTo(scenesRect.center.dx, 0.1));
    expect(
      brightnessLabelRect.center.dx,
      closeTo(brightnessRect.center.dx, 0.1),
    );
    expect(colorLabelRect.center.dx, closeTo(colorRect.center.dx, 0.1));
    expect(
      tester.getRect(_roomSegment('Scenes')).contains(scenesLabelRect.center),
      isTrue,
    );
    expect(
      tester
          .getRect(_roomSegment('Brightness'))
          .contains(brightnessLabelRect.center),
      isTrue,
    );
    expect(
      tester.getRect(_roomSegment('Color')).contains(colorLabelRect.center),
      isTrue,
    );
    expect(
      find.descendant(of: _scenesControl(), matching: find.byType(Text)),
      findsNothing,
    );
    expect(
      find.descendant(of: _brightnessControl(), matching: find.byType(Text)),
      findsNothing,
    );
    expect(
      find.descendant(of: _colorControl(), matching: find.byType(Text)),
      findsNothing,
    );
    expect(powerRect.width, greaterThan(scenesRect.width));
    expect(powerRect.height, greaterThan(scenesRect.height));
    expect(powerRect.size, const Size(88, 64));
    expect(tester.getSize(_powerSwitch()), const Size(86, 40));
    expect(colorRect.right, lessThan(powerRect.left));
    expect(
      find.descendant(
        of: _powerToggle(),
        matching: find.byType(Icon),
      ),
      findsOneWidget,
    );
    expect(brightnessRect.center.dy, closeTo(scenesRect.center.dy, 0.1));
    expect(colorRect.center.dy, closeTo(scenesRect.center.dy, 0.1));
    expect(scenesRect.top, closeTo(powerRect.top, 0.1));
    final cardRect = tester.getRect(
      find.byKey(const ValueKey('room-card-surface-room-1')),
    );
    final cardWidth = cardRect.width;
    final firstCenterGap = brightnessRect.center.dx - scenesRect.center.dx;
    final secondCenterGap = colorRect.center.dx - brightnessRect.center.dx;
    final thirdCenterGap = powerRect.center.dx - colorRect.center.dx;
    expect(secondCenterGap, closeTo(firstCenterGap, 0.1));
    expect(thirdCenterGap, closeTo(firstCenterGap, 0.1));

    var brightnessSemantics = tester.getSemantics(_brightnessControl());
    expect(brightnessSemantics.label, 'Brightness');
    expect(brightnessSemantics.value, '42 percent');
    expect(
      brightnessSemantics.getSemanticsData().flagsCollection.isSelected,
      Tristate.isFalse,
    );
    expect(brightnessSemantics.hint, 'Show brightness control');

    await _tapRoomSegment(tester, 'Brightness');

    expect(_brightnessSlider(), findsOneWidget);
    expect(_cctSlider(), findsNothing);
    expect(
      tester.getSize(_brightnessSlider()).width,
      closeTo(cardWidth - 24, 0.1),
    );
    brightnessSemantics = tester.getSemantics(_brightnessControl());
    expect(
      brightnessSemantics.getSemanticsData().flagsCollection.isSelected,
      Tristate.isTrue,
    );
    expect(brightnessSemantics.hint, 'Hide brightness control');

    await _tapRoomSegment(tester, 'Brightness');
    expect(find.byType(Slider), findsNothing);
    await _tapRoomSegment(tester, 'Brightness');
    expect(_brightnessSlider(), findsOneWidget);

    await _tapRoomSegment(tester, 'Color');

    expect(_brightnessSlider(), findsNothing);
    expect(_cctSlider(), findsOneWidget);
    expect(
      tester.getSize(_cctSlider()).width,
      closeTo(cardWidth - 24, 0.1),
    );
    final colorSemantics = tester.getSemantics(_colorControl());
    expect(colorSemantics.label, 'Color');
    expect(colorSemantics.value, '2700 kelvin');
    expect(
      colorSemantics.getSemanticsData().flagsCollection.isSelected,
      Tristate.isTrue,
    );
    expect(colorSemantics.hint, 'Hide color temperature control');

    await _tapRoomSegment(tester, 'Color');
    expect(find.byType(Slider), findsNothing);

    await _tapRoomSegment(tester, 'Brightness');
    roomProvider.bumpResetGeneration();
    await tester.pump();
    expect(find.byType(Slider), findsNothing);

    await _tapRoomSegment(tester, 'Brightness');
    await _tapPowerAt(tester, 0.05);

    expect(find.byType(Slider), findsNothing);
    expect(roomProvider.getRoomState('room-1'), RoomModeState.hardOff);
    expect(
      tester.widget<AnimatedAlign>(_powerThumb()).alignment,
      Alignment.centerLeft,
    );
    expect(connection.api.nodePreferenceCalls, hasLength(1));
    expect(
      connection.api.nodePreferenceCalls.single.state,
      RoomModeState.hardOff,
    );
    expect(connection.api.nodeCurveBrightnessCalls, isEmpty);
    expect(connection.api.nodeCurveColorTemperatureCalls, isEmpty);
    final detailEvents = analyticsBackend.events
        .where((event) => event.name == 'room_card_detail_toggled')
        .toList();
    expect(
      detailEvents.map((event) => event.name),
      List.filled(7, 'room_card_detail_toggled'),
    );
    expect(
      detailEvents.map((event) => event.properties),
      [
        {'control': 'brightness', 'room_mode': 'on', 'expanded': 1},
        {'control': 'brightness', 'room_mode': 'on', 'expanded': 0},
        {'control': 'brightness', 'room_mode': 'on', 'expanded': 1},
        {'control': 'color', 'room_mode': 'on', 'expanded': 1},
        {'control': 'color', 'room_mode': 'on', 'expanded': 0},
        {'control': 'brightness', 'room_mode': 'on', 'expanded': 1},
        {'control': 'brightness', 'room_mode': 'on', 'expanded': 1},
      ],
    );
    expect(
      detailEvents.expand((event) => event.properties.keys),
      isNot(contains(anyOf('room_id', 'home_id', 'node_id', 'scene_id'))),
    );
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

    await _tapRoomSegment(tester, 'Brightness');
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

    await _tapRoomSegment(tester, 'Color');
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

    await _tapRoomSegment(tester, 'Color');
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

  testWidgets(
      'active Scenes exposes Mood brightness and disables color temperature',
      (tester) async {
    final semantics = tester.ensureSemantics();
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

    expect(find.byType(Slider), findsOneWidget);
    expect(_brightnessSlider(), findsNothing);
    expect(_cctSlider(), findsNothing);
    expect(_sceneBrightnessSlider(), findsOneWidget);
    expect(_roomSegment('Brightness'), findsOneWidget);
    expect(_roomSegment('Color'), findsOneWidget);
    final scenesSemantics = tester.getSemantics(_scenesControl());
    expect(scenesSemantics.label, 'Scenes');
    expect(scenesSemantics.value, 'Active');
    expect(scenesSemantics.hint, 'Choose a scene');
    final scenesDecoration = tester
        .widget<AnimatedContainer>(_scenesControl())
        .decoration as BoxDecoration;
    final scenesGradient = scenesDecoration.gradient as LinearGradient;
    const moodBlue = Color.fromARGB(255, 20, 80, 240);
    expect(
      scenesGradient.colors.first,
      Color.lerp(moodBlue, const Color(0xFF2A2133), 0.12),
    );

    final brightnessSemantics = tester.getSemantics(_brightnessControl());
    expect(brightnessSemantics.label, 'Brightness');
    expect(
      brightnessSemantics.getSemanticsData().hasAction(SemanticsAction.tap),
      isTrue,
    );
    expect(
      brightnessSemantics.getSemanticsData().flagsCollection.isSelected,
      Tristate.isTrue,
    );
    expect(brightnessSemantics.hint, 'Hide brightness control');
    final colorSemantics = tester.getSemantics(_colorControl());
    expect(colorSemantics.label, 'Color');
    expect(
      colorSemantics.getSemanticsData().hasAction(SemanticsAction.tap),
      isFalse,
    );
    expect(
      colorSemantics.hint,
      'Color temperature is unavailable while Scenes is active',
    );
    expect(
      tester
          .widget<AnimatedOpacity>(
            find.byKey(const ValueKey('room-card-reset-ring-room-1')),
          )
          .opacity,
      1,
    );
    expect(
      tester
          .widget<AnimatedOpacity>(
            find.byKey(
              const ValueKey('room-card-reset-control-opacity-room-1'),
            ),
          )
          .opacity,
      1,
    );
    expect(find.text('Reset'), findsOneWidget);

    tester.widget<Slider>(_sceneBrightnessSlider()).onChanged!(35);
    await tester.pump();
    tester.widget<Slider>(_sceneBrightnessSlider()).onChangeEnd!(35);
    await tester.pump();

    expect(connection.api.nodeBrightnessCalls, [
      (nodeId: 'room-1', brightness: 35),
    ]);
    expect(connection.api.nodeCurveBrightnessCalls, isEmpty);
    expect(connection.api.nodeCurveColorTemperatureCalls, isEmpty);
    expect(connection.api.nodeColorCalls, isEmpty);
    expect(roomProvider.getMoodBrightness('room-1'), 35);
    expect(roomProvider.getRoomState('room-1'), RoomModeState.mood);

    await _tapRoomSegment(tester, 'Brightness');
    expect(_sceneBrightnessSlider(), findsNothing);

    await _tapRoomSegment(tester, 'Scenes');
    await tester.pump(const Duration(milliseconds: 300));
    expect(find.byType(MoodSheet), findsOneWidget);
    expect(_sceneBrightnessSlider(), findsOneWidget);
    Navigator.of(tester.element(find.byType(MoodSheet))).pop();
    await tester.pump(const Duration(milliseconds: 320));

    tester
        .widget<GestureDetector>(
          find.byKey(const ValueKey('room-card-reset-control-room-1')),
        )
        .onTap!();
    await tester.pump();

    expect(
      connection.api.nodeActionCalls,
      [(nodeId: 'room-1', action: 'reset')],
    );
    semantics.dispose();
  });

  testWidgets('room detail orb is a power control without countdown controls',
      (tester) async {
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.binding.setSurfaceSize(const Size(390, 900));

    const room = RoomDto(
      id: 'room-1',
      name: 'Kitchen',
      source: RoomSourceDto.hue,
      kind: RoomNodeKind.room,
      deviceIds: ['light-1'],
      rhythmEnabled: true,
      disabled: false,
      lightsOn: false,
      timeOffsetMinutes: 0,
      brightnessOffset: 0,
    );
    final roomProvider = RoomProvider();
    await roomProvider.addRoom(room);
    await roomProvider.applyServerNodeState(
      'room-1',
      rhythmEnabled: true,
      timeOffset: 0,
      brightnessOffset: 0,
      state: RoomModeState.hardOff,
      lightsOn: false,
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
        child: const MaterialApp(
          home: Scaffold(body: RoomSettingsSheet(room: room)),
        ),
      ),
    );
    await tester.pump();

    final powerControl = find.byKey(const ValueKey('room-detail-power-room-1'));
    final powerStatus =
        find.byKey(const ValueKey('room-detail-power-status-room-1'));

    expect(powerControl, findsOneWidget);
    expect(find.byIcon(Icons.power_settings_new_rounded), findsOneWidget);
    expect(find.byIcon(Icons.play_arrow_rounded), findsNothing);
    expect(find.byIcon(Icons.pause_rounded), findsNothing);
    expect(find.text('Paused'), findsNothing);
    expect(tester.widget<Text>(powerStatus).data, 'Off');

    tester.widget<GestureDetector>(powerControl).onTap!();
    await tester.pump();

    expect(roomProvider.getRoom('room-1')?.lightsOn, isTrue);
    expect(roomProvider.getRoom('room-1')?.rhythmEnabled, isTrue);
    expect(roomProvider.getRoomState('room-1'), RoomModeState.active);
    expect(tester.widget<Text>(powerStatus).data, 'On');
    expect(
      connection.api.nodeActionCalls,
      [(nodeId: 'room-1', action: 'reset')],
    );

    tester.widget<GestureDetector>(powerControl).onTap!();
    await tester.pump();

    expect(roomProvider.getRoom('room-1')?.lightsOn, isFalse);
    expect(roomProvider.getRoomState('room-1'), RoomModeState.hardOff);
    expect(tester.widget<Text>(powerStatus).data, 'Off');
    expect(connection.api.nodePreferenceCalls, hasLength(1));
    expect(
      connection.api.nodePreferenceCalls.single.state,
      RoomModeState.hardOff,
    );
  });
}
