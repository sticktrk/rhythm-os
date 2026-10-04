import 'dart:async';
import 'package:flutter/services.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:rhythm_core/rhythm_core.dart' show DirectHubAccess;

enum PhoneMatterBackend { rhythm, haAddon }

/// Stable failure returned by the platform Matter commissioning boundary.
class PhoneMatterCommissioningException implements Exception {
  const PhoneMatterCommissioningException({
    required this.stage,
    required this.message,
  });

  final String stage;
  final String message;

  @override
  String toString() => message;
}

/// Uses the phone for BLE/network setup, then hands commissioning to the
/// selected Rhythm appliance or Home Assistant addon.
class PhoneMatterCommissioner {
  const PhoneMatterCommissioner({
    MethodChannel channel = const MethodChannel(
      'lighting.rhythm.app/phone_matter_commissioner',
    ),
  }) : _channel = channel;

  final MethodChannel _channel;

  Future<bool> isSupported() async {
    try {
      return await _channel.invokeMethod<bool>('isSupported') ?? false;
    } on PlatformException {
      return false;
    } on MissingPluginException {
      return false;
    }
  }

  Future<RhythmMatterPairingResponse> commission({
    required String baseUrl,
    required String originalSetupPayload,
    required String sessionId,
    String? authToken,
    PhoneMatterBackend backend = PhoneMatterBackend.rhythm,
    HaMatterCodeSource codeSource = HaMatterCodeSource.originalLabel,
    bool Function()? isCurrentTarget,
  }) async {
    final lease = DirectHubAccess.capture();
    bool current() =>
        (backend == PhoneMatterBackend.haAddon
            ? lease.isSameSelection
            : lease.isCurrent) &&
        (isCurrentTarget?.call() ?? true);
    if (!current()) throw StateError('Matter commissioning target changed');
    var cancelled = false;
    void onTargetChanged() {
      if (!current()) {
        cancelled = true;
        unawaited(cancel(sessionId));
      }
    }

    DirectHubAccess.changes.addListener(onTargetChanged);
    try {
      final result =
          await _channel.invokeMapMethod<String, Object?>('commission', {
        'base_url': baseUrl,
        'setup_payload': originalSetupPayload,
        'session_id': sessionId,
        if (backend == PhoneMatterBackend.haAddon) ...{
          'backend': 'ha_addon',
          'code_source': codeSource.wire,
        },
        if (authToken?.trim().isNotEmpty == true)
          'auth_token': authToken!.trim(),
      });
      if (cancelled || !current()) {
        throw const PhoneMatterCommissioningException(
            stage: 'cancelled',
            message:
                'The selected home changed. Check the previous pairing result before trying again.');
      }
      if (result == null) {
        throw const PhoneMatterCommissioningException(
          stage: 'invalid_response',
          message: 'The phone returned no Matter commissioning result.',
        );
      }
      return RhythmMatterPairingResponse.fromHttp(
        statusCode: result['http_status'] as int?,
        data: result['body'],
      );
    } on PlatformException catch (error) {
      throw PhoneMatterCommissioningException(
        stage: _boundedStage(error.code),
        message: _safeMessage(error.message),
      );
    } on MissingPluginException {
      throw const PhoneMatterCommissioningException(
        stage: 'native_unavailable',
        message: 'Phone Matter commissioning is unavailable on this device.',
      );
    } finally {
      DirectHubAccess.changes.removeListener(onTargetChanged);
    }
  }

  Future<void> cancel(String sessionId) async {
    try {
      await _channel.invokeMethod<void>('cancel', {'session_id': sessionId});
    } on PlatformException {
      // Older hosts may not support cancellation; the durable receipt remains.
    } on MissingPluginException {
      // No native operation exists on this platform.
    }
  }

  static String _boundedStage(String value) => switch (value) {
        'native_unavailable' ||
        'platform_commissioning' ||
        'handoff' ||
        'server_rejected' ||
        'invalid_response' ||
        'cancelled' ||
        'timeout' =>
          value,
        _ => 'platform_commissioning',
      };

  static String _safeMessage(String? value) {
    final message = value?.trim() ?? '';
    if (message.isEmpty) {
      return 'Phone Matter commissioning did not complete. Please try again.';
    }
    return message.length <= 240 ? message : '${message.substring(0, 240)}…';
  }
}
