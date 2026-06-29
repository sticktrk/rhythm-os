import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../backend/auth/supabase_auth_backend.dart';
import '../backend/backend_provider.dart';

typedef SupportAccessAuthApiFactory = RhythmAuthApi Function({
  required String baseUrl,
  String? authToken,
});

class SupportAccessService {
  SupportAccessService._({
    SupportAccessAuthApiFactory? authApiFactory,
    dynamic Function()? supabaseClientFactory,
  })  : _authApiFactory = authApiFactory ?? _defaultAuthApiFactory,
        _supabaseClientFactory = supabaseClientFactory;

  static final SupportAccessService instance = SupportAccessService._();
  static const _grantFunctionName = 'support-access-grant';
  static const _defaultScope = 'beta_admin';

  @visibleForTesting
  factory SupportAccessService.testing({
    SupportAccessAuthApiFactory? authApiFactory,
    dynamic Function()? supabaseClientFactory,
  }) {
    return SupportAccessService._(
      authApiFactory: authApiFactory,
      supabaseClientFactory: supabaseClientFactory,
    );
  }

  final SupportAccessAuthApiFactory _authApiFactory;
  final dynamic Function()? _supabaseClientFactory;

  Future<void> grantForHub(
    Hub serverHub, {
    String label = 'Rhythm beta admin support',
  }) async {
    _ensureServerHubWithOwnerToken(serverHub);
    final supportToken = await _authApiFactory(
      baseUrl: serverHub.endpoint.baseUrl,
      authToken: serverHub.token,
    ).issueSupportToken(label: label);

    final client = _supabaseClient();
    await client.functions.invoke(
      _grantFunctionName,
      body: buildGrantBody(
        serverHub: serverHub,
        tokenId: supportToken.tokenId,
        token: supportToken.token,
        label: label,
      ),
    );
  }

  Future<void> revokeForHub(Hub serverHub) async {
    if (serverHub.type != HubType.server) return;
    final client = _supabaseClient();
    await client.functions.invoke(
      _grantFunctionName,
      body: buildRevokeBody(serverHub: serverHub),
    );
  }

  @visibleForTesting
  static Map<String, dynamic> buildGrantBody({
    required Hub serverHub,
    required String tokenId,
    required String token,
    String label = 'Rhythm beta admin support',
  }) {
    return {
      'action': 'grant',
      'hub_id': serverHub.id,
      'home_id': serverHub.homeId,
      'scope': _defaultScope,
      'token_id': tokenId,
      'token': token,
      'label':
          label.trim().isEmpty ? 'Rhythm beta admin support' : label.trim(),
    };
  }

  @visibleForTesting
  static Map<String, dynamic> buildRevokeBody({
    required Hub serverHub,
  }) {
    return {
      'action': 'revoke',
      'hub_id': serverHub.id,
      'home_id': serverHub.homeId,
      'scope': _defaultScope,
    };
  }

  void _ensureServerHubWithOwnerToken(Hub serverHub) {
    if (serverHub.type != HubType.server) {
      throw StateError('Support access is only supported for server hubs.');
    }
    if (serverHub.token?.trim().isEmpty != false) {
      throw StateError('Support access requires a saved Rhythm owner token.');
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
    throw StateError('Support access requires a Supabase backend.');
  }

  static RhythmAuthApi _defaultAuthApiFactory({
    required String baseUrl,
    String? authToken,
  }) {
    return RhythmAuthApi(baseUrl: baseUrl, authToken: authToken);
  }
}
