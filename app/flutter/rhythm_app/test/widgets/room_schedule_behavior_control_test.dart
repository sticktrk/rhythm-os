import 'dart:async';

import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/providers/room_provider.dart';
import 'package:rhythm_app/providers/server_sync_provider.dart';
import 'package:rhythm_app/services/analytics_service.dart';
import 'package:rhythm_app/widgets/room_schedule_behavior_control.dart';
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

  bool modeSetSucceeds = true;
  final List<List<RhythmModeConfig>> modeConfigCalls = [];
  final List<({String nodeId, RoomModeState? state, bool? rhythmEnabled})>
      nodePreferenceCalls = [];

  @override
  Future<bool> modeSet({
    RhythmMode? active,
    List<RhythmModeConfig>? configs,
  }) async {
    modeConfigCalls.add(List<RhythmModeConfig>.of(configs ?? const []));
    return modeSetSucceeds;
  }

  @override
  Future<RhythmWriteAck> nodePreferencesSet({
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
      state: state,
      rhythmEnabled: rhythmEnabled,
    ));
    return RhythmWriteAck.accepted;
  }
}

class _TestRhythmConnection extends RhythmConnection {
  _TestRhythmConnection() : api = _FakeRhythmServerApi();

  @override
  final _FakeRhythmServerApi api;

  final _helloController = StreamController<RhythmHello>.broadcast();

  @override
  Stream<RhythmHello> get helloEvents => _helloController.stream;

  @override
  bool get connected => true;

  @override
  RhythmConnectionState get connectionState => RhythmConnectionState.connected;

  void emitHello(RhythmHello hello) => _helloController.add(hello);

  @override
  Future<void> pingOrReconnect() async {}

  @override
  Future<void> reconnect({bool authoritative = false}) async {}

  @override
  void disconnect() {}

  @override
  void dispose() {
    _helloController.close();
    super.dispose();
  }
}

RhythmHello _helloWithRoomDefaults() => RhythmHello.fromJson({
      'nodes': const <Map<String, dynamic>>[],
      'mode': {
        'active': 'day',
        'configs': [
          {
            'mode': 'day',
            'active_profile_id': 'rhythm',
            'room_defaults': const <Map<String, dynamic>>[],
          },
          {
            'mode': 'sleep',
            'active_profile_id': 'sleep',
            'room_defaults': [
              {'room_id': 'room-1', 'state': 'standby'},
            ],
          },
        ],
      },
      'location': const <String, dynamic>{},
    });

RhythmModeConfig _configFor(
  List<RhythmModeConfig> configs,
  RhythmMode mode,
) =>
    configs.singleWhere((config) => config.mode == mode);

void main() {
  testWidgets(
    'room schedule control exposes Day and Night choices and coalesces writes',
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

      final roomProvider = RoomProvider();
      final connection = _TestRhythmConnection();
      final sync = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _FakeHomeProvider(),
      );
      addTearDown(() {
        sync.dispose();
        roomProvider.dispose();
        connection.dispose();
        analytics.resetForTesting();
        BackendProvider.resetForTesting();
      });

      connection.emitHello(_helloWithRoomDefaults());
      await tester.pump(const Duration(milliseconds: 10));
      await tester.pumpWidget(
        ChangeNotifierProvider<ServerSyncProvider>.value(
          value: sync,
          child: const MaterialApp(
            home: Scaffold(
              body: Padding(
                padding: EdgeInsets.all(12),
                child: RoomScheduleBehaviorControl(
                  roomId: 'room-1',
                  foregroundColor: Colors.white,
                ),
              ),
            ),
          ),
        ),
      );

      final day = find.byKey(
        const ValueKey('room-card-schedule-day-room-1'),
      );
      final night = find.byKey(
        const ValueKey('room-card-schedule-night-room-1'),
      );
      expect(day, findsOneWidget);
      expect(night, findsOneWidget);
      expect(find.text('Day'), findsOneWidget);
      expect(find.text('Night'), findsOneWidget);
      expect(find.text('Auto'), findsOneWidget);
      expect(find.text('Low glow'), findsOneWidget);
      expect(tester.getSize(day).height, greaterThanOrEqualTo(44));
      expect(tester.getSize(night).height, greaterThanOrEqualTo(44));
      expect(tester.getSemantics(day).label, 'Day schedule behavior');
      expect(tester.getSemantics(day).value, 'Auto');
      expect(tester.getSemantics(night).value, 'Low glow');

      await tester.tap(day);
      await tester.pumpAndSettle();
      expect(find.text('Auto'), findsNWidgets(2));
      expect(find.text('Low glow'), findsNWidgets(2));
      expect(find.text('Off'), findsOneWidget);
      expect(find.text('On'), findsOneWidget);
      await tester.tap(find.text('On'));
      await tester.pump(const Duration(milliseconds: 220));
      expect(sync.roomDefaultStateForMode('room-1', RhythmMode.day), 'active');

      tester
          .widget<PopupMenuButton<RoomScheduleBehavior>>(night)
          .onSelected!(RoomScheduleBehavior.off);
      await tester.pump();
      expect(
        sync.roomDefaultStateForMode('room-1', RhythmMode.sleep),
        'hard_off',
      );

      // A reconnect snapshot can race the debounce; it must not erase either
      // optimistic choice before the write resolves.
      connection.emitHello(_helloWithRoomDefaults());
      await tester.pump(const Duration(milliseconds: 10));
      expect(sync.roomDefaultStateForMode('room-1', RhythmMode.day), 'active');
      expect(
        sync.roomDefaultStateForMode('room-1', RhythmMode.sleep),
        'hard_off',
      );

      expect(connection.api.modeConfigCalls, isEmpty);
      await tester.pump(const Duration(milliseconds: 801));
      await tester.pump();
      expect(connection.api.modeConfigCalls, hasLength(1));
      final saved = connection.api.modeConfigCalls.single;
      expect(
        _configFor(saved, RhythmMode.day).roomDefaults.single.state,
        'active',
      );
      expect(
        _configFor(saved, RhythmMode.sleep).roomDefaults.single.state,
        'hard_off',
      );

      final events = analyticsBackend.events
          .where(
            (event) => event.name == 'light_profile_room_default_changed',
          )
          .toList();
      expect(events, hasLength(2));
      expect(events.first.properties, {
        'profile': 'rhythm',
        'cleared': 0,
        'source': 'room_card',
      });
      expect(events.last.properties, {
        'profile': 'sleep',
        'cleared': 0,
        'source': 'room_card',
      });
      expect(
        events.expand((event) => event.properties.keys),
        isNot(contains(anyOf('room_id', 'node_id', 'home_id', 'room_name'))),
      );
    },
  );

  testWidgets('Schedule surface emits a typed privacy-safe preset event',
      (tester) async {
    await tester.binding.setSurfaceSize(const Size(390, 700));
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

    final roomProvider = RoomProvider();
    final connection = _TestRhythmConnection();
    final sync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _FakeHomeProvider(),
    );
    addTearDown(() {
      sync.dispose();
      roomProvider.dispose();
      connection.dispose();
      analytics.resetForTesting();
      BackendProvider.resetForTesting();
    });
    connection.emitHello(_helloWithRoomDefaults());
    await tester.pump(const Duration(milliseconds: 10));
    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<ServerSyncProvider>.value(value: sync),
          // The segments live-apply active-mode selections through
          // RoomProvider's optimistic room state.
          ChangeNotifierProvider<RoomProvider>.value(value: roomProvider),
        ],
        child: const MaterialApp(
          home: Scaffold(
            body: RoomScheduleBehaviorSegments(
              roomId: 'room-1',
            ),
          ),
        ),
      ),
    );

    final dayAuto = find.byKey(
      const ValueKey('room-schedule-presets-day-automatic-room-1'),
    );
    final dayLowGlow = find.byKey(
      const ValueKey('room-schedule-presets-day-standby-room-1'),
    );
    final sleepLowGlow = find.byKey(
      const ValueKey('room-schedule-presets-night-standby-room-1'),
    );
    expect(tester.getSize(dayAuto).height, greaterThanOrEqualTo(44));
    expect(tester.getSize(dayLowGlow).height, greaterThanOrEqualTo(44));
    expect(tester.getSize(sleepLowGlow).height, greaterThanOrEqualTo(44));

    await tester.tap(dayLowGlow);
    await tester.pump();

    final event = analyticsBackend.events.singleWhere(
      (event) => event.name == 'room_schedule_inline_preset_changed',
    );
    expect(event.properties['journey_id'], startsWith('room-schedule-preset-'));
    expect(event.properties, containsPair('attempt_number', 1));
    expect(event.properties, containsPair('input_method', 'segment'));
    expect(event.properties, containsPair('mode', 'wake'));
    expect(event.properties, containsPair('behavior', 'standby'));

    // Day is the active mode, so the selection also moved the room's lights
    // to Low glow immediately.
    expect(connection.api.nodePreferenceCalls, hasLength(1));
    expect(connection.api.nodePreferenceCalls.single.nodeId, 'room-1');
    expect(
      connection.api.nodePreferenceCalls.single.state,
      RoomModeState.standby,
    );
    expect(
      event.properties.keys,
      isNot(contains(anyOf('room_id', 'node_id', 'home_id', 'room_name'))),
    );
    await tester.pump(const Duration(milliseconds: 801));
  });

  testWidgets('rejected schedule behavior write restores the saved state',
      (tester) async {
    final roomProvider = RoomProvider();
    final connection = _TestRhythmConnection();
    connection.api.modeSetSucceeds = false;
    final sync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _FakeHomeProvider(),
    );
    addTearDown(() {
      sync.dispose();
      roomProvider.dispose();
      connection.dispose();
    });
    connection.emitHello(_helloWithRoomDefaults());
    await tester.pump(const Duration(milliseconds: 10));
    await tester.pumpWidget(
      ChangeNotifierProvider<ServerSyncProvider>.value(
        value: sync,
        child: const MaterialApp(
          home: Scaffold(
            body: RoomScheduleBehaviorControl(
              roomId: 'room-1',
              foregroundColor: Colors.white,
            ),
          ),
        ),
      ),
    );

    final day = find.byKey(
      const ValueKey('room-card-schedule-day-room-1'),
    );
    await tester.tap(day);
    await tester.pumpAndSettle();
    await tester.tap(find.text('On'));
    await tester.pump(const Duration(milliseconds: 220));
    expect(sync.roomDefaultStateForMode('room-1', RhythmMode.day), 'active');

    await tester.pump(const Duration(milliseconds: 801));
    await tester.pump();
    expect(sync.roomDefaultStateForMode('room-1', RhythmMode.day), isNull);
    expect(tester.getSemantics(day).value, 'Auto');
  });
}
