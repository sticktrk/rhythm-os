import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/ota_service.dart';

void main() {
  group('OtaLastRollback.tryParse', () {
    test('parses a full server payload', () {
      final rollback = OtaLastRollback.tryParse(<String, Object?>{
        'version': '0.4.264-beta',
        'from_version': '0.4.263-beta',
        'at_epoch_ms': 1770000123456,
        'kind': 'image_slot',
      });

      expect(rollback, isNotNull);
      expect(rollback!.version, '0.4.264-beta');
      expect(rollback.fromVersion, '0.4.263-beta');
      expect(rollback.atEpochMs, 1770000123456);
      expect(rollback.kind, 'image_slot');
    });

    test('tolerates missing optional fields', () {
      final rollback = OtaLastRollback.tryParse(<String, Object?>{
        'version': '0.4.264-beta',
        'kind': 'component_bundle',
      });

      expect(rollback, isNotNull);
      expect(rollback!.fromVersion, isNull);
      expect(rollback.atEpochMs, isNull);
    });

    test('rejects null, non-map, and version-less payloads', () {
      expect(OtaLastRollback.tryParse(null), isNull);
      expect(OtaLastRollback.tryParse('rolled back'), isNull);
      expect(OtaLastRollback.tryParse(<String, Object?>{'kind': 'x'}), isNull);
      expect(
        OtaLastRollback.tryParse(<String, Object?>{'version': '  '}),
        isNull,
      );
    });

    test('parses epoch sent as string or double', () {
      expect(
        OtaLastRollback.tryParse(<String, Object?>{
          'version': '1.0.0',
          'at_epoch_ms': '1770000123456',
          'kind': 'component_bundle',
        })?.atEpochMs,
        1770000123456,
      );
      expect(
        OtaLastRollback.tryParse(<String, Object?>{
          'version': '1.0.0',
          'at_epoch_ms': 1770000123456.0,
          'kind': 'component_bundle',
        })?.atEpochMs,
        1770000123456,
      );
    });
  });

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

    test('reports last rollback and flags retry of a rolled-back version',
        () async {
      server = await _FakeOtaServer.start(
        initialVersion: '1.0.0',
        latestVersion: '1.1.0',
        scenario: _FakeOtaScenario.idleAfterRestart,
        lastRollback: <String, Object?>{
          'version': '1.1.0',
          'from_version': '1.0.0',
          'at_epoch_ms': 1770000123456,
          'kind': 'component_bundle',
        },
      );
      service = OtaService();

      await service!.initialize(
        host: InternetAddress.loopbackIPv4.address,
        port: server!.port,
      );

      expect(service!.state, OtaState.available);
      final rollback = service!.lastRollback;
      expect(rollback, isNotNull);
      expect(rollback!.version, '1.1.0');
      expect(rollback.fromVersion, '1.0.0');
      expect(rollback.atEpochMs, 1770000123456);
      expect(rollback.kind, 'component_bundle');
      expect(
        service!.availableUpdateWasRolledBack,
        isTrue,
        reason: 'the offered release is the one that was rolled back',
      );
    });

    test('does not flag rollback when the offered version differs', () async {
      server = await _FakeOtaServer.start(
        initialVersion: '1.0.0',
        latestVersion: '1.2.0',
        scenario: _FakeOtaScenario.idleAfterRestart,
        lastRollback: <String, Object?>{
          'version': '1.1.0',
          'kind': 'image_slot',
        },
      );
      service = OtaService();

      await service!.initialize(
        host: InternetAddress.loopbackIPv4.address,
        port: server!.port,
      );

      expect(service!.lastRollback, isNotNull);
      expect(
        service!.availableUpdateWasRolledBack,
        isFalse,
        reason: 'a newer release than the rolled-back one is on offer',
      );
    });

    test('clears rollback state when the server stops reporting it', () async {
      server = await _FakeOtaServer.start(
        initialVersion: '1.0.0',
        latestVersion: '1.1.0',
        scenario: _FakeOtaScenario.idleAfterRestart,
        lastRollback: <String, Object?>{
          'version': '1.1.0',
          'kind': 'component_bundle',
        },
      );
      service = OtaService();

      await service!.initialize(
        host: InternetAddress.loopbackIPv4.address,
        port: server!.port,
      );
      expect(service!.lastRollback, isNotNull);

      // After an update completes, the server clears the rollback record;
      // post-update status payloads omit `last_rollback` entirely.
      server!.lastRollbackCleared = true;
      await service!.startUpdate(
        InternetAddress.loopbackIPv4.address,
        port: server!.port,
      );
      await _waitFor(
        () => service!.state == OtaState.complete,
        description: 'self-pull update to complete after restart',
      );

      expect(
        service!.lastRollback,
        isNull,
        reason: 'a status payload without last_rollback means none on record',
      );
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

    test('resets restored self-pull check results on initialize when requested',
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
        resetCheckStateOnInitialize: true,
      );

      expect(service!.isSelfPull, isTrue);
      expect(service!.state, OtaState.idle);
      expect(service!.availableRelease, isNull);
      expect(service!.latestVersion, '1.1.0');
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

    test('surfaces image base drift metadata from explicit check response',
        () async {
      server = await _FakeOtaServer.start(
        initialVersion: '1.1.0',
        latestVersion: '1.1.0',
        currentImageVersion: '1.0.8',
        latestImageVersion: '1.0.9',
        scenario: _FakeOtaScenario.idleAfterRestart,
        initialUpdateAvailable: false,
        updateReason: 'image_base_drift',
        otaScope: 'rootfs_slot',
        capabilityPayloads: const ['archive_bundle', 'rootfs_image'],
        statusHasPreviousCheck: false,
        imageAssets: const [
          {
            'name': 'rootfs.ext2.gz',
            'version': '1.0.9',
            'url': 'https://example.invalid/rootfs.ext2.gz',
          },
        ],
      );
      service = OtaService();

      await service!.initialize(
        host: InternetAddress.loopbackIPv4.address,
        port: server!.port,
      );

      expect(service!.state, OtaState.idle);

      await service!.checkForUpdate('1.1.0');

      expect(service!.state, OtaState.available);
      expect(service!.updateReason, OtaUpdateReason.imageBaseDrift);
      expect(service!.currentImageVersion, '1.0.8');
      expect(service!.latestImageVersion, '1.0.9');
      expect(service!.availableRelease?.currentImageVersion, '1.0.8');
      expect(service!.availableRelease?.latestImageVersion, '1.0.9');
      expect(service!.imageAssets.map((entry) => entry.title).toList(), [
        'rootfs.ext2.gz',
      ]);
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

    test('surfaces image base drift as an available self-pull update',
        () async {
      server = await _FakeOtaServer.start(
        initialVersion: '1.1.0',
        latestVersion: '1.1.0',
        currentImageVersion: '1.0.8',
        latestImageVersion: '1.0.9',
        scenario: _FakeOtaScenario.idleAfterRestart,
        initialUpdateAvailable: false,
        updateReason: 'image_base_drift',
        otaScope: 'rootfs_slot',
        capabilityPayloads: const ['archive_bundle', 'rootfs_image'],
        imageAssets: const [
          {
            'name': 'rootfs.ext2.gz',
            'version': '1.0.9',
            'url': 'https://example.invalid/rootfs.ext2.gz',
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
      expect(service!.updateReason, OtaUpdateReason.imageBaseDrift);
      expect(service!.availableRelease?.version, '1.1.0');
      expect(service!.currentPackageVersion, '1.1.0');
      expect(service!.latestPackageVersion, '1.1.0');
      expect(service!.currentImageVersion, '1.0.8');
      expect(service!.latestImageVersion, '1.0.9');
      expect(service!.availableRelease?.currentImageVersion, '1.0.8');
      expect(service!.availableRelease?.latestImageVersion, '1.0.9');
      expect(service!.imageAssets.map((entry) => entry.title).toList(), [
        'rootfs.ext2.gz',
      ]);

      await service!.startUpdate(
        InternetAddress.loopbackIPv4.address,
        port: server!.port,
      );

      await _waitFor(
        () => service!.state == OtaState.complete,
        description: 'image-base drift update to complete',
      );

      expect(service!.currentVersion, '1.1.0');
      expect(service!.currentImageVersion, '1.0.9');
      expect(service!.latestImageVersion, '1.0.9');
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

    test('keeps polling a slow rootfs update within the completion window',
        () async {
      server = await _FakeOtaServer.start(
        initialVersion: '1.0.0',
        latestVersion: '1.1.0',
        scenario: _FakeOtaScenario.idleAfterRestart,
        updatingStatusResponses: 12,
      );
      service = OtaService(
        selfPullPollInterval: const Duration(milliseconds: 20),
        selfPullCompletionTimeout: const Duration(milliseconds: 500),
      );

      await service!.initialize(
        host: InternetAddress.loopbackIPv4.address,
        port: server!.port,
      );

      await service!.startUpdate(
        InternetAddress.loopbackIPv4.address,
        port: server!.port,
      );

      await _waitFor(
        () => service!.state == OtaState.complete,
        description: 'slow rootfs update to finish within its wait budget',
      );

      expect(service!.currentVersion, '1.1.0');
      expect(service!.errorMessage, isNull);
      expect(server!.statusCallsAfterUpdate, greaterThan(12));
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
  final String? currentPackageVersion;
  final String? latestPackageVersion;
  final String? currentImageVersion;
  final String? latestImageVersion;
  final String stateVersionAfterUpdate;
  final _FakeOtaScenario scenario;
  final Duration startResponseDelay;
  final int updatingStatusResponses;
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
  final Map<String, Object?>? lastRollback;

  bool _updateStarted = false;
  int _statusCallsAfterUpdate = 0;

  /// Simulates the server clearing its rollback record (e.g. after a later
  /// update verifies): subsequent payloads omit `last_rollback` entirely.
  bool lastRollbackCleared = false;

  _FakeOtaServer._({
    required HttpServer server,
    required this.initialVersion,
    required this.latestVersion,
    required this.currentPackageVersion,
    required this.latestPackageVersion,
    required this.currentImageVersion,
    required this.latestImageVersion,
    required this.stateVersionAfterUpdate,
    required this.scenario,
    required this.startResponseDelay,
    required this.updatingStatusResponses,
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
    required this.lastRollback,
  }) : _server = server;

  int get port => _server.port;
  int get statusCallsAfterUpdate => _statusCallsAfterUpdate;

  static Future<_FakeOtaServer> start({
    required String initialVersion,
    required String latestVersion,
    String? currentPackageVersion,
    String? latestPackageVersion,
    String? currentImageVersion,
    String? latestImageVersion,
    required _FakeOtaScenario scenario,
    String? stateVersionAfterUpdate,
    Duration startResponseDelay = Duration.zero,
    int updatingStatusResponses = 0,
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
    Map<String, Object?>? lastRollback,
  }) async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final fake = _FakeOtaServer._(
      server: server,
      initialVersion: initialVersion,
      latestVersion: latestVersion,
      currentPackageVersion: currentPackageVersion,
      latestPackageVersion: latestPackageVersion,
      currentImageVersion: currentImageVersion,
      latestImageVersion: latestImageVersion,
      stateVersionAfterUpdate: stateVersionAfterUpdate ?? latestVersion,
      scenario: scenario,
      startResponseDelay: startResponseDelay,
      updatingStatusResponses: updatingStatusResponses,
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
      lastRollback: lastRollback,
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

    if (_statusCallsAfterUpdate <= updatingStatusResponses) {
      return _otaPayload(
        state: 'updating',
        currentVersion: initialVersion,
        latestVersion: latestVersion,
        updateAvailable: initialUpdateAvailable,
      );
    }

    return switch (scenario) {
      _FakeOtaScenario.idleAfterRestart =>
        _statusCallsAfterUpdate == updatingStatusResponses + 1
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
        _statusCallsAfterUpdate == updatingStatusResponses + 1
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
      'current_package_version': currentPackageVersion ?? currentVersion,
      'latest_package_version': latestPackageVersion ?? latestVersion,
      if (currentImageVersion != null)
        'current_image_version': currentImageVersion,
      if (latestImageVersion != null)
        'latest_image_version': latestImageVersion,
      if (updateAvailable) 'target_version': latestVersion,
      'update_available': updateAvailable,
      if (includeUpdateReason && updateReason != null)
        'update_reason': updateReason,
      if (installTargets.isNotEmpty) 'install_targets': installTargets,
      if (imageAssets.isNotEmpty) 'image_assets': imageAssets,
      if (checksumVerified != null) 'checksum_verified': checksumVerified,
      if (lastRollback != null && !lastRollbackCleared)
        'last_rollback': lastRollback,
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
