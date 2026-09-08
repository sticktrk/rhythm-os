import 'package:dio/dio.dart';

import '../json_parsing.dart';
import '../models/rhythm_pairing.dart';

/// Validated envelope shared by generic pairing and protocol-specific APIs.
/// Keep the original terminal object for protocol-owned, additive result fields.
class PairingReceiptEnvelope {
  const PairingReceiptEnvelope(this.status, this.terminalData);

  final RhythmPairingResultStatus status;
  final Object? terminalData;
}

Future<PairingReceiptEnvelope?> readPairingReceipt(
  Dio dio,
  String sessionId, {
  String? expectedHubType,
}) async {
  try {
    final response = await dio.get(
      'api/devices/pair/${Uri.encodeComponent(sessionId)}',
      options: Options(
        receiveTimeout: const Duration(seconds: 10),
        validateStatus: (status) => status == 200 || status == 404,
      ),
    );
    final data = jsonMap(response.data);
    if (data == null) return null;
    final receipt = RhythmPairingResultStatus.fromJson(data);
    if (receipt.sessionId != sessionId ||
        (expectedHubType != null &&
            receipt.hubType != null &&
            receipt.hubType != expectedHubType)) {
      return null;
    }
    final matchesHttp = switch (response.statusCode) {
      200 => receipt.state != RhythmPairingResultState.notFound,
      404 => receipt.state == RhythmPairingResultState.notFound,
      _ => false,
    };
    return matchesHttp ? PairingReceiptEnvelope(receipt, data['result']) : null;
  } catch (_) {
    // A failed/malformed read is never the Box's causal not-found fence.
    return null;
  }
}
