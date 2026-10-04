import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:connectivity_plus/connectivity_plus.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/providers/room_provider.dart';
import 'package:rhythm_app/providers/server_sync_provider.dart';
import 'package:rhythm_app/services/app_startup_performance.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class _Home extends HomeProvider {
  _Home(this.hub);
  final Hub hub;
  @override
  Hub? get activeServerHub => hub;
  @override
  List<Hub> get currentHomeHubs => [hub];
}

class _Connection extends RhythmConnection {
  final endpoints = <String>[];
  @override
  Future<void> connect(
    String host, {
    int port = 80,
    bool useSsl = false,
    String? webBaseUrl,
    String? authToken,
    bool authoritative = false,
    bool refresh = false,
  }) async {
    endpoints.add(host);
  }
}

Future<void> _until(bool Function() condition, {String? description}) async {
  final deadline = DateTime.now().add(const Duration(seconds: 2));
  while (!condition()) {
    if (DateTime.now().isAfter(deadline)) {
      fail('Timed out waiting for ${description ?? 'the connection event'}');
    }
    await Future<void>.delayed(const Duration(milliseconds: 5));
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final originalHttpOverrides = HttpOverrides.current;
  setUpAll(() => HttpOverrides.global = null);
  tearDownAll(() => HttpOverrides.global = originalHttpOverrides);

  for (final legacy in [false, true]) {
    for (final roomCount in [1, 24]) {
      test(
          'resume uses an ordinary snapshot and applies later power events '
          '(legacy=$legacy, rooms=$roomCount)', () async {
        final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
        final stateRequests = <Uri>[];
        final streams = <HttpResponse>[];
        final releaseAuthoritative = Completer<void>();
        var cachedPowerOn = true;
        var nodePolls = 0;
        server.listen((request) async {
          if (request.uri.path == '/api/state') {
            stateRequests.add(request.uri);
            if (request.uri.queryParameters['authoritative'] == 'true') {
              // Model an unreachable integration. No room-count-dependent
              // physical read may be on the foreground readiness path.
              await releaseAuthoritative.future;
            }
            request.response.headers.contentType = ContentType.json;
            request.response.write(jsonEncode({
              'version': legacy ? '0.6.632' : '0.6.659',
              'server_instance_id': 'synthetic-server',
              'platform': 'desktop',
              'active_profile': {},
              'location': {},
              'mode': {},
              'review': {},
              'transitions': [],
              'profiles': [],
              'scenes': [],
              'input_bindings': [],
              if (!legacy) ...{
                'capabilities': {
                  'features': [RhythmFeature.stateIncludesV1],
                },
                'state_scope': {
                  'schema_version': 1,
                  'included': ['base', 'controls', 'configuration'],
                  'nodes': 'controls',
                },
              },
              'nodes': List.generate(
                  roomCount,
                  (i) => {
                        'id': 'room-$i',
                        'name': 'Room $i',
                        'kind': 'room',
                        'state': 'active',
                        'rhythm_enabled': true,
                        'lights_on': true,
                      }),
            }));
            await request.response.close();
            if (roomCount == 24 &&
                request.uri.queryParameters['refresh_observed_power'] ==
                    'true') {
              // Simulate a fast observation finishing before SSE subscribes.
              cachedPowerOn = false;
            }
          } else if (request.uri.path == '/api/nodes/state') {
            nodePolls++;
            request.response.headers.contentType = ContentType.json;
            request.response.write(jsonEncode({
              'nodes': [
                {
                  'id': 'room-0',
                  'state': 'active',
                  'rhythm_enabled': true,
                  'lights_on': cachedPowerOn,
                }
              ],
            }));
            await request.response.close();
          } else if (request.uri.path == '/api/events') {
            request.response.bufferOutput = false;
            request.response.headers.contentType =
                ContentType('text', 'event-stream', charset: 'utf-8');
            request.response.write(': connected\n\n');
            await request.response.flush();
            streams.add(request.response);
          } else {
            request.response.headers.contentType = ContentType.json;
            request.response.write('{}');
            await request.response.close();
          }
        });
        final connection = RhythmConnection();
        final rooms = RoomProvider();
        final home = _Home(Hub.server(
          id: 'server',
          homeId: 'home',
          name: 'Server',
          host: '127.0.0.1',
          port: server.port,
          token: 'synthetic-owner-token',
        ));
        final sync = ServerSyncProvider(
          connection: connection,
          roomProvider: rooms,
          homeProvider: home,
          activityCloudCanProvision: () => false,
          authStateChanges: const Stream.empty(),
        );
        addTearDown(() async {
          sync.dispose();
          connection.dispose();
          rooms.dispose();
          home.dispose();
          releaseAuthoritative.complete();
          await server.close(force: true);
        });

        await sync.retryActiveServerConnection();
        await _until(() => sync.roomsReadyForDisplay && streams.isNotEmpty,
            description: 'initial hello and SSE');
        expect(rooms.getNode('room-0')!.lightsOn, isTrue);

        // The initial connection is not a resume and carries no resume path.
        final performance = AppStartupPerformance.instance;
        performance.start();
        performance.recordResumePath('snapshot');
        expect(performance.resumePathForTesting, isNull);

        performance.start(resumed: true);
        await sync.resumeActiveServerConnection().timeout(
              const Duration(seconds: 1),
            );
        expect(performance.resumePathForTesting, 'snapshot');
        await _until(() => sync.roomsReadyForDisplay && streams.length == 2,
            description: 'resume hello and SSE');
        expect(sync.canDispatchActions, isTrue);
        expect(stateRequests, hasLength(2));
        expect(stateRequests.last.queryParameters['authoritative'], isNull);
        expect(stateRequests.last.queryParameters['refresh_observed_power'],
            'true');
        if (roomCount == 24) {
          await _until(() => !rooms.getNode('room-0')!.lightsOn,
              description: 'power correction missed before SSE subscribed');
          expect(nodePolls, greaterThan(0));
        }

        // Supporting servers broadcast the background refresh; older servers
        // ignore the additive query and keep their periodic/event contract.
        final correction = jsonEncode({
          'nodes': [
            {
              'id': 'room-0',
              'state': 'active',
              'rhythm_enabled': true,
              if (legacy)
                'lights_on': false
              else
                'observed_power': {
                  'lights_on': false,
                  'fresh': true,
                  'source': 'periodic',
                },
            },
          ],
        });
        streams.last.write('event: node_state\ndata: $correction\n\n');
        await streams.last.flush();
        await _until(() => connection.lastSseEvents.containsKey('node_state'),
            description: 'SSE correction arrival');
        await _until(() => !rooms.getNode('room-0')!.lightsOn,
            description: 'observed power event');
        expect(sync.roomsReadyForDisplay, isTrue);

        // A stream that delivered traffic after suspension proves the selected
        // endpoint is still alive. Resume keeps it and skips hello/probing.
        performance.start(resumed: true);
        await sync.resumeActiveServerConnection(
          suspendedAt: connection.lastSseActivity
              .subtract(const Duration(milliseconds: 1)),
        );
        expect(performance.resumePathForTesting, 'live_stream');
        expect(stateRequests, hasLength(2));
        expect(streams, hasLength(2));
        expect(sync.roomsReadyForDisplay, isTrue);
      });
    }
  }

  group('endpoint race', () {
    late _Connection connection;
    late RoomProvider rooms;
    late _Home home;
    setUp(() {
      connection = _Connection();
      rooms = RoomProvider();
      home = _Home(Hub.server(
        id: 'server',
        homeId: 'home',
        name: 'Server',
        host: '192.0.2.10',
        token: 'synthetic-owner-token',
        remoteEndpoint: const HubEndpoint(
          host: 'tunnel.example.test',
          port: 443,
          useSsl: true,
        ),
      ));
    });
    tearDown(() {
      connection.dispose();
      rooms.dispose();
      home.dispose();
    });

    ServerSyncProvider provider(
      Future<bool> Function(HubEndpoint, String?) probe,
    ) {
      final sync = ServerSyncProvider(
        connection: connection,
        roomProvider: rooms,
        homeProvider: home,
        connectivityCheck: () async => [ConnectivityResult.wifi],
        endpointReachability: probe,
        activityCloudCanProvision: () => false,
        authStateChanges: const Stream.empty(),
      );
      addTearDown(sync.dispose);
      return sync;
    }

    test('working tunnel starts while the LAN probe is still pending',
        () async {
      final lan = Completer<bool>();
      final probed = <String>[];
      final sync = provider((endpoint, _) {
        probed.add(endpoint.host);
        return endpoint == home.hub.endpoint ? lan.future : Future.value(true);
      });
      await sync.retryActiveServerConnection().timeout(
            const Duration(milliseconds: 500),
          );
      expect(lan.isCompleted, isFalse);
      expect(probed, containsAll(['192.0.2.10', 'tunnel.example.test']));
      expect(connection.endpoints, ['tunnel.example.test']);
      lan.complete(true);
      await Future<void>.delayed(Duration.zero);
      expect(connection.endpoints, ['tunnel.example.test'],
          reason: 'a late LAN result cannot replace the selected transport');
    });

    test('LAN wins when both endpoints answer within the preference window',
        () async {
      final sync = provider((endpoint, _) async {
        if (endpoint == home.hub.endpoint) {
          await Future<void>.delayed(const Duration(milliseconds: 30));
        }
        return true;
      });
      await sync.retryActiveServerConnection();
      expect(connection.endpoints, ['192.0.2.10']);
    });

    test('resume reselects a tunnel after the saved LAN becomes stale',
        () async {
      var lanWorks = true;
      final sync = provider((endpoint, _) async =>
          endpoint == home.hub.endpoint ? lanWorks : true);
      await sync.retryActiveServerConnection();
      lanWorks = false;
      await sync.resumeActiveServerConnection();
      expect(connection.endpoints, ['192.0.2.10', 'tunnel.example.test']);
    });

    test('failed tunnel probe cannot preempt reachable LAN', () async {
      final sync = provider((endpoint, _) async {
        if (endpoint != home.hub.endpoint) throw const SocketException('down');
        await Future<void>.delayed(const Duration(milliseconds: 20));
        return true;
      });
      await sync.retryActiveServerConnection();
      expect(connection.endpoints, ['192.0.2.10']);
    });

    test('selecting the tunnel closes the losing LAN probe socket', () async {
      final lanServer =
          await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
      final tunnel = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      final lanClosed = Completer<void>();
      final clients = <Socket>[];
      lanServer.listen((socket) {
        clients.add(socket);
        socket.listen((_) {}, onDone: () {
          if (!lanClosed.isCompleted) lanClosed.complete();
          socket.destroy();
        });
      });
      tunnel.listen((request) async {
        expect(request.uri.path, '/api/auth/status');
        expect(request.headers.value(HttpHeaders.authorizationHeader),
            'Bearer synthetic-owner-token');
        request.response.headers.contentType = ContentType.json;
        request.response.write('{}');
        await request.response.close();
      });
      addTearDown(() async {
        for (final client in clients) {
          client.destroy();
        }
        await lanServer.close();
        await tunnel.close(force: true);
      });
      home.dispose();
      home = _Home(Hub.server(
        id: 'server',
        homeId: 'home',
        name: 'Server',
        host: '127.0.0.1',
        port: lanServer.port,
        token: 'synthetic-owner-token',
        remoteEndpoint: HubEndpoint(host: '127.0.0.1', port: tunnel.port),
      ));
      final sync = ServerSyncProvider(
        connection: connection,
        roomProvider: rooms,
        homeProvider: home,
        connectivityCheck: () async => [ConnectivityResult.wifi],
        activityCloudCanProvision: () => false,
        authStateChanges: const Stream.empty(),
      );
      addTearDown(sync.dispose);
      await sync.retryActiveServerConnection().timeout(
            const Duration(milliseconds: 500),
          );
      expect(sync.activeConnectionEndpoint!.port, tunnel.port);
      await lanClosed.future.timeout(const Duration(milliseconds: 500));
    });
  });
}
