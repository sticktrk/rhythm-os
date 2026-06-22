import 'dart:async';

import 'package:connectivity_plus/connectivity_plus.dart';
import 'package:dio/dio.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../config/feature_flags.dart';
import '../providers/server_sync_provider.dart';
import 'employee_mode_service.dart';
import 'support_proxy_dio.dart';

enum ResolvedServerEndpointSource {
  lan,
  remote,
}

class ResolvedServerEndpoint {
  const ResolvedServerEndpoint({
    required this.hub,
    required this.endpoint,
    required this.source,
  });

  final Hub hub;
  final HubEndpoint endpoint;
  final ResolvedServerEndpointSource source;

  String get baseUrl => endpoint.baseUrl;
  bool get isRemote => source == ResolvedServerEndpointSource.remote;

  RhythmDiagnosticsApi diagnosticsApi({
    Duration connectTimeout = RhythmDiagnosticsApi.defaultConnectTimeout,
    Duration receiveTimeout = RhythmDiagnosticsApi.defaultReceiveTimeout,
    Duration debugBundleReceiveTimeout =
        RhythmDiagnosticsApi.defaultDebugBundleReceiveTimeout,
  }) {
    return RhythmDiagnosticsApi.fromBaseUrl(
      baseUrl: baseUrl,
      dio: supportProxyDio(
        connectTimeout: connectTimeout,
        receiveTimeout: receiveTimeout,
      ),
      connectTimeout: connectTimeout,
      receiveTimeout: receiveTimeout,
      debugBundleReceiveTimeout: debugBundleReceiveTimeout,
      authToken: hub.token,
    );
  }

  RhythmConfigApi configApi({
    Duration connectTimeout = const Duration(seconds: 10),
    Duration receiveTimeout = const Duration(seconds: 10),
    String? authToken,
  }) {
    return RhythmConfigApi(
      baseUrl: baseUrl,
      dio: supportProxyDio(
        connectTimeout: connectTimeout,
        receiveTimeout: receiveTimeout,
      ),
      authToken: authToken ?? hub.token,
    );
  }

  RhythmAuthApi authApi({String? authToken}) {
    return RhythmAuthApi(
      baseUrl: baseUrl,
      dio: supportProxyDio(),
      authToken: authToken ?? hub.token,
    );
  }

  RhythmMatterApi matterApi({
    Duration connectTimeout = const Duration(seconds: 5),
    Duration receiveTimeout = const Duration(seconds: 45),
    String? authToken,
  }) {
    return RhythmMatterApi(
      baseUrl: baseUrl,
      dio: supportProxyDio(
        connectTimeout: connectTimeout,
        receiveTimeout: receiveTimeout,
      ),
      authToken: authToken ?? hub.token,
    );
  }

  RhythmBundleApi bundleApi() {
    return RhythmBundleApi(
      baseUrl: baseUrl,
      dio: supportProxyDio(
        receiveTimeout: const Duration(seconds: 15),
      ),
      authToken: hub.token,
    );
  }

  Dio? supportProxyDio({
    Duration connectTimeout = const Duration(seconds: 5),
    Duration receiveTimeout = const Duration(seconds: 10),
  }) {
    if (!EmployeeModeService.instance.isActive) return null;
    return SupportProxyDioFactory(
      employeeMode: EmployeeModeService.instance,
    ).configDio(
      connectTimeout: connectTimeout,
      receiveTimeout: receiveTimeout,
    );
  }
}

class ServerEndpointResolver {
  const ServerEndpointResolver._();

  static const Duration defaultLanProbeTimeout = Duration(seconds: 2);
  static const Duration defaultConnectivityTimeout =
      Duration(milliseconds: 300);

  static Future<ResolvedServerEndpoint> resolve(
    Hub hub, {
    ServerSyncProvider? syncProvider,
    Duration lanProbeTimeout = defaultLanProbeTimeout,
    Future<bool> Function(HubEndpoint endpoint, String? authToken)?
        lanReachability,
    Future<List<ConnectivityResult>> Function()? connectivityCheck,
  }) async {
    if (EmployeeModeService.instance.isActive) {
      return _resolved(hub, hub.endpoint, ResolvedServerEndpointSource.remote);
    }

    final remote = FeatureFlags.remoteAccessTunnel ? hub.remoteEndpoint : null;
    if (remote == null || remote == hub.endpoint) {
      return _resolved(hub, hub.endpoint, ResolvedServerEndpointSource.lan);
    }

    final connectedEndpoint = syncProvider?.activeConnectionEndpoint;
    final connectedHub = syncProvider?.connectedServerHub;
    if (connectedHub?.id == hub.id &&
        syncProvider?.connectionState == RhythmConnectionState.connected &&
        connectedEndpoint == hub.endpoint) {
      return _resolved(hub, hub.endpoint, ResolvedServerEndpointSource.lan);
    }

    final shouldProbeLan = await _shouldProbeLan(connectivityCheck);
    if (!shouldProbeLan) {
      return _resolved(hub, remote, ResolvedServerEndpointSource.remote);
    }

    final canReachLan = await (lanReachability ?? _canReachLan)(
      hub.endpoint,
      hub.token,
    ).timeout(
      lanProbeTimeout + const Duration(milliseconds: 200),
      onTimeout: () => false,
    );
    if (canReachLan) {
      return _resolved(hub, hub.endpoint, ResolvedServerEndpointSource.lan);
    }

    return _resolved(hub, remote, ResolvedServerEndpointSource.remote);
  }

  static ResolvedServerEndpoint local(Hub hub) {
    return _resolved(hub, hub.endpoint, ResolvedServerEndpointSource.lan);
  }

  static ResolvedServerEndpoint _resolved(
    Hub hub,
    HubEndpoint endpoint,
    ResolvedServerEndpointSource source,
  ) {
    return ResolvedServerEndpoint(
      hub: hub,
      endpoint: endpoint,
      source: source,
    );
  }

  static Future<bool> _canReachLan(
    HubEndpoint endpoint,
    String? authToken,
  ) async {
    try {
      await RhythmAuthApi(
        baseUrl: endpoint.baseUrl,
        authToken: authToken,
      ).getStatus().timeout(defaultLanProbeTimeout);
      return true;
    } catch (_) {
      return false;
    }
  }

  static Future<bool> _shouldProbeLan(
    Future<List<ConnectivityResult>> Function()? connectivityCheck,
  ) async {
    try {
      final results =
          await (connectivityCheck ?? Connectivity().checkConnectivity)
              .call()
              .timeout(defaultConnectivityTimeout);
      return _canConnectivityReachLan(results);
    } catch (_) {
      return true;
    }
  }

  static bool _canConnectivityReachLan(List<ConnectivityResult> results) {
    if (results.isEmpty) return true;
    return results.contains(ConnectivityResult.wifi) ||
        results.contains(ConnectivityResult.ethernet);
  }
}
