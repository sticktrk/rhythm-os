import 'dart:async';
import 'dart:convert';

import 'package:http/http.dart' as http;
import 'package:rhythm_admin_api/src/config.dart';
import 'package:rhythm_admin_api/src/device_probe_service.dart';
import 'package:rhythm_admin_api/src/server.dart';
import 'package:rhythm_admin_api/src/supabase_rest_client.dart';
import 'package:rhythm_admin_api/src/support_access_service.dart';
import 'package:rhythm_admin_api/src/support_service.dart';
import 'package:shelf/shelf.dart';
import 'package:test/test.dart';

void main() {
  test('diagnostic routes proxy remote device requests server-side', () async {
    final deviceRequests = <http.BaseRequest>[];
    final client = _HandlerClient((request) async {
      if (request.url.host == 'supabase.test') {
        return _supabaseResponse(request);
      }

      if (request.url.host == 'device.test' &&
          request.url.path == '/api/diag/debug-bundle') {
        deviceRequests.add(request);
        expect(request.method, 'POST');
        expect(request.headers['Authorization'], 'Bearer legacy-token');
        return http.Response.bytes(
          [0x1f, 0x8b, 0x08],
          200,
          headers: {
            'content-type': 'application/gzip',
            'content-disposition':
                'attachment; filename="rhythm-debug-bundle-rpiz-test.tar.gz"',
          },
        );
      }

      if (request.url.host == 'device.test' &&
          request.url.path == '/api/diag/logs') {
        deviceRequests.add(request);
        expect(request.method, 'GET');
        expect(request.headers['Authorization'], 'Bearer legacy-token');
        return _jsonResponse({
          'status': 'ok',
          'sources': [
            {
              'id': 'rhythm-server.log',
              'file_name': 'rhythm-server.log',
              'bytes': 512,
              'modified_at': '2026-07-02T16:00:00Z',
            },
          ],
        });
      }

      if (request.url.host == 'device.test' &&
          request.url.path == '/api/diag/logs/rhythm-server.log/tail') {
        deviceRequests.add(request);
        expect(request.method, 'GET');
        expect(request.url.queryParameters['lines'], '120');
        expect(request.headers['Authorization'], 'Bearer legacy-token');
        return _jsonResponse({
          'status': 'ok',
          'tail': {
            'source': {
              'id': 'rhythm-server.log',
              'file_name': 'rhythm-server.log',
              'bytes': 512,
            },
            'requested_lines': 120,
            'returned_lines': 2,
            'lines': [
              {
                'source': 'rhythm-server.log',
                'line_number': 41,
                'text': 'server started',
              },
              {
                'source': 'rhythm-server.log',
                'line_number': 42,
                'text': 'cloudflared connected',
              },
            ],
          },
        });
      }

      if (request.url.host == 'device.test' && request.url.path == '/health') {
        deviceRequests.add(request);
        expect(request.method, 'GET');
        expect(request.headers.containsKey('Authorization'), isFalse);
        return _jsonResponse({'status': 'healthy'});
      }

      if (request.url.host == 'device.test' &&
          request.url.path == '/api/state') {
        deviceRequests.add(request);
        expect(request.method, 'GET');
        expect(request.headers['Authorization'], 'Bearer legacy-token');
        return _jsonResponse({
          'version': '0.4.200-beta',
          'server_instance_id': 'srv-test',
          'platform': 'appliance',
          'context': 'rpiz',
          'listen_port': 80,
          'last_tick_epoch_ms': 1783000000000,
          'nodes': [
            {
              'id': 'light-1',
              'name': 'Lamp',
              'kind': 'light_device',
              'devices': [],
            },
          ],
          'hubs': [
            {'type': 'matter', 'connected': true},
          ],
          'mode': {'id': 'auto'},
          'settings': {'light_runtime': 'rhythm_adaptive'},
          'active_profile': {},
          'location': {},
        });
      }

      if (request.url.host == 'device.test' &&
          request.url.path == '/api/remote-access/status') {
        deviceRequests.add(request);
        expect(request.method, 'GET');
        expect(request.headers['Authorization'], 'Bearer legacy-token');
        return _jsonResponse({
          'enabled': true,
          'configured': true,
          'hostname': 'device.example.com',
          'service_running': true,
          'connector_healthy': true,
          'registered_connections': 4,
        });
      }

      if (request.url.host == 'device.test' &&
          request.url.path == '/api/auth/status') {
        deviceRequests.add(request);
        expect(request.method, 'GET');
        expect(request.headers['Authorization'], 'Bearer legacy-token');
        return _jsonResponse({
          'requires_auth': true,
          'owner_configured': true,
          'token_count': 2,
          'via_remote_access': true,
        });
      }

      if (request.url.host == 'device.test' &&
          request.url.path == '/api/ota/status') {
        deviceRequests.add(request);
        expect(request.method, 'GET');
        expect(request.headers['Authorization'], 'Bearer legacy-token');
        return _jsonResponse({
          'state': 'idle',
          'current_version': '0.4.200-beta',
        });
      }

      if (request.url.host == 'device.test' &&
          request.url.path == '/api/ota/check') {
        deviceRequests.add(request);
        expect(request.method, 'GET');
        expect(request.headers['Authorization'], 'Bearer legacy-token');
        return _jsonResponse({
          'status': 'ok',
          'current_version': '0.4.200-beta',
          'latest_version': '0.6.205-beta',
          'update_available': true,
        });
      }

      if (request.url.host == 'device.test' &&
          request.url.path == '/api/ota/update') {
        deviceRequests.add(request);
        expect(request.method, 'POST');
        expect(request.headers['Authorization'], 'Bearer legacy-token');
        return _jsonResponse({
          'status': 'ok',
          'message': 'Update started',
          'current_version': '0.4.200-beta',
          'latest_version': '0.6.205-beta',
          'update_available': true,
        });
      }

      return http.Response('not found', 404);
    });

    final config = AdminApiConfig(
      supabaseUrl: Uri.parse('https://supabase.test'),
      supabaseAnonKey: 'anon-key',
      supabaseServiceRoleKey: 'service-role-key',
      supportAccessEncryptionKey: 'support-key',
      supportAccessKeyId: 'default',
      host: '127.0.0.1',
      port: 8787,
      allowedOrigins: {'http://127.0.0.1:5173'},
    );
    final supabase = SupabaseRestClient(config: config, httpClient: client);
    final supportAccess = SupportAccessService(
      config: config,
      supabase: supabase,
      httpClient: client,
    );
    final server = AdminApiServer(
      config: config,
      supabase: supabase,
      support: SupportService(supabase: supabase),
      probes: DeviceProbeService(
        supabase: supabase,
        supportAccess: supportAccess,
        httpClient: client,
      ),
    );

    final response = await server.handler(
      Request(
        'POST',
        Uri.parse('http://admin.test/api/hubs/hub-1/debug-bundle'),
        headers: {
          'authorization': 'Bearer staff-session',
          'origin': 'http://127.0.0.1:5173',
        },
      ),
    );

    expect(response.statusCode, 200);
    expect(_header(response, 'content-type'), 'application/gzip');
    expect(
      _header(response, 'content-disposition'),
      'attachment; filename="rhythm-debug-bundle-rpiz-test.tar.gz"',
    );
    expect(_header(response, 'x-rhythm-route'), 'remote');
    expect(_header(response, 'x-rhythm-base-url'), 'https://device.test:443');
    expect(
      _header(response, 'access-control-expose-headers'),
      contains('Content-Disposition'),
    );
    expect(await _bodyBytes(response), [0x1f, 0x8b, 0x08]);

    final logsResponse = await server.handler(
      Request(
        'GET',
        Uri.parse('http://admin.test/api/hubs/hub-1/logs'),
        headers: {
          'authorization': 'Bearer staff-session',
          'origin': 'http://127.0.0.1:5173',
        },
      ),
    );
    expect(logsResponse.statusCode, 200);
    final logs = jsonDecode(await logsResponse.readAsString()) as Map;
    expect(logs['route'], 'remote');
    expect(logs['baseUrl'], 'https://device.test:443');
    expect((logs['sources'] as List).single, {
      'id': 'rhythm-server.log',
      'fileName': 'rhythm-server.log',
      'bytes': 512,
      'modifiedAt': '2026-07-02T16:00:00.000Z',
    });

    final tailResponse = await server.handler(
      Request(
        'GET',
        Uri.parse(
          'http://admin.test/api/hubs/hub-1/logs/rhythm-server.log/tail?lines=120',
        ),
        headers: {
          'authorization': 'Bearer staff-session',
          'origin': 'http://127.0.0.1:5173',
        },
      ),
    );
    expect(tailResponse.statusCode, 200);
    final tail = jsonDecode(await tailResponse.readAsString()) as Map;
    expect(tail['route'], 'remote');
    expect(tail['requestedLines'], 120);
    expect(tail['returnedLines'], 2);
    expect((tail['lines'] as List).last['text'], 'cloudflared connected');

    final statusResponse = await server.handler(
      Request(
        'GET',
        Uri.parse('http://admin.test/api/hubs/hub-1/status'),
        headers: {
          'authorization': 'Bearer staff-session',
          'origin': 'http://127.0.0.1:5173',
        },
      ),
    );
    expect(statusResponse.statusCode, 200);
    final status = jsonDecode(await statusResponse.readAsString()) as Map;
    expect(status['route'], 'remote');
    expect(status['health'], {'status': 'healthy'});
    expect(status['state']['serverVersion'], '0.4.200-beta');
    expect(status['state']['platformContext'], 'rpiz');
    expect(status['state']['inventory']['lights'], 1);
    expect(status['remoteAccess']['connector_healthy'], isTrue);
    expect(status['auth']['via_remote_access'], isTrue);
    expect(status['ota']['state'], 'idle');
    expect(status['errors'], isEmpty);

    final otaCheckResponse = await server.handler(
      Request(
        'POST',
        Uri.parse('http://admin.test/api/hubs/hub-1/ota/check'),
        headers: {
          'authorization': 'Bearer staff-session',
          'origin': 'http://127.0.0.1:5173',
        },
      ),
    );
    expect(otaCheckResponse.statusCode, 200);
    final otaCheck = jsonDecode(await otaCheckResponse.readAsString()) as Map;
    expect(otaCheck['route'], 'remote');
    expect(otaCheck['baseUrl'], 'https://device.test:443');
    expect(otaCheck['action'], 'check');
    expect(otaCheck['tokenAvailable'], isTrue);
    expect(otaCheck['result']['update_available'], isTrue);
    expect(otaCheck['result']['latest_version'], '0.6.205-beta');

    final otaUpdateResponse = await server.handler(
      Request(
        'POST',
        Uri.parse('http://admin.test/api/hubs/hub-1/ota/update'),
        headers: {
          'authorization': 'Bearer staff-session',
          'origin': 'http://127.0.0.1:5173',
        },
      ),
    );
    expect(otaUpdateResponse.statusCode, 200);
    final otaUpdate = jsonDecode(await otaUpdateResponse.readAsString()) as Map;
    expect(otaUpdate['route'], 'remote');
    expect(otaUpdate['baseUrl'], 'https://device.test:443');
    expect(otaUpdate['action'], 'update');
    expect(otaUpdate['tokenAvailable'], isTrue);
    expect(otaUpdate['result']['message'], 'Update started');

    final readyResponse = await server.handler(
      Request('GET', Uri.parse('http://admin.test/ready')),
    );
    expect(readyResponse.statusCode, 200);
    final ready = jsonDecode(await readyResponse.readAsString()) as Map;
    expect(ready['remoteDebugReady'], isTrue);
    expect(ready['missing'], isEmpty);
    expect(deviceRequests, hasLength(10));
  });
}

http.Response _supabaseResponse(http.BaseRequest request) {
  if (request.url.path == '/auth/v1/user') {
    expect(request.headers['apikey'], 'anon-key');
    expect(request.headers['Authorization'], 'Bearer staff-session');
    return _jsonResponse({
      'id': 'staff-user',
      'email': 'staff@example.com',
    });
  }

  if (request.url.path == '/rest/v1/rhythm_staff') {
    expect(request.headers['apikey'], 'anon-key');
    expect(request.headers['Authorization'], 'Bearer staff-session');
    return _jsonResponse([
      {'role': 'admin', 'enabled': true},
    ]);
  }

  if (request.url.path == '/rest/v1/hubs') {
    expect(request.headers['apikey'], 'service-role-key');
    expect(request.headers['Authorization'], 'Bearer service-role-key');
    return _jsonResponse([
      {
        'id': 'hub-1',
        'home_id': 'home-1',
        'type': 'server',
        'name': 'Kitchen Light Box',
        'endpoint': {'host': 'local.test', 'port': 80, 'useSsl': false},
        'remote_endpoint': {
          'host': 'device.test',
          'port': 443,
          'useSsl': true,
        },
        'enabled': true,
        'token': 'legacy-token',
        'encrypted_token': null,
        'server_instance_id': 'srv-test',
      },
    ]);
  }

  return http.Response('not found', 404);
}

http.Response _jsonResponse(Object body) {
  return http.Response(
    jsonEncode(body),
    200,
    headers: {'content-type': 'application/json'},
  );
}

Future<List<int>> _bodyBytes(Response response) {
  return response.read().expand((chunk) => chunk).toList();
}

String? _header(Response response, String name) {
  return response.headers[name] ?? response.headers[name.toLowerCase()];
}

class _HandlerClient extends http.BaseClient {
  _HandlerClient(this._handler);

  final FutureOr<http.Response> Function(http.BaseRequest request) _handler;

  @override
  Future<http.StreamedResponse> send(http.BaseRequest request) async {
    final response = await _handler(request);
    return http.StreamedResponse(
      Stream.value(response.bodyBytes),
      response.statusCode,
      request: request,
      headers: response.headers,
      reasonPhrase: response.reasonPhrase,
    );
  }
}
