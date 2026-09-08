import 'package:flutter/services.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

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

/// Uses the phone for Matter BLE/network setup, then waits for the appliance
/// to complete commissioning on Rhythm's durable fabric.
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
  }) async {
    try {
      final result =
          await _channel.invokeMapMethod<String, Object?>('commission', {
        'base_url': baseUrl,
        'setup_payload': originalSetupPayload,
        'session_id': sessionId,
        if (authToken?.trim().isNotEmpty == true)
          'auth_token': authToken!.trim(),
      });
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
