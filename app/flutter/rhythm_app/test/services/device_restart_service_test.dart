import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/device_restart_service.dart';

void main() {
  group('DeviceRestartService', () {
    _FakeRestartServer? server;

    tearDown(() async {
      await server?.close();
      server = null;
    });

    test('posts restart endpoint and returns server message', () async {
      server = await _FakeRestartServer.start();
      final service = DeviceRestartService(baseUrl: server!.baseUrl);

      final result = await service.scheduleRestart();

      expect(result.success, isTrue);
      expect(result.message, 'Restart scheduled');
      expect(server!.requests, ['/api/restart']);
    });

    test('reports unsupported endpoint on 404', () async {
      server = await _FakeRestartServer.start(
        restartStatusCode: HttpStatus.notFound,
      );
      final service = DeviceRestartService(baseUrl: server!.baseUrl);

      final result = await service.scheduleRestart();

      expect(result.success, isFalse);
      expect(
        result.message,
        'This server does not support remote restart yet.',
      );
      expect(server!.requests, ['/api/restart']);
    });
  });
}

class _FakeRestartServer {
  _FakeRestartServer._(
    this._server, {
    required this.restartStatusCode,
  });

  final HttpServer _server;
  final int restartStatusCode;
  final List<String> requests = [];

  String get baseUrl => 'http://127.0.0.1:${_server.port}';

  static Future<_FakeRestartServer> start({
    int restartStatusCode = HttpStatus.ok,
  }) async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final fake = _FakeRestartServer._(
      server,
      restartStatusCode: restartStatusCode,
    );
    server.listen(fake._handleRequest);
    return fake;
  }

  Future<void> close() => _server.close(force: true);

  Future<void> _handleRequest(HttpRequest request) async {
    requests.add(request.uri.path);

    if (request.method == 'POST' && request.uri.path == '/api/restart') {
      request.response.statusCode = restartStatusCode;
      if (restartStatusCode == HttpStatus.ok) {
        await _writeJson(
          request.response,
          const {
            'status': 'ok',
            'message': 'Restart scheduled',
          },
        );
      } else {
        await request.response.close();
      }
      return;
    }

    request.response.statusCode = HttpStatus.notFound;
    await request.response.close();
  }

  Future<void> _writeJson(
    HttpResponse response,
    Map<String, dynamic> body,
  ) async {
    response.headers.contentType = ContentType.json;
    response.write(jsonEncode(body));
    await response.close();
  }
}
