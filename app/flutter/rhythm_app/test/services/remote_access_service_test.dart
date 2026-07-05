import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/remote_access_service.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:shared_preferences/shared_preferences.dart';

const _optOutPrefsKey = 'remote_access_opt_out_hub_ids';

void main() {
  group('RemoteAccessService', () {
    setUp(() {
      SharedPreferences.setMockInitialValues(<String, Object>{});
    });

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

    test('bootstrap body includes server instance id when available', () {
      final body = RemoteAccessService.buildBootstrapBody(
        serverHub: _serverHub(),
        serverInstanceId: ' srv-test-instance ',
      );

      expect(body, {
        'hub_id': 'hub-1',
        'server_instance_id': 'srv-test-instance',
      });
    });

    test('teardown body marks the bootstrap function for disable', () {
      final home = Home.create(
        id: 'home-1',
        name: 'Kitchen',
        ownerId: 'anonymous-user',
      );

      final body = RemoteAccessService.buildTeardownBody(
        serverHub: _serverHub(),
        home: home,
      );

      expect(body['hub_id'], 'hub-1');
      expect(body['action'], 'disable');
      expect(body['home'], isA<Map<String, dynamic>>());
      expect(body['server_hub'], isA<Map<String, dynamic>>());
    });

    test('enable provisions the tunnel and grants support access', () async {
      late _FakeRemoteAccessApi remoteApi;
      final grants = <Hub>[];
      final home = Home.create(
        id: 'home-1',
        name: 'Kitchen',
        ownerId: 'anonymous-user',
      );
      final supabase = _FakeSupabaseClient(
        responseData: {
          'remote_endpoint': {
            'host': 'hub.devices.rhythm.lighting',
            'port': 443,
            'useSsl': true,
          },
          'hostname': 'hub.devices.rhythm.lighting',
          'connector_token': 'connector-token',
          'tunnel_id': 'tunnel-id',
          'tunnel_name': 'tunnel-name',
          'remote_url': 'https://hub.devices.rhythm.lighting',
        },
      );
      final service = RemoteAccessService.testing(
        apiFactory: ({required String baseUrl, String? authToken}) {
          remoteApi = _FakeRemoteAccessApi(baseUrl: baseUrl);
          return remoteApi;
        },
        supabaseClientFactory: () => supabase,
        stateLoader: ({required endpoint, String? authToken}) async {
          return RhythmHello.fromJson({
            'server_instance_id': 'srv-test-instance',
          });
        },
        supportGrant: (hub) async {
          grants.add(hub);
        },
        canUseRemoteAccessOverride: true,
      );

      final result = await service.enableForHub(_serverHub(), home: home);

      expect(result.updatedHub.remoteEndpoint?.host,
          'hub.devices.rhythm.lighting');
      expect(result.updatedHub.serverInstanceId, 'srv-test-instance');
      expect(remoteApi.putConfigCalls, 1);
      expect(remoteApi.lastHostname, 'hub.devices.rhythm.lighting');
      expect(remoteApi.lastConnectorToken, 'connector-token');
      expect(supabase.functions.invocations.single.name,
          'remote-access-bootstrap');
      expect(grants, hasLength(1));
      expect(grants.single.remoteEndpoint?.host, 'hub.devices.rhythm.lighting');
      expect(grants.single.token, 'owner-token');
    });

    test('enable waits for the public remote hostname to route', () async {
      final stateLoads = <String>[];
      var remoteRouteAttempts = 0;
      final home = Home.create(
        id: 'home-1',
        name: 'Kitchen',
        ownerId: 'anonymous-user',
      );
      final supabase = _FakeSupabaseClient(
        responseData: {
          'remote_endpoint': {
            'host': 'hub.devices.rhythm.lighting',
            'port': 443,
            'useSsl': true,
          },
          'hostname': 'hub.devices.rhythm.lighting',
          'connector_token': 'connector-token',
          'tunnel_id': 'tunnel-id',
          'tunnel_name': 'tunnel-name',
        },
      );
      final service = RemoteAccessService.testing(
        apiFactory: ({required String baseUrl, String? authToken}) {
          return _FakeRemoteAccessApi(baseUrl: baseUrl);
        },
        supabaseClientFactory: () => supabase,
        stateLoader: ({required endpoint, String? authToken}) async {
          stateLoads.add('${endpoint.baseUrl}|$authToken');
          if (endpoint.useSsl) {
            remoteRouteAttempts += 1;
            if (remoteRouteAttempts == 1) {
              throw const RhythmApiException('remote hostname not ready');
            }
          }
          return RhythmHello.fromJson({
            'server_instance_id': 'srv-test-instance',
          });
        },
        remoteRoutePollAttempts: 2,
        supportGrant: (_) async {},
        canUseRemoteAccessOverride: true,
      );

      final result = await service.enableForHub(_serverHub(), home: home);

      expect(result.updatedHub.remoteEndpoint?.host,
          'hub.devices.rhythm.lighting');
      expect(result.updatedHub.serverInstanceId, 'srv-test-instance');
      expect(stateLoads, [
        'http://192.168.5.123:54448|owner-token',
        'https://hub.devices.rhythm.lighting:443|owner-token',
        'https://hub.devices.rhythm.lighting:443|owner-token',
      ]);
    });

    test('enable fails when the public remote hostname never routes', () async {
      final supabase = _FakeSupabaseClient(
        responseData: {
          'remote_endpoint': {
            'host': 'hub.devices.rhythm.lighting',
            'port': 443,
            'useSsl': true,
          },
          'hostname': 'hub.devices.rhythm.lighting',
          'connector_token': 'connector-token',
          'tunnel_id': 'tunnel-id',
          'tunnel_name': 'tunnel-name',
        },
      );
      final service = RemoteAccessService.testing(
        apiFactory: ({required String baseUrl, String? authToken}) {
          return _FakeRemoteAccessApi(baseUrl: baseUrl);
        },
        supabaseClientFactory: () => supabase,
        stateLoader: ({required endpoint, String? authToken}) async {
          if (endpoint.useSsl) {
            throw const RhythmApiException('remote hostname not ready');
          }
          return RhythmHello.fromJson({
            'server_instance_id': 'srv-test-instance',
          });
        },
        remoteRoutePollAttempts: 2,
        supportGrant: (_) async {},
        canUseRemoteAccessOverride: true,
      );

      await expectLater(
        service.enableForHub(_serverHub()),
        throwsA(isA<RemoteAccessRouteException>()),
      );
    });

    test('required support grant failure surfaces during enable', () async {
      final supabase = _FakeSupabaseClient(
        responseData: {
          'remote_endpoint': {
            'host': 'hub.devices.rhythm.lighting',
            'port': 443,
            'useSsl': true,
          },
          'hostname': 'hub.devices.rhythm.lighting',
          'connector_token': 'connector-token',
          'tunnel_id': 'tunnel-id',
          'tunnel_name': 'tunnel-name',
        },
      );
      final service = RemoteAccessService.testing(
        apiFactory: ({required String baseUrl, String? authToken}) {
          return _FakeRemoteAccessApi(baseUrl: baseUrl);
        },
        supabaseClientFactory: () => supabase,
        stateLoader: ({required endpoint, String? authToken}) async {
          return RhythmHello.fromJson({
            'server_instance_id': 'srv-test-instance',
          });
        },
        supportGrant: (_) async {
          throw StateError('grant failed');
        },
        canUseRemoteAccessOverride: true,
      );

      await expectLater(
        service.enableForHub(
          _serverHub(),
          requireSupportGrant: true,
        ),
        throwsA(isA<StateError>()),
      );
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

    test('owner token claim returns a hub that can be saved', () async {
      late _FakeAuthApi authApi;
      final service = RemoteAccessService.testing(
        authApiFactory: ({required String baseUrl}) {
          authApi = _FakeAuthApi(
            baseUrl: baseUrl,
            claim: const RhythmOwnerClaim(
              tokenId: 'owner-token-id',
              token: 'claimed-owner-token',
            ),
          );
          return authApi;
        },
      );

      final updated = await service.ensureOwnerTokenForHub(
        _serverHub(token: null),
      );

      expect(authApi.baseUrl, 'http://192.168.5.123:54448');
      expect(authApi.statusCalls, 1);
      expect(authApi.claimCalls, 1);
      expect(updated.token, 'claimed-owner-token');
      expect(updated.pendingSync, isTrue);
    });

    test('owner token claim stops when the server is already owner configured',
        () async {
      late _FakeAuthApi authApi;
      final service = RemoteAccessService.testing(
        authApiFactory: ({required String baseUrl}) {
          authApi = _FakeAuthApi(
            baseUrl: baseUrl,
            claim: const RhythmOwnerClaim(
              tokenId: 'owner-token-id',
              token: 'claimed-owner-token',
            ),
            status: const RhythmAuthStatus(
              requiresAuth: true,
              ownerConfigured: true,
              tokenCount: 13,
              claimAvailable: false,
            ),
          );
          return authApi;
        },
      );

      await expectLater(
        service.ensureOwnerTokenForHub(_serverHub(token: null)),
        throwsA(isA<StateError>()),
      );
      expect(authApi.statusCalls, 1);
      expect(authApi.claimCalls, 0);
    });

    test('owner token claim reuses an existing saved token', () async {
      final service = RemoteAccessService.testing(
        authApiFactory: ({required String baseUrl}) {
          throw StateError('claim should not be called');
        },
      );

      final hub = _serverHub(token: 'saved-owner-token');

      expect(await service.ensureOwnerTokenForHub(hub), same(hub));
    });

    test('auto-enable grants support access for an existing remote endpoint',
        () async {
      final home = Home.create(
        id: 'home-1',
        name: 'Kitchen',
        ownerId: 'anonymous-user',
      );
      final grants = <Hub>[];
      final saved = <Hub>[];
      final service = RemoteAccessService.testing(
        authApiFactory: ({required String baseUrl}) {
          throw StateError('claim should not be called');
        },
        supportGrant: (hub) async {
          grants.add(hub);
        },
      );

      await service.autoEnableForHubForTesting(
        home: home,
        serverHub: _serverHub(
          remoteEndpoint: const HubEndpoint(
            host: 'hub.devices.rhythm.lighting',
            port: 443,
            useSsl: true,
          ),
        ),
        saveHub: (hub) async {
          saved.add(hub);
          return true;
        },
      );

      expect(saved, isEmpty);
      expect(grants, hasLength(1));
      expect(grants.single.id, 'hub-1');
      expect(grants.single.token, 'owner-token');
      expect(
        grants.single.remoteEndpoint?.host,
        'hub.devices.rhythm.lighting',
      );
    });

    test('auto-enable skips existing remote access without an owner token',
        () async {
      final home = Home.create(
        id: 'home-1',
        name: 'Kitchen',
        ownerId: 'anonymous-user',
      );
      final grants = <Hub>[];
      final saved = <Hub>[];
      final service = RemoteAccessService.testing(
        authApiFactory: ({required String baseUrl}) {
          throw StateError('claim should not be called');
        },
        supportGrant: (hub) async {
          grants.add(hub);
        },
      );

      await service.autoEnableForHubForTesting(
        home: home,
        serverHub: _serverHub(
          token: null,
          remoteEndpoint: const HubEndpoint(
            host: 'hub.devices.rhythm.lighting',
            port: 443,
            useSsl: true,
          ),
        ),
        saveHub: (hub) async {
          saved.add(hub);
          return true;
        },
      );

      expect(saved, isEmpty);
      expect(grants, isEmpty);
    });

    test('disable falls back to the remote endpoint when LAN is unreachable',
        () async {
      final calls = <String>[];
      final supabase = _FakeSupabaseClient();
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
        supabaseClientFactory: () => supabase,
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
      expect(supabase.functions.invocations, hasLength(1));
      expect(
        supabase.functions.invocations.single.name,
        'remote-access-bootstrap',
      );
      expect(supabase.functions.invocations.single.body, {
        'hub_id': 'hub-1',
        'server_instance_id': 'endpoint:http://192.168.5.123:54448',
        'action': 'disable',
      });
      expect(updated.remoteEndpoint, isNull);
      expect(updated.pendingSync, isTrue);
    });

    test('disable surfaces the endpoint failure when no fallback exists',
        () async {
      final calls = <String>[];
      final supabase = _FakeSupabaseClient();
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
        supabaseClientFactory: () => supabase,
      );

      await expectLater(
        service.disableForHub(_serverHub()),
        throwsA(isA<RhythmApiException>()),
      );
      expect(calls, ['http://192.168.5.123:54448']);
      expect(supabase.functions.invocations, isEmpty);

      final prefs = await SharedPreferences.getInstance();
      expect(prefs.getStringList(_optOutPrefsKey), isNull);
    });

    test('disable records the opt-out even when cloud teardown fails',
        () async {
      final supabase = _FakeSupabaseClient(
        invokeError: const RhythmApiException('cloud teardown unavailable'),
      );
      final service = RemoteAccessService.testing(
        apiFactory: ({required String baseUrl, String? authToken}) {
          return _FakeRemoteAccessApi(baseUrl: baseUrl);
        },
        supabaseClientFactory: () => supabase,
      );

      final updated = await service.disableForHub(_serverHub());

      expect(updated.remoteEndpoint, isNull);
      expect(updated.pendingSync, isTrue);
      expect(supabase.functions.invocations, hasLength(1));

      final prefs = await SharedPreferences.getInstance();
      expect(prefs.getStringList(_optOutPrefsKey), ['hub-1']);
    });

    test('auto-enable is skipped for a hub the user opted out of', () async {
      SharedPreferences.setMockInitialValues(<String, Object>{
        _optOutPrefsKey: <String>['hub-1'],
      });
      final home = Home.create(
        id: 'home-1',
        name: 'Kitchen',
        ownerId: 'anonymous-user',
      );
      final saved = <Hub>[];
      final supabase = _FakeSupabaseClient();
      final service = RemoteAccessService.testing(
        apiFactory: ({required String baseUrl, String? authToken}) {
          throw StateError('device api should not be used');
        },
        authApiFactory: ({required String baseUrl}) {
          throw StateError('owner token claim should not be called');
        },
        supabaseClientFactory: () => supabase,
        supportGrant: (_) async {
          throw StateError('support grant should not be called');
        },
        canUseRemoteAccessOverride: true,
      );

      await service.autoEnableForHubForTesting(
        home: home,
        serverHub: _serverHub(),
        saveHub: (hub) async {
          saved.add(hub);
          return true;
        },
      );

      expect(saved, isEmpty);
      expect(supabase.functions.invocations, isEmpty);
    });

    test('explicit enable clears a previously recorded opt-out', () async {
      SharedPreferences.setMockInitialValues(<String, Object>{
        _optOutPrefsKey: <String>['hub-1', 'hub-2'],
      });
      final home = Home.create(
        id: 'home-1',
        name: 'Kitchen',
        ownerId: 'anonymous-user',
      );
      final supabase = _FakeSupabaseClient(
        responseData: {
          'remote_endpoint': {
            'host': 'hub.devices.rhythm.lighting',
            'port': 443,
            'useSsl': true,
          },
          'hostname': 'hub.devices.rhythm.lighting',
          'connector_token': 'connector-token',
          'tunnel_id': 'tunnel-id',
          'tunnel_name': 'tunnel-name',
        },
      );
      final service = RemoteAccessService.testing(
        apiFactory: ({required String baseUrl, String? authToken}) {
          return _FakeRemoteAccessApi(baseUrl: baseUrl);
        },
        supabaseClientFactory: () => supabase,
        stateLoader: ({required endpoint, String? authToken}) async {
          return RhythmHello.fromJson({
            'server_instance_id': 'srv-test-instance',
          });
        },
        supportGrant: (_) async {},
        canUseRemoteAccessOverride: true,
      );

      final result = await service.enableForHub(_serverHub(), home: home);

      expect(result.updatedHub.remoteEndpoint, isNotNull);
      final prefs = await SharedPreferences.getInstance();
      expect(prefs.getStringList(_optOutPrefsKey), ['hub-2']);
    });
  });
}

Hub _serverHub({
  HubEndpoint? remoteEndpoint,
  String? token = 'owner-token',
}) {
  final now = DateTime.utc(2026, 6, 4);
  return Hub.server(
    id: 'hub-1',
    homeId: 'home-1',
    name: 'Kitchen Server',
    host: '192.168.5.123',
    token: token,
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
  int putConfigCalls = 0;
  String? lastHostname;
  String? lastConnectorToken;
  String? lastTunnelId;
  String? lastTunnelName;

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
  Future<RhythmRemoteAccessStatus> putConfig({
    required String hostname,
    required String connectorToken,
    bool enabled = true,
    String? tunnelId,
    String? tunnelName,
  }) async {
    putConfigCalls += 1;
    lastHostname = hostname;
    lastConnectorToken = connectorToken;
    lastTunnelId = tunnelId;
    lastTunnelName = tunnelName;
    return _remoteStatus(registeredConnections: 1);
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

class _FakeAuthApi extends RhythmAuthApi {
  _FakeAuthApi({
    required this.baseUrl,
    required this.claim,
    this.status = const RhythmAuthStatus(
      requiresAuth: true,
      ownerConfigured: false,
      tokenCount: 0,
      claimAvailable: true,
    ),
  }) : super(baseUrl: 'http://127.0.0.1');

  final String baseUrl;
  final RhythmOwnerClaim claim;
  final RhythmAuthStatus status;
  int statusCalls = 0;
  int claimCalls = 0;

  @override
  Future<RhythmAuthStatus> getStatus() async {
    statusCalls += 1;
    return status;
  }

  @override
  Future<RhythmOwnerClaim> claimOwnerToken({
    String label = 'Rhythm app',
  }) async {
    claimCalls += 1;
    return claim;
  }
}

class _FakeSupabaseClient {
  _FakeSupabaseClient({Object? responseData, Object? invokeError})
      : functions = _FakeFunctions(
          responseData ?? const {'status': 'ok'},
          invokeError,
        );

  final _FakeFunctions functions;
}

class _FakeFunctions {
  _FakeFunctions(this.responseData, [this.invokeError]);

  final Object? responseData;
  final Object? invokeError;
  final invocations = <({String name, Map<String, dynamic> body})>[];

  Future<_FakeFunctionResponse> invoke(
    String name, {
    Object? body,
  }) async {
    invocations.add((
      name: name,
      body: Map<String, dynamic>.from(body as Map),
    ));
    final error = invokeError;
    if (error != null) throw error;
    return _FakeFunctionResponse(responseData);
  }
}

class _FakeFunctionResponse {
  const _FakeFunctionResponse(this.data);

  final Object? data;
}
