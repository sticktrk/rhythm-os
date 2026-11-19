import 'dart:convert';
import 'dart:io';
import 'dart:math';

import 'package:shelf/shelf.dart';

import 'assistant_read_operations.dart';
import 'device_json.dart';
import 'staff_hub_catalog.dart';
import 'device_probe_service.dart';
import 'fleet_report_service.dart';
import 'models.dart';
import 'support_service.dart';

typedef _Json = Map<String, dynamic>;
typedef _Reference = ({
  String actor,
  String hub,
  String kind,
  Object value,
  DateTime expires
});

const _str = {'type': 'string'};
const _hub = {'hub_ref': _str};
const _plan = {
  'plan_id': _str,
  'confirmed': {'type': 'boolean', 'const': true}
};
const _version = '2025-06-18';
const _categories = [
  'network_offline',
  'network_restored',
  'authentication',
  'timeout',
  'restart',
  'integration_unavailable',
  'device_error'
];

class _Tool {
  const _Tool(this.id, this.description, this.properties, this.required,
      {this.tier = 'read'});
  final String id, description;
  final _Json properties;
  final List<String> required;
  final String tier;
  bool get writes => tier != 'read';

  _Json get descriptor => {
        'id': id,
        'location': 'staff_cloud',
        'method': 'POST',
        'path': '/api/assistant/mcp',
        'effect': writes ? tier : 'read_only',
        'physical_confirmation_required': false,
        'tier': tier,
        'description': description,
        'confirmation':
            tier == 'persistent_change' ? 'explicit_user_confirmation' : 'none',
        'freshness_precondition': switch (tier) {
          'persistent_change' => 'actor_bound_review_and_live_preconditions',
          'temporary_light_output' => 'live_hub_identity_and_support_grant',
          _ => 'current_staff_session',
        },
        'verification': 'server_receipt',
        'input_schema': {
          'type': 'object',
          'properties': properties,
          'required': required,
          'additionalProperties': false,
        },
        'result_schema': {'type': 'object'},
      };
  _Json get mcp => {
        'name': id,
        'description': description,
        'inputSchema': descriptor['input_schema'],
        'annotations': {'readOnlyHint': !writes},
        '_meta': {'rhythm/descriptor': descriptor}
      };

  void validate(_Json args) {
    if (args.keys.any((key) => !properties.containsKey(key)) ||
        required.any((key) => !args.containsKey(key))) {
      throw _badInput;
    }
    for (final field in args.entries) {
      final schema = properties[field.key] as Map, value = field.value;
      final valid = switch (schema['type']) {
        'string' => value is String && value.isNotEmpty && value.length <= 256,
        'boolean' => value is bool,
        'integer' => value is int,
        'object' => value is Map<String, dynamic>,
        _ => false,
      };
      if (!valid ||
          schema['enum'] is List && !(schema['enum'] as List).contains(value)) {
        throw _badInput;
      }
    }
  }
}

const _tools = [
  _Tool(
      'staff_find_homes',
      'Search the support directory for a home, hub or account. Matching happens in the staff service. Return private selection references.',
      {
        'query': _str,
        'offset': {'type': 'integer', 'minimum': 0}
      },
      []),
  _Tool(
      'staff_list_fleet',
      'List enabled server hubs, optionally resolving the record selected in the console.',
      {
        'hub_id': _str,
        'offset': {'type': 'integer', 'minimum': 0}
      },
      []),
  _Tool(
      'staff_probe_hub',
      'Check whether the selected hub answers with the recorded identity under the current support grant.',
      _hub,
      ['hub_ref']),
  _Tool(
      'staff_read_hub_status',
      'Read bounded status and inventory counts from the selected hub.',
      _hub,
      ['hub_ref']),
  _Tool(
      'staff_list_logs',
      'List available diagnostic log sources as short-lived references.',
      _hub,
      ['hub_ref']),
  _Tool(
      'staff_read_log',
      'Read recent diagnostic categories and counts from a selected source. Raw lines stay on the staff service.',
      {
        ..._hub,
        'source_ref': _str,
        'lines': {'type': 'integer', 'minimum': 1, 'maximum': 500}
      },
      [
        'hub_ref',
        'source_ref'
      ]),
  _Tool(
      'staff_select_hub',
      'Discover the tools on this hub under the customer’s current grant. Continue with staff_call_hub_tool in this conversation.',
      _hub,
      ['hub_ref']),
  _Tool(
      'staff_call_hub_tool',
      'Invoke an advertised read or preview operation, including plan_hub_update. Pass returned references as arguments. Use staff_apply_plan after a reviewed proposal.',
      {
        ..._hub,
        'operation_id': _str,
        'arguments': {'type': 'object'}
      },
      [
        'hub_ref',
        'operation_id',
        'arguments'
      ]),
  _Tool(
      'staff_control_hub',
      'Run one advertised temporary light output operation on the selected hub.',
      {
        ..._hub,
        'operation_id': _str,
        'arguments': {'type': 'object'}
      },
      ['hub_ref', 'operation_id', 'arguments'],
      tier: 'temporary_light_output'),
  _Tool(
      'staff_plan_fleet_report',
      'Review a support submission containing a diagnostic bundle from this hub and a finding category.',
      {
        ..._hub,
        'category': {'type': 'string', 'enum': _categories},
        'severity': {
          'type': 'string',
          'enum': ['warning', 'error', 'critical']
        }
      },
      [
        'hub_ref',
        'category',
        'severity'
      ]),
  _Tool(
      'staff_plan_delete_hub',
      'Review deleting the selected cloud hub record and associated rows. The action keeps the home and sends no device command.',
      _hub,
      ['hub_ref']),
  _Tool(
      'staff_apply_plan',
      'Apply one reviewed staff proposal after explicit human confirmation. This requires the admin role.',
      _plan,
      ['plan_id', 'confirmed'],
      tier: 'persistent_change'),
];
const _badInput =
    AdminApiException(400, 'Use the advertised operation arguments.');
const _stale = AdminApiException(409,
    'The selection or plan changed or expired. Read the current state and preview again.');
const _denied = AdminApiException(403,
    'This action requires an enabled admin and any customer grant for the selected hub.');

class _Proposal {
  _Proposal(this.hub, this.operation, this.arguments);
  final SupportHubDto hub;
  final String operation;
  final _Json arguments;
}

/// Staff MCP shares the staff session and grants with the console. Selection
/// handles and reviewed proposals are actor-bound and expire on restart.
final _trustedHubOperations = {
  for (final descriptor
      in (jsonDecode(staffHubCatalogJson) as List).cast<_Json>())
    descriptor['id'] as String: descriptor,
};

class StaffAssistant {
  static const supportedVersions = [_version, '2025-03-26', '2025-11-25'];
  StaffAssistant(this.support, this.probes, this.reports);
  final SupportService support;
  final DeviceProbeService probes;
  final FleetReportService reports;
  final _refs = <String, _Reference>{};
  final _rng = Random.secure();

  static _Json _rpcError(Object? id, int code, String message) => {
        'jsonrpc': '2.0',
        'id': id,
        'error': {'code': code, 'message': message}
      };

  Future<Response> handle(
      AdminSession session, Map<String, dynamic> rpc) async {
    final id = rpc['id'], method = rpc['method'];
    Response json(_Json body) => Response.ok(jsonEncode(body), headers: {
          'Content-Type': 'application/json',
          'Cache-Control': 'no-store'
        });
    if (rpc['jsonrpc'] != '2.0' ||
        method is! String ||
        (rpc.containsKey('id') && id is! String && id is! int)) {
      return json(_rpcError(null, -32600, 'Invalid request'));
    }
    if (!rpc.containsKey('id')) return Response(202);
    final params = rpc['params'] ?? <String, dynamic>{};
    if (params is! Map<String, dynamic>) {
      return json(_rpcError(id, -32602, 'Invalid parameters'));
    }
    late _Json result;
    switch (method) {
      case 'initialize':
        final requested = params['protocolVersion'];
        result = {
          'protocolVersion':
              supportedVersions.contains(requested) ? requested : _version,
          'serverInfo': {'name': 'rhythm-staff', 'version': '1'},
          'capabilities': {'tools': {}, 'resources': {}},
          'instructions':
              'Select the intended hub. Confirm with the person before applying a proposal. Read back after an uncertain response.'
        };
      case 'ping':
        result = {};
      case 'tools/list':
        result = {
          'tools': [
            for (final t in _tools)
              if (!t.writes || session.staff.isAdmin) t.mcp
          ]
        };
      case 'resources/list':
        result = {
          'resources': [
            {
              'uri': 'rhythm://staff/contract',
              'name': 'Staff operation contract',
              'mimeType': 'application/json'
            }
          ]
        };
      case 'resources/read':
        if (params['uri'] != 'rhythm://staff/contract') {
          return json(_rpcError(id, -32002, 'Resource unavailable'));
        }
        result = {
          'contents': [
            {
              'uri': 'rhythm://staff/contract',
              'mimeType': 'application/json',
              'text': jsonEncode({
                'schema_version': 1,
                'operations': [
                  for (final t in _tools)
                    if (!t.writes || session.staff.isAdmin) t.descriptor
                ]
              })
            }
          ]
        };
      case 'tools/call':
        final tool = _tools.where((t) => t.id == params['name']).firstOrNull;
        final args = params['arguments'] ?? <String, dynamic>{};
        if (tool == null || args is! _Json) {
          return json(
              _rpcError(id, -32602, 'Unknown tool or invalid arguments'));
        }
        try {
          if (tool.writes && !session.staff.isAdmin) throw _denied;
          tool.validate(args);
          final body = await _call(session, tool.id, args);
          result = {
            'content': [
              {'type': 'text', 'text': jsonEncode(body)}
            ],
            'structuredContent': body
          };
        } on AdminApiException catch (error) {
          // HTTP failures may include base URLs, request bodies and vendor
          // diagnostics. Only bounded descriptions can leave this service.
          final message = switch (error.statusCode) {
            401 ||
            403 =>
              'Access refused. Verify staff permissions and the current customer support grant.',
            409 || 428 => _stale.message,
            400 => _badInput.message,
            404 => 'The selected resource is unavailable.',
            429 => 'Too many active selections. Try again after they expire.',
            _ => 'The operation could not be verified. Reread before retrying.',
          };
          result = {
            'isError': true,
            'content': [
              {'type': 'text', 'text': message}
            ]
          };
        } catch (_) {
          result = {
            'isError': true,
            'content': [
              {
                'type': 'text',
                'text':
                    'The operation could not be verified. Reread before retrying.'
              }
            ]
          };
        }
      default:
        return json(_rpcError(id, -32601, 'Method not found'));
    }
    return json({'jsonrpc': '2.0', 'id': id, 'result': result});
  }

  String _reference(AdminSession s, String hub, String kind, Object value) {
    final now = DateTime.now().toUtc();
    _refs.removeWhere((_, r) => !r.expires.isAfter(now));
    for (final entry in _refs.entries) {
      final r = entry.value;
      if (r.actor == s.user.id &&
          r.hub == hub &&
          r.kind == kind &&
          r.value == value) {
        return entry.key;
      }
    }
    if (_refs.length >= 8192 ||
        _refs.values.where((r) => r.actor == s.user.id).length >= 2048) {
      throw const AdminApiException(429, 'Too many active references.');
    }
    final id =
        'ref_${base64UrlEncode(List.generate(18, (_) => _rng.nextInt(256)))}';
    _refs[id] = (
      actor: s.user.id,
      hub: hub,
      kind: kind,
      value: value,
      expires: now.add(const Duration(minutes: 10))
    );
    return id;
  }

  Object _resolve(AdminSession s, String hub, String kind, Object? value,
      {bool consume = false}) {
    final r = _refs[value];
    if (r == null ||
        r.actor != s.user.id ||
        r.kind != kind ||
        r.hub != hub ||
        !r.expires.isAfter(DateTime.now().toUtc())) {
      throw _stale;
    }
    if (consume) _refs.remove(value);
    return r.value;
  }

  Future<SupportHubDto> _hub(AdminSession s, String id) async =>
      (await support.loadFleetHubs(s)).where((h) => h.id == id).firstOrNull ??
      (throw _stale);

  Future<_Json> _request(
      AdminSession s, SupportHubDto h, String method, String path,
      {_Json? body, Map<String, String>? query}) async {
    final req = DeviceAdminProxyRequestDto.fromJson({
      'method': method,
      'path': path,
      'body': body,
      if (query != null) 'query': query,
      'expectedServerInstanceId': h.serverInstanceId,
      'requestId':
          'agent-${base64UrlEncode(List.generate(18, (_) => _rng.nextInt(256)))}'
    });
    var outcome = 'unverified';
    try {
      final result = await probes.proxyJson(
          session: s, hubId: h.id, request: req, requireAssistantGrant: true);
      final receipt = asStringMap(result.body) ??
          (throw const AdminApiException(502, 'Invalid hub receipt.'));
      outcome = 'acknowledged';
      return receipt;
    } finally {
      if (req.isMutation) {
        _audit(s, h, path.split('/').last, outcome, req.requestId);
      }
    }
  }

  Future<_Json> _identity(AdminSession s, SupportHubDto h) async {
    final body =
        await _request(s, h, 'GET', 'api/state', query: {'include': 'base'});
    if (h.serverInstanceId == null ||
        body['server_instance_id'] != h.serverInstanceId) {
      throw _stale;
    }
    return body;
  }

  Future<Map<String, _Json>> _catalog(AdminSession s, SupportHubDto h) async {
    final body = await _request(s, h, 'GET', 'api/assistant/contract');
    final result = <String, _Json>{};
    for (final op in (body['operations'] as List? ?? []).whereType<_Json>()) {
      final id = op['id'], tier = op['tier'];
      final trusted = _trustedHubOperations[id];
      if (id is! String ||
          trusted == null ||
          tier != trusted['tier'] ||
          canonicalJsonString(op['input_schema']) !=
              canonicalJsonString(trusted['input_schema']) ||
          op['location'] != 'hub' ||
          op['invocation_path'] != '/api/assistant/operations/$id' ||
          !(tier == 'read' && assistantReadOperations.contains(id) ||
              s.staff.isAdmin &&
                  (tier == 'temporary_light_output' ||
                      tier == 'persistent_change'))) {
        continue;
      }
      result[id] = trusted;
    }
    return result;
  }

  _Json _hubRow(AdminSession s, SupportHubDto h) => {
        'hub_ref': _reference(s, '', 'hub', h.id),
        'home_ref': _reference(s, '', 'home', h.homeId),
        'enabled': h.enabled,
        'has_recorded_identity': h.serverInstanceId != null,
      };
  Future<_Json> _call(AdminSession s, String id, _Json args) async {
    if (id == 'staff_list_fleet') {
      final offset = args['offset'] as int? ?? 0;
      if (offset < 0) throw _badInput;
      final matches = (await support.loadFleetHubs(s))
          .where((h) => args['hub_id'] == null || h.id == args['hub_id'])
          .toList();
      return {
        'hubs': [for (final h in matches.skip(offset).take(100)) _hubRow(s, h)],
        if (offset + 100 < matches.length) 'next_offset': offset + 100
      };
    }
    if (id == 'staff_find_homes') {
      final needle = (args['query'] as String? ?? '').trim().toLowerCase();
      final snapshot = await support.loadSnapshot(s);
      final offset = args['offset'] as int? ?? 0;
      if (offset < 0) throw _badInput;
      final matches = [
        for (final customer in snapshot.customers)
          for (final entry in customer.homes)
            if ([
              customer.customerName,
              customer.customerEmail,
              entry.home.name,
              entry.home.id,
              ...entry.hubs.map((h) => h.name)
            ].whereType<String>().any((v) => v.toLowerCase().contains(needle)))
              entry,
      ];
      return {
        'homes': [
          for (final entry in matches.skip(offset).take(100))
            {
              'home_ref': _reference(s, '', 'home', entry.home.id),
              'hubs': [for (final h in entry.hubs) _hubRow(s, h)],
            }
        ],
        if (offset + 100 < matches.length) 'next_offset': offset + 100,
      };
    }
    if (id == 'staff_apply_plan') return _apply(s, args);
    final h = await _hub(s, _resolve(s, '', 'hub', args['hub_ref']) as String);
    if (id == 'staff_plan_delete_hub') {
      return _preview(s, h, 'delete', {}, {
        'effect':
            'Delete the cloud hub record and its dependent rows. The home and physical hub remain.',
        ..._hubRow(s, h)
      });
    }
    final _Json state;
    try {
      state = await _identity(s, h);
    } on AdminApiException catch (error) {
      if (id == 'staff_probe_hub' && [502, 503].contains(error.statusCode)) {
        return {
          'status': 'unreachable',
          'identity_verified': false,
          'hub_ref': args['hub_ref']
        };
      }
      rethrow;
    }
    switch (id) {
      case 'staff_probe_hub':
        return {
          'status': 'online',
          'identity_verified': true,
          'hub_ref': args['hub_ref']
        };
      case 'staff_read_hub_status':
        final status = await _request(
            s, h, 'POST', 'api/assistant/operations/read_hub_status',
            body: {});
        return {
          'online': true,
          'server_version': _safeVersion(state['version']),
          'status': _project(s, h.id, status)
        };
      case 'staff_list_logs':
        final data = await _request(s, h, 'GET', 'api/diag/logs');
        return {
          'sources': [
            for (final source in (data['sources'] as List).whereType<Map>())
              if (source['id'] is String &&
                  RegExp(r'^[A-Za-z0-9_.-]{1,160}$').hasMatch(source['id']) &&
                  source['id'] != '.' &&
                  source['id'] != '..')
                {
                  'source_ref': _reference(s, h.id, 'log', source['id']),
                  'bytes': source['bytes'] is int ? source['bytes'] : null
                }
          ]
        };
      case 'staff_read_log':
        final source = _resolve(s, h.id, 'log', args['source_ref']) as String;
        final limit = args['lines'] as int? ?? 100;
        if (limit < 1 || limit > 500) throw _badInput;
        final body = await _request(s, h, 'GET', 'api/diag/logs/$source/tail',
            query: {'lines': '$limit'});
        final lines = (body['tail']?['lines'] as List).whereType<Map>();
        return {
          'lines_reviewed': lines.length,
          'diagnostics': [
            for (final item in lines)
              if (_category(item['text']) case final category?)
                {
                  'line':
                      item['line_number'] is int ? item['line_number'] : null,
                  'category': category
                }
          ]
        };
      case 'staff_plan_fleet_report':
        return _preview(s, h, 'report', {
          'category': args['category'],
          'severity': args['severity']
        }, {
          'hub_ref': args['hub_ref'],
          'category': args['category'],
          'severity': args['severity'],
          'attachments': ['private diagnostic bundle'],
          'destination': 'Rhythm support bug intake'
        });
      case 'staff_select_hub':
        final catalog = await _catalog(s, h);
        return {
          'selected': true,
          'hub_ref': args['hub_ref'],
          'reads_and_previews': 'staff_call_hub_tool',
          if (s.staff.isAdmin) 'temporary_controls': 'staff_control_hub',
          'tools': [
            for (final op in catalog.values)
              if (op['tier'] != 'persistent_change')
                {
                  'id': op['id'],
                  'tier': op['tier'],
                  'description': op['description'],
                  'input_schema': op['input_schema']
                }
          ]
        };
      case 'staff_call_hub_tool':
      case 'staff_control_hub':
        final catalog = await _catalog(s, h),
            op = catalog[args['operation_id']];
        if (op == null ||
            op['tier'] !=
                (id == 'staff_control_hub'
                    ? 'temporary_light_output'
                    : 'read')) {
          throw _denied;
        }
        final tool = op['id'] as String;
        final arguments = _restore(s, h.id, args['arguments']) as _Json;
        final body = await _request(
            s, h, 'POST', 'api/assistant/operations/$tool',
            body: arguments);
        final result = tool == 'read_lighting_history'
            ? <String, dynamic>{'status': 'available_in_private_hub_console'}
            : _project(s, h.id, body) as _Json;
        final apply = body['operation'],
            reviewedArgs = asStringMap(body['apply_arguments']);
        if (reviewedArgs != null &&
            body['server_instance_id'] == h.serverInstanceId &&
            catalog[apply]?['tier'] == 'persistent_change') {
          return _preview(s, h, 'hub', {
            'id': apply,
            'arguments': reviewedArgs
          }, {
            'reviewed_operation': tool,
            'description': op['description'],
            ...result,
          });
        }
        return result;
      default:
        throw _badInput;
    }
  }

  _Json _preview(
      AdminSession s, SupportHubDto h, String op, _Json args, _Json review) {
    final plan = _reference(s, '', 'plan', _Proposal(h, op, args));
    return {
      'status': 'review',
      'location': 'staff_cloud',
      'hub_ref': _reference(s, '', 'hub', h.id),
      'change': op == 'hub' ? args['id'] : op,
      'scope': review,
      'requires_confirmation': true,
      'operation': 'staff_apply_plan',
      'plan_id': plan,
      'apply_arguments': {'plan_id': plan},
    };
  }

  Future<_Json> _apply(AdminSession s, _Json args) async {
    if (args['confirmed'] != true || !s.staff.isAdmin) throw _denied;
    final p =
        _resolve(s, '', 'plan', args['plan_id'], consume: true) as _Proposal;
    final h = await _hub(s, p.hub.id);
    if (h.homeId != p.hub.homeId ||
        h.serverInstanceId != p.hub.serverInstanceId ||
        h.updatedAt != p.hub.updatedAt) {
      throw _stale;
    }
    var outcome = 'unverified';
    try {
      if (p.operation == 'delete') {
        await support.deleteServerHub(h.id, reviewed: h);
      } else if (p.operation == 'report') {
        await _identity(s, h);
        final finding = FleetFindingReportRequestDto.fromJson({
          'title': 'Staff diagnostic: ${p.arguments['category']}',
          'severity': p.arguments['severity'],
          'scanRunId': args['plan_id'],
          'detectedAt': DateTime.now().toUtc().toIso8601String()
        });
        final report = await reports.report(
            session: s,
            hubId: h.id,
            finding: finding,
            reviewedServerId: h.serverInstanceId!);
        outcome = 'submitted';
        return {
          'status': 'submitted',
          'plan_id': args['plan_id'],
          'support_reference':
              _reference(s, h.id, 'report', report.submissionId),
          'issue_created': report.reportResult['issue_created'] == true
        };
      } else {
        await _identity(s, h);
        final catalog = await _catalog(s, h), id = p.arguments['id'];
        if (catalog[id]?['tier'] != 'persistent_change') throw _denied;
        final result = await _request(
            s, h, 'POST', 'api/assistant/operations/$id',
            body: {...p.arguments['arguments'] as _Json, 'confirmed': true});
        outcome = 'acknowledged';
        return _project(s, h.id, result) as _Json;
      }
      outcome = 'applied';
      return {
        'status': 'applied',
        'operation': p.operation,
        'plan_id': args['plan_id']
      };
    } finally {
      _audit(s, h, p.operation, outcome, args['plan_id'] as String);
    }
  }

  void _audit(AdminSession s, SupportHubDto h, String op, String outcome,
          String? requestId) =>
      stdout.writeln(jsonEncode({
        'event': 'staff_assistant_operation',
        'request_id': requestId,
        'actor': s.user.id,
        'hub_id': h.id,
        'operation': op,
        'outcome': outcome
      }));

  Object? _restore(AdminSession s, String hub, Object? value) {
    if (value is String && value.startsWith('ref_')) {
      return _resolve(s, hub, 'value', value);
    }
    if (value is List) return [for (final x in value) _restore(s, hub, x)];
    if (value is _Json) {
      return {for (final e in value.entries) e.key: _restore(s, hub, e.value)};
    }
    return value;
  }

  /// Allowlisted shapes only. Unknown vendor fields, labels, addresses, logs,
  /// and opaque plan payloads are never copied into model responses.
  Object? _project(AdminSession s, String hub, Object? value,
      [String field = '']) {
    if (value is List) {
      return [
        for (final item in value.take(256)) _project(s, hub, item, field)
      ];
    }
    if (value is Map) {
      return {
        for (final e in value.entries)
          if (_publicFields.contains(e.key))
            e.key as String: _project(s, hub, e.value, e.key as String)
      };
    }
    if ((value is String || value is num) && _idFields.contains(field)) {
      return _reference(s, hub, 'value', value as Object);
    }
    if (value is bool || value is num || value == null) return value;
    if (value is String) {
      if (_symbols.contains(value)) return value;
      if (_versionFields.contains(field)) return _safeVersion(value);
    }
    return null;
  }
}

const _idFields = {
  'id',
  'target_id',
  'bridge_id',
  'endpoint_id',
  'selected_endpoint_id',
  'entry_id',
  'scene_id',
  'binding_id',
  'source_node_id',
  'schedule_id',
  'from_room_id',
  'to_room_id',
  'previous_parent_id',
  'current_parent_id',
  'current_targets',
  'current_entities',
  'requested_entities',
  'profile_ids',
  'room_ids',
  'entities',
  'entity_ids',
  'control_target_ids',
  'preview_id',
  'room_id',
  'device_id',
  'sensor_id',
  'light_id',
  'physical_room_id',
  'profile_id',
  'walk_id',
  'trial_id',
  'parent_id',
  'control_targets',
  'before_targets',
  'after_targets',
  'routed_targets',
  'node_id',
  'affected_rooms',
  'from_room',
  'to_room'
};
const _versionFields = {
  'server_version',
  'version',
  'current_version',
  'latest_version',
  'current_image_version',
  'latest_image_version',
  'current_package_version',
  'latest_package_version'
};
const _publicFields = {
  ..._versionFields,
  ..._idFields,
  'status',
  'scope',
  'state',
  'rooms',
  'nodes',
  'lights',
  'controls',
  'kind',
  'stage',
  'reason',
  'availability',
  'readback',
  'power_on',
  'resident_power_on',
  'server_acknowledged',
  'canonical_readback_verified',
  'physical_verification',
  'observed',
  'on',
  'brightness',
  'kelvin',
  'rgb',
  'before',
  'after',
  'samples',
  'hour',
  'mode',
  'brightness_offset',
  'time_offset_minutes',
  'effect',
  'requested',
  'proposal',
  'source',
  'requested_rooms',
  'device',
  'schedule',
  'definition',
  'after_profile',
  'before_profile',
  'minimum_brightness',
  'evening_brightness_width',
  'enabled',
  'value',
  'setting',
  'offset_minutes',
  'decision',
  'target_mode',
  'update_available',
  'connected',
  'restart_required',
  'integration_status',
  'schedules',
  'bindings',
  'transitions',
  'modes',
  'updates',
  'auto',
  'auto_update',
  'update_channel',
  'keep_scene',
  'home_scene',
  'scene',
  'binding',
  'assignment',
  'schedule_override',
  'preferences',
  'breaker',
  'runtime',
  'affected_devices',
  'affected_count',
  'room_count',
  'device_count',
  'canonical_brightness',
  'canonical_kelvin',
  'canonical_power',
  'light_readback',
  'verification',
  'requires_confirmation',
  'plan',
  'cleared_offsets',
  'before_minutes',
  'after_minutes',
  'scenes',
  'mode_settings',
  'handoff',
  'actor',
  'surface',
  'window_seconds',
  'readiness',
  'platform',
  'node_count',
  'hub_count',
  'health',
  'available',
  'adaptation_enabled',
  'online',
  'managed_ha_light_count',
  'integration_connected',
  'light_breaker_enabled'
};
const _symbols = {
  'update_accepted',
  'restart_accepted',
  'unpair_endpoint',
  'forget_rhythm_record',
  'disconnect_and_restart',
  'rhythm_record',
  'minimum_brightness',
  'evening_brightness_width',
  'merge',
  'bind',
  'keep_separate',
  'dismiss',
  'snooze',
  'still_installed',
  'devices',
  'rooms',
  'beta',
  'stable',
  'not_observed',
  'not_required',
  'applied',
  'ready',
  'available',
  'unavailable',
  'unknown',
  'active',
  'day',
  'sleep',
  'standby',
  'mood',
  'wake',
  'warning',
  'hard_off',
  'room',
  'light_device',
  'motion_sensor',
  'sensor',
  'button',
  'switch_device',
  'motion',
  'fresh_device_readback',
  'no_fresh_readback',
  'walk_started',
  'walk_active',
  'walk_finished',
  'waiting_for_detection',
  'no_fresh_detection',
  'detected',
  'missing_target',
  'routed',
  'suppressed',
  'command_not_needed',
  'command_queued',
  'command_attempted',
  'command_acknowledged',
  'command_uncertain',
  'no_command_issued',
  'already_linked',
  'handoff',
  'resident',
  'ha_administrator',
  'home_assistant',
  'rhythm_app',
  'integration',
  'shared_profile',
  'room_profile_override',
  'server',
  'rpiz',
  'ha_addon',
  'healthy',
  'online',
  'offline',
  'enabled',
  'disabled',
  'read',
  'already_on',
  'motion_disabled',
  'scene_suppression'
};
String? _safeVersion(Object? value) => value is String &&
        RegExp(r'^v?\d{1,4}\.\d{1,4}\.\d{1,6}(?:-(?:alpha|beta|rc)(?:[.-]\d{1,6})?)?$')
            .hasMatch(value)
    ? value
    : null;
String? _category(Object? value) {
  if (value is! String) return null;
  final line = value.toLowerCase();
  if (RegExp('disconnect|network unreachable|tunnel.*(closed|down)')
      .hasMatch(line)) {
    return 'network_offline';
  }
  if (RegExp('reconnect|tunnel.*connected').hasMatch(line)) {
    return 'network_restored';
  }
  if (RegExp('auth.*(fail|denied)|unauthorized|forbidden').hasMatch(line)) {
    return 'authentication';
  }
  if (line.contains('timeout') || line.contains('timed out')) return 'timeout';
  if (RegExp('reboot|server started|restart').hasMatch(line)) return 'restart';
  if (RegExp('integration.*(unavailable|offline)').hasMatch(line)) {
    return 'integration_unavailable';
  }
  if (RegExp(r'\berror\b').hasMatch(line)) return 'device_error';
  return null;
}
