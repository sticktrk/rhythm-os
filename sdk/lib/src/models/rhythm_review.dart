import '../json_parsing.dart';

class RhythmReviewSummary {
  final RhythmReviewCounts pending;
  final List<RhythmReviewHub> disconnectedHubs;
  final List<RhythmPreferredEndpoint> preferredEndpoints;
  final List<RhythmReviewEntry> hubConfiguredConflicts;
  final List<RhythmReviewEntry> triageEntries;

  const RhythmReviewSummary({
    this.pending = const RhythmReviewCounts(),
    this.disconnectedHubs = const [],
    this.preferredEndpoints = const [],
    this.hubConfiguredConflicts = const [],
    this.triageEntries = const [],
  });

  bool get hasPending => pending.total > 0;
  bool get hasAttention =>
      hasPending ||
      disconnectedHubs.isNotEmpty ||
      preferredEndpoints.isNotEmpty ||
      hubConfiguredConflicts.isNotEmpty;

  List<RhythmReviewEntry> get resolvedEntries =>
      triageEntries.where((entry) => !entry.isPending).toList(growable: false);

  factory RhythmReviewSummary.fromJson(Map<String, dynamic> json) {
    final pendingJson = jsonMap(json['pending']) ?? const <String, dynamic>{};

    List<T> parseList<T>(
      Object? value,
      T Function(Map<String, dynamic> json) parse,
    ) {
      return (value as List<dynamic>? ?? const <dynamic>[])
          .map(jsonMap)
          .nonNulls
          .map(parse)
          .toList(growable: false);
    }

    return RhythmReviewSummary(
      pending: RhythmReviewCounts.fromJson(pendingJson),
      disconnectedHubs:
          parseList(json['disconnected_hubs'], RhythmReviewHub.fromJson),
      preferredEndpoints: parseList(
        json['preferred_endpoints'],
        RhythmPreferredEndpoint.fromJson,
      ),
      hubConfiguredConflicts: parseList(
        json['hub_configured_conflicts'],
        RhythmReviewEntry.fromJson,
      ),
      triageEntries:
          parseList(json['triage_entries'], RhythmReviewEntry.fromJson),
    );
  }
}

class RhythmReviewCounts {
  final int devices;
  final int rooms;
  final int unassigned;
  final int hubConfigured;
  final int total;

  const RhythmReviewCounts({
    this.devices = 0,
    this.rooms = 0,
    this.unassigned = 0,
    this.hubConfigured = 0,
    this.total = 0,
  });

  factory RhythmReviewCounts.fromJson(Map<String, dynamic> json) {
    return RhythmReviewCounts(
      devices: jsonInt(json['devices'], preferredKeys: const ['devices']) ?? 0,
      rooms: jsonInt(json['rooms'], preferredKeys: const ['rooms']) ?? 0,
      unassigned:
          jsonInt(json['unassigned'], preferredKeys: const ['unassigned']) ?? 0,
      hubConfigured: jsonInt(
            json['hub_configured'],
            preferredKeys: const ['hub_configured'],
          ) ??
          0,
      total: jsonInt(json['total'], preferredKeys: const ['total']) ?? 0,
    );
  }
}

class RhythmReviewHub {
  final String hubType;
  final String address;

  const RhythmReviewHub({
    required this.hubType,
    required this.address,
  });

  String get label => [hubTypeLabel(hubType), address]
      .where((part) => part.isNotEmpty)
      .join(' - ');

  factory RhythmReviewHub.fromJson(Map<String, dynamic> json) {
    return RhythmReviewHub(
      hubType: json['type'] as String? ?? '',
      address: json['address'] as String? ?? '',
    );
  }
}

class RhythmPreferredEndpoint {
  final String canonicalId;
  final String name;
  final String hubType;
  final String hubAddress;
  final String nativeId;
  final int endpointCount;

  const RhythmPreferredEndpoint({
    required this.canonicalId,
    required this.name,
    required this.hubType,
    required this.hubAddress,
    required this.nativeId,
    required this.endpointCount,
  });

  String get hubLabel => [hubTypeLabel(hubType), hubAddress]
      .where((part) => part.isNotEmpty)
      .join(' - ');

  factory RhythmPreferredEndpoint.fromJson(Map<String, dynamic> json) {
    return RhythmPreferredEndpoint(
      canonicalId: json['canonical_id'] as String? ?? '',
      name: json['name'] as String? ?? '',
      hubType: json['type'] as String? ?? '',
      hubAddress: json['hub_address'] as String? ?? '',
      nativeId: json['native_id'] as String? ?? '',
      endpointCount: jsonInt(
            json['endpoint_count'],
            preferredKeys: const ['endpoint_count'],
          ) ??
          0,
    );
  }
}

class RhythmReviewEntry {
  final String id;
  final String kind;
  final String status;
  final String hubType;
  final String hubAddress;
  final String nativeId;
  final String name;
  final int createdAt;
  final int? resolvedAt;
  final String? resolvedBy;
  final String summary;
  final String? guidance;

  const RhythmReviewEntry({
    required this.id,
    required this.kind,
    required this.status,
    required this.hubType,
    required this.hubAddress,
    required this.nativeId,
    required this.name,
    required this.createdAt,
    this.resolvedAt,
    this.resolvedBy,
    required this.summary,
    this.guidance,
  });

  bool get isPending => status == 'pending';
  bool get isKeepSeparate =>
      status == 'new_device' || status == 'kept_separate';
  bool get isDismissed => status == 'dismissed';
  bool get isConfirmed => status == 'confirmed' || status == 'auto_resolved';

  DateTime get createdAtDateTime =>
      DateTime.fromMillisecondsSinceEpoch(createdAt * 1000);

  DateTime? get resolvedAtDateTime => resolvedAt == null
      ? null
      : DateTime.fromMillisecondsSinceEpoch(resolvedAt! * 1000);

  String get statusLabel {
    return switch (status) {
      'pending' => 'Pending',
      'confirmed' => 'Confirmed',
      'auto_resolved' => 'Auto-resolved',
      'new_device' => 'Kept separate',
      'kept_separate' => 'Kept separate',
      'dismissed' => 'Dismissed',
      _ => status,
    };
  }

  factory RhythmReviewEntry.fromJson(Map<String, dynamic> json) {
    return RhythmReviewEntry(
      id: json['id'] as String? ?? '',
      kind: json['kind'] as String? ?? '',
      status: json['status'] as String? ?? '',
      hubType: json['type'] as String? ?? '',
      hubAddress: json['hub_address'] as String? ?? '',
      nativeId: json['native_id'] as String? ?? '',
      name: json['name'] as String? ?? '',
      createdAt:
          jsonInt(json['created_at'], preferredKeys: const ['created_at']) ?? 0,
      resolvedAt:
          jsonInt(json['resolved_at'], preferredKeys: const ['resolved_at']),
      resolvedBy: json['resolved_by'] as String?,
      summary: json['summary'] as String? ?? '',
      guidance: json['guidance'] as String?,
    );
  }
}

String hubTypeLabel(String hubType) {
  return switch (hubType) {
    'homeassistant' || 'home_assistant' => 'Home Assistant',
    'hue' => 'Hue',
    'matter' => 'Matter',
    'esp32' => 'ESP32',
    _ => hubType,
  };
}
