/// Owner-only Wi-Fi metadata. Passwords are only returned by the explicit
/// no-store credential route and must never be cached by the app.
class RhythmWifiProfile {
  const RhythmWifiProfile({required this.id, required this.ssid});
  final String id;
  final String ssid;
  factory RhythmWifiProfile.fromJson(Map<String, dynamic> json) =>
      RhythmWifiProfile(id: json['id'] as String, ssid: json['ssid'] as String);
  @override
  String toString() => 'RhythmWifiProfile(<redacted>)';
}

class RhythmWifiProfiles {
  const RhythmWifiProfiles({
    required this.revision,
    required this.profiles,
    this.defaultId,
    this.boxProfileId,
  });
  final int revision;

  /// The network new accessories receive. Chosen by the owner.
  final String? defaultId;

  /// The network the Box itself uses. Read-only here: it follows the Box
  /// connection, and choosing a default never moves the Box.
  final String? boxProfileId;
  final List<RhythmWifiProfile> profiles;
  factory RhythmWifiProfiles.fromJson(Map<String, dynamic> json) =>
      RhythmWifiProfiles(
        revision: json['revision'] as int,
        defaultId: json['default_id'] as String?,
        boxProfileId: json['box_profile_id'] as String?,
        profiles: (json['profiles'] as List)
            .map(
              (p) => RhythmWifiProfile.fromJson(
                Map<String, dynamic>.from(p as Map),
              ),
            )
            .toList(),
      );
}

class RhythmWifiChangeReceipt {
  const RhythmWifiChangeReceipt({
    required this.operationId,
    required this.status,
    this.code,
    this.rollbackVerified = false,
    this.retryAfterMs = 0,
  });
  final String operationId;
  final String status;
  final String? code;
  final bool rollbackVerified;
  final int retryAfterMs;
  // Unknown future phases remain non-terminal and cannot enable another write.
  bool get isPending => status != 'complete';
  bool get succeeded => status == 'complete' && code == 'succeeded';
  factory RhythmWifiChangeReceipt.fromJson(Map<String, dynamic> json) {
    final outcome = json['outcome'] as Map?;
    return RhythmWifiChangeReceipt(
      operationId: json['operation_id'] as String,
      status: json['status'] as String,
      code: outcome?['code'] as String?,
      rollbackVerified: outcome?['rollback_verified'] == true,
      retryAfterMs: (json['retry_after_ms'] as num?)?.toInt() ?? 0,
    );
  }
}

/// Contains only a bounded category, never the server body, network or Dio error.
class RhythmWifiException implements Exception {
  const RhythmWifiException(this.category);
  final String category;
  @override
  String toString() => 'Wi-Fi request: $category';
}
