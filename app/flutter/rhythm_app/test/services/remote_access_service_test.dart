import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/remote_access_service.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

void main() {
  group('RemoteAccessService', () {
    test('bootstrap body includes local home and server hub repair snapshots',
        () {
      final home = Home.create(
        id: 'home-1',
        name: 'Kitchen',
        ownerId: 'anonymous-user',
      );
      final hub = _serverHub();

      final body = RemoteAccessService.buildBootstrapBody(
        serverHub: hub,
        home: home,
      );
      final homePayload = Map<String, dynamic>.from(body['home'] as Map);
      final hubPayload = Map<String, dynamic>.from(body['server_hub'] as Map);

      expect(body['hub_id'], 'hub-1');
      expect(homePayload['id'], 'home-1');
      expect(hubPayload['id'], 'hub-1');
      expect(hubPayload['home_id'], 'home-1');
      expect(hubPayload['type'], 'server');
      expect(hubPayload['token'], isNull);
    });

    test('bootstrap body falls back to hub id when home is unavailable', () {
      final body = RemoteAccessService.buildBootstrapBody(
        serverHub: _serverHub(),
      );

      expect(body, {'hub_id': 'hub-1'});
    });

    test('activation succeeds when the local connector is healthy', () async {
      final service = RemoteAccessService.testing();
      final api = _FakeRemoteAccessApi(
        baseUrl: 'http://192.168.5.123:54448',
      );

      final status = await service.waitForActivationForTesting(
        api: api,
        initialStatus: _remoteStatus(registeredConnections: 1),
      );

      expect(status.serviceRunning, isTrue);
      expect(status.connectorHealthy, isTrue);
    });

    test('activation waits for the connector to become healthy', () async {
      final service = RemoteAccessService.testing(
        activationPollAttempts: 2,
      );
      final api = _FakeRemoteAccessApi(
        baseUrl: 'http://192.168.5.123:54448',
        statuses: [_remoteStatus(registeredConnections: 1)],
      );

      final status = await service.waitForActivationForTesting(
        api: api,
        initialStatus: _remoteStatus(),
      );

      expect(status.connectorHealthy, isTrue);
      expect(api.statusCalls, 1);
    });

    test('activation fails when the connector never becomes healthy', () async {
      final service = RemoteAccessService.testing(
        activationPollAttempts: 2,
      );
      final api = _FakeRemoteAccessApi(
        baseUrl: 'http://192.168.5.123:54448',
        statuses: [_remoteStatus()],
      );

      await expectLater(
        service.waitForActivationForTesting(
          api: api,
          initialStatus: _remoteStatus(),
        ),
        throwsA(isA<RemoteAccessActivationException>()),
      );
      expect(api.statusCalls, 1);
    });

    test('disable falls back to the remote endpoint when LAN is unreachable',
        () async {
      final calls = <String>[];
      final service = RemoteAccessService.testing(
        apiFactory: ({required String baseUrl, String? authToken}) {
          return _FakeRemoteAccessApi(
            baseUrl: baseUrl,
            onClear: () async {
              calls.add('$baseUrl|$authToken');
              if (baseUrl.startsWith('http://192.168.5.123')) {
                throw const RhythmApiException('LAN unavailable');
              }
            },
          );
        },
      );
      final hub = _serverHub(
        remoteEndpoint: const HubEndpoint(
          host: 'hub.devices.rhythm.lighting',
          port: 443,
          useSsl: true,
        ),
      );

      final updated = await service.disableForHub(hub);

      expect(calls, [
        'http://192.168.5.123:54448|owner-token',
        'https://hub.devices.rhythm.lighting:443|owner-token',
      ]);
      expect(updated.remoteEndpoint, isNull);
      expect(updated.pendingSync, isTrue);
    });

    test('disable surfaces the endpoint failure when no fallback exists',
        () async {
      final calls = <String>[];
      final service = RemoteAccessService.testing(
        apiFactory: ({required String baseUrl, String? authToken}) {
          return _FakeRemoteAccessApi(
            baseUrl: baseUrl,
            onClear: () async {
              calls.add(baseUrl);
              throw const RhythmApiException('device unavailable');
            },
          );
        },
      );

      await expectLater(
        service.disableForHub(_serverHub()),
        throwsA(isA<RhythmApiException>()),
      );
      expect(calls, ['http://192.168.5.123:54448']);
    });
  });
}

Hub _serverHub({HubEndpoint? remoteEndpoint}) {
  final now = DateTime.utc(2026, 6, 4);
  return Hub.server(
    id: 'hub-1',
    homeId: 'home-1',
    name: 'Kitchen Server',
    host: '192.168.5.123',
    token: 'owner-token',
    remoteEndpoint: remoteEndpoint,
  ).copyWith(
    createdAt: now,
    updatedAt: now,
    pendingSync: false,
  );
}

RhythmRemoteAccessStatus _remoteStatus({
  bool serviceRunning = true,
  int? registeredConnections,
}) {
  return RhythmRemoteAccessStatus(
    enabled: true,
    configured: true,
    hostname: 'hub.rhythm.lighting',
    updatedAtEpochMs: 0,
    cloudflaredAvailable: true,
    serviceAvailable: true,
    serviceRunning: serviceRunning,
    restartCount: 0,
    metricsAvailable: true,
    connectorHealthy:
        registeredConnections != null && registeredConnections > 0,
    registeredConnections: registeredConnections,
  );
}

class _FakeRemoteAccessApi extends RhythmRemoteAccessApi {
  _FakeRemoteAccessApi({
    required this.baseUrl,
    this.onClear,
    this.statuses = const [],
  }) : super(baseUrl: 'http://127.0.0.1');

  final String baseUrl;
  final Future<void> Function()? onClear;
  final List<RhythmRemoteAccessStatus> statuses;
  int statusCalls = 0;

  @override
  Future<RhythmRemoteAccessStatus> getStatus() async {
    final index = statusCalls;
    statusCalls += 1;
    if (statuses.isEmpty) return _remoteStatus();
    final resolvedIndex =
        index >= statuses.length ? statuses.length - 1 : index;
    return statuses[resolvedIndex];
  }

  @override
  Future<RhythmRemoteAccessStatus> clearConfig() async {
    await onClear?.call();
    return const RhythmRemoteAccessStatus(
      enabled: false,
      configured: false,
      updatedAtEpochMs: 0,
      cloudflaredAvailable: true,
      serviceAvailable: true,
      serviceRunning: false,
      restartCount: 0,
      metricsAvailable: false,
      connectorHealthy: false,
    );
  }
}
