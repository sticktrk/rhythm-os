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

    test('falls back to version mismatch when update_available is false',
        () async {
      server = await _FakeOtaServer.start(
        initialVersion: '1.0.0',
        latestVersion: '1.1.0',
        scenario: _FakeOtaScenario.idleAfterRestart,
        initialUpdateAvailable: false,
        otaScope: 'component_bundle',
      );
      service = OtaService();

      await service!.initialize(
        host: InternetAddress.loopbackIPv4.address,
        port: server!.port,
      );

      expect(service!.isSelfPull, isTrue);
      expect(service!.state, OtaState.available);
      expect(service!.availableRelease?.version, '1.1.0');
    });

    test('stays up to date when versions match and update_available is false',
        () async {
      server = await _FakeOtaServer.start(
        initialVersion: '1.1.0',
        latestVersion: '1.1.0',
        scenario: _FakeOtaScenario.idleAfterRestart,
        initialUpdateAvailable: false,
        otaScope: 'component_bundle',
      );
      service = OtaService();

      await service!.initialize(
        host: InternetAddress.loopbackIPv4.address,
        port: server!.port,
      );

      expect(service!.isSelfPull, isTrue);
      expect(service!.state, OtaState.upToDate);
      expect(service!.availableRelease, isNull);
    });

    test('stays idle before any self-pull check has run', () async {
      server = await _FakeOtaServer.start(
        initialVersion: '1.0.0',
        latestVersion: '1.1.0',
        scenario: _FakeOtaScenario.idleAfterRestart,
        statusHasPreviousCheck: false,
      );
      service = OtaService();

      await service!.initialize(
        host: InternetAddress.loopbackIPv4.address,
        port: server!.port,
      );

      expect(service!.isSelfPull, isTrue);
      expect(service!.state, OtaState.idle);
      expect(service!.availableRelease, isNull);
    });

    test('surfaces component drift metadata from server bundle checks',
        () async {
      server = await _FakeOtaServer.start(
        initialVersion: '1.1.0',
        latestVersion: '1.1.0',
        scenario: _FakeOtaScenario.idleAfterRestart,
        updateReason: 'component_drift',
        otaScope: 'component_bundle',
        installTargets: const [
          {'name': 'rhythm-server', 'path': '/usr/local/bin/rhythm-server'},
          {'name': 'rhythm-chipd', 'path': '/usr/local/bin/rhythm-chipd'},
        ],
        imageAssets: const [
          {
            'name': 'server.img.zst',
            'url': 'https://example.invalid/server.img.zst',
          },
        ],
      );
      service = OtaService();

      await service!.initialize(
        host: InternetAddress.loopbackIPv4.address,
        port: server!.port,
      );

      expect(service!.isSelfPull, isTrue);
      expect(service!.state, OtaState.available);
      expect(service!.isBundleRepair, isTrue);
      expect(service!.updateReason, OtaUpdateReason.componentDrift);
      expect(service!.capabilities?.supportsRootfsImageOta, isFalse);
      expect(service!.availableRelease?.version, '1.1.0');
      expect(
        service!.installTargets.map((entry) => entry.title).toList(),
        ['rhythm-server', 'rhythm-chipd'],
      );
      expect(
        service!.imageAssets.map((entry) => entry.title).toList(),
        ['server.img.zst'],
      );
    });

    test('accepts v-prefixed versions when status reports the updated version',
        () async {
      server = await _FakeOtaServer.start(
        initialVersion: '1.0.0',
        latestVersion: '1.1.0',
        stateVersionAfterUpdate: 'v1.1.0',
        scenario: _FakeOtaScenario.restartingThenStatusVersion,
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
        description: 'self-pull update to complete from /api/ota/status',
      );

      expect(service!.currentVersion, 'v1.1.0');
    });

    test('captures installed targets for component drift repairs', () async {
      server = await _FakeOtaServer.start(
        initialVersion: '1.1.0',
        latestVersion: '1.1.0',
        scenario: _FakeOtaScenario.idleAfterRestart,
        updateReason: 'component_drift',
        otaScope: 'component_bundle',
        installTargets: const [
          {'name': 'rhythm-chipd', 'path': '/usr/local/bin/rhythm-chipd'},
        ],
        installedTargetsResponse: const [
          'rhythm-chipd',
          {'name': 'rhythm-server', 'path': '/usr/local/bin/rhythm-server'},
        ],
        checksumVerified: true,
      );
      service = OtaService();

      await service!.initialize(
        host: InternetAddress.loopbackIPv4.address,
        port: server!.port,
      );

      expect(service!.state, OtaState.available);
      expect(service!.isBundleRepair, isTrue);

      await service!.startUpdate(
        InternetAddress.loopbackIPv4.address,
        port: server!.port,
      );

      await _waitFor(
        () => service!.state == OtaState.complete,
        description: 'component drift repair to complete',
      );

      expect(service!.currentVersion, '1.1.0');
      expect(service!.statusMessage, 'Bundle repaired on v1.1.0');
      expect(service!.checksumVerified, isTrue);
      expect(
        service!.installedTargets.map((entry) => entry.title).toList(),
        ['rhythm-chipd', 'rhythm-server'],
      );
    });

    test('accepts rootfs_slot appliance capabilities', () async {
      server = await _FakeOtaServer.start(
        initialVersion: '1.0.0',
        latestVersion: '1.1.0',
        scenario: _FakeOtaScenario.idleAfterRestart,
        otaScope: 'rootfs_slot',
        capabilityPayloads: const ['archive_bundle', 'rootfs_image'],
      );
      service = OtaService();

      await service!.initialize(
        host: InternetAddress.loopbackIPv4.address,
        port: server!.port,
      );

      expect(service!.isSelfPull, isTrue);
      expect(service!.capabilities?.supportsRootfsImageOta, isTrue);
      expect(service!.state, OtaState.available);
    });

    test(
        'recovers when the start request times out after the device begins updating',
        () async {
      server = await _FakeOtaServer.start(
        initialVersion: '1.0.0',
        latestVersion: '1.1.0',
        scenario: _FakeOtaScenario.idleAfterRestart,
        startResponseDelay: const Duration(milliseconds: 200),
      );
      service = OtaService(
        startUpdateReceiveTimeout: const Duration(milliseconds: 50),
        startUpdateRecoveryWindow: const Duration(seconds: 1),
        selfPullPollInterval: const Duration(milliseconds: 20),
      );

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
        description:
            'self-pull update to recover from a delayed start response',
        timeout: const Duration(seconds: 3),
      );

      expect(service!.currentVersion, '1.1.0');
      expect(service!.errorMessage, isNull);
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
  restartingThenStatusVersion,
}

class _FakeOtaServer {
  final HttpServer _server;
  final String initialVersion;
  final String latestVersion;
  final String stateVersionAfterUpdate;
  final _FakeOtaScenario scenario;
  final Duration startResponseDelay;
  final bool initialUpdateAvailable;
  final String? updateReason;
  final String otaScope;
  final List<Object?> installTargets;
  final List<Object?> imageAssets;
  final List<Object?> installedTargetsResponse;
  final bool? checksumVerified;
  final List<String> capabilityPayloads;
  final bool? supportsRootfsImage;
  final bool statusHasPreviousCheck;

  bool _updateStarted = false;
  int _statusCallsAfterUpdate = 0;

  _FakeOtaServer._({
    required HttpServer server,
    required this.initialVersion,
    required this.latestVersion,
    required this.stateVersionAfterUpdate,
    required this.scenario,
    required this.startResponseDelay,
    required this.initialUpdateAvailable,
    required this.updateReason,
    required this.otaScope,
    required this.installTargets,
    required this.imageAssets,
    required this.installedTargetsResponse,
    required this.checksumVerified,
    required this.capabilityPayloads,
    required this.supportsRootfsImage,
    required this.statusHasPreviousCheck,
  }) : _server = server;

  int get port => _server.port;

  static Future<_FakeOtaServer> start({
    required String initialVersion,
    required String latestVersion,
    required _FakeOtaScenario scenario,
    String? stateVersionAfterUpdate,
    Duration startResponseDelay = Duration.zero,
    bool initialUpdateAvailable = true,
    String? updateReason,
    String otaScope = 'component_bundle',
    List<Object?> installTargets = const [],
    List<Object?> imageAssets = const [],
    List<Object?> installedTargetsResponse = const [],
    bool? checksumVerified,
    List<String>? capabilityPayloads,
    bool? supportsRootfsImage,
    bool statusHasPreviousCheck = true,
  }) async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final fake = _FakeOtaServer._(
      server: server,
      initialVersion: initialVersion,
      latestVersion: latestVersion,
      stateVersionAfterUpdate: stateVersionAfterUpdate ?? latestVersion,
      scenario: scenario,
      startResponseDelay: startResponseDelay,
      initialUpdateAvailable: initialUpdateAvailable,
      updateReason: updateReason,
      otaScope: otaScope,
      installTargets: installTargets,
      imageAssets: imageAssets,
      installedTargetsResponse: installedTargetsResponse,
      checksumVerified: checksumVerified,
      capabilityPayloads:
          capabilityPayloads ?? _defaultCapabilityPayloadsForScope(otaScope),
      supportsRootfsImage: supportsRootfsImage,
      statusHasPreviousCheck: statusHasPreviousCheck,
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
          'scope': otaScope,
          'can_check': true,
          'can_update': true,
          'can_upload': false,
          'requires_restart': true,
          'rollback': 'unsupported',
          'payloads': capabilityPayloads,
          if (supportsRootfsImage != null)
            'supports_rootfs_image': supportsRootfsImage,
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
        _otaPayload(
          state: 'ready',
          currentVersion: initialVersion,
          latestVersion: latestVersion,
          updateAvailable: initialUpdateAvailable,
        ),
      );
      return;
    }

    if (request.method == 'POST' && path == '/api/ota/update') {
      _updateStarted = true;
      _statusCallsAfterUpdate = 0;
      if (startResponseDelay > Duration.zero) {
        await Future<void>.delayed(startResponseDelay);
      }
      try {
        await _writeJson(
          request.response,
          <String, dynamic>{
            'status': 'ok',
            if (installedTargetsResponse.isNotEmpty)
              'installed_targets': installedTargetsResponse,
            if (checksumVerified != null) 'checksum_verified': checksumVerified,
          },
        );
      } on HttpException {
        // The client may time out and close the connection after the device
        // has already accepted the update request.
      } on SocketException {
        // Ignore disconnects from the timed-out client in this test server.
      }
      return;
    }

    request.response.statusCode = HttpStatus.notFound;
    await request.response.close();
  }

  static List<String> _defaultCapabilityPayloadsForScope(String scope) {
    return switch (scope) {
      'rootfs_slot' => const ['archive_bundle', 'rootfs_image'],
      'component_bundle' => const ['archive_bundle'],
      'bundle' => const ['archive_bundle'],
      'rootfs_image' => const ['rootfs_image'],
      _ => const <String>[],
    };
  }

  Map<String, dynamic> _statusPayload() {
    if (!_updateStarted) {
      if (!statusHasPreviousCheck) {
        return <String, dynamic>{
          'state': 'idle',
          'current_version': initialVersion,
        };
      }

      return _otaPayload(
        state: 'ready',
        currentVersion: initialVersion,
        latestVersion: latestVersion,
        updateAvailable: initialUpdateAvailable,
      );
    }

    _statusCallsAfterUpdate++;

    return switch (scenario) {
      _FakeOtaScenario.idleAfterRestart => _statusCallsAfterUpdate == 1
          ? _otaPayload(
              state: 'restarting',
              currentVersion: initialVersion,
              latestVersion: latestVersion,
              updateAvailable: initialUpdateAvailable,
              includeUpdateReason: false,
            )
          : _otaPayload(
              state: 'idle',
              currentVersion: stateVersionAfterUpdate,
              latestVersion: latestVersion,
              updateAvailable: false,
              includeUpdateReason: false,
            ),
      _FakeOtaScenario.restartingThenStatusVersion =>
        _statusCallsAfterUpdate == 1
            ? _otaPayload(
                state: 'restarting',
                currentVersion: initialVersion,
                latestVersion: latestVersion,
                updateAvailable: initialUpdateAvailable,
                includeUpdateReason: false,
              )
            : _otaPayload(
                state: 'idle',
                currentVersion: stateVersionAfterUpdate,
                latestVersion: latestVersion,
                updateAvailable: false,
                includeUpdateReason: false,
              ),
    };
  }

  Map<String, dynamic> _otaPayload({
    required String state,
    required String currentVersion,
    required String latestVersion,
    required bool updateAvailable,
    bool includeUpdateReason = true,
  }) {
    return <String, dynamic>{
      'state': state,
      'current_version': currentVersion,
      'latest_version': latestVersion,
      if (updateAvailable) 'target_version': latestVersion,
      'update_available': updateAvailable,
      if (includeUpdateReason && updateReason != null) 'update_reason': updateReason,
      if (installTargets.isNotEmpty) 'install_targets': installTargets,
      if (imageAssets.isNotEmpty) 'image_assets': imageAssets,
      if (checksumVerified != null) 'checksum_verified': checksumVerified,
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
