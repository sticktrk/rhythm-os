import 'dart:convert';
import 'dart:io';
import 'package:dio/dio.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  const session = '12345678-1234-4234-8234-123456789012';
  const device = HaMatterDevice(
      deviceId: 'ha-device',
      identity: 'durable-proof',
      name: 'Test bulb',
      nodeId: '42');
  late HttpServer server;
  late RhythmHaMatterApi api;
  late Future<void> Function(HttpRequest) handler;
  setUp(() async {
    server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    api = RhythmHaMatterApi(Dio(BaseOptions(
        baseUrl: 'http://127.0.0.1:${server.port}/',
        headers: {'Authorization': 'Bearer owner-token'})));
    server.listen((request) => handler(request));
  });
  tearDown(() => server.close(force: true));
  Future<void> reply(HttpRequest request, Object body) async {
    request.response.headers.contentType = ContentType.json;
    request.response.write(jsonEncode(body));
    await request.response.close();
  }

  test('unknown capability schema and omitted fields fail closed', () {
    expect(
        HaMatterCatalog.fromJson({
          'schema_version': 2,
          'available': true,
          'capabilities': {'pair_on_network': true}
        }).pairOnNetwork,
        isFalse);
    expect(
        HaMatterCatalog.fromJson({'schema_version': 1, 'available': true})
            .phoneCommissioning,
        isFalse);
    expect(
        HaMatterCatalog.fromJson({'schema_version': 1, 'available': true})
            .acknowledgePairing,
        isFalse);
    expect(
        HaMatterCatalog.fromJson({
          'schema_version': 1,
          'available': true,
          'capabilities': {'acknowledge_pairing': true}
        }).acknowledgePairing,
        isTrue);
    expect(
        HaMatterCatalog.fromJson({
          'schema_version': 1,
          'available': false,
          'capabilities': {'acknowledge_pairing': true}
        }).acknowledgePairing,
        isTrue);
    expect(
        HaMatterCatalog.fromJson({
          'schema_version': 2,
          'available': false,
          'capabilities': {'acknowledge_pairing': true}
        }).acknowledgePairing,
        isFalse);
    expect(
        RhythmDeploymentCapabilities.fromJson({'ha_device_management': true})
            .haMatterManagement,
        isFalse);
    expect(
        RhythmDeploymentCapabilities.fromJson({'ha_matter_management': true})
            .haMatterManagement,
        isTrue);
  });
  test('pairs using addon and preserves original versus sharing provenance',
      () async {
    for (final source in HaMatterCodeSource.values) {
      handler = (request) async {
        expect(request.uri.path, '/api/addon/matter/pair');
        expect(request.headers.value('authorization'), 'Bearer owner-token');
        final body = jsonDecode(await utf8.decoder.bind(request).join());
        expect(body, {
          'session_id': session,
          'setup_code': 'MT:TEST',
          'code_source': source.wire,
          'rendezvous': 'on_network'
        });
        await reply(request, {
          'session_id': session,
          'status': 'completed',
          'needs_device_confirmation':
              source == HaMatterCodeSource.originalLabel
        });
      };
      final result = await api.pair(
          sessionId: session, setupCode: 'MT:TEST', codeSource: source);
      expect(result.state, HaMatterPairingState.completed);
      expect(result.needsDeviceConfirmation,
          source == HaMatterCodeSource.originalLabel);
    }
  });
  test('pending unknown and future statuses do not imply success', () async {
    for (final status in ['pending', 'unknown', 'future']) {
      handler = (r) => reply(r, {'session_id': session, 'status': status});
      expect((await api.getPairing(session)).unresolved, isTrue);
    }
    handler = (r) =>
        reply(r, {'session_id': 'another-session', 'status': 'completed'});
    await expectLater(api.getPairing(session), throwsFormatException);
  });
  test('explicit device confirmation binds only opaque identity', () async {
    handler = (request) async {
      expect(request.uri.path, '/api/addon/matter/pairing/$session/device');
      expect(jsonDecode(await utf8.decoder.bind(request).join()),
          {'device_id': device.deviceId, 'identity': device.identity});
      await reply(request, {
        'session_id': session,
        'status': 'completed',
        'original_code_saved': true
      });
    };
    expect(
        (await api.confirmDevice(session, device)).originalCodeSaved, isTrue);
  });
  test('acknowledgement uses matching terminal receipt identity', () async {
    handler = (request) async {
      expect(request.method, 'DELETE');
      expect(request.uri.path, '/api/addon/matter/pairing/$session');
      expect(request.headers.value('authorization'), 'Bearer owner-token');
      expect(await utf8.decoder.bind(request).join(), isEmpty);
      await reply(request, {'session_id': session, 'acknowledged': true});
    };
    await api.acknowledgePairing(session);
    for (final response in [
      {'session_id': 'other', 'acknowledged': true},
      {'session_id': session, 'acknowledged': false},
      {'session_id': session}
    ]) {
      handler = (request) => reply(request, response);
      await expectLater(api.acknowledgePairing(session), throwsFormatException);
    }
  });
  test('consumed receipt is explicit and old receipts default unacknowledged',
      () async {
    handler = (request) => reply(request,
        {'session_id': session, 'status': 'failed', 'acknowledged': true});
    expect((await api.getPairing(session)).acknowledged, isTrue);
    handler = (request) =>
        reply(request, {'session_id': session, 'status': 'completed'});
    expect((await api.getPairing(session)).acknowledged, isFalse);
  });
  test('setup-code requires matching identity and original provenance',
      () async {
    for (final bad in [false, true]) {
      handler = (request) async {
        expect(request.uri.queryParameters['identity'], device.identity);
        await reply(request, {
          'device_id': device.deviceId,
          'identity': bad ? 'other' : device.identity,
          'code_source': 'original_label',
          'available': true,
          'setup_code': 'MT:ORIGINAL'
        });
      };
      if (bad) {
        await expectLater(api.getOriginalCode(device), throwsFormatException);
      } else {
        expect(await api.getOriginalCode(device), 'MT:ORIGINAL');
      }
    }
    handler = (r) => reply(r, {
          'device_id': device.deviceId,
          'identity': device.identity,
          'code_source': 'sharing',
          'available': true,
          'setup_code': 'MT:TEMPORARY'
        });
    await expectLater(api.getOriginalCode(device), throwsFormatException);
  });
  test('original save and removal use identity-fenced addon routes', () async {
    handler = (request) async {
      final body = jsonDecode(await utf8.decoder.bind(request).join());
      expect(body['identity'], device.identity);
      if (request.method == 'PUT') {
        expect(body['code_source'], 'original_label');
        expect(body['setup_code'], 'MT:OWNER');
      } else {
        expect(request.method, 'DELETE');
      }
      await reply(request, {'ok': true});
    };
    await api.saveOriginalCode(device, 'MT:OWNER');
    await api.removeDevice(device);
  });
  test('errors never contain server response secrets', () async {
    handler = (r) async {
      r.response.statusCode = 403;
      await reply(r, {'error': 'MT:PRIVATE owner-token'});
    };
    try {
      await api.getCatalog();
      fail('Expected rejection');
    } catch (e) {
      expect(e, isA<HaMatterRequestException>());
      expect(e.toString(), isNot(contains('PRIVATE')));
    }
  });
}
