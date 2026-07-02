import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../backend/auth/supabase_auth_backend.dart';
import '../backend/backend_provider.dart';
import '../config/feature_flags.dart';
import '../models/plan_tier.dart';
import 'account_cloud_sync_service.dart';
import 'entitlements_service.dart';
import 'support_access_service.dart';

typedef RemoteAccessApiFactory = RhythmRemoteAccessApi Function({
  required String baseUrl,
  String? authToken,
});

typedef RemoteAccessAuthApiFactory = RhythmAuthApi Function({
  required String baseUrl,
});

typedef RemoteAccessStateLoader = Future<RhythmHello> Function({
  required HubEndpoint endpoint,
  String? authToken,
});

typedef RemoteAccessHubSaver = Future<bool> Function(Hub hub);
typedef RemoteAccessLatestHubResolver = Hub? Function(
    String homeId, String hubId);
typedef RemoteAccessEnabledCallback = void Function(Hub hub);
typedef RemoteAccessSupportGrant = Future<void> Function(Hub hub);

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
    return 'Remote access tunnel did not finish starting '
        '(configured=${status.configured}, '
        'service_running=${status.serviceRunning}, '
        'connector_healthy=${status.connectorHealthy}, '
        'registered_connections=${status.registeredConnections}, '
        'metrics_error=${status.metricsError})';
  }
}

class RemoteAccessService {
  RemoteAccessService._({
    RemoteAccessApiFactory? apiFactory,
    Duration activationPollDelay = const Duration(seconds: 2),
    int activationPollAttempts = 6,
    dynamic Function()? supabaseClientFactory,
    RemoteAccessStateLoader? stateLoader,
    RemoteAccessAuthApiFactory? authApiFactory,
    RemoteAccessSupportGrant? supportGrant,
    bool? canUseRemoteAccessOverride,
  })  : _apiFactory = apiFactory ?? _defaultApiFactory,
        _activationPollDelay = activationPollDelay,
        _activationPollAttempts = activationPollAttempts,
        _supabaseClientFactory = supabaseClientFactory,
        _stateLoader = stateLoader ?? _defaultStateLoader,
        _authApiFactory = authApiFactory ?? _defaultAuthApiFactory,
        _supportGrant =
            supportGrant ?? SupportAccessService.instance.grantForHub,
        _canUseRemoteAccessOverride = canUseRemoteAccessOverride;

  static final RemoteAccessService instance = RemoteAccessService._();
  static const _bootstrapFunctionName = 'remote-access-bootstrap';

  @visibleForTesting
  factory RemoteAccessService.testing({
    RemoteAccessApiFactory? apiFactory,
    Duration activationPollDelay = Duration.zero,
    int activationPollAttempts = 1,
    dynamic Function()? supabaseClientFactory,
    RemoteAccessStateLoader? stateLoader,
    RemoteAccessAuthApiFactory? authApiFactory,
    RemoteAccessSupportGrant? supportGrant,
    bool? canUseRemoteAccessOverride,
  }) {
    return RemoteAccessService._(
      apiFactory: apiFactory,
      activationPollDelay: activationPollDelay,
      activationPollAttempts: activationPollAttempts,
      supabaseClientFactory: supabaseClientFactory,
      stateLoader: stateLoader ?? _emptyStateLoader,
      authApiFactory: authApiFactory,
      supportGrant: supportGrant,
      canUseRemoteAccessOverride: canUseRemoteAccessOverride,
    );
  }

  final RemoteAccessApiFactory _apiFactory;
  final Duration _activationPollDelay;
  final int _activationPollAttempts;
  final dynamic Function()? _supabaseClientFactory;
  final RemoteAccessStateLoader _stateLoader;
  final RemoteAccessAuthApiFactory _authApiFactory;
  final RemoteAccessSupportGrant _supportGrant;
  final bool? _canUseRemoteAccessOverride;
  final Set<String> _autoEnableInFlight = <String>{};

  bool get isEnabledByFlag => FeatureFlags.remoteAccessTunnel;

  bool get canUseRemoteAccess {
    final override = _canUseRemoteAccessOverride;
    if (override != null) return override;
    return FeatureFlags.remoteAccessTunnel &&
        AccountCloudSyncService.instance.canUseSignedInCloudFeatures &&
        EntitlementsService.instance.has(Entitlement.remoteAccess);
  }

  Future<RemoteAccessEnableResult> enableForHub(
    Hub serverHub, {
    Home? home,
    bool requireSupportGrant = false,
  }) async {
    _ensureCanUse(serverHub);

    await AccountCloudSyncService.instance.syncHomeAndServerHubs(
      home: home,
      hubs: [serverHub],
      reason: 'remote_access_enable',
    );

    final serverInstanceId = await _serverInstanceIdFor(serverHub);
    final client = _supabaseClient();
    final response = await client.functions.invoke(
      _bootstrapFunctionName,
      body: buildBootstrapBody(
        serverHub: serverHub,
        home: home,
        serverInstanceId: serverInstanceId,
      ),
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
      initialStatus: initialStatus,
    );

    final stableServerInstanceId =
        serverInstanceId.startsWith('endpoint:') ? null : serverInstanceId;
    final updatedHub = serverHub.copyWith(
      remoteEndpoint: remoteEndpoint,
      serverInstanceId: stableServerInstanceId,
      updatedAt: DateTime.now(),
      pendingSync: true,
    );
    if (requireSupportGrant) {
      await _supportGrant(updatedHub);
    } else {
      await _grantSupportAccessForHub(updatedHub);
    }

    return RemoteAccessEnableResult(
      updatedHub: updatedHub,
      status: status,
      remoteUrl: remoteUrl,
      tunnelId: tunnelId,
      tunnelName: tunnelName,
    );
  }

  void scheduleAutoEnableForHub({
    required Home home,
    required Hub serverHub,
    required RemoteAccessHubSaver saveHub,
    RemoteAccessLatestHubResolver? resolveLatestHub,
    RemoteAccessEnabledCallback? onEnabled,
  }) {
    try {
      if (!canUseRemoteAccess) return;
    } catch (error) {
      debugPrint(
        'RemoteAccessService: auto-enable unavailable for '
        'hub=${serverHub.id}: $error',
      );
      return;
    }

    final key = '${home.id}:${serverHub.id}';
    if (!_autoEnableInFlight.add(key)) return;

    unawaited(
      _autoEnableForHub(
        home: home,
        serverHub: serverHub,
        saveHub: saveHub,
        resolveLatestHub: resolveLatestHub,
        onEnabled: onEnabled,
      ).whenComplete(() => _autoEnableInFlight.remove(key)),
    );
  }

  @visibleForTesting
  Future<void> autoEnableForHubForTesting({
    required Home home,
    required Hub serverHub,
    required RemoteAccessHubSaver saveHub,
    RemoteAccessLatestHubResolver? resolveLatestHub,
    RemoteAccessEnabledCallback? onEnabled,
  }) {
    return _autoEnableForHub(
      home: home,
      serverHub: serverHub,
      saveHub: saveHub,
      resolveLatestHub: resolveLatestHub,
      onEnabled: onEnabled,
    );
  }

  Future<void> _autoEnableForHub({
    required Home home,
    required Hub serverHub,
    required RemoteAccessHubSaver saveHub,
    RemoteAccessLatestHubResolver? resolveLatestHub,
    RemoteAccessEnabledCallback? onEnabled,
  }) async {
    try {
      var hub = resolveLatestHub?.call(home.id, serverHub.id) ?? serverHub;
      if (hub.remoteEndpoint != null) {
        if (hub.token?.trim().isEmpty != false) {
          debugPrint(
            'RemoteAccessService: support access auto-grant skipped for '
            'hub=${hub.id}: no saved owner token for existing remote access',
          );
          return;
        }
        await _grantSupportAccessForHub(hub);
        return;
      }

      final tokenHub = await ensureOwnerTokenForHub(hub);
      if (tokenHub.token != hub.token) {
        final saved = await saveHub(tokenHub);
        if (!saved) return;
        hub = resolveLatestHub?.call(home.id, serverHub.id) ?? tokenHub;
      }

      final result = await enableForHub(hub, home: home);
      final saved = await saveHub(result.updatedHub);
      if (!saved) return;

      onEnabled?.call(result.updatedHub);
      debugPrint(
        'RemoteAccessService: auto-enabled remote access for '
        'hub=${result.updatedHub.id}',
      );
    } catch (error, stackTrace) {
      debugPrint(
        'RemoteAccessService: auto-enable skipped for hub=${serverHub.id}: '
        '$error',
      );
      debugPrint('$stackTrace');
    }
  }

  Future<void> _grantSupportAccessForHub(Hub hub) async {
    try {
      await _supportGrant(hub);
    } catch (error) {
      debugPrint(
          'RemoteAccessService: support access auto-grant failed: $error');
    }
  }

  Future<Hub> ensureOwnerTokenForHub(Hub serverHub) async {
    if (!FeatureFlags.remoteAccessTunnel) {
      throw StateError('Remote access is not enabled in this build.');
    }
    if (serverHub.type != HubType.server) {
      throw StateError(
          'Remote access is only supported for Rhythm Server hubs.');
    }

    final existingToken = serverHub.token?.trim();
    if (existingToken != null && existingToken.isNotEmpty) {
      return serverHub;
    }

    final authApi = _authApiFactory(
      baseUrl: serverHub.endpoint.baseUrl,
    );
    final status = await authApi.getStatus();
    if (!status.claimAvailable) {
      throw StateError(
        'Remote access has no saved owner token for ${serverHub.id}, and '
        '${serverHub.endpoint.baseUrl} is already owner-configured.',
      );
    }

    final claim = await authApi.claimOwnerToken();
    final token = claim.token.trim();
    if (token.isEmpty) {
      throw StateError('Server returned an empty owner token.');
    }

    return serverHub.copyWith(
      token: token,
      updatedAt: DateTime.now(),
      pendingSync: true,
    );
  }

  Future<Hub> disableForHub(
    Hub serverHub, {
    Home? home,
  }) async {
    if (!FeatureFlags.remoteAccessTunnel) {
      throw StateError('Remote access is not enabled in this build.');
    }
    if (serverHub.type != HubType.server) {
      throw StateError(
          'Remote access is only supported for Rhythm Server hubs.');
    }

    final serverInstanceId = await _serverInstanceIdFor(serverHub);
    Object? lastError;
    StackTrace? lastStackTrace;
    var deviceConfigCleared = false;
    for (final endpoint in _disableEndpoints(serverHub)) {
      try {
        await _apiForEndpoint(endpoint, serverHub.token).clearConfig();
        deviceConfigCleared = true;
        break;
      } catch (error, stackTrace) {
        lastError = error;
        lastStackTrace = stackTrace;
        debugPrint(
          'RemoteAccessService: failed to clear device config via '
          '${endpoint.baseUrl}: $error',
        );
      }
    }

    if (!deviceConfigCleared && lastError != null && lastStackTrace != null) {
      Error.throwWithStackTrace(lastError, lastStackTrace);
    }
    if (!deviceConfigCleared) {
      throw StateError('Remote access has no endpoint to disable.');
    }

    await _tearDownCloudRemoteAccess(
      serverHub,
      home: home,
      serverInstanceId: serverInstanceId,
    );

    final stableServerInstanceId =
        serverInstanceId.startsWith('endpoint:') ? null : serverInstanceId;
    return serverHub.copyWith(
      clearRemoteEndpoint: true,
      serverInstanceId: stableServerInstanceId,
      updatedAt: DateTime.now(),
      pendingSync: true,
    );
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

  Future<void> _tearDownCloudRemoteAccess(
    Hub serverHub, {
    Home? home,
    String? serverInstanceId,
  }) async {
    final client = _supabaseClient();
    await client.functions.invoke(
      _bootstrapFunctionName,
      body: buildTeardownBody(
        serverHub: serverHub,
        home: home,
        serverInstanceId: serverInstanceId,
      ),
    );
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
    required RhythmRemoteAccessStatus initialStatus,
  }) {
    return _waitForActivation(
      api: api,
      initialStatus: initialStatus,
    );
  }

  Future<RhythmRemoteAccessStatus> _waitForActivation({
    required RhythmRemoteAccessApi api,
    required RhythmRemoteAccessStatus initialStatus,
  }) async {
    var latest = initialStatus;
    final attempts = _activationPollAttempts < 1 ? 1 : _activationPollAttempts;

    for (var attempt = 0; attempt < attempts; attempt += 1) {
      if (attempt > 0) {
        await Future<void>.delayed(_activationPollDelay);
        latest = await api.getStatus();
      }

      if (_isActivated(latest)) return latest;
    }

    throw RemoteAccessActivationException(latest);
  }

  bool _isActivated(RhythmRemoteAccessStatus status) {
    return status.configured &&
        status.serviceRunning &&
        status.connectorHealthy;
  }

  List<HubEndpoint> _disableEndpoints(Hub serverHub) {
    final remote = serverHub.remoteEndpoint;
    if (remote == null || remote == serverHub.endpoint) {
      return [serverHub.endpoint];
    }
    return [serverHub.endpoint, remote];
  }

  Future<String> _serverInstanceIdFor(Hub serverHub) async {
    for (final endpoint in _disableEndpoints(serverHub)) {
      try {
        final state = await _stateLoader(
          endpoint: endpoint,
          authToken: serverHub.token,
        );
        final serverInstanceId = state.serverInstanceId?.trim();
        if (serverInstanceId != null && serverInstanceId.isNotEmpty) {
          return serverInstanceId;
        }
      } catch (error) {
        debugPrint(
          'RemoteAccessService: failed to read server identity via '
          '${endpoint.baseUrl}: $error',
        );
      }
    }

    return _fallbackServerInstanceId(serverHub.endpoint);
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

  static RhythmAuthApi _defaultAuthApiFactory({
    required String baseUrl,
  }) {
    return RhythmAuthApi(baseUrl: baseUrl);
  }

  static Future<RhythmHello> _defaultStateLoader({
    required HubEndpoint endpoint,
    String? authToken,
  }) {
    return RhythmConfigApi(
      baseUrl: endpoint.baseUrl,
      authToken: authToken,
    ).getState();
  }

  static Future<RhythmHello> _emptyStateLoader({
    required HubEndpoint endpoint,
    String? authToken,
  }) async {
    return RhythmHello.fromJson(const {});
  }

  static String _fallbackServerInstanceId(HubEndpoint endpoint) {
    return 'endpoint:${endpoint.baseUrl.toLowerCase()}';
  }

  static Map<String, dynamic> buildBootstrapBody({
    required Hub serverHub,
    Home? home,
    String? serverInstanceId,
    bool clearRemoteEndpoint = false,
  }) {
    final normalizedServerInstanceId = serverInstanceId?.trim();
    return {
      'hub_id': serverHub.id,
      if (normalizedServerInstanceId != null &&
          normalizedServerInstanceId.isNotEmpty)
        'server_instance_id': normalizedServerInstanceId,
      if (home != null) ...{
        'home': AccountCloudSyncService.homeSnapshotPayload(home),
        'server_hub': AccountCloudSyncService.serverHubSnapshotPayload(
          serverHub,
          clearRemoteEndpoint: clearRemoteEndpoint,
        ),
      },
    };
  }

  static Map<String, dynamic> buildTeardownBody({
    required Hub serverHub,
    Home? home,
    String? serverInstanceId,
  }) {
    return {
      ...buildBootstrapBody(
        serverHub: serverHub,
        home: home,
        serverInstanceId: serverInstanceId,
        clearRemoteEndpoint: true,
      ),
      'action': 'disable',
    };
  }
}
