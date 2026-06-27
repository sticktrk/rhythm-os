import 'dart:convert';

import 'package:shelf/shelf.dart';
import 'package:shelf_router/shelf_router.dart';

import 'config.dart';
import 'device_probe_service.dart';
import 'models.dart';
import 'supabase_rest_client.dart';
import 'support_service.dart';

class AdminApiServer {
  AdminApiServer({
    required AdminApiConfig config,
    required SupabaseRestClient supabase,
    required SupportService support,
    required DeviceProbeService probes,
  })  : _config = config,
        _supabase = supabase,
        _support = support,
        _probes = probes;

  final AdminApiConfig _config;
  final SupabaseRestClient _supabase;
  final SupportService _support;
  final DeviceProbeService _probes;

  Handler get handler {
    final router = Router()
      ..get('/health', _health)
      ..get('/api/me', _me)
      ..get('/api/support/snapshot', _supportSnapshot)
      ..post('/api/hubs/<hubId>/probe', _probeHub);

    return Pipeline()
        .addMiddleware(logRequests())
        .addMiddleware(_cors())
        .addMiddleware(_errors())
        .addHandler(router.call);
  }

  Response _health(Request request) {
    return _json({
      'ok': true,
      'service': 'rhythm-admin-api',
      'serviceRoleConfigured': _config.hasServiceRoleKey,
    });
  }

  Future<Response> _me(Request request) async {
    final session = await _requireStaff(request);
    return _json({
      'user': session.user.toJson(),
      'staffStatus': session.staff.toJson(),
    });
  }

  Future<Response> _supportSnapshot(Request request) async {
    final session = await _requireStaff(request);
    final snapshot = await _support.loadSnapshot(session);
    return _json(snapshot.toJson());
  }

  Future<Response> _probeHub(Request request, String hubId) async {
    final session = await _requireStaff(request);
    final result = await _probes.probeHub(session: session, hubId: hubId);
    return _json(result.toJson());
  }

  Future<AdminSession> _requireStaff(Request request) async {
    final token = _bearerToken(request);
    if (token == null) {
      throw const AdminApiException(401, 'Missing bearer token.');
    }
    final user = await _supabase.fetchUser(token);
    final rows = await _supabase.select(
      table: 'rhythm_staff',
      select: 'role,enabled',
      accessToken: token,
      filters: {'user_id': 'eq.${user.id}'},
      limit: 1,
    );
    final staff = rows.isEmpty
        ? const StaffStatus.none()
        : StaffStatus(
            role: cleanString(rows.first['role']),
            enabled: rows.first['enabled'] == true,
          );
    if (!staff.isActive) {
      throw const AdminApiException(
        403,
        'An enabled Rhythm staff account is required.',
      );
    }
    return AdminSession(accessToken: token, user: user, staff: staff);
  }

  String? _bearerToken(Request request) {
    final header = request.headers['authorization'];
    if (header == null) return null;
    final parts = header.split(' ');
    if (parts.length != 2 || parts.first.toLowerCase() != 'bearer') {
      return null;
    }
    final token = parts.last.trim();
    return token.isEmpty ? null : token;
  }

  Middleware _errors() {
    return (innerHandler) {
      return (request) async {
        try {
          return await innerHandler(request);
        } on AdminApiException catch (error) {
          return _json({'error': error.message}, status: error.statusCode);
        } catch (error, stackTrace) {
          print('Unhandled admin-api error: $error');
          print(stackTrace);
          return _json(
            {'error': 'Internal admin-api error.'},
            status: 500,
          );
        }
      };
    };
  }

  Middleware _cors() {
    return (innerHandler) {
      return (request) async {
        if (request.method == 'OPTIONS') {
          return _withCors(Response(204), request);
        }
        final response = await innerHandler(request);
        return _withCors(response, request);
      };
    };
  }

  Response _withCors(Response response, Request request) {
    final origin = request.headers['origin'];
    final allowedOrigin = _config.allowedOrigins.contains('*')
        ? '*'
        : _config.allowsOrigin(origin)
            ? origin
            : null;
    if (allowedOrigin == null) return response;
    return response.change(
      headers: {
        'Access-Control-Allow-Origin': allowedOrigin,
        'Access-Control-Allow-Methods': 'GET,POST,OPTIONS',
        'Access-Control-Allow-Headers': 'Authorization,Content-Type',
        'Access-Control-Max-Age': '86400',
        'Vary': 'Origin',
      },
    );
  }
}

Response _json(Object body, {int status = 200}) {
  return Response(
    status,
    body: jsonEncode(body),
    headers: {'Content-Type': 'application/json; charset=utf-8'},
  );
}
