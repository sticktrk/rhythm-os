import 'dart:convert';
import 'dart:io';
import 'dart:math';

import 'package:shelf/shelf.dart';

import 'assistant_read_operations.dart';
import 'assistant_audit.dart';
import 'device_json.dart';
import 'staff_hub_catalog.dart';
import 'staff_diagnostics.dart';
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
  'wifi_interface_unavailable',
  'wifi_address_unavailable',
  'wifi_configuration_unavailable',
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

const _brightness = {'type': 'integer', 'minimum': 1, 'maximum': 100};
const _kelvin = {'type': 'integer', 'minimum': 500, 'maximum': 25000};
const _width = {'type': 'number', 'minimum': 0.2, 'maximum': 2.0};
const _profileSettings = {
  'min_brightness': _brightness,
  'max_brightness': _brightness,
  'min_color_temp': _kelvin,
  'max_color_temp': _kelvin,
  'curve.width_left_bri': _width,
  'curve.width_right_bri': _width,
  'curve.width_left_cct': _width,
  'curve.width_right_cct': _width,
};

const _tools = [
  _Tool(
      'staff_find_homes',
      'Find an exact home or hub name, ID or account email in the support directory. Use staff_list_fleet to browse. Return private selection references.',
      {
        'query': _str,
        'account_email_only': {'type': 'boolean'},
        'offset': {'type': 'integer', 'minimum': 0}
      },
      [
        'query'
      ]),
  _Tool(
      'staff_list_fleet',
      'List enabled server hubs. Narrow by a returned home_ref and the hub name provided by the person when multiple hubs match. The console can pass its selected hub_id.',
      {
        'hub_id': _str,
        'home_ref': _str,
        'query': _str,
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
      'Read recent infrastructure diagnostic categories and UTC timestamps from one log tail. Use since and until (exclusive) with an explicit time zone. An incomplete tail or missing timestamp cannot establish an outage time.',
      {
        ..._hub,
        'source_ref': _str,
        'include_timings': {'type': 'boolean'},
        'lines': {'type': 'integer', 'minimum': 1, 'maximum': 500},
        'since': {'type': 'string', 'format': 'date-time'},
        'until': {'type': 'string', 'format': 'date-time'}
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
      'staff_find_hub_nodes',
      'Find a room, sensor or light by the name supplied by the person on the selected hub. Search stays inside the staff service. Use returned IDs in hub tools and ask for clarification when multiple results match.',
      {
        ..._hub,
        'query': _str,
        'exact': {'type': 'boolean'},
        'kind': {
          'type': 'string',
          'enum': [
            'room',
            'motion_sensor',
            'light_device',
            'switch_device',
            'button',
            'sensor',
            'other_device'
          ]
        }
      },
      [
        'hub_ref',
        'query'
      ]),
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
      'staff_plan_profile_tuning',
      'Review numeric changes to one shared lighting profile and compare curves sampled on this hub. The administrator confirms the saved hub plan in the console.',
      {
        ..._hub,
        'profile_id': _str,
        'date': _str,
        'overrides': {
          'type': 'object',
          'properties': _profileSettings,
          'additionalProperties': false,
          'minProperties': 1
        }
      },
      [
        'hub_ref',
        'profile_id',
        'overrides'
      ]),
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
      'staff_read_plan_outcome',
      'Read the outcome and canonical readback of your pending or recently approved plan. Outcome records expire after ten minutes.',
      {'plan_id': _str},
      ['plan_id']),
  _Tool(
      'staff_apply_plan',
      'Request console approval for a reviewed staff proposal. An administrator must open the selected hub in the staff console and approve it.',
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
  _Proposal(this.hub, this.operation, this.arguments, this.review);
  final _Json review;
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
  StaffAssistant(this.support, this.probes, this.reports, this._auditKey,
      {void Function(String)? auditLog})
      : _auditLog = auditLog ?? stdout.writeln;
  final void Function(String) _auditLog;
  final String? _auditKey;
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
              'Select the intended hub. Send proposed changes to the selected hub in the staff console for administrator approval. Read back after an uncertain response.'
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
      case 'resources/templates/list':
        result = {'resourceTemplates': []};
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
        final target = args is _Json
            ? _auditTarget(session, args)
            : const <String, dynamic>{};
        var outcome = 'error';
        try {
          if (tool == null || args is! _Json) {
            return json(
                _rpcError(id, -32602, 'Unknown tool or invalid arguments'));
          }
          if (tool.writes && !session.staff.isAdmin) throw _denied;
          tool.validate(args);
          final body = await _call(session, tool.id, args);
          if (tool.id == 'staff_probe_hub' &&
              body['identity_verified'] == true) {
            // A human can match this observation to its hub in the private audit
            // after the scan actor's selection expires. It grants no access.
            final ref =
                base64UrlEncode(List.generate(18, (_) => _rng.nextInt(256)));
            target['audit_ref'] = ref;
            body['audit_ref'] = ref;
          }
          outcome = 'ok';
          result = {
            'content': [
              {'type': 'text', 'text': jsonEncode(body)}
            ],
            'structuredContent': body
          };
        } on AdminApiException catch (error) {
          if (error.statusCode == 401 || error.statusCode == 403)
            outcome = 'refused';
          // HTTP failures may include base URLs, request bodies and vendor
          // diagnostics. Only bounded descriptions can leave this service.
          final message = error is AssistantDeviceRejection
              ? error.message
              : switch (error.statusCode) {
                  401 ||
                  403 =>
                    'Access refused. Verify staff permissions and the current customer support grant.',
                  409 || 428 => _stale.message,
                  400 => _badInput.message,
                  404 => 'The selected resource is unavailable.',
                  429 =>
                    'Too many active selections. Try again after they expire.',
                  _ =>
                    'The operation could not be verified. Reread before retrying.',
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
        } finally {
          _auditLog(jsonEncode({
            'event': 'staff_assistant_tool',
            'at': DateTime.now().toUtc().toIso8601String(),
            'actor': session.user.id,
            'operation': tool?.id ?? 'unknown_tool',
            ...target,
            'outcome': outcome
          }));
        }
      default:
        return json(_rpcError(id, -32601, 'Method not found'));
    }
    return json({'jsonrpc': '2.0', 'id': id, 'result': result});
  }

  _Json _auditTarget(AdminSession s, _Json args) {
    final now = DateTime.now().toUtc();
    for (final (key, kind) in [
      ('plan_id', 'plan'),
      ('hub_ref', 'hub'),
      ('home_ref', 'home')
    ]) {
      final ref = _refs[args[key]];
      if (ref == null ||
          ref.actor != s.user.id ||
          ref.kind != kind ||
          !ref.expires.isAfter(now)) continue;
      if (kind == 'plan') return {'hub_id': (ref.value as _Proposal).hub.id};
      return {'${kind}_id': ref.value};
    }
    return {};
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

  Future<SupportHubDto> _hub(AdminSession s, String id,
      {bool cloudOnly = false}) async {
    final fleet = await support.loadFleetHubs(s);
    final selected = fleet.where((h) => h.id == id).firstOrNull;
    if (selected == null) throw _stale;
    if (!cloudOnly &&
        (selected.serverInstanceId == null ||
            fleet.any((h) =>
                h.id != id &&
                h.serverInstanceId == selected.serverInstanceId))) {
      throw _stale;
    }
    return selected;
  }

  Future<_Json> _request(
      AdminSession s, SupportHubDto h, String method, String path,
      {_Json? body, Map<String, String>? query, String? requestId}) async {
    final req = DeviceAdminProxyRequestDto.fromJson({
      'method': method,
      'path': path,
      'body': body,
      if (query != null) 'query': query,
      'expectedServerInstanceId': h.serverInstanceId,
      'requestId': requestId ??
          'agent-${base64UrlEncode(List.generate(18, (_) => _rng.nextInt(256)))}'
    });
    var outcome = 'unverified';
    _Json? receipt;
    try {
      final result = await probes.proxyJson(
          session: s, hubId: h.id, request: req, requireAssistantGrant: true);
      receipt = asStringMap(result.body) ??
          (throw const AdminApiException(502, 'Invalid hub receipt.'));
      outcome = 'acknowledged';
      return receipt;
    } finally {
      if (req.isMutation) {
        final details = await assistantAuditReceipt(
            hubId: h.id, secret: _auditKey, arguments: body, receipt: receipt);
        _audit(s, h, path.split('/').last, outcome, req.requestId, details);
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
          trusted['observes_household'] == true && !s.staff.isAdmin ||
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
      final homeId = args['home_ref'] == null
          ? null
          : _resolve(s, '', 'home', args['home_ref']);
      final needle = (args['query'] as String? ?? '').trim().toLowerCase();
      final matches = (await support.loadFleetHubs(s))
          .where((h) =>
              (args['hub_id'] == null || h.id == args['hub_id']) &&
              (homeId == null || h.homeId == homeId) &&
              (needle.isEmpty || h.name.trim().toLowerCase() == needle))
          .toList();
      return {
        'hubs': [for (final h in matches.skip(offset).take(100)) _hubRow(s, h)],
        if (offset + 100 < matches.length) 'next_offset': offset + 100
      };
    }
    if (id == 'staff_find_homes') {
      final needle = (args['query'] as String? ?? '').trim().toLowerCase();
      if (needle.length < 3) throw _badInput;
      final snapshot = await support.loadSnapshot(s);
      final offset = args['offset'] as int? ?? 0;
      if (offset < 0) throw _badInput;
      final matches = [
        for (final customer in snapshot.customers)
          for (final entry in customer.homes)
            if ([
              customer.customerEmail,
              if (args['account_email_only'] != true) ...[
                entry.home.name,
                entry.home.id,
                for (final h in entry.hubs) ...[h.name, h.id]
              ]
            ].whereType<String>().any((v) => v.trim().toLowerCase() == needle))
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
    if (id == 'staff_read_plan_outcome') {
      final plan = args['plan_id'];
      if (_refs[plan]?.kind == 'plan') {
        _resolve(s, '', 'plan', plan);
        return {'status': 'awaiting_console_approval', 'plan_id': plan};
      }
      return {..._resolve(s, '', 'outcome', plan) as _Json, 'plan_id': plan};
    }
    if (id == 'staff_apply_plan') {
      if (args['confirmed'] != true || !s.staff.isAdmin) throw _denied;
      _resolve(s, '', 'plan', args['plan_id']);
      return {
        'status': 'awaiting_console_approval',
        'plan_id': args['plan_id'],
        'next_check':
            'Open the selected hub in the staff console and approve the pending change.'
      };
    }
    final h = await _hub(s, _resolve(s, '', 'hub', args['hub_ref']) as String,
        cloudOnly: id == 'staff_plan_delete_hub');
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
        final basic = {
          'online': true,
          'server_version': _safeVersion(state['version'])
        };
        try {
          final status = await _request(
              s, h, 'POST', 'api/assistant/operations/read_hub_status',
              body: {});
          return {
            ...basic,
            'details_available': true,
            'status': _project(s, h.id, status)
          };
        } on AdminApiException catch (error) {
          if (error.statusCode != 502 && error.statusCode != 404) rethrow;
          return {
            ...basic,
            'details_available': false,
            'next_check':
                'This hub answered the identity check. Detailed lighting status could not be read.'
          };
        }
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
                  'family': staffLogFamily(source['id'] as String),
                  if (staffDiagnosticTime(source['modified_at'])
                      case final modified?)
                    'modified_at': modified.toIso8601String(),
                  'bytes': source['bytes'] is int ? source['bytes'] : null
                }
          ]
        };
      case 'staff_read_log':
        final source = _resolve(s, h.id, 'log', args['source_ref']) as String;
        final limit = args['lines'] as int? ?? 100;
        if (limit < 1 || limit > 500) throw _badInput;
        final since = staffDiagnosticTime(args['since']),
            until = staffDiagnosticTime(args['until']);
        final includeTimings = args['include_timings'] == true;
        if ((includeTimings && (since == null || until == null)) ||
            (args.containsKey('since') && since == null) ||
            (args.containsKey('until') && until == null) ||
            (since != null && until != null && !since.isBefore(until))) {
          throw _badInput;
        }
        final body = await _request(s, h, 'GET', 'api/diag/logs/$source/tail',
            query: {'lines': '$limit'});
        return staffLogDiagnostics(body['tail']['lines'] as List,
            since: since, until: until, includeTimings: includeTimings);
      case 'staff_plan_profile_tuning':
        return _planProfileTuning(s, h, args);
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
          'resolve_room_or_device': 'staff_find_hub_nodes',
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
      case 'staff_find_hub_nodes':
        final catalog = await _catalog(s, h);
        if (catalog['get_topology_snapshot'] == null) {
          throw const AdminApiException(404, 'Hub topology is unavailable.');
        }
        final topology = await _request(
            s, h, 'POST', 'api/assistant/operations/get_topology_snapshot',
            body: {});
        final query = (args['query'] as String).trim().toLowerCase();
        if (query.isEmpty) throw _badInput;
        final nodes = (topology['nodes'] as List)
            .whereType<_Json>()
            .where((n) =>
                n['id'] is String &&
                n['name'] is String &&
                (args['kind'] == null || n['kind'] == args['kind']))
            .toList();
        final exact = nodes
            .where((n) => (n['name'] as String).toLowerCase() == query)
            .toList();
        final matches = exact.isNotEmpty || args['exact'] == true
            ? exact
            : nodes
                .where(
                    (n) => (n['name'] as String).toLowerCase().contains(query))
                .toList();
        return {
          'total_matches': matches.length,
          'nodes': [
            for (final n in matches.take(50))
              _project(s, h.id, {
                'id': n['id'],
                'kind': n['kind'],
                'parent_id': n['parent_id'],
                'controls': n['controls']
              })
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
        if (tool == 'read_profile_curve' && arguments['view'] == 'day') {
          return _curveSamples(body);
        }
        if (tool == 'read_lighting_history') {
          return {'status': 'available_in_private_hub_console'};
        }
        if (body['status'] == 'handoff') {
          final original = asStringMap(body['handoff']) ?? body;
          final steps = op['input_schema']['properties']['step']?['enum'];
          final step = arguments['step'];
          final knownStep =
              step is String && steps is List && steps.contains(step);
          final actor = switch (original['actor']) {
            'resident' || 'owner' || 'ha_administrator' => original['actor'],
            _ => 'staff',
          };
          final surface = switch (original['surface']) {
            'home_assistant' ||
            'home_assistant_addon' ||
            'rhythm_app' ||
            'integration' ||
            'integration_settings' =>
              original['surface'],
            _
                when actor == 'ha_administrator' ||
                    (knownStep && step.startsWith('ha_')) =>
              'home_assistant',
            _ when actor == 'owner' || actor == 'resident' => 'rhythm_app',
            _ => 'staff_hub_console',
          };
          final instructions = _handoffs[step] ??
              (tool == 'start_motion_walk' || tool == 'plan_motion_target'
                  ? (arguments['sensor_id'] == ''
                      ? 'Pair the sensor on this hub, then refresh discovery and start a new walk.'
                      : 'Check discovery and room automation ownership on the indicated surface, then repeat the walk.')
                  : 'Open the indicated surface, complete the requested setup step and refresh this hub.');
          final resume = original['resume_operation'];
          return {
            'status': 'handoff',
            'hub_ref': args['hub_ref'],
            if (knownStep) 'step': step,
            'actor': actor,
            'surface': surface,
            'instructions': instructions,
            'handoff': {
              'actor': actor,
              'surface': surface,
              'instruction': instructions,
              'resume_operation': _trustedHubOperations.containsKey(resume)
                  ? resume
                  : 'get_topology_snapshot'
            }
          };
        }
        if (tool == 'inspect_motion_walk') {
          final result = _project(s, h.id, body) as _Json;
          result['next_check'] = _walkGuidance[result['stage']] ??
              'Inspect the selected walk again on the same hub.';
          if (result['stage'] == 'waiting_for_detection' &&
              result['status'] == 'walk_finished') {
            result['next_check'] =
                'Check sensor discovery, power and integration availability before starting a new walk.';
          }
          return result;
        }
        final apply = body['operation'];
        final reviewedArgs = asStringMap(body['apply_arguments']);
        if (reviewedArgs == null) return _project(s, h.id, body) as _Json;
        if (body['server_instance_id'] != h.serverInstanceId) throw _stale;

        // Omit only envelope fields represented by the service's own plan,
        // hub handle and trusted operation descriptor. Compare the remaining
        // review recursively so new fields and truncated lists fail closed.
        final scope = Map.of(body)
          ..removeWhere((key, value) =>
              _reviewEnvelope.contains(key) ||
              key == 'change' && value == tool);
        final result = _project(s, h.id, scope) as _Json;
        if (!scope.keys.any((key) => !_reviewMetadata.contains(key)) ||
            !_completeProjection(scope, result)) {
          return {
            'status': 'handoff',
            'hub_ref': args['hub_ref'],
            'actor': 'staff',
            'surface': 'staff_hub_console',
            'instructions':
                'Open Hub tools for this hub and run the same preview. Review the full scope there before an administrator confirms the change.',
            if (body['potentially_affected_rooms'] is List)
              'affected_room_count':
                  (body['potentially_affected_rooms'] as List).length,
            if (body['pauses_adaptation'] is bool)
              'pauses_adaptation': body['pauses_adaptation'],
          };
        }
        if (_trustedHubOperations[apply]?['tier'] != 'persistent_change') {
          throw _denied;
        }
        final review = {
          'reviewed_operation': tool,
          'description': op['description'],
          ...result
        };
        if (s.staff.isAdmin && catalog[apply]?['tier'] != 'persistent_change') {
          throw _denied;
        }
        return _preview(
            s, h, 'hub', {'id': apply, 'arguments': reviewedArgs}, review);
      default:
        throw _badInput;
    }
  }

  Future<_Json> _planProfileTuning(
      AdminSession s, SupportHubDto h, _Json args) async {
    final catalog = await _catalog(s, h);
    if (catalog['read_profile_curve'] == null ||
        catalog['plan_profile_edit'] == null ||
        (s.staff.isAdmin && catalog['apply_profile_edit'] == null)) {
      throw _denied;
    }
    final profile = _restore(s, h.id, args['profile_id']);
    final before = await _request(s, h, 'GET', 'api/config',
        query: {'id': profile as String});
    if (before['id'] != profile) throw _stale;
    final draft = jsonDecode(jsonEncode(before)) as _Json;
    final changes = <String, dynamic>{};
    final overrides = args['overrides'] as _Json;
    if (overrides.isEmpty) throw _badInput;
    for (final e in overrides.entries) {
      final spec = _profileSettings[e.key], v = e.value;
      if (spec == null ||
          v is! num ||
          !v.isFinite ||
          spec['type'] == 'integer' && v is! int ||
          v < (spec['minimum'] as num) ||
          v > (spec['maximum'] as num)) {
        throw _badInput;
      }
      final parts = e.key.split('.');
      final obj = parts.length == 1 ? draft : draft[parts.first];
      if (obj is! _Json || obj[parts.last] is! num) throw _badInput;
      changes[e.key] = {'before': obj[parts.last], 'after': v};
      obj[parts.last] = v;
    }
    final curveArgs = {
      'profile_id': profile,
      'view': 'day',
      'samples_per_hour': 12,
      if (args['date'] != null) 'date': args['date']
    };
    Future<_Json> sample(_Json config) async {
      final response = await _request(
          s, h, 'POST', 'api/assistant/operations/read_profile_curve',
          body: {...curveArgs, 'profile': config});
      return _curveSamples(response);
    }

    final currentSamples = await sample(before),
        draftSamples = await sample(draft);
    final native = await _request(
        s, h, 'POST', 'api/assistant/operations/plan_profile_edit',
        body: {'profile_id': profile, 'profile': draft});
    final reviewed = asStringMap(native['review']),
        apply = asStringMap(native['apply_arguments']);
    if (native['operation'] != 'apply_profile_edit' ||
        native['server_instance_id'] != h.serverInstanceId ||
        native['plan_id'] is! String ||
        apply == null ||
        apply['plan_id'] != native['plan_id'] ||
        reviewed == null ||
        canonicalJsonString(reviewed['before']) !=
            canonicalJsonString(before) ||
        canonicalJsonString(reviewed['after']) != canonicalJsonString(draft)) {
      throw _stale;
    }
    final rooms = native['potentially_affected_rooms'];
    if (rooms is! List) throw _stale;
    return _preview(s, h, 'hub', {
      'id': 'apply_profile_edit',
      'arguments': apply
    }, {
      'reviewed_operation': 'plan_profile_edit',
      'profile_id': _reference(s, h.id, 'value', profile),
      'affected_room_count': rooms.length,
      'changes': changes,
      'current_curve': currentSamples,
      'candidate_curve': draftSamples,
    }, consoleReview: {
      'review': reviewed,
      'potentially_affected_rooms': rooms,
      'pauses_adaptation': native['pauses_adaptation']
    });
  }

  _Json _preview(
      AdminSession s, SupportHubDto h, String op, _Json args, _Json review,
      {_Json? consoleReview}) {
    final base = {
      'status': 'review',
      'location': 'staff_cloud',
      'hub_ref': _reference(s, '', 'hub', h.id),
      'change': op == 'hub' ? args['id'] : op,
      'scope': review,
      'requires_confirmation': true,
      'can_apply': s.staff.isAdmin,
    };
    if (!s.staff.isAdmin) {
      return {
        ...base,
        'next_check':
            'Ask an administrator to run a fresh preview before confirming this change.'
      };
    }
    final plan = _reference(
        s, '', 'plan', _Proposal(h, op, args, consoleReview ?? review));
    return {
      ...base,
      'operation': 'staff_apply_plan',
      'plan_id': plan,
      'apply_arguments': {'plan_id': plan}
    };
  }

  List<_Json> pendingForHub(AdminSession s, String hubId) {
    if (!s.staff.isAdmin) throw _denied;
    final now = DateTime.now().toUtc();
    return [
      for (final entry in _refs.entries)
        if (entry.value.actor == s.user.id &&
            entry.value.kind == 'plan' &&
            entry.value.expires.isAfter(now) &&
            entry.value.value is _Proposal)
          if (entry.value.value as _Proposal case final proposal
              when proposal.hub.id == hubId &&
                  proposal.operation != 'console_hub')
            {
              'plan_id': entry.key,
              'operation': proposal.operation,
              'review': proposal.review
            },
    ];
  }

  Future<String?> registerConsolePreview(
      AdminSession s, String hubId, String path, _Json body) async {
    if (!s.staff.isAdmin ||
        !path.startsWith('api/assistant/operations/') ||
        _trustedHubOperations[path.split('/').last]?['tier'] != 'read')
      return null;
    final operation = body['operation'];
    final args = asStringMap(body['apply_arguments']);
    if (_trustedHubOperations[operation]?['tier'] != 'persistent_change' ||
        args == null ||
        body['plan_id'] is! String ||
        args['plan_id'] != body['plan_id']) return null;
    final h = await _hub(s, hubId);
    if (h.serverInstanceId == null ||
        body['server_instance_id'] != h.serverInstanceId) throw _stale;
    return _reference(s, '', 'plan',
        _Proposal(h, 'console_hub', {'id': operation, 'arguments': args}, {}));
  }

  Future<_Json> approve(AdminSession s, String planId) async {
    if (!s.staff.isAdmin) throw _denied;
    final p = _resolve(s, '', 'plan', planId, consume: true) as _Proposal;
    final h = await _hub(s, p.hub.id, cloudOnly: p.operation == 'delete');
    if (h.homeId != p.hub.homeId ||
        h.serverInstanceId != p.hub.serverInstanceId ||
        h.updatedAt != p.hub.updatedAt) {
      throw _stale;
    }
    var outcome = 'unverified';
    final receipt = <String, dynamic>{'status': 'pending'};
    _refs[planId] = (
      actor: s.user.id,
      hub: '',
      kind: 'outcome',
      value: receipt,
      expires: DateTime.now().toUtc().add(const Duration(minutes: 10))
    );
    try {
      if (p.operation == 'delete') {
        await support.deleteServerHub(h.id, reviewed: h);
      } else if (p.operation == 'report') {
        await _identity(s, h);
        final finding = FleetFindingReportRequestDto.fromJson({
          'title': 'Staff diagnostic: ${p.arguments['category']}',
          'severity': p.arguments['severity'],
          'scanRunId': planId,
          'detectedAt': DateTime.now().toUtc().toIso8601String()
        });
        final report = await reports.report(
            session: s,
            hubId: h.id,
            finding: finding,
            reviewedServerId: h.serverInstanceId!);
        outcome = 'submitted';
        receipt['issue_created'] = report.reportResult['issue_created'] == true;
        return {
          'status': 'submitted',
          'plan_id': planId,
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
            requestId: planId,
            body: {...p.arguments['arguments'] as _Json, 'confirmed': true});
        final projected = _project(s, h.id, result) as _Json;
        outcome = switch (projected['status']) {
          'applied' => 'applied',
          'partially_applied' => 'partially_applied',
          'queued' => 'queued',
          'failed' => 'failed',
          'uncertain' => 'uncertain',
          _ => 'acknowledged'
        };
        if (p.operation == 'hub' && outcome == 'applied') {
          if (id == 'apply_profile_edit' && p.review['review'] is Map) {
            final expected = p.review['review']['after'];
            receipt['canonical_readback_verified'] = expected is Map &&
                canonicalJsonString(result['canonical_readback']) ==
                    canonicalJsonString(expected);
          } else if (projected['canonical_readback_verified'] is bool) {
            receipt['canonical_readback_verified'] =
                projected['canonical_readback_verified'];
          }
        }
        return p.operation == 'console_hub'
            ? result
            : {
                ...projected,
                if (receipt['canonical_readback_verified'] is bool)
                  'canonical_readback_verified':
                      receipt['canonical_readback_verified'],
                'plan_id': planId
              };
      }
      outcome = 'applied';
      return {'status': 'applied', 'operation': p.operation, 'plan_id': planId};
    } finally {
      receipt['status'] = outcome;
      _audit(s, h, p.operation, outcome, planId);
    }
  }

  void _audit(AdminSession s, SupportHubDto h, String op, String outcome,
          String? requestId,
          [Map<String, dynamic> details = const {}]) =>
      stdout.writeln(jsonEncode({
        'event': 'staff_assistant_operation',
        'request_id': requestId,
        'actor': s.user.id,
        'hub_id': h.id,
        'operation': op,
        'outcome': outcome,
        ...details,
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
      if (_symbols.contains(value) ||
          (field == 'integrations' && _integrationKinds.contains(value)) ||
          (field == 'reason' && _updateReasons.contains(value))) {
        return value;
      }
      if (_versionFields.contains(field)) return _safeVersion(value);
    }
    return null;
  }
}

// Staff-authored guidance is independent of free-form strings on the customer's hub.
const _walkGuidance = {
  'waiting_for_detection':
      'Walk past the selected sensor and check the walk again.',
  'detected': 'Preview the intended room target with plan_motion_target.',
  'routed': 'Check the walk again for the automation decision.',
  'suppressed':
      'Review motion enablement, the active scene and room automation ownership.',
  'command_not_needed':
      'Check each light and ask the resident to confirm its visible state.',
  'command_queued': 'Check the walk again for dispatch and acknowledgement.',
  'command_attempted':
      'Check the walk again for acknowledgement and fresh light readback.',
  'command_acknowledged':
      'Check each light. Ask the resident about lights with no fresh readback.',
  'no_command_issued':
      'Check whether the intended room has a reachable managed light.',
  'command_uncertain':
      'Inspect readback and integration availability before retrying.',
};
const _handoffs = {
  'first_wifi':
      'Use Add Light Box near the box to provision Wi-Fi over Bluetooth.',
  'pairing_code':
      'Scan the device setup code in Add Device in the Rhythm phone app.',
  'phone_location': 'Review the location permission in the Rhythm phone app.',
  'apple_health': 'Review Health sync permissions in the iOS app.',
  'ha_integration':
      'Open Devices & services in Home Assistant settings to configure the integration.',
  'ha_area':
      'Review the device area in Home Assistant, then refresh Rhythm discovery.',
  'ha_location':
      'Review the installation location in Home Assistant system settings.',
  'ha_update': 'Review the Rhythm add-on update in Home Assistant.',
  'cloud_account': 'Open account and home settings in Rhythm.',
  'owner_access': 'Open hub settings in Rhythm to review owner access.',
  'restore': 'Review the destination in Backup & Restore in Rhythm.',
  'factory_reset':
      'Review factory reset in the selected hub’s system settings.',
  'ha_matter':
      'Review Matter commissioning in the Home Assistant add-on panel.',
  'pair': 'Use Add Device on the phone to commission the device on this hub.',
  'mobile_enrollment': 'Enroll the phone on this hub in the Rhythm app.',
  'bridge_credentials': 'Authorize the bridge in the integration settings.',
  'archived_device': 'Review archived devices in the device settings.',
};

const _integrationKinds = {
  'hue',
  'hue_ble',
  'local_ble',
  'ha',
  'homeassistant',
  'matter',
  'monster',
};
const _updateReasons = {
  'version_mismatch',
  'component_drift',
  'image_base_drift'
};
const _reviewEnvelope = {
  'schema_version',
  'contract_sha256',
  'topology_resource_sha256',
  'server_instance_id',
  'plan_id',
  'correlation_id',
  'operation',
  'apply_arguments',
};
const _reviewMetadata = {
  'status',
  'expires_in_seconds',
  'expires_at_epoch_ms',
  'requires_confirmation',
};

// Only calculated numeric samples and solar hours leave the trusted service.
_Json _curveSamples(_Json response) {
  final curve = asStringMap(response['curve']);
  final hours = curve?['hours'],
      brightness = curve?['brightness'],
      kelvin = curve?['kelvin'];
  if (hours is! List ||
      brightness is! List ||
      kelvin is! List ||
      hours.isEmpty ||
      hours.length > 288 ||
      hours.length != brightness.length ||
      hours.length != kelvin.length ||
      ![...hours, ...brightness, ...kelvin]
          .every((v) => v is num && v.isFinite)) {
    throw const AdminApiException(502, 'Invalid curve samples');
  }
  final solar = asStringMap(response['solar']);
  return {
    'hours': hours,
    'brightness': brightness,
    'kelvin': kelvin,
    for (final key in ['sunrise', 'sunset'])
      if (solar?[key] is num) key: solar![key]
  };
}

bool _completeProjection(Object? source, Object? projected) {
  if (source is Map) {
    return projected is Map &&
        source.entries.every((entry) =>
            projected.containsKey(entry.key) &&
            _completeProjection(entry.value, projected[entry.key]));
  }
  if (source is List) {
    if (projected is! List || source.length != projected.length) return false;
    for (var i = 0; i < source.length; i++) {
      if (!_completeProjection(source[i], projected[i])) return false;
    }
    return true;
  }
  // Scalars may be replaced with opaque references. A redacted value is null.
  return source == null || projected != null;
}

const _idFields = {
  'review_revision',
  'id',
  'target_id',
  'source_room_id',
  'target_room_id',
  'mood_scene_id',
  'session_id',
  'affected_node_ids',
  'unresolved_node_ids',
  'skipped_node_ids',
  'preferred_endpoint_id',
  'source_node_ids',
  'scene_ids',
  'profile_ids_to_reset',
  'suggested_room',
  'room_key',
  'integration_room_id',
  'selected_room_id',
  'canonical_id',
  'canonical_device_id',
  'entity_id',
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
  'inherited_targets',
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
  ..._reviewMetadata,
  'needs_reprovision',
  ..._versionFields,
  ..._idFields,
  'scope',
  'state',
  'rooms',
  'nodes',
  'lights',
  'controls',
  'inherited',
  'placement',
  'configured',
  'supported',
  'endpoints',
  'candidates',
  'candidate_matches',
  'room_binding',
  'entries',
  'integration',
  'integrations',
  'onboarding',
  'supports_removal',
  'room_automation_owners',
  'topology_sync_enabled',
  'owner',
  'control_scope',
  'resident_confirmation',
  'xy',
  'pending_dispatch',
  'legacy_schedule',
  'profile_override',
  'default_output',
  'profiles',
  'active_mode',
  'schedule_ownership',
  'room_defaults',
  'rhythm_enabled',
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
  'result',
  'dispatch',
  'targets',
  'applied',
  'queued',
  'applied_target_count',
  'failed_target_count',
  'skipped_target_count',
  'current_room_state',
  'pauses_adaptation',
  'skipped',
  'mapping',
  'affected_profiles',
  'mapped',
  'members',
  'removed',
  'pairing_status',
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
  'acknowledged',
  'partially_applied',
  'queued',
  'failed',
  'complete',
  'pending',
  'profile_sample',
  'reread_resource',
  'uncertain',
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
  'evaluating_policy',
  'external_room_authority',
  'light_breaker_disabled',
  'motion_activation_disabled',
  'active_scene',
  'active_countdown',
  'already_tracking',
  'awaiting_dispatch',
  'awaiting_acknowledgement',
  'check_each_light',
  'no_dispatch_route',
  'dispatch_failed_or_partial',
  'owner',
  'integration_settings',
  'ha_selected',
  'room_member',
  'outside_ha_selection',
  'outside_control_scope',
  'see_per_light_readback',
  'reported',
  'not_recorded',
  'other_device',
  'hue',
  'ha',
  'matter',
  'rhythm',
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
