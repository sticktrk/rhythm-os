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

    test('getDiagLogs returns null when device is unreachable', () async {
      final api = RhythmDiagnosticsApi(host: '192.0.2.1', port: 1);

      final result = await api.getDiagLogs();
      expect(result, isNull);
    });

    test('submitDebugBundle reports a device-side upload on new firmware',
        () async {
      server = await _FakeDiagnosticsServer.start(supportsDirectUpload: true);
      final api = RhythmDiagnosticsApi(
        host: '127.0.0.1',
        port: server!.port,
      );

      final result = await api.submitDebugBundle(
        uploadUrl: 'https://storage.example.com/signed-upload',
        appLog: 'line one\nline two\n',
        appMetadata: {'app_version': '9.9.9'},
      );

      expect(result.uploadedByDevice, isTrue);
      expect(result.uploadedFileName, 'rhythm-debug-bundle-test.tar.gz');
      expect(result.uploadedSizeBytes, 4321);
      expect(result.legacyBundle, isNull);
      expect(server!.debugBundleBodies, hasLength(1));
      expect(
        server!.debugBundleBodies.single['upload_url'],
        'https://storage.example.com/signed-upload',
      );
      expect(
        server!.debugBundleBodies.single['app_log'],
        'line one\nline two\n',
      );
    });

    test('submitDebugBundle falls back to bundle bytes on older firmware',
        () async {
      server = await _FakeDiagnosticsServer.start();
      final api = RhythmDiagnosticsApi(
        host: '127.0.0.1',
        port: server!.port,
      );

      final result = await api.submitDebugBundle(
        uploadUrl: 'https://storage.example.com/signed-upload',
        appLog: 'line one\n',
      );

      expect(result.uploadedByDevice, isFalse);
      expect(result.legacyBundle, isNotNull);
      expect(result.legacyBundle!.bytes, [1, 2, 3, 4]);
      expect(
        result.legacyBundle!.fileName,
        'rhythm-debug-bundle-test.tar.gz',
      );
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

    test('downloadDebugBundle supports full base URLs', () async {
      server = await _FakeDiagnosticsServer.start();
      final api = RhythmDiagnosticsApi.fromBaseUrl(
        baseUrl: 'http://127.0.0.1:${server!.port}',
      );

      final bundle = await api.downloadDebugBundle();

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

    test('resetWifi returns false when device is unreachable', () async {
      final api = RhythmDiagnosticsApi(host: '192.0.2.1', port: 1);

      final result = await api.resetWifi();
      expect(result, isFalse);
    });

    test('changeWifi puts credentials to wifi endpoint', () async {
      server = await _FakeDiagnosticsServer.start();
      final api = RhythmDiagnosticsApi(
        host: '127.0.0.1',
        port: server!.port,
      );

      final result = await api.changeWifi(
        ssid: 'New Network',
        password: 'new-secret',
      );

      expect(result.accepted, isTrue);
      expect(result.status, 'accepted');
      expect(result.ssid, 'New Network');
      expect(server!.requests, ['/api/wifi']);
      expect(server!.wifiBodies, [
        {'ssid': 'New Network', 'password': 'new-secret'},
      ]);
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

    test('factoryResetDetailed preserves a server safety-barrier error',
        () async {
      server = await _FakeDiagnosticsServer.start(
        factoryResetStatusCode: HttpStatus.badRequest,
        factoryResetBody: {
          'error': 'Hue Bluetooth bulb is offline; keep it powered on nearby',
        },
      );
      final api = RhythmDiagnosticsApi(
        host: '127.0.0.1',
        port: server!.port,
      );

      final result = await api.factoryResetDetailed(platformContext: 'rpiz');

      expect(result.success, isFalse);
      expect(result.httpStatus, HttpStatus.badRequest);
      expect(
        result.error,
        'Hue Bluetooth bulb is offline; keep it powered on nearby',
      );
      expect(server!.requests, ['/api/factory-reset']);
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
    required this.supportsDirectUpload,
    required this.factoryResetStatusCode,
    required this.factoryResetBody,
  });

  final HttpServer _server;
  final int debugBundleStatusCode;
  final Duration debugBundleDelay;

  /// Mimics firmware with device-direct upload: a request body carrying
  /// `upload_url` gets a JSON `{uploaded: true}` reply instead of bundle
  /// bytes. When false the body is ignored, like pre-upload firmware.
  final bool supportsDirectUpload;
  final int factoryResetStatusCode;
  final Map<String, dynamic> factoryResetBody;
  final List<String> requests = [];
  final List<Map<String, dynamic>> wifiBodies = [];
  final List<Map<String, dynamic>> debugBundleBodies = [];

  int get port => _server.port;

  static Future<_FakeDiagnosticsServer> start({
    int debugBundleStatusCode = HttpStatus.ok,
    Duration debugBundleDelay = Duration.zero,
    bool supportsDirectUpload = false,
    int factoryResetStatusCode = HttpStatus.ok,
    Map<String, dynamic> factoryResetBody = const {'ok': true},
  }) async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final fake = _FakeDiagnosticsServer._(
      server,
      debugBundleStatusCode: debugBundleStatusCode,
      debugBundleDelay: debugBundleDelay,
      supportsDirectUpload: supportsDirectUpload,
      factoryResetStatusCode: factoryResetStatusCode,
      factoryResetBody: factoryResetBody,
    );
    server.listen(fake._handleRequest);
    return fake;
  }

  Future<void> close() => _server.close(force: true);

  Future<void> _handleRequest(HttpRequest request) async {
    requests.add(request.uri.path);

    if (request.method == 'POST' && request.uri.path == '/api/factory-reset') {
      request.response.statusCode = factoryResetStatusCode;
      await _writeJson(request.response, factoryResetBody);
      return;
    }

    if (request.method == 'POST' &&
        request.uri.path == '/api/diag/debug-bundle') {
      final rawBody = await utf8.decoder.bind(request).join();
      Map<String, dynamic>? body;
      if (rawBody.trim().isNotEmpty) {
        try {
          body = Map<String, dynamic>.from(jsonDecode(rawBody) as Map);
          debugBundleBodies.add(body);
        } catch (_) {}
      }

      if (debugBundleDelay > Duration.zero) {
        await Future<void>.delayed(debugBundleDelay);
      }

      if (debugBundleStatusCode != HttpStatus.ok) {
        request.response.statusCode = debugBundleStatusCode;
        request.response.write('bundle failed');
        await request.response.close();
        return;
      }

      if (supportsDirectUpload && body?['upload_url'] != null) {
        await _writeJson(request.response, {
          'uploaded': true,
          'file_name': 'rhythm-debug-bundle-test.tar.gz',
          'size_bytes': 4321,
        });
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

    if (request.method == 'PUT' && request.uri.path == '/api/wifi') {
      final body = await utf8.decoder.bind(request).join();
      wifiBodies.add(Map<String, dynamic>.from(jsonDecode(body) as Map));
      await _writeJson(request.response, {
        'status': 'accepted',
        'message': 'Wi-Fi change scheduled',
        'ssid': wifiBodies.last['ssid'],
      });
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
