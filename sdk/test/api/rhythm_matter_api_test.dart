import 'dart:convert';
import 'dart:io';

import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  group('RhythmMatterApi', () {
    _FakeMatterServer? server;
    RhythmMatterApi? api;

    tearDown(() async {
      await server?.close();
      server = null;
      api = null;
    });

    test('parses wifi preflight status', () async {
      server = await _FakeMatterServer.start(
        wifiPayload: {
          'config_present': true,
          'connected': true,
        },
      );
      api = RhythmMatterApi(baseUrl: 'http://127.0.0.1:${server!.port}');

      final status = await api!.getWifiStatus();

      expect(status, isNotNull);
      expect(status!.configPresent, isTrue);
      expect(status.connected, isTrue);
      expect(status.readyForMatterPairing, isTrue);
    });

    test('surfaces plain-text pairing failures', () async {
      server = await _FakeMatterServer.start(
        pairHandler: (request) async {
          request.response.statusCode = HttpStatus.internalServerError;
          request.response.headers.contentType = ContentType.text;
          request.response.write(
            'appliance Wi-Fi must already be provisioned on the server',
          );
          await request.response.close();
        },
      );
      api = RhythmMatterApi(baseUrl: 'http://127.0.0.1:${server!.port}');

      final response = await api!.pairDevice(
        setupPayload: 'MT:Y.K908OC16750648G00',
      );

      expect(response.httpStatus, HttpStatus.internalServerError);
      expect(
        response.error,
        contains('appliance Wi-Fi must already be provisioned on the server'),
      );
    });

    test('forwards sessionId at top level and in params', () async {
      Map<String, dynamic>? capturedBody;
      server = await _FakeMatterServer.start(
        pairHandler: (request) async {
          final raw = await utf8.decoder.bind(request).join();
          capturedBody = jsonDecode(raw) as Map<String, dynamic>;
          request.response.headers.contentType = ContentType.json;
          request.response.write(jsonEncode({
            'hub_type': 'matter',
            'status': 'complete',
            'device': {
              'device_id': 'matter-1',
              'name': 'Bulb',
              'device_type': 'light',
            },
          }));
          await request.response.close();
        },
      );
      api = RhythmMatterApi(baseUrl: 'http://127.0.0.1:${server!.port}');

      await api!.pairDevice(
        setupPayload: 'MT:Y.K908OC16750648G00',
        sessionId: 'pair-42',
      );

      expect(capturedBody, isNotNull);
      expect(capturedBody!['hub_type'], 'matter');
      expect(capturedBody!['session_id'], 'pair-42');
      final params = capturedBody!['params'] as Map<String, dynamic>;
      expect(params['session_id'], 'pair-42');
      expect(params['setup_payload'], 'MT:Y.K908OC16750648G00');
      expect(params['network'], 'wifi');
      expect(params['rendezvous'], 'auto');
    });

    test('omits sessionId fields when not provided', () async {
      Map<String, dynamic>? capturedBody;
      server = await _FakeMatterServer.start(
        pairHandler: (request) async {
          final raw = await utf8.decoder.bind(request).join();
          capturedBody = jsonDecode(raw) as Map<String, dynamic>;
          request.response.headers.contentType = ContentType.json;
          request.response.write(jsonEncode({
            'hub_type': 'matter',
            'status': 'complete',
            'device': {
              'device_id': 'matter-2',
              'name': 'Bulb',
              'device_type': 'light',
            },
          }));
          await request.response.close();
        },
      );
      api = RhythmMatterApi(baseUrl: 'http://127.0.0.1:${server!.port}');

      await api!.pairDevice(setupPayload: 'MT:Y.K908OC16750648G00');

      expect(capturedBody, isNotNull);
      expect(capturedBody!.containsKey('session_id'), isFalse);
      final params = capturedBody!['params'] as Map<String, dynamic>;
      expect(params.containsKey('session_id'), isFalse);
    });

    test('reads Matter capture summaries and full capture JSON', () async {
      server = await _FakeMatterServer.start(
        capturesPayload: {
          'captures': [
            {
              'id': 'matter-102',
              'file': 'matter-102.json',
              'source': 'on_demand_probe',
              'captured_at_unix_ms': 200,
              'vendor_name': 'Shenzen',
              'product_name': 'Bulb',
              'vendor_id': 4921,
              'product_id': 171,
              'node_id': 102,
              'light_endpoint': 1,
              'color_modes': ['xy', 'color_temperature'],
              'derived_quirks': ['needs_explicit_on'],
              'db_match_name': 'Known Bulb',
            },
          ],
        },
        capturePayloads: {
          'matter-102': {
            'device_id': 'matter-102',
            'source': 'pair',
            'commissioned': {
              'vendor_id': 4921,
              'product_id': 171,
            },
          },
        },
      );
      api = RhythmMatterApi(baseUrl: 'http://127.0.0.1:${server!.port}');

      final captures = await api!.getMatterCaptures();
      final capture = await api!.getMatterCapture('matter-102');

      expect(captures.captures, hasLength(1));
      expect(captures.captures.single.id, 'matter-102');
      expect(captures.captures.single.vendorId, 4921);
      expect(captures.captures.single.colorModes, [
        'xy',
        'color_temperature',
      ]);
      expect(captures.captures.single.derivedQuirks, ['needs_explicit_on']);
      expect(capture!['device_id'], 'matter-102');
      expect(
        (capture['commissioned'] as Map<String, dynamic>)['vendor_id'],
        4921,
      );
    });
  });
}

class _FakeMatterServer {
  _FakeMatterServer._({
    required HttpServer server,
    this.wifiPayload,
    this.pairHandler,
    this.capturesPayload,
    this.capturePayloads = const <String, Map<String, dynamic>>{},
  }) : _server = server;

  final HttpServer _server;
  final Map<String, dynamic>? wifiPayload;
  final Future<void> Function(HttpRequest request)? pairHandler;
  final Map<String, dynamic>? capturesPayload;
  final Map<String, Map<String, dynamic>> capturePayloads;

  int get port => _server.port;

  static Future<_FakeMatterServer> start({
    Map<String, dynamic>? wifiPayload,
    Future<void> Function(HttpRequest request)? pairHandler,
    Map<String, dynamic>? capturesPayload,
    Map<String, Map<String, dynamic>> capturePayloads =
        const <String, Map<String, dynamic>>{},
  }) async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final fake = _FakeMatterServer._(
      server: server,
      wifiPayload: wifiPayload,
      pairHandler: pairHandler,
      capturesPayload: capturesPayload,
      capturePayloads: capturePayloads,
    );
    server.listen(fake._handleRequest);
    return fake;
  }

  Future<void> close() => _server.close(force: true);

  Future<void> _handleRequest(HttpRequest request) async {
    if (request.method == 'GET' && request.uri.path == '/api/wifi') {
      if (wifiPayload == null) {
        request.response.statusCode = HttpStatus.notFound;
        await request.response.close();
        return;
      }

      await _writeJson(request.response, wifiPayload!);
      return;
    }

    if (request.method == 'POST' && request.uri.path == '/api/devices/pair') {
      if (pairHandler != null) {
        await pairHandler!(request);
        return;
      }

      await _writeJson(request.response, {
        'hub_type': 'matter',
        'status': 'complete',
        'device': {
          'device_id': 'matter-100',
          'name': 'Vendor Product',
          'device_type': 'light',
          'manufacturer': 'Vendor',
          'model': 'Product',
        },
      });
      return;
    }

    if (request.method == 'GET' && request.uri.path == '/api/matter/captures') {
      await _writeJson(
        request.response,
        capturesPayload ?? const {'captures': []},
      );
      return;
    }

    if (request.method == 'GET' &&
        request.uri.pathSegments.length == 4 &&
        request.uri.pathSegments[0] == 'api' &&
        request.uri.pathSegments[1] == 'matter' &&
        request.uri.pathSegments[2] == 'captures') {
      final capture = capturePayloads[request.uri.pathSegments[3]];
      if (capture == null) {
        request.response.statusCode = HttpStatus.notFound;
        await request.response.close();
        return;
      }
      await _writeJson(request.response, capture);
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
