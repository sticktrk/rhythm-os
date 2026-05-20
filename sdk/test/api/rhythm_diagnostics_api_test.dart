import 'dart:convert';
import 'dart:io';

import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  group('RhythmDiagnosticsApi', () {
    _FakeDiagnosticsServer? server;

    tearDown(() async {
      await server?.close();
      server = null;
    });

    test('constructs with host and default port', () {
      final api = RhythmDiagnosticsApi(host: '192.168.1.42');

      expect(api, isNotNull);
    });

    test('constructs with custom port', () {
      final api = RhythmDiagnosticsApi(host: '10.0.0.5', port: 8080);

      expect(api, isNotNull);
    });

    test('healthCheck returns false when device is unreachable', () async {
      final api = RhythmDiagnosticsApi(host: '192.0.2.1', port: 1);

      final result = await api.healthCheck();
      expect(result, isFalse);
    });

    test('getDiagVitals returns null when device is unreachable', () async {
      final api = RhythmDiagnosticsApi(host: '192.0.2.1', port: 1);

      final result = await api.getDiagVitals();
      expect(result, isNull);
    });

    test('getDiagLogs returns null when device is unreachable', () async {
      final api = RhythmDiagnosticsApi(host: '192.0.2.1', port: 1);

      final result = await api.getDiagLogs();
      expect(result, isNull);
    });

    test('downloadDebugBundle posts bundle route and returns attachment',
        () async {
      server = await _FakeDiagnosticsServer.start();
      final api = RhythmDiagnosticsApi(
        host: '127.0.0.1',
        port: server!.port,
      );

      final bundle = await api.downloadDebugBundle();

      expect(bundle.fileName, 'rhythm-debug-bundle-test.tar.gz');
      expect(bundle.contentType, 'application/gzip');
      expect(bundle.bytes, [1, 2, 3, 4]);
      expect(server!.requests, ['/api/diag/debug-bundle']);
    });

    test('downloadDebugBundle uses its extended receive timeout', () async {
      server = await _FakeDiagnosticsServer.start(
        debugBundleDelay: const Duration(milliseconds: 60),
      );
      final api = RhythmDiagnosticsApi(
        host: '127.0.0.1',
        port: server!.port,
        receiveTimeout: const Duration(milliseconds: 10),
        debugBundleReceiveTimeout: const Duration(seconds: 1),
      );

      final bundle = await api.downloadDebugBundle();

      expect(bundle.bytes, [1, 2, 3, 4]);
      expect(server!.requests, ['/api/diag/debug-bundle']);
    });

    test('downloadDebugBundle surfaces server errors', () async {
      server = await _FakeDiagnosticsServer.start(
        debugBundleStatusCode: HttpStatus.internalServerError,
      );
      final api = RhythmDiagnosticsApi(
        host: '127.0.0.1',
        port: server!.port,
      );

      expect(
        () => api.downloadDebugBundle(),
        throwsA(
          isA<RhythmApiException>()
              .having((e) => e.statusCode, 'statusCode', 500)
              .having((e) => e.serverMessage, 'serverMessage', 'bundle failed'),
        ),
      );
    });

    test('clearCrashInfo returns false when device is unreachable', () async {
      final api = RhythmDiagnosticsApi(host: '192.0.2.1', port: 1);

      final result = await api.clearCrashInfo();
      expect(result, isFalse);
    });

    test('resetWifi returns false when device is unreachable', () async {
      final api = RhythmDiagnosticsApi(host: '192.0.2.1', port: 1);

      final result = await api.resetWifi();
      expect(result, isFalse);
    });

    test('factoryReset posts the shared reset endpoint', () async {
      server = await _FakeDiagnosticsServer.start();
      final api = RhythmDiagnosticsApi(
        host: '127.0.0.1',
        port: server!.port,
      );

      final result = await api.factoryReset();

      expect(result, isTrue);
      expect(server!.requests, ['/api/factory-reset']);
    });

    test('factoryReset sends wifi delete after reset for rpiz context',
        () async {
      server = await _FakeDiagnosticsServer.start();
      final api = RhythmDiagnosticsApi(
        host: '127.0.0.1',
        port: server!.port,
      );

      final result = await api.factoryReset(platformContext: 'rpiz');

      expect(result, isTrue);
      expect(
        server!.requests,
        ['/api/factory-reset', '/api/wifi'],
      );
    });

    test('reboot returns false when device is unreachable', () async {
      final api = RhythmDiagnosticsApi(host: '192.0.2.1', port: 1);

      final result = await api.reboot();
      expect(result, isFalse);
    });
  });
}

class _FakeDiagnosticsServer {
  _FakeDiagnosticsServer._(
    this._server, {
    required this.debugBundleStatusCode,
    required this.debugBundleDelay,
  });

  final HttpServer _server;
  final int debugBundleStatusCode;
  final Duration debugBundleDelay;
  final List<String> requests = [];

  int get port => _server.port;

  static Future<_FakeDiagnosticsServer> start({
    int debugBundleStatusCode = HttpStatus.ok,
    Duration debugBundleDelay = Duration.zero,
  }) async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final fake = _FakeDiagnosticsServer._(
      server,
      debugBundleStatusCode: debugBundleStatusCode,
      debugBundleDelay: debugBundleDelay,
    );
    server.listen(fake._handleRequest);
    return fake;
  }

  Future<void> close() => _server.close(force: true);

  Future<void> _handleRequest(HttpRequest request) async {
    requests.add(request.uri.path);

    if (request.method == 'POST' && request.uri.path == '/api/factory-reset') {
      await _writeJson(request.response, {'ok': true});
      return;
    }

    if (request.method == 'POST' &&
        request.uri.path == '/api/diag/debug-bundle') {
      if (debugBundleDelay > Duration.zero) {
        await Future<void>.delayed(debugBundleDelay);
      }

      if (debugBundleStatusCode != HttpStatus.ok) {
        request.response.statusCode = debugBundleStatusCode;
        request.response.write('bundle failed');
        await request.response.close();
        return;
      }

      request.response.headers.contentType = ContentType('application', 'gzip');
      request.response.headers.set(
        'content-disposition',
        'attachment; filename="rhythm-debug-bundle-test.tar.gz"',
      );
      request.response.add([1, 2, 3, 4]);
      await request.response.close();
      return;
    }

    if (request.method == 'DELETE' && request.uri.path == '/api/wifi') {
      await _writeJson(request.response, {'ok': true});
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
