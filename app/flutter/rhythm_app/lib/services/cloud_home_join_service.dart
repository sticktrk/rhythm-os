import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';

import '../backend/backend.dart';
import 'account_cloud_sync_service.dart';
import 'auth_service.dart';

class CloudHomeJoinService {
  CloudHomeJoinService._();

  static const functionName = 'join-home-by-server-instance';

  static final CloudHomeJoinService instance = CloudHomeJoinService._();

  bool get canJoin {
    if (!BackendProvider.isInitialized) return false;
    final auth = AuthService();
    return BackendProvider.instance.auth is SupabaseAuthBackend &&
        auth.isSignedIn &&
        !auth.isAnonymous &&
        auth.currentUserId != null;
  }

  Future<AccountHomeServerHubs?> joinByServerInstanceId({
    required String? serverInstanceId,
    required String host,
    required int port,
    required String? ownerToken,
    required String hubName,
  }) async {
    final cleanServerInstanceId = _cleanOptional(serverInstanceId);
    if (!canJoin || cleanServerInstanceId == null) return null;

    final client =
        (BackendProvider.instance.auth as SupabaseAuthBackend).client;
    final response = await client.functions.invoke(
      functionName,
      body: {'server_instance_id': cleanServerInstanceId},
    );

    if (response.status == 404) return null;
    if (response.status < 200 || response.status >= 300) {
      final data = response.data;
      final message = data is Map && data['error'] != null
          ? data['error'].toString()
          : 'Cloud Home join failed (${response.status})';
      throw StateError(message);
    }

    return joinedHomeFromFunctionResponseForTesting(
      response.data,
      ownerToken: ownerToken,
      lanEndpoint: HubEndpoint(host: host, port: port),
      hubName: hubName,
      serverInstanceId: cleanServerInstanceId,
    );
  }
}

@visibleForTesting
AccountHomeServerHubs? joinedHomeFromFunctionResponseForTesting(
  Object? data, {
  required String? ownerToken,
  required HubEndpoint lanEndpoint,
  required String hubName,
  required String? serverInstanceId,
}) {
  if (data is! Map) return null;
  final homeData = data['home'];
  final hubData = data['server_hub'];
  if (homeData is! Map || hubData is! Map) return null;

  final home = Home.fromSupabase(Map<String, dynamic>.from(homeData));
  final cloudHub = Hub.fromSupabase(Map<String, dynamic>.from(hubData));
  final token = _cleanOptional(ownerToken);
  final name = cloudHub.name.trim().isNotEmpty ? cloudHub.name : hubName;
  final cleanServerInstanceId =
      _cleanOptional(serverInstanceId) ?? cloudHub.serverInstanceId;

  final hub = cloudHub.copyWith(
    homeId: home.id,
    name: name,
    endpoint: lanEndpoint,
    token: token,
    serverInstanceId: cleanServerInstanceId,
    updatedAt: DateTime.now(),
    pendingSync: true,
  );

  return AccountHomeServerHubs(
    home: home,
    serverHubs: [hub],
  );
}

String? _cleanOptional(String? value) {
  final clean = value?.trim();
  return clean != null && clean.isNotEmpty ? clean : null;
}
