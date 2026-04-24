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
  });
}
