import 'dart:convert';
import 'dart:io';

import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:rhythm_admin_api/src/device_json.dart';
import 'package:rhythm_admin_api/src/local_admin.dart';
import 'package:shelf/shelf.dart';
import 'package:test/test.dart';

const runtimeToken = 'abcdefghijklmnopqrstuvwxyz123456';
const admin = {
  'id': 'admin',
  'name': 'Alex',
  'is_active': true,
  'group_ids': ['system-admin']
};

Request browserRequest(
        {String path = 'api/session',
        Map<String, dynamic>? operation,
        Map<String, String> headers = const {}}) =>
    Request(
      operation == null ? 'GET' : 'POST',
      Uri.parse('http://127.0.0.1:8787/$path'),
      headers: {
        'x-remote-user-id': 'admin',
        'x-forwarded-host': 'ha.example',
        'x-forwarded-proto': 'https',
        'origin': 'https://ha.example',
        'x-rhythm-local-request': '1',
        'content-type': 'application/json',
        ...headers
      },
      body: operation == null ? null : jsonEncode(operation),
    );

void main() {
  late List<http.Request> requests;
  late LocalDeviceProxy proxy;
  late LocalAdminServer server;
  Map<String, dynamic>? user;

  setUp(() {
    requests = [];
    user = Map.of(admin);
    proxy = LocalDeviceProxy(
        token: runtimeToken,
        client: MockClient((request) async {
          requests.add(request);
          expect(request.headers['Authorization'], 'Bearer $runtimeToken');
          return http.Response(
              jsonEncode(switch (request.url.path) {
                '/api/state' => {'server_instance_id': 'instance-1'},
                '/api/config' => {'brightness': 50},
                '/api/addon/status' => {'deployment': 'home_assistant_addon'},
                _ => {'status': 'ok'},
              }),
              200);
        }));
    server = LocalAdminServer(
        proxy: proxy, lookupUser: (_) async => user, trustedPeer: (_) => true);
  });

  test('session uses HA identity without a cloud session or exposed token',
      () async {
    final response = await server.handler(browserRequest());
    expect(response.statusCode, 200);
    final body = await response.readAsString();
    expect(body, contains('home_assistant_addon'));
    expect(body, contains('Alex'));
    expect(body, isNot(contains(runtimeToken)));
    expect(body, isNot(contains('group_ids')));
    expect(response.headers['cache-control'], 'no-store');
  });

  test('a forged role cannot authorize an ordinary HA user', () async {
    user = {
      'id': 'admin',
      'is_active': true,
      'group_ids': ['system-users']
    };
    final response = await server
        .handler(browserRequest(headers: {'x-remote-user-role': 'admin'}));
    expect(response.statusCode, 403);
    expect(requests, isEmpty);
  });

  test('missing, inactive and mismatched identities are rejected', () async {
    expect(
        (await server
                .handler(browserRequest(headers: {'x-remote-user-id': ''})))
            .statusCode,
        403);
    user = {...admin, 'is_active': false};
    expect((await server.handler(browserRequest())).statusCode, 403);
    user = {...admin, 'id': 'somebody-else'};
    expect((await server.handler(browserRequest())).statusCode, 403);
    expect(requests, isEmpty);
  });

  test('untrusted peers cannot spoof Ingress even for read requests', () async {
    final strict =
        LocalAdminServer(proxy: proxy, lookupUser: (_) async => admin);
    expect((await strict.handler(browserRequest())).statusCode, 403);
    expect(requests, isEmpty);
  });

  test('cross-origin proxy calls are rejected before dispatch', () async {
    final response = await server.handler(browserRequest(
        path: 'api/local/device-admin/proxy',
        operation: {'method': 'GET', 'path': 'api/state'},
        headers: {'origin': 'https://attacker.example'}));
    expect(response.statusCode, 403);
    expect(requests, isEmpty);
  });

  test('mutations revalidate a revoked role despite a cached read session',
      () async {
    expect((await server.handler(browserRequest())).statusCode, 200);
    requests.clear();
    user = {
      ...admin,
      'group_ids': ['system-users']
    };
    final response = await server.handler(
        browserRequest(path: 'api/local/device-admin/proxy', operation: {
      'method': 'PUT',
      'path': 'api/mode',
      'body': {'mode': 'sleep'},
      'requestId': 'local-test:1234',
      'expectedServerInstanceId': 'instance-1'
    }));
    expect(response.statusCode, 403);
    expect(requests, isEmpty);
  });

  test('config writes preserve identity and reviewed resource preconditions',
      () async {
    final hash = await canonicalJsonSha256({'brightness': 50});
    final response = await server.handler(
        browserRequest(path: 'api/local/device-admin/proxy', operation: {
      'method': 'PUT',
      'path': 'api/config',
      'body': {'brightness': 60},
      'requestId': 'local-test:1234',
      'expectedServerInstanceId': 'instance-1',
      'resourcePrecondition': {'path': 'api/config', 'bodySha256': hash}
    }));
    expect(response.statusCode, 200);
    expect(requests.map((r) => r.method), ['GET', 'GET', 'PUT']);
    expect(requests.last.headers['X-Expected-Resource-Sha256'], hash);
    expect(
        requests.last.headers['X-Expected-Server-Instance-Id'], 'instance-1');
    expect(requests.last.headers['X-Request-Id'], 'local-test:1234');
    final body = await response.readAsString();
    expect(body, isNot(contains(runtimeToken)));
    expect(body, isNot(contains('hubId')));
  });

  test('stale identity and unguarded writes never dispatch a mutation',
      () async {
    final stale = await server.handler(
        browserRequest(path: 'api/local/device-admin/proxy', operation: {
      'method': 'PUT',
      'path': 'api/mode',
      'body': {'mode': 'sleep'},
      'requestId': 'local-test:1234',
      'expectedServerInstanceId': 'old-instance'
    }));
    expect(stale.statusCode, 409);
    expect(requests.every((r) => r.method == 'GET'), isTrue);
    requests.clear();
    final missing = await server.handler(browserRequest(
        path: 'api/local/device-admin/proxy',
        operation: {'method': 'PUT', 'path': 'api/mode'}));
    expect(missing.statusCode, 428);
    expect(requests, isEmpty);
  });

  test('absolute and encoded proxy escapes are rejected', () async {
    for (final path in [
      'http://supervisor/core/api/',
      'api/%2e%2e/auth/claim',
      'api/../settings'
    ]) {
      final response = await server.handler(browserRequest(
          path: 'api/local/device-admin/proxy',
          operation: {'method': 'GET', 'path': path}));
      expect(response.statusCode, 400);
    }
    expect(requests, isEmpty);
  });

  test('HA user lookup authenticates WebSocket then reads authoritative users',
      () async {
    final httpServer = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final seen = <Map<String, dynamic>>[];
    httpServer.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      socket.add(jsonEncode({'type': 'auth_required'}));
      await for (final frame in socket) {
        final message = jsonDecode(frame as String) as Map<String, dynamic>;
        seen.add(message);
        if (message['type'] == 'auth') {
          socket.add(jsonEncode({'type': 'auth_ok'}));
        } else {
          socket.add(jsonEncode({
            'type': 'result',
            'id': 1,
            'success': true,
            'result': [admin]
          }));
        }
      }
    });
    try {
      final users = HomeAssistantUsers('supervisor-test-token',
          websocketUrl: 'ws://127.0.0.1:${httpServer.port}/core/websocket');
      expect((await users.lookup('admin'))?['id'], 'admin');
      expect(
          seen[0], {'type': 'auth', 'access_token': 'supervisor-test-token'});
      expect(seen[1], {'id': 1, 'type': 'config/auth/list'});
    } finally {
      await httpServer.close(force: true);
    }
  });
}
