enum ServerIdentityKind { provisional, durable }

String? normalizeServerIdentity(String? value) {
  final clean = value?.trim().toLowerCase();
  return clean == null || clean.isEmpty ? null : clean;
}

ServerIdentityKind? serverIdentityKind(String? value) {
  final clean = normalizeServerIdentity(value);
  if (clean == null) return null;
  return clean.startsWith('endpoint:')
      ? ServerIdentityKind.provisional
      : ServerIdentityKind.durable;
}

bool serverIdentitiesMatch(String? left, String? right) {
  final cleanLeft = normalizeServerIdentity(left);
  final cleanRight = normalizeServerIdentity(right);
  return cleanLeft != null && cleanRight != null && cleanLeft == cleanRight;
}

bool serverIdentitiesConflict(String? left, String? right) {
  final cleanLeft = normalizeServerIdentity(left);
  final cleanRight = normalizeServerIdentity(right);
  return cleanLeft != null &&
      cleanRight != null &&
      cleanLeft != cleanRight &&
      serverIdentityKind(cleanLeft) == ServerIdentityKind.durable &&
      serverIdentityKind(cleanRight) == ServerIdentityKind.durable;
}

String? preferredServerIdentity({
  String? existing,
  String? candidate,
}) {
  final cleanExisting = normalizeServerIdentity(existing);
  final cleanCandidate = normalizeServerIdentity(candidate);
  if (cleanCandidate == null) return cleanExisting;
  if (cleanExisting == null) return cleanCandidate;
  if (serverIdentityKind(cleanExisting) == ServerIdentityKind.durable &&
      serverIdentityKind(cleanCandidate) == ServerIdentityKind.provisional) {
    return cleanExisting;
  }
  return cleanCandidate;
}
