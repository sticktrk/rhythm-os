import 'dart:async';
import 'dart:convert';

import 'package:http/http.dart' as http;
import 'package:rhythm_admin_api/src/config.dart';
import 'package:rhythm_admin_api/src/device_probe_service.dart';
import 'package:rhythm_admin_api/src/fleet_report_service.dart';
import 'package:rhythm_admin_api/src/server.dart';
import 'package:rhythm_admin_api/src/supabase_rest_client.dart';
import 'package:rhythm_admin_api/src/support_access_service.dart';
import 'package:rhythm_admin_api/src/support_service.dart';
import 'package:shelf/shelf.dart';
import 'package:test/test.dart';

void main() {
  test('support snapshot associates customer emails with homes', () async {
    final client = _HandlerClient((request) async {
      if (request.url.host == 'supabase.test') {
        return _supabaseResponse(request);
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
    final support = SupportService(supabase: supabase);
    final probes = DeviceProbeService(
      supabase: supabase,
      supportAccess: supportAccess,
      httpClient: client,
    );
    final server = AdminApiServer(
      config: config,
      supabase: supabase,
      support: support,
      probes: probes,
      fleetReports: FleetReportService(
        supabase: supabase,
        support: support,
        probes: probes,
      ),
    );

    final response = await server.handler(
      Request(
        'GET',
        Uri.parse('http://admin.test/api/support/snapshot'),
        headers: {
          'authorization': 'Bearer staff-session',
          'origin': 'http://127.0.0.1:5173',
        },
      ),
    );

    expect(response.statusCode, 200);
    final body = jsonDecode(await response.readAsString()) as Map;
    expect(body['totals'], {
      'customers': 1,
      'homes': 1,
      'hubs': 1,
    });

    final customer = (body['customers'] as List).single as Map;
    expect(customer['ownerId'], 'owner-1');
    expect(customer['customerLabel'], 'Ada Homeowner');
    expect(customer['customerEmail'], 'ada@example.com');
    expect(customer['customerName'], 'Ada Homeowner');
    expect(customer['secondaryLabel'], 'ada@example.com');

    final homeEntry = (customer['homes'] as List).single as Map;
    expect(homeEntry['home']['name'], 'Ada Home');
    expect((homeEntry['hubs'] as List).single['name'], 'Kitchen Light Box');
  });

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
        if (request is http.Request && request.body.isNotEmpty) {
          final body = jsonDecode(request.body) as Map;
          expect(
            body['upload_url'],
            startsWith(
              'https://supabase.test/storage/v1/object/upload/sign/'
              'support-debug-bundles/staff-user/',
            ),
          );
          expect(body['upload_url'], endsWith('?token=signed-upload-token'));
          return _jsonResponse({
            'uploaded': true,
            'file_name': 'rhythm-debug-bundle-rpiz-direct.tar.gz',
            'size_bytes': 4321,
          });
        }
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

      if (request.url.host == 'device.test' &&
          request.url.path == '/api/settings') {
        deviceRequests.add(request);
        expect(request.method, 'PUT');
        expect(request.url.queryParameters['source'], 'admin');
        expect(request.headers['Authorization'], 'Bearer legacy-token');
        expect(request.headers['content-type'], contains('application/json'));
        expect(
          jsonDecode((request as http.Request).body),
          {'auto_update': false},
        );
        return _jsonResponse({
          'auto_update': false,
          'power_save': false,
        });
      }

      if (request.url.host == 'device.test' &&
          request.url.path == '/api/future-settings') {
        deviceRequests.add(request);
        expect(request.method, 'PATCH');
        expect(request.headers['Authorization'], 'Bearer legacy-token');
        expect(
          jsonDecode((request as http.Request).body),
          {
            'zones': ['kitchen', 'den']
          },
        );
        return _jsonResponse([
          {'id': 'kitchen', 'enabled': true},
          {'id': 'den', 'enabled': true},
        ]);
      }

      if (request.url.host == 'device.test' &&
          request.url.path == '/api/future-settings/kitchen') {
        deviceRequests.add(request);
        expect(request.method, 'DELETE');
        expect(request.headers['Authorization'], 'Bearer legacy-token');
        return http.Response('', 204);
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
    final support = SupportService(supabase: supabase);
    final probes = DeviceProbeService(
      supabase: supabase,
      supportAccess: supportAccess,
      httpClient: client,
    );
    final server = AdminApiServer(
      config: config,
      supabase: supabase,
      support: support,
      probes: probes,
      fleetReports: FleetReportService(
        supabase: supabase,
        support: support,
        probes: probes,
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
      'attachment; filename="rhythm-debug-bundle-rpiz-direct.tar.gz"',
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

    final fleetResponse = await server.handler(
      Request(
        'GET',
        Uri.parse('http://admin.test/api/fleet/hubs'),
        headers: {'authorization': 'Bearer staff-session'},
      ),
    );
    expect(fleetResponse.statusCode, 200);
    final fleet = jsonDecode(await fleetResponse.readAsString()) as Map;
    expect(fleet['total'], 2);
    expect(
      (fleet['hubs'] as List).map((hub) => hub['id']),
      containsAll(['hub-1', 'hub-orphan']),
    );

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

    final fleetReportResponse = await server.handler(
      Request(
        'POST',
        Uri.parse('http://admin.test/api/hubs/hub-1/fleet-report'),
        headers: {
          'authorization': 'Bearer staff-session',
          'content-type': 'application/json',
        },
        body: jsonEncode({
          'title': 'Matter worker repeatedly failed',
          'findingIds': ['20260712T120000Z:0001'],
          'severity': 'error',
          'scanRunId': '20260712T120000Z',
          'detectedAt': '2026-07-12T12:00:00Z',
          'occurrences': 4,
          'affectedHubCount': 1,
          'journeyStage': 'integration commissioning',
          'serverVersion': '0.4.200-beta',
          'platformContext': 'rpiz',
          'samples': ['ERROR Matter worker failed token=actual-secret'],
        }),
      ),
    );
    expect(fleetReportResponse.statusCode, 200);
    final fleetReport =
        jsonDecode(await fleetReportResponse.readAsString()) as Map;
    expect(fleetReport['submissionId'], isNotEmpty);
    expect(fleetReport['referenceCode'], 'DBG-FLEET01');
    expect(fleetReport['issue_created'], isTrue);
    expect(fleetReport['issue_number'], 177);
    final bundleRequests = deviceRequests
        .where((request) => request.url.path == '/api/diag/debug-bundle')
        .cast<http.Request>()
        .toList();
    expect(bundleRequests, hasLength(2));
    expect(bundleRequests.first.body, contains('"upload_url"'));
    expect(bundleRequests.last.body, contains('"upload_url"'));

    final adminProxyResponse = await server.handler(
      Request(
        'POST',
        Uri.parse('http://admin.test/api/hubs/hub-1/device-admin/proxy'),
        headers: {
          'authorization': 'Bearer staff-session',
          'origin': 'http://127.0.0.1:5173',
          'content-type': 'application/json',
        },
        body: jsonEncode({
          'method': 'PUT',
          'path': '/api/settings',
          'query': {'source': 'admin'},
          'body': {'auto_update': false},
        }),
      ),
    );
    expect(adminProxyResponse.statusCode, 200);
    final adminProxy =
        jsonDecode(await adminProxyResponse.readAsString()) as Map;
    expect(adminProxy['route'], 'remote');
    expect(adminProxy['baseUrl'], 'https://device.test:443');
    expect(adminProxy['method'], 'PUT');
    expect(adminProxy['path'], 'api/settings');
    expect(adminProxy['statusCode'], 200);
    expect(adminProxy['tokenAvailable'], isTrue);
    expect(adminProxy['body'], {
      'auto_update': false,
      'power_save': false,
    });

    final patchProxyResponse = await server.handler(
      Request(
        'POST',
        Uri.parse('http://admin.test/api/hubs/hub-1/device-admin/proxy'),
        headers: {
          'authorization': 'Bearer staff-session',
          'origin': 'http://127.0.0.1:5173',
          'content-type': 'application/json',
        },
        body: jsonEncode({
          'method': 'PATCH',
          'path': 'api/future-settings',
          'body': {
            'zones': ['kitchen', 'den'],
          },
        }),
      ),
    );
    expect(patchProxyResponse.statusCode, 200);
    final patchProxy =
        jsonDecode(await patchProxyResponse.readAsString()) as Map;
    expect(patchProxy['method'], 'PATCH');
    expect(patchProxy['path'], 'api/future-settings');
    expect(patchProxy['body'], [
      {'id': 'kitchen', 'enabled': true},
      {'id': 'den', 'enabled': true},
    ]);

    final deleteProxyResponse = await server.handler(
      Request(
        'POST',
        Uri.parse('http://admin.test/api/hubs/hub-1/device-admin/proxy'),
        headers: {
          'authorization': 'Bearer staff-session',
          'origin': 'http://127.0.0.1:5173',
          'content-type': 'application/json',
        },
        body: jsonEncode({
          'method': 'DELETE',
          'path': 'api/future-settings/kitchen',
        }),
      ),
    );
    expect(deleteProxyResponse.statusCode, 200);
    final deleteProxy =
        jsonDecode(await deleteProxyResponse.readAsString()) as Map;
    expect(deleteProxy['method'], 'DELETE');
    expect(deleteProxy['path'], 'api/future-settings/kitchen');
    expect(deleteProxy['statusCode'], 204);
    expect(deleteProxy['body'], isNull);

    final invalidProxyResponse = await server.handler(
      Request(
        'POST',
        Uri.parse('http://admin.test/api/hubs/hub-1/device-admin/proxy'),
        headers: {
          'authorization': 'Bearer staff-session',
          'origin': 'http://127.0.0.1:5173',
          'content-type': 'application/json',
        },
        body: jsonEncode({
          'method': 'GET',
          'path': 'https://device.test/api/settings',
        }),
      ),
    );
    expect(invalidProxyResponse.statusCode, 400);
    final invalidProxy =
        jsonDecode(await invalidProxyResponse.readAsString()) as Map;
    expect(
      invalidProxy['error'],
      'Device admin path must be a relative device API path.',
    );

    final readyResponse = await server.handler(
      Request('GET', Uri.parse('http://admin.test/ready')),
    );
    expect(readyResponse.statusCode, 200);
    final ready = jsonDecode(await readyResponse.readAsString()) as Map;
    expect(ready['remoteDebugReady'], isTrue);
    expect(ready['missing'], isEmpty);
    expect(deviceRequests, hasLength(14));
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

  if (request.url.path == '/storage/v1/object/support-debug-bundles') {
    expect(request.method, 'DELETE');
    expect(request.headers['apikey'], 'service-role-key');
    expect(request.headers['Authorization'], 'Bearer service-role-key');
    final body = jsonDecode((request as http.Request).body) as Map;
    expect((body['prefixes'] as List).single, contains('/fleet-review/'));
    return _jsonResponse([]);
  }

  if (request.url.path.startsWith(
    '/storage/v1/object/upload/sign/support-debug-bundles/staff-user/',
  )) {
    expect(request.method, 'POST');
    expect(request.headers['apikey'], 'service-role-key');
    expect(request.headers['Authorization'], 'Bearer service-role-key');
    expect((request as http.Request).body, '{}');
    return _jsonResponse({
      'url': '${request.url.path.replaceFirst('/storage/v1', '')}'
          '?token=signed-upload-token',
    });
  }

  if (request.url.path.startsWith(
    '/storage/v1/object/support-debug-bundles/staff-user/',
  )) {
    if (request.method == 'GET') {
      expect(request.headers['apikey'], 'service-role-key');
      expect(request.headers['Authorization'], 'Bearer service-role-key');
      return http.Response.bytes(
        [0x1f, 0x8b, 0x08],
        200,
        headers: {'content-type': 'application/gzip'},
      );
    }
    return http.Response('legacy upload should not be used', 500);
  }

  if (request.url.path == '/rest/v1/support_debug_bundle_submissions') {
    expect(request.method, 'POST');
    expect(request.headers['apikey'], 'service-role-key');
    final body = jsonDecode((request as http.Request).body) as Map;
    expect(body['user_id'], 'staff-user');
    expect(body['app_platform'], 'admin-fleet');
    expect(body['bundle_file_name'], 'rhythm-debug-bundle-rpiz-direct.tar.gz');
    expect(body['bundle_size_bytes'], 4321);
    expect(
        body['summary'], contains('Run-local findings: 20260712T120000Z:0001'));
    expect(body['summary'], isNot(contains('actual-secret')));
    expect(body['summary'], contains('credential=<redacted>'));
    return _jsonResponse([
      {
        ...body,
        'reference_code': 'DBG-FLEET01',
      }
    ]);
  }

  if (request.url.path == '/functions/v1/report-bug') {
    expect(request.method, 'POST');
    expect(request.headers['apikey'], 'anon-key');
    expect(request.headers['Authorization'], 'Bearer staff-session');
    return _jsonResponse({
      'issue_created': true,
      'status': 'reported',
      'issue_url': 'https://github.com/sticktrk/cross/issues/177',
      'issue_number': 177,
    });
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
      {
        'id': 'hub-orphan',
        'home_id': 'missing-home',
        'type': 'server',
        'name': 'Orphan Light Box',
        'endpoint': {'host': 'orphan.test', 'port': 80, 'useSsl': false},
        'enabled': true,
        'token': 'legacy-token',
        'encrypted_token': null,
        'server_instance_id': 'srv-orphan',
      },
    ]);
  }

  if (request.url.path == '/rest/v1/homes') {
    expect(request.headers['apikey'], 'anon-key');
    expect(request.headers['Authorization'], 'Bearer staff-session');
    return _jsonResponse([
      {
        'id': 'home-1',
        'name': 'Ada Home',
        'owner_id': 'owner-1',
        'member_ids': ['owner-1'],
        'location': {'cityName': 'Raleigh'},
        'timezone': 'America/New_York',
        'created_at': '2026-06-01T12:00:00Z',
        'updated_at': '2026-07-01T12:00:00Z',
      },
    ]);
  }

  if (request.url.path == '/rest/v1/rhythm_support_customers') {
    expect(request.headers['apikey'], 'anon-key');
    expect(request.headers['Authorization'], 'Bearer staff-session');
    return _jsonResponse([
      {
        'user_id': 'owner-1',
        'email': 'ada@example.com',
        'name': 'Ada Homeowner',
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
