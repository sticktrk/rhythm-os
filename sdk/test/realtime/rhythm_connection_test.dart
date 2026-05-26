import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  group('RhythmConnection', () {
    late HttpServer server;
    late int nodeStateRequests;
    late int sseConnections;
    late List<String> sseEventChunks;
    late Duration sseCloseDelay;

    setUp(() async {
      nodeStateRequests = 0;
      sseConnections = 0;
      sseEventChunks = const [];
      sseCloseDelay = const Duration(milliseconds: 10);

      server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      server.listen((request) async {
        switch (request.uri.path) {
          case '/api/state':
            request.response.headers.contentType = ContentType.json;
            request.response.write(jsonEncode({
              'version': '1.2.3',
              'platform': 'desktop',
              'context': 'server',
              'nodes': [
                {
                  'id': 'room-1',
                  'name': 'Living Room',
                  'kind': 'room',
                  'state': 'active',
                  'rhythm_enabled': true,
                  'disabled': false,
                  'time_offset': 0.0,
                  'brightness_offset': 0.0,
                  'lights_on': true,
                  'brightness': 70,
                  'kelvin': 3200,
                },
              ],
            }));
            await request.response.close();
            break;
          case '/api/nodes/state':
            nodeStateRequests++;
            request.response.headers.contentType = ContentType.json;
            request.response.write(jsonEncode({
              'nodes': [
                {
                  'id': 'room-1',
                  'name': 'Living Room',
                  'kind': 'room',
                  'state': 'active',
                  'rhythm_enabled': true,
                  'disabled': false,
                  'time_offset': 0.0,
                  'brightness_offset': 0.0,
                  'lights_on': true,
                  'brightness': 70,
                  'kelvin': 3200,
                },
              ],
            }));
            await request.response.close();
            break;
          case '/api/events':
            sseConnections++;
            request.response.headers
              ..contentType =
                  ContentType('text', 'event-stream', charset: 'utf-8')
              ..set(HttpHeaders.cacheControlHeader, 'no-cache');
            request.response.write(': connected\n\n');
            await request.response.flush();
            for (final chunk in sseEventChunks) {
              request.response.write(chunk);
              await request.response.flush();
            }
            await Future<void>.delayed(sseCloseDelay);
            await request.response.close();
            break;
          default:
            request.response.statusCode = HttpStatus.notFound;
            await request.response.close();
            break;
        }
      });
    });

    tearDown(() async {
      await server.close(force: true);
    });

    test('does not issue a fresh poll on every SSE disconnect edge', () async {
      final connection = RhythmConnection();
      addTearDown(connection.dispose);

      await connection.connect('127.0.0.1', port: server.port);
      await Future<void>.delayed(const Duration(milliseconds: 2500));

      expect(sseConnections, greaterThanOrEqualTo(2));
      expect(
        nodeStateRequests,
        0,
        reason:
            'Recent hello/SSE state should debounce fast poll restarts during stream flaps.',
      );
    });

    test('parses warning_active and addressed hub status from SSE', () async {
      sseEventChunks = [
        'event: motion_timer\n'
            'data: {"timers":[{"node_id":"room-1","motion_active":false,"motion_owned":true,"remaining_secs":15,"timeout_secs":1200,"warning_active":true}]}\n\n',
        'event: hub_status\n'
            'data: {"hub_type":"hue","address":"192.168.1.20:443","connected":false}\n\n',
      ];
      sseCloseDelay = const Duration(milliseconds: 100);

      final connection = RhythmConnection();
      addTearDown(connection.dispose);

      final motionFuture = connection.motionTimerEvents.first.timeout(
        const Duration(seconds: 2),
      );
      final hubFuture = connection.hubEvents.first.timeout(
        const Duration(seconds: 2),
      );

      await connection.connect('127.0.0.1', port: server.port);

      final motion = await motionFuture;
      final hub = await hubFuture;

      expect(motion.nodeId, 'room-1');
      expect(motion.motionOwned, isTrue);
      expect(motion.remainingSecs, 15);
      expect(motion.timeoutSecs, 1200);
      expect(motion.warningActive, isTrue);
      expect(hub.event, 'disconnected');
      expect(hub.hubType, 'hue');
      expect(hub.address, '192.168.1.20:443');
    });

    test(
        'parses mode_changed SSE without re-helloing on compatibility settings_changed',
        () async {
      sseEventChunks = [
        'event: mode_changed\n'
            'data: {"type":"mode_changed","data":{"active":"sleep","cause":"sunset","transition_id":"day_to_sleep","epoch_ms":1778058932588}}\n\n',
        'event: settings_changed\n'
            'data: {"type":"settings_changed","data":{}}\n\n',
      ];
      sseCloseDelay = const Duration(milliseconds: 100);

      final connection = RhythmConnection();
      addTearDown(connection.dispose);

      var newNodesDetected = false;
      final newNodesSub = connection.newNodesDetected.listen((_) {
        newNodesDetected = true;
      });
      addTearDown(newNodesSub.cancel);

      final modeFuture = connection.modeChangedEvents.first.timeout(
        const Duration(seconds: 2),
      );

      await connection.connect('127.0.0.1', port: server.port);

      final mode = await modeFuture;
      await Future<void>.delayed(const Duration(milliseconds: 150));

      expect(mode.active, RhythmMode.sleep);
      expect(mode.lastChange?.cause, 'sunset');
      expect(mode.lastChange?.transitionId, 'day_to_sleep');
      expect(mode.lastChange?.epochMs, 1778058932588);
      expect(newNodesDetected, isFalse);
    });

    test('parses settings_changed SSE payload', () async {
      sseEventChunks = [
        'event: settings_changed\n'
            'data: {"type":"settings_changed","data":{"settings":{"power_save":true}}}\n\n',
      ];
      sseCloseDelay = const Duration(milliseconds: 100);

      final connection = RhythmConnection();
      addTearDown(connection.dispose);

      final settingsFuture = connection.settingsChangedEvents.first.timeout(
        const Duration(seconds: 2),
      );

      await connection.connect('127.0.0.1', port: server.port);

      final settings = await settingsFuture;
      expect(settings.powerSave, isTrue);
    });

    test('parses light_breaker_changed SSE payload', () async {
      sseEventChunks = [
        'event: light_breaker_changed\n'
            'data: {"type":"light_breaker_changed","data":{"light_breaker":{"enabled":false}}}\n\n',
      ];
      sseCloseDelay = const Duration(milliseconds: 100);

      final connection = RhythmConnection();
      addTearDown(connection.dispose);

      final lightBreakerFuture =
          connection.lightBreakerChangedEvents.first.timeout(
        const Duration(seconds: 2),
      );

      await connection.connect('127.0.0.1', port: server.port);

      final lightBreaker = await lightBreakerFuture;
      expect(lightBreaker.enabled, isFalse);
    });

    test('parses input_event SSE events', () async {
      sseEventChunks = [
        'event: input_event\n'
            'data: {"type":"input_event","data":{"kind":"button","epoch_ms":1778058932588,"route":"node_control","hub_type":"hue","source_node_id":"button-1","target_node_id":"room-1","native_device_id":"native-button","button_action":"on_press"}}\n\n',
      ];
      sseCloseDelay = const Duration(milliseconds: 100);

      final connection = RhythmConnection();
      addTearDown(connection.dispose);

      final inputFuture = connection.inputEvents.first.timeout(
        const Duration(seconds: 2),
      );

      await connection.connect('127.0.0.1', port: server.port);

      final input = await inputFuture;
      expect(input, isA<RhythmButtonInputEvent>());
      final button = input as RhythmButtonInputEvent;
      expect(button.sourceNodeId, 'button-1');
      expect(button.targetNodeId, 'room-1');
      expect(button.nativeDeviceId, 'native-button');
      expect(button.buttonAction, RhythmButtonAction.onPress);
    });

    test('parses pairing_progress SSE events', () async {
      sseEventChunks = [
        'event: pairing_progress\n'
            'data: {"type":"pairing_progress","data":{"hub_type":"matter","session_id":"pair-1","status":"commissioning","stage":"commissioning","message":"Commissioning Matter device"}}\n\n',
        'event: pairing_progress\n'
            'data: {"type":"pairing_progress","data":{"hub_type":"matter","session_id":"pair-1","status":"complete","stage":"complete","message":"Pairing complete","device":{"device_id":"matter-100","name":"Test Bulb","device_type":"light","manufacturer":"Acme","model":"A19"}}}\n\n',
      ];
      sseCloseDelay = const Duration(milliseconds: 100);

      final connection = RhythmConnection();
      addTearDown(connection.dispose);

      final events = <RhythmPairingProgress>[];
      final sub = connection.pairingProgressEvents.listen(events.add);
      addTearDown(sub.cancel);

      await connection.connect('127.0.0.1', port: server.port);
      await Future<void>.delayed(const Duration(milliseconds: 200));

      expect(events.length, 2);
      expect(events[0].sessionId, 'pair-1');
      expect(events[0].stage, RhythmPairingStage.commissioning);
      expect(events[0].status, RhythmPairingStatus.commissioning);
      expect(events[1].stage, RhythmPairingStage.complete);
      expect(events[1].device?.deviceId, 'matter-100');
      expect(events[1].device?.name, 'Test Bulb');
    });

    test('parses ota_update_progress SSE events with percent', () async {
      sseEventChunks = [
        'event: ota_update_progress\n'
            'data: {"type":"ota_update_progress","data":{"stage":"downloading","message":"Downloading update bundle","current_version":"0.4.192-beta","target_version":"0.4.193-beta","update_available":true,"downloaded_bytes":50,"total_bytes":100,"percent":50}}\n\n',
        'event: ota_update_progress\n'
            'data: {"type":"ota_update_progress","data":{"stage":"restarting","message":"Restarting"}}\n\n',
      ];
      sseCloseDelay = const Duration(milliseconds: 100);

      final connection = RhythmConnection();
      addTearDown(connection.dispose);

      final events = <RhythmOtaUpdateProgress>[];
      final sub = connection.otaUpdateProgressEvents.listen(events.add);
      addTearDown(sub.cancel);

      await connection.connect('127.0.0.1', port: server.port);
      await Future<void>.delayed(const Duration(milliseconds: 200));

      expect(events.length, 2);
      expect(events[0].stage, RhythmOtaUpdateStage.downloading);
      expect(events[0].percent, 50);
      expect(events[0].targetVersion, '0.4.193-beta');
      expect(events[1].stage, RhythmOtaUpdateStage.restarting);
      expect(events[1].isTerminal, isTrue);
    });
  });
}
