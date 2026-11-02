import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

// Drive the ordinary 15-second timer without clearing the connection's cache
// (pollNow deliberately clears it and would conceal a missing diff field).
class _PollTimer implements Timer {
  _PollTimer(this.callback);
  final void Function(Timer) callback;
  bool _active = true;
  int _tick = 0;
  @override
  bool get isActive => _active;
  @override
  int get tick => _tick;
  @override
  void cancel() => _active = false;
  void fire() {
    if (_active) {
      _tick++;
      callback(this);
    }
  }
}

class _PollClock {
  final timers = <_PollTimer>[];
  Future<void> run(Future<void> Function() body) =>
      runZoned(body, zoneSpecification: ZoneSpecification(
        createPeriodicTimer: (self, parent, zone, duration, callback) {
          if (duration != const Duration(seconds: 15)) {
            return parent.createPeriodicTimer(zone, duration, callback);
          }
          final timer = _PollTimer(callback);
          timers.add(timer);
          return timer;
        },
      ));
  void fire() {
    final active = timers.where((timer) => timer.isActive).toList();
    expect(active, hasLength(1));
    active.single.fire();
  }
}

Future<void> _waitFor(bool Function() condition) async {
  final deadline = DateTime.now().add(const Duration(seconds: 2));
  while (!condition()) {
    if (DateTime.now().isAfter(deadline)) fail('Loopback request timed out');
    await Future<void>.delayed(const Duration(milliseconds: 5));
  }
}

class _Fixture {
  _Fixture(this.server, this.clock, this.connection, this.subscription);
  final HttpServer server;
  final _PollClock clock;
  final RhythmConnection connection;
  final StreamSubscription<RhythmRoomState> subscription;
  final events = <RhythmRoomState>[];
  String? resolvedMode = 'sleep';
  bool includeProfileSettings = true;
  bool includeNode = true;
  bool motion = false;
  int polls = 0;
  HttpResponse? sse;
  Map<String, dynamic>? actionResponse;

  Map<String, dynamic> get node => {
        'id': 'room-1',
        'kind': 'room',
        'state': 'active',
        'rhythm_enabled': true,
        'time_offset': 0.0,
        'brightness_offset': 0.0,
        'lights_on': true,
        'brightness': 70,
        'kelvin': 3200,
        if (includeProfileSettings)
          'profile_settings': {
            if (resolvedMode != null) 'resolved_mode': resolvedMode,
          },
        if (motion) ...{
          'motion_active': false,
          'motion_owned': false,
          'remaining_secs': 0,
          'timeout_secs': 120,
          'warning_active': false,
        },
      };

  static Future<_Fixture> start(_PollClock clock,
      {bool withSse = false}) async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final connection = RhythmConnection();
    late _Fixture fixture;
    final subscription = connection.rhythmStateEvents.listen(
      (event) => fixture.events.add(event),
    );
    fixture = _Fixture(server, clock, connection, subscription);
    server.listen((request) async {
      switch (request.uri.path) {
        case '/api/state':
          request.response.headers.contentType = ContentType.json;
          request.response.write(jsonEncode({
            'platform': 'desktop',
            'server_instance_id': 'synthetic-schedule-server',
            'nodes': [fixture.node],
          }));
          await request.response.close();
        case '/api/nodes/state':
          request.response.headers.contentType = ContentType.json;
          request.response.write(jsonEncode({
            'nodes': [if (fixture.includeNode) fixture.node],
          }));
          await request.response.close();
          fixture.polls++;
        case '/api/nodes/action':
          request.response.headers.contentType = ContentType.json;
          request.response
              .write(jsonEncode(fixture.actionResponse ?? fixture.node));
          await request.response.close();
        case '/api/events' when withSse:
          request.response.bufferOutput = false;
          request.response.headers.contentType =
              ContentType('text', 'event-stream', charset: 'utf-8');
          request.response.write(': connected\n\n');
          await request.response.flush();
          fixture.sse = request.response;
        default:
          request.response.statusCode = HttpStatus.notFound;
          await request.response.close();
      }
    });
    addTearDown(() async {
      connection.dispose();
      await subscription.cancel();
      await server.close(force: true);
    });
    await connection.connect('127.0.0.1', port: server.port);
    await _waitFor(
        () => withSse ? connection.sseConnected : !connection.sseSupported);
    return fixture;
  }

  Future<void> poll() async {
    final previous = polls;
    clock.fire();
    await _waitFor(() => polls > previous);
    // Let the real HTTP response decode and broadcast its diff before checking
    // the absence of an event, as well as the event-producing cases.
    await Future<void>.delayed(const Duration(milliseconds: 20));
  }

  Future<void> send(String type, Map<String, dynamic> payload) async {
    final previous = connection.lastSseEvents[type]?.time;
    sse!.write('event: $type\ndata: ${jsonEncode(payload)}\n\n');
    await sse!.flush();
    await _waitFor(() =>
        connection.lastSseEvents.containsKey(type) &&
        connection.lastSseEvents[type]?.time != previous);
    await Future<void>.delayed(Duration.zero);
  }

  Future<void> loseSse() async {
    await sse!.close();
    await _waitFor(() => !connection.sseConnected);
  }
}

void main() {
  test('ordinary polling emits mode-only changes and suppresses duplicates',
      () async {
    final clock = _PollClock();
    await clock.run(() async {
      final f = await _Fixture.start(clock);
      await f.poll();
      expect(f.events, isEmpty, reason: 'hello seeds the resolved mode');
      f.resolvedMode = 'day';
      await f.poll();
      expect(f.events.map((event) => event.profileSettings?.resolvedMode),
          [RhythmMode.day]);
      await f.poll();
      expect(f.events, hasLength(1));
      f.resolvedMode = 'sleep';
      await f.poll();
      expect(f.events.last.profileSettings?.resolvedMode, RhythmMode.sleep);
      f.includeProfileSettings = false;
      await f.poll();
      expect(f.events.last.profileSettings, isNull,
          reason: 'a full older-server payload clears unsupported metadata');
      expect(f.events, hasLength(3));
      await f.poll();
      expect(f.events, hasLength(3));
    });
  });

  test('action readbacks seed modes and omitted partial settings preserve them',
      () async {
    final clock = _PollClock();
    await clock.run(() async {
      final f = await _Fixture.start(clock);
      f.resolvedMode = 'day';
      final result =
          await f.connection.api.nodeAction(nodeId: 'room-1', action: 'reset');
      expect(result?.profileSettings?.resolvedMode, RhythmMode.day);
      await f.poll();
      expect(f.events, isEmpty,
          reason: 'action already delivered the new mode');
      f.actionResponse = Map.of(f.node)..remove('profile_settings');
      await f.connection.api.nodeAction(nodeId: 'room-1', action: 'reset');
      await f.poll();
      expect(f.events, isEmpty,
          reason: 'partial action readback must retain mode');
      f.actionResponse = Map.of(f.node)
        ..['profile_settings'] = <String, dynamic>{};
      await f.connection.api.nodeAction(nodeId: 'room-1', action: 'reset');
      await f.poll();
      expect(f.events.single.profileSettings?.resolvedMode, RhythmMode.day,
          reason:
              'explicit settings lacking metadata invalidate the old cache');
    });
  });

  test('SSE and motion cache rewrites retain modes before fallback polling',
      () async {
    final clock = _PollClock();
    await clock.run(() async {
      final f = await _Fixture.start(clock, withSse: true);
      f.resolvedMode = 'day';
      await f.send('node_state', {
        'nodes': [f.node]
      });
      expect(f.events.single.profileSettings?.resolvedMode, RhythmMode.day);
      await f.send('motion_timer', {
        'timers': [
          {
            'node_id': 'room-1',
            'motion_active': false,
            'motion_owned': false,
            'remaining_secs': 0,
            'timeout_secs': 120,
            'warning_active': false
          }
        ]
      });
      await f.send('motion_timer', {'timers': []});
      await f.send('node_state', {
        'nodes': [Map.of(f.node)..remove('profile_settings')]
      });
      expect(f.events, hasLength(2));
      f.events.clear();
      await f.loseSse();
      await f.poll();
      expect(f.events, isEmpty,
          reason: 'SSE partial and motion events retain the mode');
      f.motion = true;
      await f.poll();
      f.includeNode = false;
      await f.poll();
      f.includeNode = true;
      f.motion = false;
      await f.poll();
      expect(f.events, isEmpty,
          reason: 'clearing a removed node motion timer retains mode');
      f.resolvedMode = 'sleep';
      await f.poll();
      expect(f.events.single.profileSettings?.resolvedMode, RhythmMode.sleep);
    });
  });
}
