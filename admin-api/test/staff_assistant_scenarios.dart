part of 'admin_api_server_test.dart';

void _staffAssistantScenarios() {
  group('staff MCP', () {
    late _StaffFixture service;
    setUp(() async {
      service = await _StaffFixture.create();
    });

    test('rejects untrusted browser origins and unsupported transport versions',
        () async {
      final uri = Uri.parse('http://admin.test/api/assistant/mcp');
      final handler = service.server.handler;
      expect((await handler(Request('GET', uri))).statusCode, 401);
      expect(
          (await handler(Request('GET', uri,
                  headers: {'authorization': 'Bearer staff-session'})))
              .statusCode,
          405);
      expect(
          (await handler(Request('POST', uri,
                  headers: {'origin': 'https://untrusted.test'})))
              .statusCode,
          403);
      expect(
          (await handler(Request('POST', uri,
                  headers: {'mcp-protocol-version': '1900-01-01'})))
              .statusCode,
          400);
      expect(
          (await handler(Request('POST', uri,
                  headers: {'origin': 'http://127.0.0.1:5173'})))
              .statusCode,
          401);
    });
    test(
        'finds a home, reads status and log diagnoses, continues with scoped tools in one session',
        () async {
      final init = await service.rpc('initialize', {});
      expect(init['serverInfo']['name'], 'rhythm-staff');
      final templates = await service.rpc('resources/templates/list', {});
      expect(templates['resourceTemplates'], isEmpty);
      final found = await service.call('staff_find_homes', {'query': 'Ada'});
      final hub = found['homes'].single['hubs'].single['hub_ref'];
      final selected = {'hub_ref': hub};
      final listed =
          await service.call('staff_list_fleet', {'hub_id': 'hub-1'});
      expect(listed['hubs'].single['hub_ref'], hub);
      final probe = await service.call('staff_probe_hub', selected);
      expect(probe['identity_verified'], true);
      final status = await service.call('staff_read_hub_status', selected);
      expect(status['server_version'], '1.2.3');
      final logs = await service.call('staff_list_logs', selected);
      final tail = await service.call('staff_read_log',
          {...selected, 'source_ref': logs['sources'].single['source_ref']});
      expect(tail['diagnostics'], [
        {'category': 'network_offline', 'line': 10}
      ]);
      final catalog = await service.call('staff_select_hub', selected);
      expect(catalog['tools'].map((e) => e['id']),
          contains('get_topology_snapshot'));
      expect(catalog['tools'].map((e) => e['id']),
          isNot(contains('explain_room_output')));
      final topo = await service.call('staff_call_hub_tool', {
        ...selected,
        'operation_id': 'get_topology_snapshot',
        'arguments': {}
      });
      final room = topo['nodes'].first['id'];
      expect(room, startsWith('ref_'));
      final preview = await service.call('staff_call_hub_tool', {
        ...selected,
        'operation_id': 'plan_motion_target',
        'arguments': {
          'room_id': room,
          'sensor_id': room,
          'correlation_id': 'test-walk'
        }
      });
      expect(preview['scope']['after_targets'], [room]);
      await service.call('staff_apply_plan',
          {'plan_id': preview['plan_id'], 'confirmed': false},
          failure: true);
      expect(service.writes, 0);
      await service.call('staff_apply_plan',
          {...preview['apply_arguments'], 'confirmed': true});
      expect(service.writes, 1);
      await service.call('staff_apply_plan',
          {...preview['apply_arguments'], 'confirmed': true},
          failure: true);
      expect(service.writes, 1);
      expect(service.legacyRequests, 0);
    });

    test('checks role, actor, live identity and current grant at every call',
        () async {
      final hub =
          (await service.call('staff_list_fleet', {'hub_id': 'hub-1'}))['hubs']
              .single['hub_ref'];
      final selected = {'hub_ref': hub};
      final logs = await service.call('staff_list_logs', selected);
      final scoped = {
        ...selected,
        'source_ref': logs['sources'].single['source_ref']
      };
      final preview = await service.call('staff_plan_delete_hub', selected);
      service.actor = 'other';
      await service.call('staff_select_hub', selected, failure: true);
      await service.call('staff_apply_plan',
          {'plan_id': preview['plan_id'], 'confirmed': true},
          failure: true);
      service.actor = 'staff-user';
      service.role = 'support';
      final tools = await service.rpc('tools/list', {});
      expect(tools['tools'].map((e) => e['name']),
          isNot(contains('staff_apply_plan')));
      await service.call('staff_apply_plan',
          {'plan_id': preview['plan_id'], 'confirmed': true},
          failure: true);
      final catalog = await service.call('staff_select_hub', selected);
      expect(catalog['tools'].map((e) => e['id']),
          isNot(contains('adjust_room_brightness')));
      await service.call(
          'staff_control_hub',
          {
            ...selected,
            'operation_id': 'adjust_room_brightness',
            'arguments': {}
          },
          failure: true);
      await service.call('staff_call_hub_tool',
          {...selected, 'operation_id': 'apply_motion_target', 'arguments': {}},
          failure: true);
      await service.call('staff_call_hub_tool', {
        ...selected,
        'operation_id': 'plan_motion_target',
        'arguments': {
          'room_id': 'room',
          'sensor_id': 'sensor',
          'correlation_id': 'safe'
        }
      });
      service.liveHub = 'other-hub';
      await service.call('staff_select_hub', selected, failure: true);
      service.liveHub = 'srv-test';
      service.granted = false;
      final callsBefore = service.deviceCalls;
      for (final op in [
        'staff_probe_hub',
        'staff_select_hub',
        'staff_read_hub_status',
        'staff_list_logs'
      ]) {
        final error = await service.call(op, selected, failure: true);
        expect(error['content'].single['text'],
            contains('current customer support grant'));
      }
      await service.call('staff_read_log', scoped, failure: true);
      expect(service.deviceCalls, callsBefore);
      expect(service.legacyRequests, 0);
      expect(service.writes, 0);
      expect(service.deleted, 0);
    });

    test('submits a reviewed fleet report once with a private bundle transfer',
        () async {
      final hub =
          (await service.call('staff_list_fleet', {'hub_id': 'hub-1'}))['hubs']
              .single['hub_ref'];
      final review = await service.call('staff_plan_fleet_report', {
        'hub_ref': hub,
        'category': 'network_offline',
        'severity': 'warning'
      });
      expect(review['scope']['attachments'], ['private diagnostic bundle']);
      expect(service.uploads, 0);
      final args = {
        ...review['apply_arguments'] as Map<String, dynamic>,
        'confirmed': true
      };
      final result = await service.call('staff_apply_plan', args);
      expect(result['status'], 'submitted');
      expect(result['issue_created'], true);
      expect(result['support_reference'], startsWith('ref_'));
      expect(service.uploads, 1);
      await service.call('staff_apply_plan', args, failure: true);
      expect(service.uploads, 1);
    });

    test('requires an unchanged cloud record for the confirmed delete',
        () async {
      final hub =
          (await service.call('staff_list_fleet', {'hub_id': 'hub-1'}))['hubs']
              .single['hub_ref'];
      final preview =
          await service.call('staff_plan_delete_hub', {'hub_ref': hub});
      final args = {'plan_id': preview['plan_id'], 'confirmed': true};
      service.rowChanged = true;
      await service.call('staff_apply_plan', args, failure: true);
      expect(service.deleted, 0);
      service.rowChanged = false;
      final fresh =
          await service.call('staff_plan_delete_hub', {'hub_ref': hub});
      final res = await service.call(
          'staff_apply_plan', {'plan_id': fresh['plan_id'], 'confirmed': true});
      expect(res['status'], 'applied');
      expect(service.deleted, 1);
      await service.call(
          'staff_apply_plan', {'plan_id': fresh['plan_id'], 'confirmed': true},
          failure: true);
      expect(service.deleted, 1);
    });
  });
}

class _StaffFixture {
  late final AdminApiServer server;
  late final Map<String, dynamic> envelope;
  String role = 'admin', actor = 'staff-user', liveHub = 'srv-test';
  bool granted = true, rowChanged = false;
  int deviceCalls = 0, writes = 0, deleted = 0, legacyRequests = 0, uploads = 0;

  static Future<_StaffFixture> create() async {
    const aad = 'hub:hub-1:token:t1:scope:beta_admin';
    final f = _StaffFixture();
    final digest = await Sha256().hash(utf8.encode('support-key'));
    final encrypted = await AesGcm.with256bits().encrypt(
        utf8.encode('synthetic-support'),
        secretKey: SecretKey(digest.bytes),
        aad: utf8.encode(aad));
    f.envelope = {
      'version': 'support_access_token_v1',
      'algorithm': 'aes-gcm-256',
      'key_derivation': 'sha256-env-v1',
      'key_id': 'default',
      'aad': aad,
      'ciphertext': base64Url.encode(encrypted.cipherText),
      'nonce': base64Url.encode(encrypted.nonce),
      'mac': base64Url.encode(encrypted.mac.bytes)
    };
    f.server = _testServer(client: _HandlerClient(f.handle));
    return f;
  }

  Future<Map<String, dynamic>> rpc(
      String method, Map<String, dynamic> params) async {
    final response = await server.handler(Request(
        'POST', Uri.parse('http://admin.test/api/assistant/mcp'),
        headers: {
          'authorization': 'Bearer staff-session',
          'content-type': 'application/json'
        },
        body: jsonEncode(
            {'jsonrpc': '2.0', 'id': 1, 'method': method, 'params': params})));
    expect(response.statusCode, 200);
    final serialized = await response.readAsString();
    for (final private in [
      'legacy-token',
      'service-role-key',
      'synthetic-support',
      'synthetic-short-session',
      'signed-upload',
      'device.test',
      'private_canary',
      'native-plan',
      'home-1',
      'ada@example.com',
      'Ada Home',
      'Raleigh',
      'Kitchen Light Box'
    ]) {
      expect(serialized, isNot(contains(private)),
          reason: 'Privacy boundary for $method');
    }
    return (jsonDecode(serialized) as Map)['result'] as Map<String, dynamic>;
  }

  Future<Map<String, dynamic>> call(String name, Map<String, dynamic> args,
      {bool failure = false}) async {
    final result = await rpc('tools/call', {'name': name, 'arguments': args});
    expect(result['isError'] == true, failure,
        reason: '$name: ${result['content']}');
    return failure
        ? result
        : result['structuredContent'] as Map<String, dynamic>;
  }

  Future<http.Response> handle(http.BaseRequest request) async {
    final path = request.url.path;
    if (path == '/auth/v1/user') {
      return _jsonResponse({'id': actor, 'email': 'ada@example.com'});
    }
    final auth = _staffAuthResponse(request, role: role);
    if (auth != null) return auth;
    if (path == '/rest/v1/hub_support_access_grants') {
      expect(request.url.queryParameters['hub_id'], 'eq.hub-1');
      expect(request.url.queryParameters['revoked_at'], 'is.null');
      return _jsonResponse([
        if (granted)
          {
            'id': 'grant1',
            'hub_id': 'hub-1',
            'token_id': 't1',
            'scope': 'beta_admin',
            'encrypted_token': envelope
          }
      ]);
    }
    if (path == '/rest/v1/hubs' && request.method == 'DELETE') {
      expect(request.url.queryParameters['id'], 'eq.hub-1');
      expect(request.url.queryParameters['server_instance_id'], 'eq.srv-test');
      expect(request.url.queryParameters['home_id'], 'eq.home-1');
      expect(request.url.queryParameters['updated_at'], 'is.null');
      if (rowChanged) return _jsonResponse([]);
      deleted++;
      return _jsonResponse([
        {'id': 'hub-1', 'home_id': 'home-1', 'name': 'private_canary'}
      ]);
    }
    if (path == '/rest/v1/support_debug_bundle_submissions') {
      final body = jsonDecode((request as http.Request).body) as Map;
      expect(body['summary'], contains('network_offline'));
      expect(body['bundle_storage_path'], contains('staff-user/fleet/'));
      return _jsonResponse([
        {...body, 'reference_code': 'private_canary'}
      ]);
    }
    if (request.url.host == 'supabase.test') return _supabaseResponse(request);
    if (path == '/api/auth/support-session-token') {
      expect(request.headers['Authorization'], 'Bearer synthetic-support');
      return _jsonResponse({'token': 'synthetic-short-session'});
    }
    deviceCalls++;
    if (request.headers['Authorization'] == 'Bearer legacy-token') {
      legacyRequests++;
    }
    expect(request.headers['Authorization'], 'Bearer synthetic-short-session');
    if (path == '/api/diag/debug-bundle') {
      final body = jsonDecode((request as http.Request).body) as Map;
      expect(body['upload_url'], endsWith('?token=signed-upload-token'));
      uploads++;
      return _jsonResponse({
        'uploaded': true,
        'file_name': 'private_canary.tar.gz',
        'size_bytes': 400
      });
    }
    if (path == '/api/state') {
      return _jsonResponse(
          {'server_instance_id': liveHub, 'version': '1.2.3', 'nodes': []});
    }
    if (path == '/api/diag/logs') {
      return _jsonResponse({
        'sources': [
          {'id': 'private_canary', 'bytes': 20}
        ]
      });
    }
    if (path == '/api/diag/logs/private_canary/tail') {
      return _jsonResponse({
        'tail': {
          'lines': [
            {
              'line_number': 10,
              'text':
                  'network unreachable https://device.test/?signed-upload=private_canary'
            },
            {'line_number': 11, 'text': 'private_canary motion activity'}
          ]
        }
      });
    }
    if (path == '/api/assistant/contract') {
      final operations = (jsonDecode(staffHubCatalogJson) as List)
          .cast<Map<String, dynamic>>();
      return _jsonResponse({
        'operations': [
          for (final op in operations.where((op) => [
                'get_topology_snapshot',
                'plan_motion_target',
                'apply_motion_target',
                'adjust_room_brightness',
                'explain_room_output'
              ].contains(op['id'])))
            {
              ...op,
              'location': 'hub',
              'invocation_path': '/api/assistant/operations/${op['id']}',
              'description': 'private_canary',
              if (op['id'] == 'explain_room_output')
                'input_schema': {
                  'type': 'object',
                  'properties': {
                    'private_canary': {
                      'enum': ['private_canary']
                    }
                  },
                  'required': ['private_canary']
                }
            }
        ]
      });
    }
    if (path.endsWith('/get_topology_snapshot')) {
      return _jsonResponse({
        'nodes': [
          {
            'id': 'private_canary',
            'kind': 'room',
            'name': 'Ada Home',
            'private_canary': 'private_canary'
          }
        ]
      });
    }
    if (path.endsWith('/plan_motion_target')) {
      final args = jsonDecode((request as http.Request).body);
      expect(args['room_id'], anyOf('private_canary', 'room'));
      return _jsonResponse({
        'server_instance_id': 'srv-test',
        'operation': 'apply_motion_target',
        'status': 'ready',
        'before_targets': [],
        'after_targets': ['private_canary'],
        'apply_arguments': {
          'plan_id': 'native-plan',
          'server_instance_id': 'srv-test'
        }
      });
    }
    if (path.endsWith('/apply_motion_target')) {
      final args = jsonDecode((request as http.Request).body);
      expect(args, {
        'plan_id': 'native-plan',
        'server_instance_id': 'srv-test',
        'confirmed': true
      });
      writes++;
      return _jsonResponse(
          {'status': 'applied', 'canonical_readback_verified': true});
    }
    if (path.endsWith('/read_hub_status')) {
      return _jsonResponse({
        'server_version': '1.2.3',
        'status': 'private_canary',
        'secret': 'synthetic-support',
        'signed_url': 'https://device.test/?signed-upload'
      });
    }
    throw StateError('Unexpected ${request.method} $path');
  }
}
