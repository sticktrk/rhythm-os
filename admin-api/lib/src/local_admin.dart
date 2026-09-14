import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:http/http.dart' as http;
import 'package:shelf/shelf.dart';

import 'device_json.dart';
import 'models.dart';

typedef HaUserLookup = Future<Map<String, dynamic>?> Function(String userId);

/// The service token authenticates the backend; this lookup authorizes the
/// human identified by the trusted Ingress gateway. Never accept a browser role.
class HomeAssistantUsers {
  HomeAssistantUsers(this.token,
      {this.websocketUrl = 'ws://supervisor/core/websocket'});
  final String token;
  final String websocketUrl;

  Future<Map<String, dynamic>?> lookup(String userId) async {
    final socket = await WebSocket.connect(websocketUrl,
            headers: {'Authorization': 'Bearer $token'})
        .timeout(const Duration(seconds: 8));
    try {
      await for (final frame in socket.timeout(const Duration(seconds: 8))) {
        if (frame is! String || frame.length > 1024 * 1024) {
          throw const AdminApiException(
              502, 'Invalid Home Assistant identity response.');
        }
        final message = jsonDecode(frame) as Map<String, dynamic>;
        switch (message['type']) {
          case 'auth_required':
            socket.add(jsonEncode({'type': 'auth', 'access_token': token}));
          case 'auth_ok':
            socket.add(jsonEncode({'id': 1, 'type': 'config/auth/list'}));
          case 'auth_invalid':
            throw const AdminApiException(
                503, 'Home Assistant identity service is unavailable.');
          case 'result':
            if (message['id'] != 1) continue;
            if (message['success'] != true || message['result'] is! List) {
              throw const AdminApiException(
                  503, 'Home Assistant could not verify access.');
            }
            for (final user in message['result'] as List) {
              if (user is Map && user['id'] == userId) {
                return Map<String, dynamic>.from(user);
              }
            }
            return null;
        }
      }
      throw const AdminApiException(
          503, 'Home Assistant identity connection closed.');
    } finally {
      await socket.close();
    }
  }
}

/// Fixed-target local transport. Both compositions use the same request DTO,
/// mutation guards and canonical hash. The Rust add-on owns operation admission.
class LocalDeviceProxy {
  LocalDeviceProxy({required this.token, http.Client? client, Uri? baseUri})
      : client = client ?? http.Client(),
        baseUri = baseUri ?? Uri.parse('http://127.0.0.1:54448/');

  final String token;
  final http.Client client;
  final Uri baseUri;

  Future<Map<String, dynamic>> run(DeviceAdminProxyRequestDto request) async {
    request.validateMutationGuards();
    // DTO path validation is shared with staff mode. Encoded paths are rejected
    // here as well as in Rust so URL normalization cannot bypass admission.
    if (request.path.contains('%') || request.path.split('/').contains('.')) {
      throw const AdminApiException(
          400, 'Encoded or ambiguous API paths are not accepted.');
    }
    String? identity;
    String? resourceHash;
    if (request.isMutation) {
      final live = await _send('GET', 'api/state');
      _requireSuccess(live);
      identity = cleanString((live['body'] as Map?)?['server_instance_id']);
      if (identity == null || identity != request.expectedServerInstanceId) {
        throw const AdminApiException(
            409, 'The server identity changed. Refresh before retrying.');
      }
      final precondition = request.resourcePrecondition;
      if (precondition != null) {
        final resource = await _send('GET', precondition.path,
            query: precondition.queryParameters);
        _requireSuccess(resource);
        resourceHash = await canonicalJsonSha256(resource['body']);
        if (resourceHash != precondition.bodySha256) {
          throw const AdminApiException(
              409, 'The configuration changed. Review the current values.');
        }
      }
    }
    final response = await _send(request.method, request.path,
        query: request.queryParameters,
        body: request.body,
        headers: {
          if (request.requestId != null) 'X-Request-Id': request.requestId!,
          if (identity != null) 'X-Expected-Server-Instance-Id': identity,
          if (resourceHash != null) 'X-Expected-Resource-Sha256': resourceHash,
        },
        timeout: request.timeout);
    return {
      ...response,
      'route': 'local',
      'method': request.method,
      'path': request.path,
      'completedAt': DateTime.now().toUtc().toIso8601String(),
      if (request.requestId != null) 'requestId': request.requestId,
      if (identity != null) 'verifiedServerInstanceId': identity,
      if (resourceHash != null) 'preconditionBodySha256': resourceHash,
      'bodySha256': await canonicalJsonSha256(response['body']),
    };
  }

  Future<Map<String, dynamic>> status() async {
    final result = await _send('GET', 'api/addon/status');
    _requireSuccess(result);
    return Map<String, dynamic>.from(result['body'] as Map);
  }

  void _requireSuccess(Map<String, dynamic> response) {
    if ((response['statusCode'] as int) >= 400) {
      throw const AdminApiException(
          503, 'The local Rhythm service is unavailable.');
    }
  }

  Future<Map<String, dynamic>> _send(
    String method,
    String path, {
    Map<String, String> query = const {},
    Object? body,
    Map<String, String> headers = const {},
    Duration timeout = const Duration(seconds: 15),
  }) async {
    final uri = baseUri
        .resolve(path)
        .replace(queryParameters: query.isEmpty ? null : query);
    if (uri.origin != baseUri.origin) {
      throw const AdminApiException(
          400, 'Only the local Rhythm service is available.');
    }
    final request = http.Request(method, uri)
      ..followRedirects = false
      ..headers.addAll({
        'Authorization': 'Bearer $token',
        'Accept': 'application/json',
        ...headers
      });
    if (body != null && method != 'GET') {
      request.headers['Content-Type'] = 'application/json';
      request.body = jsonEncode(body);
    }
    // No retries after dispatch: a lost response does not prove a write failed.
    return (() async {
      final response = await client.send(request);
      final bytes = <int>[];
      await for (final chunk in response.stream) {
        if (bytes.length + chunk.length > 4 * 1024 * 1024) {
          throw const AdminApiException(
              502, 'Local response exceeds the supported size.');
        }
        bytes.addAll(chunk);
      }
      final Object? decoded =
          bytes.isEmpty ? null : jsonDecode(utf8.decode(bytes));
      return <String, dynamic>{
        'statusCode': response.statusCode,
        'body': decoded
      };
    })()
        .timeout(timeout);
  }
}

class LocalAdminServer {
  LocalAdminServer(
      {required this.proxy,
      required this.lookupUser,
      bool Function(Request)? trustedPeer})
      : trustedPeer = trustedPeer ?? _loopbackPeer;
  final LocalDeviceProxy proxy;
  final HaUserLookup lookupUser;
  final bool Function(Request) trustedPeer;
  final _users = <String, ({DateTime expires, Map<String, dynamic> user})>{};

  static bool _loopbackPeer(Request request) {
    final info = request.context['shelf.io.connection_info'];
    return info is HttpConnectionInfo && info.remoteAddress.isLoopback;
  }

  Handler get handler => _handle;

  Future<Response> _handle(Request request) async {
    try {
      if (!trustedPeer(request)) {
        throw const AdminApiException(
            403, 'Use Home Assistant to open Rhythm.');
      }
      if (request.url.path == 'health' && request.method == 'GET') {
        await proxy.status();
        return _json(
            200, {'status': 'ok', 'deployment': 'home_assistant_addon'});
      }
      if (request.method != 'GET' && request.method != 'POST') {
        throw const AdminApiException(405, 'Method not allowed.');
      }
      DeviceAdminProxyRequestDto? operation;
      if (request.url.path == 'api/local/device-admin/proxy' &&
          request.method == 'POST') {
        _checkBrowserOrigin(request);
        operation = DeviceAdminProxyRequestDto.fromJson(await _body(request));
      } else if (request.url.path != 'api/session' || request.method != 'GET') {
        throw const AdminApiException(404, 'Local admin endpoint not found.');
      }
      final user =
          await _authorize(request, fresh: operation?.isMutation ?? false);
      if (operation != null) return _json(200, await proxy.run(operation));
      Map<String, dynamic>? status;
      try {
        status = await proxy.status();
      } catch (_) {/* UI can describe startup. */}
      return _json(200, {
        'deployment': 'home_assistant_addon',
        'schemaVersion': 1,
        'user': {
          'id': user['id'],
          'name': user['name'] ?? 'Home Assistant administrator'
        },
        'status': status,
      });
    } on AdminApiException catch (error) {
      return _json(error.statusCode, {'error': error.message});
    } on TimeoutException {
      return _json(504, {
        'error':
            'The service did not respond in time. Refresh before retrying a change.'
      });
    } on FormatException {
      return _json(400, {'error': 'Invalid JSON payload.'});
    } catch (_) {
      return _json(503,
          {'error': 'Rhythm or Home Assistant is temporarily unavailable.'});
    }
  }

  Future<Map<String, dynamic>> _authorize(Request request,
      {required bool fresh}) async {
    final id = request.headers['x-remote-user-id'];
    if (id == null || !RegExp(r'^[a-zA-Z0-9_-]{1,128}$').hasMatch(id)) {
      throw const AdminApiException(
          403, 'Home Assistant user identity is required.');
    }
    final now = DateTime.now().toUtc();
    _users.removeWhere((_, entry) => !entry.expires.isAfter(now));
    Map<String, dynamic>? user = fresh ? null : _users[id]?.user;
    final fetched = user == null;
    if (user == null) {
      _users.remove(id);
      user = await lookupUser(id);
    }
    if (user == null ||
        user['id'] != id ||
        user['is_active'] != true ||
        !(user['is_owner'] == true ||
            (user['group_ids'] is List &&
                (user['group_ids'] as List).contains('system-admin')))) {
      throw const AdminApiException(
          403, 'A Home Assistant administrator account is required.');
    }
    if (_users.length >= 32) _users.clear();
    // Cache reads briefly. Every mutation resolves the current HA role again.
    if (fetched) {
      _users[id] = (expires: now.add(const Duration(seconds: 15)), user: user);
    }
    return user;
  }

  void _checkBrowserOrigin(Request request) {
    final origin = Uri.tryParse(request.headers['origin'] ?? '');
    final host = request.headers['x-forwarded-host'];
    final scheme = request.headers['x-forwarded-proto'];
    if (request.headers['x-rhythm-local-request'] != '1' ||
        origin == null ||
        !origin.hasAuthority ||
        !const {'http', 'https'}.contains(scheme) ||
        origin.origin != '$scheme://$host') {
      throw const AdminApiException(
          403, 'Open Rhythm inside Home Assistant before making changes.');
    }
  }

  Future<Map<String, dynamic>> _body(Request request) async {
    if (!(request.headers['content-type'] ?? '')
        .startsWith('application/json')) {
      throw const AdminApiException(415, 'JSON is required.');
    }
    final bytes = <int>[];
    await for (final chunk
        in request.read().timeout(const Duration(seconds: 15))) {
      if (bytes.length + chunk.length > 1024 * 1024) {
        throw const AdminApiException(
            413, 'Request exceeds the supported size.');
      }
      bytes.addAll(chunk);
    }
    final value = jsonDecode(utf8.decode(bytes));
    if (value is! Map<String, dynamic>) {
      throw const FormatException('Expected object');
    }
    return value;
  }

  Response _json(int status, Object body) =>
      Response(status, body: jsonEncode(body), headers: {
        'Content-Type': 'application/json',
        'Cache-Control': 'no-store',
        'X-Content-Type-Options': 'nosniff'
      });
}
