import 'package:flutter/foundation.dart';
import 'package:supabase_flutter/supabase_flutter.dart' show FunctionException;

import '../backend/backend.dart';
import 'auth_service.dart';

/// Result of one `monster-device` edge function call.
///
/// Secrets (setup token, ticket, LAN key) are held only for the duration of
/// the pairing journey and never logged or sent to analytics.
class MonsterCloudResponse {
  const MonsterCloudResponse({
    required this.status,
    this.error,
    this.setupToken,
    this.ticket,
    this.dsn,
    this.ip,
    this.localKey,
    this.localKeyId,
  });

  final int status;
  final String? error;
  final String? setupToken;
  final String? ticket;
  final String? dsn;
  final String? ip;
  final String? localKey;
  final int? localKeyId;

  bool get ok => status >= 200 && status < 300;

  /// The broker answers 409 `device_not_on_lan` while the strip's LAN address
  /// is still propagating after registration; the caller may retry.
  bool get lanPending => status == 409 && error == 'device_not_on_lan';

  bool get hasCredentials =>
      dsn != null && ip != null && localKey != null && localKeyId != null;

  Map<String, dynamic> adoptParams({String? name}) => {
        'stage': 'adopt',
        'dsn': dsn,
        'ip': ip,
        'local_key': localKey,
        'local_key_id': localKeyId,
        if (name != null && name.trim().isNotEmpty) 'name': name.trim(),
      };
}

typedef MonsterCloudInvoke = Future<MonsterCloudResponse> Function(
  Map<String, dynamic> body,
);

/// Client for the owner-operated Monster cloud broker. Only the signed-in
/// Rhythm user's Supabase session is sent; Monster credentials never reach
/// the app or the appliance.
class MonsterCloudService {
  MonsterCloudService({
    @visibleForTesting MonsterCloudInvoke? invoke,
    @visibleForTesting bool? canUseOverride,
  })  : _invoke = invoke ?? _invokeFunction,
        _canUseOverride = canUseOverride;

  static final MonsterCloudService instance = MonsterCloudService();
  static const functionName = 'monster-device';

  final MonsterCloudInvoke _invoke;
  final bool? _canUseOverride;

  bool get canUse {
    final override = _canUseOverride;
    if (override != null) return override;
    if (!BackendProvider.isInitialized) return false;
    final auth = AuthService();
    return BackendProvider.instance.auth is SupabaseAuthBackend &&
        auth.isSignedIn &&
        !auth.isAnonymous;
  }

  Future<MonsterCloudResponse> begin(String dsn) =>
      _invoke({'action': 'begin', 'dsn': dsn});

  Future<MonsterCloudResponse> complete(String dsn, String ticket) =>
      _invoke({'action': 'complete', 'dsn': dsn, 'ticket': ticket});

  Future<MonsterCloudResponse> key(String dsn) =>
      _invoke({'action': 'key', 'dsn': dsn});

  static Future<MonsterCloudResponse> _invokeFunction(
    Map<String, dynamic> body,
  ) async {
    if (!BackendProvider.isInitialized ||
        BackendProvider.instance.auth is! SupabaseAuthBackend) {
      return const MonsterCloudResponse(status: 401, error: 'not_signed_in');
    }
    try {
      final client =
          (BackendProvider.instance.auth as SupabaseAuthBackend).client;
      final response = await client.functions.invoke(
        functionName,
        body: body,
      );
      return parseMonsterCloudResponse(response.status, response.data);
    } on FunctionException catch (error) {
      return parseMonsterCloudResponse(error.status, error.details);
    } catch (_) {
      return const MonsterCloudResponse(status: 0, error: 'unreachable');
    }
  }
}

@visibleForTesting
MonsterCloudResponse parseMonsterCloudResponse(int status, Object? data) {
  final map = data is Map ? data.cast<String, dynamic>() : const {};
  final error = map['error'];
  final localKeyId = map['local_key_id'];
  return MonsterCloudResponse(
    status: status,
    error: error is String ? error : null,
    setupToken: _string(map['setup_token']),
    ticket: _string(map['ticket']),
    dsn: _string(map['dsn']),
    ip: _string(map['ip']),
    localKey: _string(map['local_key']),
    localKeyId: localKeyId is int
        ? localKeyId
        : localKeyId is num
            ? localKeyId.toInt()
            : null,
  );
}

String? _string(Object? value) {
  if (value is! String) return null;
  final trimmed = value.trim();
  return trimmed.isEmpty ? null : trimmed;
}

/// Human guidance for a broker failure without leaking the wire stage names.
String monsterCloudFailureMessage(MonsterCloudResponse response) {
  return switch (response.error) {
    'not_signed_in' => 'Sign in to your Rhythm account to add a Monster strip.',
    'forbidden' => 'This Rhythm account is not allowed to add Monster strips.',
    'cloud_not_configured' =>
      'Monster setup is not enabled on this Rhythm cloud yet.',
    'unsupported_model' =>
      'This Monster product is not supported yet. Only Neon Flow strips can be added.',
    'unsupported_capabilities' =>
      'This strip reports controls Rhythm does not understand.',
    'device_not_owned' =>
      'The strip is not registered yet. Finish Wi-Fi setup and try again.',
    'device_not_on_lan' =>
      'The strip has not appeared on your network yet. Wait a moment and try again.',
    'invalid_ticket' => 'This setup session expired. Start over from the scan.',
    'unreachable' => 'Could not reach the Rhythm cloud. Check your connection.',
    'monster_login' ||
    'partner_ticket' ||
    'ayla_login' =>
      'The Monster cloud rejected Rhythm\'s login. Try again later.',
    _ => 'The Rhythm cloud could not complete Monster setup.',
  };
}
