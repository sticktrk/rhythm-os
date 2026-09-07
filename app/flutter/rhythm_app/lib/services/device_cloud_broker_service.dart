import 'package:flutter/foundation.dart';
import 'package:supabase_flutter/supabase_flutter.dart' show FunctionException;

import '../backend/backend.dart';
import 'auth_service.dart';

/// Result of one call to a vendor cloud broker edge function.
///
/// Every broker Rhythm hosts answers the same shape: `begin` returns a setup
/// token plus a ticket, `complete` returns LAN credentials. Secrets are held
/// only for the duration of the pairing journey and never logged.
class DeviceCloudBrokerResponse {
  const DeviceCloudBrokerResponse({
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

  /// Brokers answer 409 `device_not_on_lan` while the device's LAN address is
  /// still propagating after registration; the caller may retry.
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

typedef DeviceCloudBrokerInvoke = Future<DeviceCloudBrokerResponse> Function(
  String functionName,
  Map<String, dynamic> body,
);

final RegExp _functionNamePattern = RegExp(r'^[a-z0-9][a-z0-9-]{0,63}$');

/// Client for the Rhythm-hosted cloud brokers that register devices with a
/// manufacturer on the person's behalf. Only the signed-in Rhythm user's
/// Supabase session is sent; manufacturer credentials never reach the app or
/// the appliance. The broker function name comes from the appliance's
/// advertised device profile.
class DeviceCloudBrokerService {
  DeviceCloudBrokerService({
    @visibleForTesting DeviceCloudBrokerInvoke? invoke,
    @visibleForTesting bool? canUseOverride,
  })  : _invoke = invoke ?? _invokeFunction,
        _canUseOverride = canUseOverride;

  static final DeviceCloudBrokerService instance = DeviceCloudBrokerService();

  final DeviceCloudBrokerInvoke _invoke;
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

  Future<DeviceCloudBrokerResponse> begin(String functionName, String dsn) =>
      _call(functionName, {'action': 'begin', 'dsn': dsn});

  Future<DeviceCloudBrokerResponse> complete(
    String functionName,
    String dsn,
    String ticket,
  ) =>
      _call(functionName, {'action': 'complete', 'dsn': dsn, 'ticket': ticket});

  Future<DeviceCloudBrokerResponse> key(String functionName, String dsn) =>
      _call(functionName, {'action': 'key', 'dsn': dsn});

  Future<DeviceCloudBrokerResponse> _call(
    String functionName,
    Map<String, dynamic> body,
  ) {
    if (!_functionNamePattern.hasMatch(functionName)) {
      return Future.value(
        const DeviceCloudBrokerResponse(status: 0, error: 'invalid_broker'),
      );
    }
    return _invoke(functionName, body);
  }

  static Future<DeviceCloudBrokerResponse> _invokeFunction(
    String functionName,
    Map<String, dynamic> body,
  ) async {
    if (!BackendProvider.isInitialized ||
        BackendProvider.instance.auth is! SupabaseAuthBackend) {
      return const DeviceCloudBrokerResponse(
        status: 401,
        error: 'not_signed_in',
      );
    }
    try {
      final client =
          (BackendProvider.instance.auth as SupabaseAuthBackend).client;
      final response = await client.functions.invoke(functionName, body: body);
      return parseDeviceCloudBrokerResponse(response.status, response.data);
    } on FunctionException catch (error) {
      return parseDeviceCloudBrokerResponse(error.status, error.details);
    } catch (_) {
      return const DeviceCloudBrokerResponse(status: 0, error: 'unreachable');
    }
  }
}

@visibleForTesting
DeviceCloudBrokerResponse parseDeviceCloudBrokerResponse(
  int status,
  Object? data,
) {
  final map = data is Map ? data.cast<String, dynamic>() : const {};
  final error = map['error'];
  final localKeyId = map['local_key_id'];
  return DeviceCloudBrokerResponse(
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

/// Human guidance for a broker failure without leaking wire stage names or
/// naming any manufacturer.
String deviceCloudBrokerFailureMessage(DeviceCloudBrokerResponse response) {
  return switch (response.error) {
    'not_signed_in' => 'Sign in to your Rhythm account to add this device.',
    'invalid_broker' =>
      'This device type needs a cloud step this app cannot run yet.',
    'forbidden' => 'This Rhythm account is not allowed to add this device.',
    'cloud_not_configured' =>
      'Setup for this device type is not enabled on this Rhythm cloud yet.',
    'unsupported_model' =>
      'This exact product is not supported yet, even though its brand is.',
    'unsupported_capabilities' =>
      'This device reports controls Rhythm does not understand.',
    'device_not_owned' =>
      'The device is not registered yet. Finish Wi-Fi setup and try again.',
    'device_not_on_lan' =>
      'The device has not appeared on your network yet. Wait a moment and try again.',
    'invalid_ticket' => 'This setup session expired. Start over from the scan.',
    'unreachable' => 'Could not reach the Rhythm cloud. Check your connection.',
    // Brokers report their own login hops as `<hop>_login` / `partner_ticket`.
    final String code
        when code.endsWith('_login') || code == 'partner_ticket' =>
      'The manufacturer\'s cloud rejected Rhythm\'s login. Try again later.',
    _ => 'The Rhythm cloud could not complete setup for this device.',
  };
}
