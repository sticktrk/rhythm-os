import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/matter_pairing_api.dart';
import 'package:rhythm_app/services/matter_setup_payload.dart';
import 'package:rhythm_core/rhythm_core.dart';

void main() {
  group('Matter setup payload detection', () {
    test('detects Matter QR payloads', () {
      expect(
        detectMatterSetupPayloadKind('  MT:Y.K908OC16750648G00  '),
        MatterSetupPayloadKind.qr,
      );
    });

    test('detects manual pairing codes with separators', () {
      expect(
        detectMatterSetupPayloadKind('3497-123-4567'),
        MatterSetupPayloadKind.manual,
      );
    });

    test('treats unrelated content as unknown', () {
      expect(
        detectMatterSetupPayloadKind('https://example.com'),
        MatterSetupPayloadKind.unknown,
      );
    });
  });

  group('MatterPairingApi', () {
    _FakeMatterServer? server;
    MatterPairingApi? api;

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
      api = MatterPairingApi(
        endpoint: HubEndpoint(
          host: InternetAddress.loopbackIPv4.address,
          port: server!.port,
        ),
      );

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
      api = MatterPairingApi(
        endpoint: HubEndpoint(
          host: InternetAddress.loopbackIPv4.address,
          port: server!.port,
        ),
      );

      final response = await api!.pairDevice(
        setupPayload: 'MT:Y.K908OC16750648G00',
      );

      expect(response.httpStatus, HttpStatus.internalServerError);
      expect(
        response.error,
        contains('appliance Wi-Fi must already be provisioned on the server'),
      );
    });
  });
}

class _FakeMatterServer {
  _FakeMatterServer._({
    required HttpServer server,
    this.wifiPayload,
    this.pairHandler,
  }) : _server = server;

  final HttpServer _server;
  final Map<String, dynamic>? wifiPayload;
  final Future<void> Function(HttpRequest request)? pairHandler;

  int get port => _server.port;

  static Future<_FakeMatterServer> start({
    Map<String, dynamic>? wifiPayload,
    Future<void> Function(HttpRequest request)? pairHandler,
  }) async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final fake = _FakeMatterServer._(
      server: server,
      wifiPayload: wifiPayload,
      pairHandler: pairHandler,
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
