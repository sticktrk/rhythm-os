import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:shared_preferences/shared_preferences.dart';

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
    required this.routeVerified,
    required this.remoteUrl,
    required this.tunnelId,
    required this.tunnelName,
  });

  final Hub updatedHub;
  final RhythmRemoteAccessStatus status;
  final bool routeVerified;
  final String remoteUrl;
  final String tunnelId;
  final String tunnelName;
}

enum _AutoEnableOutcome { complete, retry }

enum _ExistingRemoteAccessState { healthy, needsRepair, unreachable }

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
        'metrics_error=${status.metricsError}, '
        'service_error=${status.serviceError})';
  }
}

class RemoteAccessRouteException implements Exception {
  const RemoteAccessRouteException({
    required this.endpoint,
    required this.attempts,
    this.cause,
  });

  final HubEndpoint endpoint;
  final int attempts;
  final Object? cause;

  @override
  String toString() {
    return 'Remote access hostname did not become reachable '
        '(endpoint=${endpoint.baseUrl}, attempts=$attempts, cause=$cause)';
  }
}

class RemoteAccessOptedOutException implements Exception {
  const RemoteAccessOptedOutException();

  @override
  String toString() => 'Remote access is disabled for this server';
}

class RemoteAccessService {
  RemoteAccessService._({
    RemoteAccessApiFactory? apiFactory,
    Duration activationPollDelay = const Duration(seconds: 2),
    int activationPollAttempts = 31,
    Duration remoteRoutePollDelay = const Duration(seconds: 5),
    int remoteRoutePollAttempts = 36,
    List<Duration> autoEnableRetryDelays = const <Duration>[
      Duration(seconds: 5),
      Duration(seconds: 15),
      Duration(minutes: 1),
      Duration(minutes: 5),
      Duration(minutes: 15),
    ],
    dynamic Function()? supabaseClientFactory,
    RemoteAccessStateLoader? stateLoader,
    RemoteAccessAuthApiFactory? authApiFactory,
    RemoteAccessSupportGrant? supportGrant,
    Duration supportGrantTimeout = const Duration(seconds: 15),
    bool? canUseRemoteAccessOverride,
  })  : _apiFactory = apiFactory ?? _defaultApiFactory,
        _activationPollDelay = activationPollDelay,
        _activationPollAttempts = activationPollAttempts,
        _remoteRoutePollDelay = remoteRoutePollDelay,
        _remoteRoutePollAttempts = remoteRoutePollAttempts,
        _autoEnableRetryDelays = List<Duration>.unmodifiable(
          autoEnableRetryDelays.isEmpty
              ? const <Duration>[Duration(minutes: 15)]
              : autoEnableRetryDelays,
        ),
        _supabaseClientFactory = supabaseClientFactory,
        _stateLoader = stateLoader ?? _defaultStateLoader,
        _authApiFactory = authApiFactory ?? _defaultAuthApiFactory,
        _supportGrant =
            supportGrant ?? SupportAccessService.instance.grantForHub,
        _supportGrantTimeout = supportGrantTimeout,
        _canUseRemoteAccessOverride = canUseRemoteAccessOverride;

  static final RemoteAccessService instance = RemoteAccessService._();
  static const _bootstrapFunctionName = 'remote-access-bootstrap';
  static const _optOutPrefsKey = 'remote_access_opt_out_hub_ids';
  static const _retryPrefsPrefix = 'remote_access_auto_enable_retry_';

  @visibleForTesting
  factory RemoteAccessService.testing({
    RemoteAccessApiFactory? apiFactory,
    Duration activationPollDelay = Duration.zero,
    int activationPollAttempts = 1,
    Duration remoteRoutePollDelay = Duration.zero,
    int remoteRoutePollAttempts = 1,
    List<Duration> autoEnableRetryDelays = const <Duration>[Duration.zero],
    dynamic Function()? supabaseClientFactory,
    RemoteAccessStateLoader? stateLoader,
    RemoteAccessAuthApiFactory? authApiFactory,
    RemoteAccessSupportGrant? supportGrant,
    Duration supportGrantTimeout = const Duration(milliseconds: 10),
    bool? canUseRemoteAccessOverride,
  }) {
    return RemoteAccessService._(
      apiFactory: apiFactory,
      activationPollDelay: activationPollDelay,
      activationPollAttempts: activationPollAttempts,
      remoteRoutePollDelay: remoteRoutePollDelay,
      remoteRoutePollAttempts: remoteRoutePollAttempts,
      autoEnableRetryDelays: autoEnableRetryDelays,
      supabaseClientFactory: supabaseClientFactory,
      stateLoader: stateLoader ?? _emptyStateLoader,
      authApiFactory: authApiFactory,
      supportGrant: supportGrant,
      supportGrantTimeout: supportGrantTimeout,
      canUseRemoteAccessOverride: canUseRemoteAccessOverride,
    );
  }

  final RemoteAccessApiFactory _apiFactory;
  final Duration _activationPollDelay;
  final int _activationPollAttempts;
  final Duration _remoteRoutePollDelay;
  final int _remoteRoutePollAttempts;
  final List<Duration> _autoEnableRetryDelays;
  final dynamic Function()? _supabaseClientFactory;
  final RemoteAccessStateLoader _stateLoader;
  final RemoteAccessAuthApiFactory _authApiFactory;
  final RemoteAccessSupportGrant _supportGrant;
  final Duration _supportGrantTimeout;
  final bool? _canUseRemoteAccessOverride;
  final Set<String> _autoEnableInFlight = <String>{};
  final Map<String, Timer> _autoEnableRetryTimers = <String, Timer>{};
  final Map<String, int> _autoEnableGenerations = <String, int>{};
  Set<String>? _optOutHubIds;
  Future<Set<String>>? _optOutHubIdsLoad;

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
    bool explicitUserEnable = false,
  }) async {
    _ensureCanUse(serverHub);

    if (explicitUserEnable) {
      // An explicit enable always wins over a previously recorded opt-out.
      await _clearOptOut(serverHub);
    }

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
        explicitUserEnable: explicitUserEnable,
      ),
    );
    final data = Map<String, dynamic>.from(response.data as Map);
    if (data['status'] == 'disabled_by_user') {
      throw const RemoteAccessOptedOutException();
    }
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
    late final RhythmRemoteAccessStatus status;
    String? stableServerInstanceId;
    final initialStatus = await api.putConfig(
      hostname: hostname,
      connectorToken: connectorToken,
      tunnelId: tunnelId,
      tunnelName: tunnelName,
    );
    status = await _waitForActivation(
      api: api,
      initialStatus: initialStatus,
    );

    stableServerInstanceId =
        serverInstanceId.startsWith('endpoint:') ? null : serverInstanceId;
    final provisionedHub = serverHub.copyWith(
      remoteEndpoint: remoteEndpoint,
      serverInstanceId: stableServerInstanceId,
      updatedAt: DateTime.now(),
      pendingSync: true,
    );
    if (!requireSupportGrant) {
      // Grant while the phone still has a known-good LAN path. Waiting for a
      // brand-new Cloudflare route first can delay this by minutes and lets a
      // transient propagation failure strand admin access indefinitely.
      unawaited(_grantSupportAccessForHub(provisionedHub));
    }

    var routeVerified = false;
    try {
      final remoteHello = await _waitForRemoteRoute(
        endpoint: remoteEndpoint,
        authToken: serverHub.token,
        expectedServerInstanceId: stableServerInstanceId,
      );
      routeVerified = true;
      stableServerInstanceId ??= remoteHello.serverInstanceId?.trim();
      if (stableServerInstanceId?.isEmpty ?? false) {
        stableServerInstanceId = null;
      }
    } on RemoteAccessRouteException catch (error) {
      // The connector is already registered with Cloudflare. A phone-side DNS
      // or network failure must not tear down a healthy device tunnel; retain
      // the endpoint and let background reconciliation verify it later.
      debugPrint(
        'RemoteAccessService: tunnel is active but the public route is still '
        'pending for hub=${serverHub.id}: $error',
      );
    }

    final updatedHub = provisionedHub.copyWith(
      serverInstanceId: stableServerInstanceId,
      updatedAt: DateTime.now(),
      pendingSync: true,
    );
    if (requireSupportGrant) {
      await _supportGrant(updatedHub).timeout(_supportGrantTimeout);
    }

    return RemoteAccessEnableResult(
      updatedHub: updatedHub,
      status: status,
      routeVerified: routeVerified,
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

    final key = _autoEnableKey(home.id, serverHub);
    if (!_autoEnableInFlight.add(key)) {
      debugPrint(
        'RemoteAccessService: remote access attempt already in flight for '
        'hub=${serverHub.id}',
      );
      return;
    }

    _autoEnableRetryTimers.remove(key)?.cancel();
    final generation = _autoEnableGenerations[key] ?? 0;

    unawaited(
      _runScheduledAutoEnable(
        key: key,
        generation: generation,
        home: home,
        serverHub: serverHub,
        saveHub: saveHub,
        resolveLatestHub: resolveLatestHub,
        onEnabled: onEnabled,
      ),
    );
  }

  Future<void> _runScheduledAutoEnable({
    required String key,
    required int generation,
    required Home home,
    required Hub serverHub,
    required RemoteAccessHubSaver saveHub,
    RemoteAccessLatestHubResolver? resolveLatestHub,
    RemoteAccessEnabledCallback? onEnabled,
  }) async {
    late final _AutoEnableOutcome outcome;
    try {
      outcome = await _autoEnableForHub(
        home: home,
        serverHub: serverHub,
        saveHub: saveHub,
        resolveLatestHub: resolveLatestHub,
        onEnabled: onEnabled,
        operationIsCurrent: () =>
            (_autoEnableGenerations[key] ?? 0) == generation,
      );
    } finally {
      _autoEnableInFlight.remove(key);
    }

    if ((_autoEnableGenerations[key] ?? 0) != generation) {
      await _clearAutoEnableRetry(key);
      return;
    }
    if (outcome == _AutoEnableOutcome.retry) {
      await _scheduleAutoEnableRetry(
        key: key,
        home: home,
        serverHub: serverHub,
        saveHub: saveHub,
        resolveLatestHub: resolveLatestHub,
        onEnabled: onEnabled,
      );
    } else {
      await _clearAutoEnableRetry(key);
    }
  }

  Future<void> _scheduleAutoEnableRetry({
    required String key,
    required Home home,
    required Hub serverHub,
    required RemoteAccessHubSaver saveHub,
    RemoteAccessLatestHubResolver? resolveLatestHub,
    RemoteAccessEnabledCallback? onEnabled,
  }) async {
    final prefsKey = '$_retryPrefsPrefix${Uri.encodeComponent(key)}';
    var attempt = 0;
    try {
      final prefs = await SharedPreferences.getInstance();
      attempt = prefs.getInt(prefsKey) ?? 0;
      await prefs.setInt(prefsKey, attempt + 1);
    } catch (error) {
      debugPrint(
        'RemoteAccessService: failed to persist retry state for hub='
        '${serverHub.id}: $error',
      );
    }

    final delayIndex = attempt < _autoEnableRetryDelays.length
        ? attempt
        : _autoEnableRetryDelays.length - 1;
    final delay = _autoEnableRetryDelays[delayIndex];
    _autoEnableRetryTimers.remove(key)?.cancel();
    _autoEnableRetryTimers[key] = Timer(delay, () {
      _autoEnableRetryTimers.remove(key);
      debugPrint(
        'RemoteAccessService: running remote access retry for '
        'hub=${serverHub.id}',
      );
      scheduleAutoEnableForHub(
        home: home,
        serverHub: serverHub,
        saveHub: saveHub,
        resolveLatestHub: resolveLatestHub,
        onEnabled: onEnabled,
      );
    });
    debugPrint(
      'RemoteAccessService: retrying remote access for hub=${serverHub.id} '
      'in ${delay.inSeconds}s (attempt=${attempt + 1})',
    );
  }

  Future<void> _clearAutoEnableRetry(String key) async {
    _autoEnableRetryTimers.remove(key)?.cancel();
    try {
      final prefs = await SharedPreferences.getInstance();
      await prefs.remove('$_retryPrefsPrefix${Uri.encodeComponent(key)}');
    } catch (error) {
      debugPrint(
        'RemoteAccessService: failed to clear retry state for key=$key: $error',
      );
    }
  }

  @visibleForTesting
  Future<void> autoEnableForHubForTesting({
    required Home home,
    required Hub serverHub,
    required RemoteAccessHubSaver saveHub,
    RemoteAccessLatestHubResolver? resolveLatestHub,
    RemoteAccessEnabledCallback? onEnabled,
  }) async {
    await _autoEnableForHub(
      home: home,
      serverHub: serverHub,
      saveHub: saveHub,
      resolveLatestHub: resolveLatestHub,
      onEnabled: onEnabled,
    );
  }

  Future<_AutoEnableOutcome> _autoEnableForHub({
    required Home home,
    required Hub serverHub,
    required RemoteAccessHubSaver saveHub,
    RemoteAccessLatestHubResolver? resolveLatestHub,
    RemoteAccessEnabledCallback? onEnabled,
    bool Function()? operationIsCurrent,
  }) async {
    var hub = resolveLatestHub?.call(home.id, serverHub.id) ?? serverHub;
    try {
      if (operationIsCurrent?.call() == false) {
        return _AutoEnableOutcome.complete;
      }
      if (await _isOptedOut(hub)) {
        debugPrint(
          'RemoteAccessService: auto-enable skipped: user disabled remote '
          'access for hub=${hub.id}',
        );
        return _AutoEnableOutcome.complete;
      }

      final tokenHub = await ensureOwnerTokenForHub(hub);
      if (tokenHub.token != hub.token) {
        final saved = await saveHub(tokenHub);
        if (!saved) return _AutoEnableOutcome.retry;
        hub = resolveLatestHub?.call(home.id, serverHub.id) ?? tokenHub;
      }

      if (hub.remoteEndpoint != null) {
        final existingState = await _existingRemoteAccessState(hub);
        if (existingState == _ExistingRemoteAccessState.healthy) {
          await _grantSupportAccessForHub(hub);
          onEnabled?.call(hub);
          return _AutoEnableOutcome.complete;
        }
        if (existingState == _ExistingRemoteAccessState.unreachable) {
          return _AutoEnableOutcome.retry;
        }
      }

      if (operationIsCurrent?.call() == false) {
        return _AutoEnableOutcome.complete;
      }
      final result = await enableForHub(hub, home: home);
      if (operationIsCurrent?.call() == false) {
        await _cleanUpSupersededAutoEnable(hub, home: home);
        return _AutoEnableOutcome.complete;
      }
      final saved = await saveHub(result.updatedHub);
      if (!saved) return _AutoEnableOutcome.retry;
      if (operationIsCurrent?.call() == false) {
        await _cleanUpSupersededAutoEnable(result.updatedHub, home: home);
        await saveHub(result.updatedHub.copyWith(
          clearRemoteEndpoint: true,
          updatedAt: DateTime.now(),
          pendingSync: true,
        ));
        return _AutoEnableOutcome.complete;
      }

      if (result.routeVerified) {
        onEnabled?.call(result.updatedHub);
        debugPrint(
          'RemoteAccessService: auto-enabled remote access for '
          'hub=${result.updatedHub.id}',
        );
        return _AutoEnableOutcome.complete;
      }
      return _AutoEnableOutcome.retry;
    } on RemoteAccessOptedOutException {
      debugPrint(
        'RemoteAccessService: auto-enable skipped: cloud state records a '
        'manual disable for hub=${serverHub.id}',
      );
      if (hub.remoteEndpoint != null) {
        await _clearDeviceConfigBestEffort(hub);
        await saveHub(hub.copyWith(
          clearRemoteEndpoint: true,
          updatedAt: DateTime.now(),
          pendingSync: true,
        ));
      }
      return _AutoEnableOutcome.complete;
    } catch (error, stackTrace) {
      debugPrint('RemoteAccessService: auto-enable pending for '
          'hub=${serverHub.id}: $error');
      debugPrint('$stackTrace');
      return _AutoEnableOutcome.retry;
    }
  }

  Future<void> _grantSupportAccessForHub(Hub hub) async {
    try {
      await _supportGrant(hub).timeout(_supportGrantTimeout);
    } on TimeoutException {
      debugPrint(
        'RemoteAccessService: support access auto-grant timed out for '
        'hub=${hub.id}',
      );
    } catch (error) {
      debugPrint(
          'RemoteAccessService: support access auto-grant failed: $error');
    }
  }

  Future<void> _cleanUpSupersededAutoEnable(
    Hub hub, {
    required Home home,
  }) async {
    for (final endpoint in _disableEndpoints(hub)) {
      try {
        await _apiForEndpoint(endpoint, hub.token).clearConfig();
        break;
      } catch (error) {
        debugPrint(
          'RemoteAccessService: superseded enable cleanup failed via '
          '${endpoint.baseUrl}: $error',
        );
      }
    }
    try {
      await _tearDownCloudRemoteAccess(
        hub,
        home: home,
        serverInstanceId: await _serverInstanceIdFor(hub),
      );
    } catch (error) {
      debugPrint(
        'RemoteAccessService: superseded cloud enable cleanup failed for '
        'hub=${hub.id}: $error',
      );
    }
  }

  Future<void> _clearDeviceConfigBestEffort(Hub hub) async {
    for (final endpoint in _disableEndpoints(hub)) {
      try {
        await _apiForEndpoint(endpoint, hub.token).clearConfig();
        return;
      } catch (error) {
        debugPrint(
          'RemoteAccessService: stale disabled config cleanup failed via '
          '${endpoint.baseUrl}: $error',
        );
      }
    }
  }

  Future<_ExistingRemoteAccessState> _existingRemoteAccessState(
    Hub hub,
  ) async {
    RhythmRemoteAccessStatus? status;
    for (final endpoint in _disableEndpoints(hub)) {
      try {
        status = await _apiForEndpoint(endpoint, hub.token).getStatus();
        break;
      } catch (error) {
        debugPrint(
          'RemoteAccessService: remote access status unavailable via '
          '${endpoint.baseUrl} for hub=${hub.id}: $error',
        );
      }
    }

    if (status != null && !_isActivated(status)) {
      return _ExistingRemoteAccessState.needsRepair;
    }

    final remoteEndpoint = hub.remoteEndpoint;
    if (remoteEndpoint == null) {
      return _ExistingRemoteAccessState.needsRepair;
    }
    try {
      await _waitForRemoteRoute(
        endpoint: remoteEndpoint,
        authToken: hub.token,
        expectedServerInstanceId: hub.serverInstanceId,
        attemptsOverride: 1,
      );
      return _ExistingRemoteAccessState.healthy;
    } catch (error) {
      debugPrint(
        'RemoteAccessService: saved remote route is not ready for '
        'hub=${hub.id}: $error',
      );
    }

    // If device status was readable, a stopped or missing connector can be
    // repaired through that same endpoint. If neither LAN nor tunnel status
    // was reachable, retain cloud state and retry when connectivity changes.
    return status == null
        ? _ExistingRemoteAccessState.unreachable
        : _ExistingRemoteAccessState.needsRepair;
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

    final autoEnableKey =
        _autoEnableKey(home?.id ?? serverHub.homeId, serverHub);
    _autoEnableRetryTimers.remove(autoEnableKey)?.cancel();
    _autoEnableGenerations[autoEnableKey] =
        (_autoEnableGenerations[autoEnableKey] ?? 0) + 1;

    final serverInstanceId = await _serverInstanceIdFor(serverHub);
    // Persist intent before either teardown path. This prevents a background
    // enable from racing a device or cloud endpoint that is temporarily down.
    await _recordOptOut(serverHub);
    await _clearAutoEnableRetry(autoEnableKey);

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

    Object? cloudError;
    try {
      await _tearDownCloudRemoteAccess(
        serverHub,
        home: home,
        serverInstanceId: serverInstanceId,
      );
    } catch (error) {
      cloudError = error;
      debugPrint(
        'RemoteAccessService: cloud teardown failed for '
        'hub=${serverHub.id}: $error',
      );
    }

    if (!deviceConfigCleared && cloudError != null) {
      if (lastError != null && lastStackTrace != null) {
        Error.throwWithStackTrace(lastError, lastStackTrace);
      }
      throw StateError(
          'Remote access could not be disabled on device or cloud.');
    }
    if (!deviceConfigCleared) {
      debugPrint(
        'RemoteAccessService: device config was unreachable, but the cloud '
        'route was removed for hub=${serverHub.id}',
      );
    }

    final stableServerInstanceId =
        serverInstanceId.startsWith('endpoint:') ? null : serverInstanceId;
    return serverHub.copyWith(
      clearRemoteEndpoint: true,
      serverInstanceId: stableServerInstanceId,
      updatedAt: DateTime.now(),
      pendingSync: true,
    );
  }

  /// Hub ids for which the user explicitly disabled remote access. Persisted
  /// so auto-enable never resurrects a tunnel the user turned off.
  Future<Set<String>> _optOutIds() async {
    final cached = _optOutHubIds;
    if (cached != null) return cached;
    final load = _optOutHubIdsLoad ??= _readOptOutHubIds();
    final loaded = await load;
    return _optOutHubIds ??= loaded;
  }

  Future<Set<String>> _readOptOutHubIds() async {
    try {
      final prefs = await SharedPreferences.getInstance();
      return (prefs.getStringList(_optOutPrefsKey) ?? const []).toSet();
    } catch (error) {
      debugPrint('RemoteAccessService: failed to load remote access opt-outs: '
          '$error');
      return <String>{};
    }
  }

  Future<bool> _isOptedOut(Hub hub) async {
    final ids = await _optOutIds();
    return _hubPreferenceIdentities(hub).any(ids.contains);
  }

  Future<void> _recordOptOut(Hub hub) async {
    final ids = await _optOutIds();
    final changed = _hubPreferenceIdentities(hub).fold<bool>(
      false,
      (changed, identity) => ids.add(identity) || changed,
    );
    if (!changed) return;
    await _persistOptOutHubIds(ids);
  }

  Future<void> _clearOptOut(Hub hub) async {
    final ids = await _optOutIds();
    final changed = _hubPreferenceIdentities(hub).fold<bool>(
      false,
      (changed, identity) => ids.remove(identity) || changed,
    );
    if (!changed) return;
    await _persistOptOutHubIds(ids);
  }

  Iterable<String> _hubPreferenceIdentities(Hub hub) sync* {
    yield hub.id;
    final serverInstanceId = hub.serverInstanceId?.trim().toLowerCase();
    if (serverInstanceId != null &&
        serverInstanceId.isNotEmpty &&
        serverInstanceId != hub.id) {
      yield serverInstanceId;
    }
  }

  String _autoEnableKey(String homeId, Hub hub) {
    // Keep the operation key stable when first claim later discovers the
    // physical server id; otherwise a disable could miss the in-flight task.
    return '$homeId:${hub.id}';
  }

  Future<void> _persistOptOutHubIds(Set<String> ids) async {
    try {
      final prefs = await SharedPreferences.getInstance();
      await prefs.setStringList(_optOutPrefsKey, ids.toList()..sort());
    } catch (error) {
      // Keep the in-memory set as the source of truth for this session even
      // if persistence fails.
      debugPrint(
          'RemoteAccessService: failed to persist remote access opt-outs: '
          '$error');
    }
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

      if (latest.serviceError?.trim().isNotEmpty == true) {
        throw RemoteAccessActivationException(latest);
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

  Future<RhythmHello> _waitForRemoteRoute({
    required HubEndpoint endpoint,
    required String? authToken,
    String? expectedServerInstanceId,
    int? attemptsOverride,
  }) async {
    final configuredAttempts = attemptsOverride ?? _remoteRoutePollAttempts;
    final attempts = configuredAttempts < 1 ? 1 : configuredAttempts;
    Object? lastError;
    StackTrace? lastStackTrace;

    for (var attempt = 0; attempt < attempts; attempt += 1) {
      if (attempt > 0) {
        await Future<void>.delayed(_remoteRoutePollDelay);
      }

      try {
        final state = await _stateLoader(
          endpoint: endpoint,
          authToken: authToken,
        );
        final remoteServerInstanceId = state.serverInstanceId?.trim();
        if (expectedServerInstanceId != null &&
            expectedServerInstanceId.isNotEmpty &&
            remoteServerInstanceId != expectedServerInstanceId) {
          throw StateError(
            'Remote access hostname returned server_instance_id='
            '${remoteServerInstanceId ?? '(missing)'}, expected '
            '$expectedServerInstanceId.',
          );
        }
        return state;
      } catch (error, stackTrace) {
        lastError = error;
        lastStackTrace = stackTrace;
      }
    }

    Error.throwWithStackTrace(
      RemoteAccessRouteException(
        endpoint: endpoint,
        attempts: attempts,
        cause: lastError,
      ),
      lastStackTrace ?? StackTrace.current,
    );
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
    bool explicitUserEnable = false,
  }) {
    final normalizedServerInstanceId = serverInstanceId?.trim();
    final snapshotHub = normalizedServerInstanceId != null &&
            normalizedServerInstanceId.isNotEmpty
        ? serverHub.copyWith(serverInstanceId: normalizedServerInstanceId)
        : serverHub;
    return {
      'hub_id': serverHub.id,
      if (explicitUserEnable) 'explicit_enable': true,
      if (normalizedServerInstanceId != null &&
          normalizedServerInstanceId.isNotEmpty)
        'server_instance_id': normalizedServerInstanceId,
      if (home != null) ...{
        'home': AccountCloudSyncService.homeSnapshotPayload(home),
        'server_hub': AccountCloudSyncService.serverHubSnapshotPayload(
          snapshotHub,
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
