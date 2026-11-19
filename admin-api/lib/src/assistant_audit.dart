import 'dart:convert';

import 'package:cryptography/cryptography.dart';

/// Staff audit retains operation scope and verification, using keyed references
/// so that a room name or device identifier cannot be recovered from the log.
Future<Map<String, dynamic>> assistantAuditReceipt({
  required String hubId,
  required String? secret,
  required Object? arguments,
  required Object? receipt,
}) async {
  final before = arguments is Map ? arguments : const {};
  final after = receipt is Map ? receipt : const {};
  final scope = <String, dynamic>{};
  if (secret != null && secret.isNotEmpty) {
    final key = SecretKey(utf8.encode(secret));
    final mac = Hmac.sha256();
    for (final field in const [
      'room_id',
      'sensor_id',
      'device_id',
      'profile_id',
      'node_id',
      'scene_id',
      'from_room_id',
      'to_room_id',
      'schedule_id',
      'before_targets',
      'after_targets',
      'affected_rooms',
      'unresolved_node_ids'
    ]) {
      final value = after[field] ?? before[field];
      final ids = value is String
          ? [value]
          : value is List
              ? value.whereType<String>().toSet().toList()
              : <String>[];
      if (ids.isEmpty) continue;
      ids.sort();
      final digest = await mac.calculateMac(
          utf8.encode(
              jsonEncode(['staff-assistant-audit-v1', hubId, field, ids])),
          secretKey: key);
      scope[field] = {
        'count': ids.length,
        'reference': base64UrlEncode(digest.bytes)
      };
    }
  }
  final outcome = after['status'];
  return {
    if (scope.isNotEmpty) 'scope': scope,
    if (const {
      'acknowledged',
      'applied',
      'partially_applied',
      'queued',
      'failed',
      'uncertain',
      'submitted'
    }.contains(outcome))
      'outcome': outcome,
    for (final name in ['server_acknowledged', 'canonical_readback_verified'])
      if (after[name] is bool) name: after[name],
  };
}
