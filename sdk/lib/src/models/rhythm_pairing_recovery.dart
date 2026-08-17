enum RhythmPairingRecoveryPayloadKind {
  qrCode,
  manualCode;

  static RhythmPairingRecoveryPayloadKind? fromWire(String? value) =>
      switch (value) {
        'qr_code' => qrCode,
        'manual_code' => manualCode,
        _ => null,
      };
}

/// Owner-visible Matter setup material retained after commissioning.
class RhythmPairingRecoverySecret {
  const RhythmPairingRecoverySecret({
    required this.payloadKind,
    required this.setupPayload,
    required this.capturedAt,
  });

  final RhythmPairingRecoveryPayloadKind payloadKind;
  final String setupPayload;
  final DateTime capturedAt;

  factory RhythmPairingRecoverySecret.fromJson(Map<String, dynamic> json) {
    final payloadKindValue = json['payload_kind'];
    final setupPayloadValue = json['setup_payload'];
    final capturedAtValue = json['captured_at'];
    final payloadKind = RhythmPairingRecoveryPayloadKind.fromWire(
      payloadKindValue is String ? payloadKindValue : null,
    );
    final setupPayload =
        setupPayloadValue is String ? setupPayloadValue.trim() : '';
    final capturedAt = DateTime.tryParse(
      capturedAtValue is String ? capturedAtValue : '',
    );
    if (payloadKind == null || setupPayload.isEmpty || capturedAt == null) {
      throw const FormatException('Invalid pairing recovery response');
    }
    return RhythmPairingRecoverySecret(
      payloadKind: payloadKind,
      setupPayload: setupPayload,
      capturedAt: capturedAt,
    );
  }
}
