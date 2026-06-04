import 'package:dio/dio.dart';
import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../backend/auth/supabase_auth_backend.dart';
import '../backend/backend_provider.dart';
import '../config/feature_flags.dart';
import '../models/plan_tier.dart';
import 'account_cloud_sync_service.dart';
import 'entitlements_service.dart';

typedef RemoteAccessApiFactory = RhythmRemoteAccessApi Function({
  required String baseUrl,
  String? authToken,
});

typedef RemoteAccessAuthStatusFactory = Future<RhythmAuthStatus> Function({
  required HubEndpoint endpoint,
  String? authToken,
});

typedef RemoteAccessEndpointHealthFactory = Future<bool> Function({
  required HubEndpoint endpoint,
  String? authToken,
});

class RemoteAccessEnableResult {
  const RemoteAccessEnableResult({
    required this.updatedHub,
    required this.status,
    required this.remoteUrl,
    required this.tunnelId,
    required this.tunnelName,
  });

  final Hub updatedHub;
  final RhythmRemoteAccessStatus status;
  final String remoteUrl;
  final String tunnelId;
  final String tunnelName;
}

class RemoteAccessActivationException implements Exception {
  const RemoteAccessActivationException(this.status);

  final RhythmRemoteAccessStatus status;

  @override
  String toString() {
    return 'Remote access tunnel did not become reachable '
        '(service_running=${status.serviceRunning}, '
        'connector_healthy=${status.connectorHealthy}, '
        'registered_connections=${status.registeredConnections}, '
        'metrics_error=${status.metricsError})';
  }
}

class RemoteAccessService {
  RemoteAccessService._({
    RemoteAccessApiFactory? apiFactory,
    RemoteAccessAuthStatusFactory? authStatusFactory,
    RemoteAccessEndpointHealthFactory? endpointHealthFactory,
    Duration activationPollDelay = const Duration(seconds: 2),
    int activationPollAttempts = 6,
    dynamic Function()? supabaseClientFactory,
  })  : _apiFactory = apiFactory ?? _defaultApiFactory,
        _authStatusFactory = authStatusFactory ?? _defaultAuthStatusFactory,
        _endpointHealthFactory =
            endpointHealthFactory ?? _defaultEndpointHealthFactory,
        _activationPollDelay = activationPollDelay,
        _activationPollAttempts = activationPollAttempts,
        _supabaseClientFactory = supabaseClientFactory;

  static final RemoteAccessService instance = RemoteAccessService._();
  static const _bootstrapFunctionName = 'remote-access-bootstrap';

  @visibleForTesting
  factory RemoteAccessService.testing({
    RemoteAccessApiFactory? apiFactory,
    RemoteAccessAuthStatusFactory? authStatusFactory,
    RemoteAccessEndpointHealthFactory? endpointHealthFactory,
    Duration activationPollDelay = Duration.zero,
    int activationPollAttempts = 1,
    dynamic Function()? supabaseClientFactory,
  }) {
    return RemoteAccessService._(
      apiFactory: apiFactory,
      authStatusFactory: authStatusFactory,
      endpointHealthFactory: endpointHealthFactory,
      activationPollDelay: activationPollDelay,
      activationPollAttempts: activationPollAttempts,
      supabaseClientFactory: supabaseClientFactory,
    );
  }

  final RemoteAccessApiFactory _apiFactory;
  final RemoteAccessAuthStatusFactory _authStatusFactory;
  final RemoteAccessEndpointHealthFactory _endpointHealthFactory;
  final Duration _activationPollDelay;
  final int _activationPollAttempts;
  final dynamic Function()? _supabaseClientFactory;

  bool get isEnabledByFlag => FeatureFlags.remoteAccessTunnel;

  bool get canUseRemoteAccess {
    return FeatureFlags.remoteAccessTunnel &&
        AccountCloudSyncService.instance.canUseSignedInCloudFeatures &&
        EntitlementsService.instance.has(Entitlement.remoteAccess);
  }

  Future<RemoteAccessEnableResult> enableForHub(
    Hub serverHub, {
    Home? home,
  }) async {
    _ensureCanUse(serverHub);
    await _ensureServerApiAuthEnabled(serverHub);

    await AccountCloudSyncService.instance.syncHomeAndServerHubs(
      home: home,
      hubs: [serverHub],
      reason: 'remote_access_enable',
    );

    final client = _supabaseClient();
    final response = await client.functions.invoke(
      _bootstrapFunctionName,
      body: buildBootstrapBody(serverHub: serverHub, home: home),
    );
    final data = Map<String, dynamic>.from(response.data as Map);
    final remoteEndpoint = HubEndpoint.fromJson(
      Map<String, dynamic>.from(data['remote_endpoint'] as Map),
    );
    final hostname = data['hostname']?.toString() ?? remoteEndpoint.host;
    final connectorToken = data['connector_token']?.toString();
    final tunnelId = data['tunnel_id']?.toString() ?? '';
    final tunnelName = data['tunnel_name']?.toString() ?? '';
    final remoteUrl = data['remote_url']?.toString() ?? remoteEndpoint.baseUrl;

    if (connectorToken == null || connectorToken.trim().isEmpty) {
      throw StateError(
          'Remote access bootstrap did not return a connector token.');
    }
    if (tunnelId.isEmpty || tunnelName.isEmpty) {
      throw StateError(
          'Remote access bootstrap did not return tunnel metadata.');
    }

    final api = _apiForEndpoint(serverHub.endpoint, serverHub.token);
    final initialStatus = await api.putConfig(
      hostname: hostname,
      connectorToken: connectorToken,
      tunnelId: tunnelId,
      tunnelName: tunnelName,
    );
    final status = await _waitForActivation(
      api: api,
      remoteEndpoint: remoteEndpoint,
      authToken: serverHub.token,
      initialStatus: initialStatus,
    );

    final updatedHub = serverHub.copyWith(
      remoteEndpoint: remoteEndpoint,
      updatedAt: DateTime.now(),
      pendingSync: true,
    );

    return RemoteAccessEnableResult(
      updatedHub: updatedHub,
      status: status,
      remoteUrl: remoteUrl,
      tunnelId: tunnelId,
      tunnelName: tunnelName,
    );
  }

  Future<Hub> disableForHub(Hub serverHub) async {
    if (!FeatureFlags.remoteAccessTunnel) {
      throw StateError('Remote access is not enabled in this build.');
    }
    if (serverHub.type != HubType.server) {
      throw StateError(
          'Remote access is only supported for Rhythm Server hubs.');
    }

    Object? lastError;
    StackTrace? lastStackTrace;
    for (final endpoint in _disableEndpoints(serverHub)) {
      try {
        await _apiForEndpoint(endpoint, serverHub.token).clearConfig();
        return serverHub.copyWith(
          clearRemoteEndpoint: true,
          updatedAt: DateTime.now(),
          pendingSync: true,
        );
      } catch (error, stackTrace) {
        lastError = error;
        lastStackTrace = stackTrace;
        debugPrint(
          'RemoteAccessService: failed to clear device config via '
          '${endpoint.baseUrl}: $error',
        );
      }
    }

    if (lastError != null && lastStackTrace != null) {
      Error.throwWithStackTrace(lastError, lastStackTrace);
    }
    throw StateError('Remote access has no endpoint to disable.');
  }

  void _ensureCanUse(Hub serverHub) {
    if (!FeatureFlags.remoteAccessTunnel) {
      throw StateError('Remote access is not enabled in this build.');
    }
    if (serverHub.type != HubType.server) {
      throw StateError(
          'Remote access is only supported for Rhythm Server hubs.');
    }
    if (!canUseRemoteAccess) {
      throw StateError(
        'Remote access requires a signed-in account with remote access.',
      );
    }
    if (serverHub.token?.trim().isEmpty != false) {
      throw StateError('Remote access requires a saved Rhythm owner token.');
    }
  }

  Future<void> _ensureServerApiAuthEnabled(Hub serverHub) async {
    final status = await _authStatusFactory(
      endpoint: serverHub.endpoint,
      authToken: serverHub.token,
    );

    if (!status.requiresAuth) {
      throw StateError('Remote access requires API auth to be enabled first.');
    }
  }

  dynamic _supabaseClient() {
    final injectedClient = _supabaseClientFactory?.call();
    if (injectedClient != null) {
      return injectedClient;
    }
    final auth = BackendProvider.instance.auth;
    if (auth is SupabaseAuthBackend) {
      return auth.client;
    }
    throw StateError('Remote access requires a Supabase backend.');
  }

  RhythmRemoteAccessApi _apiForEndpoint(
    HubEndpoint endpoint,
    String? authToken,
  ) {
    return _apiFactory(
      baseUrl: endpoint.baseUrl,
      authToken: authToken,
    );
  }

  @visibleForTesting
  Future<RhythmRemoteAccessStatus> waitForActivationForTesting({
    required RhythmRemoteAccessApi api,
    required HubEndpoint remoteEndpoint,
    required String? authToken,
    required RhythmRemoteAccessStatus initialStatus,
  }) {
    return _waitForActivation(
      api: api,
      remoteEndpoint: remoteEndpoint,
      authToken: authToken,
      initialStatus: initialStatus,
    );
  }

  Future<RhythmRemoteAccessStatus> _waitForActivation({
    required RhythmRemoteAccessApi api,
    required HubEndpoint remoteEndpoint,
    required String? authToken,
    required RhythmRemoteAccessStatus initialStatus,
  }) async {
    var latest = initialStatus;
    final attempts = _activationPollAttempts < 1 ? 1 : _activationPollAttempts;

    for (var attempt = 0; attempt < attempts; attempt += 1) {
      if (attempt > 0) {
        await Future<void>.delayed(_activationPollDelay);
        latest = await api.getStatus();
      }

      final reachable = latest.configured &&
          latest.serviceRunning &&
          await _endpointHealthFactory(
            endpoint: remoteEndpoint,
            authToken: authToken,
          );
      if (reachable) return latest;
    }

    throw RemoteAccessActivationException(latest);
  }

  List<HubEndpoint> _disableEndpoints(Hub serverHub) {
    final remote = serverHub.remoteEndpoint;
    if (remote == null || remote == serverHub.endpoint) {
      return [serverHub.endpoint];
    }
    return [serverHub.endpoint, remote];
  }

  static RhythmRemoteAccessApi _defaultApiFactory({
    required String baseUrl,
    String? authToken,
  }) {
    return RhythmRemoteAccessApi(
      baseUrl: baseUrl,
      authToken: authToken,
    );
  }

  static Future<RhythmAuthStatus> _defaultAuthStatusFactory({
    required HubEndpoint endpoint,
    String? authToken,
  }) {
    return RhythmAuthApi(
      baseUrl: endpoint.baseUrl,
      authToken: authToken,
    ).getStatus();
  }

  static Future<bool> _defaultEndpointHealthFactory({
    required HubEndpoint endpoint,
    String? authToken,
  }) async {
    try {
      final response = await Dio(
        BaseOptions(
          baseUrl: _normalizeBaseUrl(endpoint.baseUrl),
          connectTimeout: const Duration(seconds: 5),
          receiveTimeout: const Duration(seconds: 5),
          headers: authToken?.trim().isNotEmpty == true
              ? {'Authorization': 'Bearer ${authToken!.trim()}'}
              : null,
        ),
      ).get<Map<String, dynamic>>('health');
      return response.statusCode == 200;
    } catch (error) {
      debugPrint(
        'RemoteAccessService: remote endpoint health failed for '
        '${endpoint.baseUrl}: $error',
      );
      return false;
    }
  }

  static Map<String, dynamic> buildBootstrapBody({
    required Hub serverHub,
    Home? home,
  }) {
    return {
      'hub_id': serverHub.id,
      if (home != null) ...{
        'home': AccountCloudSyncService.homeSnapshotPayload(home),
        'server_hub':
            AccountCloudSyncService.serverHubSnapshotPayload(serverHub),
      },
    };
  }

  static String _normalizeBaseUrl(String baseUrl) {
    final trimmed = baseUrl.trim();
    return trimmed.endsWith('/') ? trimmed : '$trimmed/';
  }
}
