import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:supabase_flutter/supabase_flutter.dart' show FunctionException;

import '../backend/backend.dart';
import 'account_cloud_sync_service.dart';
import 'auth_service.dart';

enum CloudHomeJoinDisposition { notAttempted, joined, blocked }

class CloudHomeJoinResult {
  const CloudHomeJoinResult._({
    required this.disposition,
    this.home,
    this.code,
  });

  const CloudHomeJoinResult.notAttempted()
      : this._(disposition: CloudHomeJoinDisposition.notAttempted);

  const CloudHomeJoinResult.joined(AccountHomeServerHubs home)
      : this._(
          disposition: CloudHomeJoinDisposition.joined,
          home: home,
        );

  const CloudHomeJoinResult.blocked({String? code})
      : this._(
          disposition: CloudHomeJoinDisposition.blocked,
          code: code,
        );

  final CloudHomeJoinDisposition disposition;
  final AccountHomeServerHubs? home;
  final String? code;

  bool get canCreateHome =>
      disposition == CloudHomeJoinDisposition.notAttempted;
}

class CloudHomeJoinBlockedException implements Exception {
  const CloudHomeJoinBlockedException({this.code});

  final String? code;

  String get userMessage => code == 'identity_conflict'
      ? 'This Box belongs to an existing Home, but its cloud identity could '
          'not be reconciled safely. No new Home was created; try again or '
          'contact support.'
      : 'Could not verify this Box\'s existing Home. Try again; no new Home '
          'was created.';

  @override
  String toString() =>
      'Cloud Home join blocked${code == null ? '' : ': $code'}';
}

class CloudHomeJoinService {
  CloudHomeJoinService._();

  static const functionName = 'join-home-by-local-device-proof';

  static final CloudHomeJoinService instance = CloudHomeJoinService._();

  bool get canJoin {
    if (!BackendProvider.isInitialized) return false;
    final auth = AuthService();
    return BackendProvider.instance.auth is SupabaseAuthBackend &&
        auth.isSignedIn &&
        !auth.isAnonymous &&
        auth.currentUserId != null;
  }

  Future<CloudHomeJoinResult> joinByLocalDeviceProof({
    required String? serverInstanceId,
    required RhythmCloudJoinProof? joinProof,
    required String host,
    required int port,
    required String? ownerToken,
    required String hubName,
  }) async {
    final cleanServerInstanceId = _cleanOptional(serverInstanceId);
    if (!canJoin || cleanServerInstanceId == null || joinProof == null) {
      return const CloudHomeJoinResult.notAttempted();
    }

    try {
      final client =
          (BackendProvider.instance.auth as SupabaseAuthBackend).client;
      final response = await client.functions.invoke(
        functionName,
        body: {
          'server_instance_id': cleanServerInstanceId,
          'join_proof': joinProof.toJson(),
        },
      );

      if (response.status < 200 || response.status >= 300) {
        return blockedCloudHomeJoinResultForTesting(response.data);
      }

      final home = joinedHomeFromFunctionResponseForTesting(
        response.data,
        ownerToken: ownerToken,
        lanEndpoint: HubEndpoint(host: host, port: port),
        hubName: hubName,
        serverInstanceId: cleanServerInstanceId,
      );
      return home == null
          ? const CloudHomeJoinResult.blocked(code: 'malformed_response')
          : CloudHomeJoinResult.joined(home);
    } on FunctionException catch (error) {
      debugPrint('Cloud Home join rejected: $error');
      return blockedCloudHomeJoinResultForTesting(error.details);
    } catch (error) {
      debugPrint('Cloud Home join failed safely: $error');
      return const CloudHomeJoinResult.blocked(code: 'unavailable');
    }
  }
}

@visibleForTesting
CloudHomeJoinResult blockedCloudHomeJoinResultForTesting(Object? data) {
  final code = data is Map ? _cleanOptional(data['code']?.toString()) : null;
  return CloudHomeJoinResult.blocked(code: code);
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
