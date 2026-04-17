import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/ota_service.dart';

void main() {
  group('OtaService self-pull updates', () {
    _FakeOtaServer? server;
    OtaService? service;

    tearDown(() async {
      service?.dispose();
      service = null;
      await server?.close();
      server = null;
    });

    test('completes when status returns idle after restart', () async {
      server = await _FakeOtaServer.start(
        initialVersion: '1.0.0',
        latestVersion: '1.1.0',
        scenario: _FakeOtaScenario.idleAfterRestart,
      );
      service = OtaService();

      await service!.initialize(
        host: InternetAddress.loopbackIPv4.address,
        port: server!.port,
      );

      expect(service!.isSelfPull, isTrue);
      expect(service!.state, OtaState.available);

      await service!.startUpdate(
        InternetAddress.loopbackIPv4.address,
        port: server!.port,
      );

      await _waitFor(
        () => service!.state == OtaState.complete,
        description: 'self-pull update to complete after restart',
      );

      expect(service!.currentVersion, '1.1.0');
      expect(service!.statusMessage, 'Updated to v1.1.0');
    });

    test('accepts v-prefixed versions when verifying restart', () async {
      server = await _FakeOtaServer.start(
        initialVersion: '1.0.0',
        latestVersion: '1.1.0',
        stateVersionAfterUpdate: 'v1.1.0',
        scenario: _FakeOtaScenario.restartingThenStateVersion,
      );
      service = OtaService();

      await service!.initialize(
        host: InternetAddress.loopbackIPv4.address,
        port: server!.port,
      );

      expect(service!.isSelfPull, isTrue);
      expect(service!.state, OtaState.available);

      await service!.startUpdate(
        InternetAddress.loopbackIPv4.address,
        port: server!.port,
      );

      await _waitFor(
        () => service!.state == OtaState.complete,
        description: 'self-pull update to complete from /api/state version',
      );

      expect(service!.currentVersion, 'v1.1.0');
    });
  });
}

Future<void> _waitFor(
  bool Function() predicate, {
  required String description,
  Duration timeout = const Duration(seconds: 5),
}) async {
  final deadline = DateTime.now().add(timeout);

  while (DateTime.now().isBefore(deadline)) {
    if (predicate()) return;
    await Future<void>.delayed(const Duration(milliseconds: 50));
  }

  fail('Timed out waiting for $description');
}

enum _FakeOtaScenario {
  idleAfterRestart,
  restartingThenStateVersion,
}

class _FakeOtaServer {
  final HttpServer _server;
  final String initialVersion;
  final String latestVersion;
  final String stateVersionAfterUpdate;
  final _FakeOtaScenario scenario;

  bool _updateStarted = false;
  int _statusCallsAfterUpdate = 0;

  _FakeOtaServer._({
    required HttpServer server,
    required this.initialVersion,
    required this.latestVersion,
    required this.stateVersionAfterUpdate,
    required this.scenario,
  }) : _server = server;

  int get port => _server.port;

  static Future<_FakeOtaServer> start({
    required String initialVersion,
    required String latestVersion,
    required _FakeOtaScenario scenario,
    String? stateVersionAfterUpdate,
  }) async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final fake = _FakeOtaServer._(
      server: server,
      initialVersion: initialVersion,
      latestVersion: latestVersion,
      stateVersionAfterUpdate: stateVersionAfterUpdate ?? initialVersion,
      scenario: scenario,
    );
    server.listen(fake._handleRequest);
    return fake;
  }

  Future<void> close() => _server.close(force: true);

  Future<void> _handleRequest(HttpRequest request) async {
    final path = request.uri.path;

    if (request.method == 'GET' && path == '/api/state') {
      await _writeJson(
        request.response,
        <String, dynamic>{
          'version': _updateStarted ? stateVersionAfterUpdate : initialVersion,
          'platform': 'desktop',
          'context': 'server',
        },
      );
      return;
    }

    if (request.method == 'GET' && path == '/api/ota/capabilities') {
      await _writeJson(
        request.response,
        <String, dynamic>{
          'strategy': 'self_pull',
          'scope': 'binary',
          'can_check': true,
          'can_update': true,
          'can_upload': false,
          'requires_restart': true,
          'rollback': 'unsupported',
        },
      );
      return;
    }

    if (request.method == 'GET' && path == '/api/ota/status') {
      await _writeJson(request.response, _statusPayload());
      return;
    }

    if (request.method == 'GET' && path == '/api/ota/check') {
      await _writeJson(
        request.response,
        <String, dynamic>{
          'current_version': initialVersion,
          'latest_version': latestVersion,
          'update_available': true,
        },
      );
      return;
    }

    if (request.method == 'POST' && path == '/api/ota/update') {
      _updateStarted = true;
      _statusCallsAfterUpdate = 0;
      await _writeJson(request.response, <String, dynamic>{'status': 'ok'});
      return;
    }

    request.response.statusCode = HttpStatus.notFound;
    await request.response.close();
  }

  Map<String, dynamic> _statusPayload() {
    if (!_updateStarted) {
      return <String, dynamic>{
        'state': 'ready',
        'current_version': initialVersion,
        'latest_version': latestVersion,
        'target_version': latestVersion,
        'update_available': true,
      };
    }

    _statusCallsAfterUpdate++;

    return switch (scenario) {
      _FakeOtaScenario.idleAfterRestart => _statusCallsAfterUpdate == 1
          ? <String, dynamic>{
              'state': 'restarting',
              'current_version': initialVersion,
              'latest_version': latestVersion,
              'target_version': latestVersion,
              'update_available': true,
            }
          : <String, dynamic>{
              'state': 'idle',
              'current_version': initialVersion,
              'latest_version': latestVersion,
              'update_available': false,
            },
      _FakeOtaScenario.restartingThenStateVersion => <String, dynamic>{
          'state': 'restarting',
          'current_version': initialVersion,
          'latest_version': latestVersion,
          'target_version': latestVersion,
          'update_available': true,
        },
    };
  }

  Future<void> _writeJson(
    HttpResponse response,
    Map<String, dynamic> body,
  ) async {
    response.statusCode = HttpStatus.ok;
    response.headers.contentType = ContentType.json;
    response.write(jsonEncode(body));
    await response.close();
  }
}
