enum MatterSetupPayloadKind {
  qr,
  manual,
  unknown,
}

String normalizeMatterSetupPayload(String value) {
  return value.trim();
}

MatterSetupPayloadKind detectMatterSetupPayloadKind(String value) {
  final normalized = normalizeMatterSetupPayload(value);
  if (normalized.isEmpty) return MatterSetupPayloadKind.unknown;

  if (normalized.toUpperCase().startsWith('MT:')) {
    return MatterSetupPayloadKind.qr;
  }

  final compact = normalized.replaceAll(RegExp(r'[\s-]'), '');
  if (compact.length >= 11 && RegExp(r'^\d+$').hasMatch(compact)) {
    return MatterSetupPayloadKind.manual;
  }

  return MatterSetupPayloadKind.unknown;
}

bool isLikelyMatterSetupPayload(String value) {
  return detectMatterSetupPayloadKind(value) != MatterSetupPayloadKind.unknown;
}
