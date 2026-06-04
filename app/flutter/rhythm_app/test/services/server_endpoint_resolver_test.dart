import 'dart:io';

import 'package:connectivity_plus/connectivity_plus.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/server_endpoint_resolver.dart';
import 'package:rhythm_core/rhythm_core.dart';

void main() {
  group('ServerEndpointResolver', () {
    test('prefers LAN when the LAN endpoint is reachable', () async {
      final hub = _serverHub(
        localPort: 1234,
        remoteHost: 'server.rhythm.lighting',
      );

      final resolved = await ServerEndpointResolver.resolve(
        hub,
        connectivityCheck: _wifi,
        lanReachability: (_, __) async => true,
      );

      expect(resolved.source, ResolvedServerEndpointSource.lan);
      expect(resolved.endpoint, hub.endpoint);
    });

    test('falls back to remote when LAN is not reachable', () async {
      final hub = _serverHub(
        localPort: 1234,
        remoteHost: 'server.rhythm.lighting',
      );

      final resolved = await ServerEndpointResolver.resolve(
        hub,
        connectivityCheck: _wifi,
        lanReachability: (_, __) async => false,
      );

      expect(resolved.source, ResolvedServerEndpointSource.remote);
      expect(resolved.endpoint, hub.remoteEndpoint);
    });

    test('skips the LAN probe on cellular and uses the remote endpoint',
        () async {
      final hub = _serverHub(
        localPort: 1234,
        remoteHost: 'server.rhythm.lighting',
      );
      var probedLan = false;

      final resolved = await ServerEndpointResolver.resolve(
        hub,
        connectivityCheck: () async => const [ConnectivityResult.mobile],
        lanReachability: (_, __) async {
          probedLan = true;
          return true;
        },
      );

      expect(resolved.source, ResolvedServerEndpointSource.remote);
      expect(resolved.endpoint, hub.remoteEndpoint);
      expect(probedLan, isFalse);
    });

    test('uses the real auth status probe for LAN reachability', () async {
      final server = await _FakeAuthStatusServer.start();
      addTearDown(server.close);
      final hub = _serverHub(
        localPort: server.port,
        remoteHost: 'server.rhythm.lighting',
      );

      final resolved = await ServerEndpointResolver.resolve(
        hub,
        connectivityCheck: _wifi,
      );

      expect(resolved.source, ResolvedServerEndpointSource.lan);
      expect(server.requests, ['/api/auth/status']);
    });
  });
}

Future<List<ConnectivityResult>> _wifi() async {
  return const [ConnectivityResult.wifi];
}

Hub _serverHub({
  required int localPort,
  required String remoteHost,
}) {
  final now = DateTime(2026, 1, 1);
  return Hub(
    id: 'hub-1',
    homeId: 'home-1',
    type: HubType.server,
    name: 'RhythmOS',
    endpoint: HubEndpoint(
      host: InternetAddress.loopbackIPv4.address,
      port: localPort,
    ),
    remoteEndpoint: HubEndpoint(
      host: remoteHost,
      port: 443,
      useSsl: true,
    ),
    requiresCredentials: false,
    token: 'owner-token',
    createdAt: now,
    updatedAt: now,
  );
}

class _FakeAuthStatusServer {
  _FakeAuthStatusServer._(this._server);

  final HttpServer _server;
  final List<String> requests = [];

  int get port => _server.port;

  static Future<_FakeAuthStatusServer> start() async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final fake = _FakeAuthStatusServer._(server);
    server.listen(fake._handleRequest);
    return fake;
  }

  Future<void> close() => _server.close(force: true);

  Future<void> _handleRequest(HttpRequest request) async {
    requests.add(request.uri.path);
    if (request.method == 'GET' && request.uri.path == '/api/auth/status') {
      request.response.headers.contentType = ContentType.json;
      request.response.write(
        '{"requires_auth":true,"owner_configured":true,'
        '"token_count":1,"claim_available":false}',
      );
      await request.response.close();
      return;
    }

    request.response.statusCode = HttpStatus.notFound;
    await request.response.close();
  }
}
