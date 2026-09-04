import '../json_parsing.dart';

enum RhythmDeviceAttentionStatus {
  pending,
  awaitingRecovery,
  unknown;

  static RhythmDeviceAttentionStatus fromWire(Object? value) => switch (value) {
        'pending' => RhythmDeviceAttentionStatus.pending,
        'awaiting_recovery' => RhythmDeviceAttentionStatus.awaitingRecovery,
        _ => RhythmDeviceAttentionStatus.unknown,
      };

  String get wireValue => switch (this) {
        RhythmDeviceAttentionStatus.pending => 'pending',
        RhythmDeviceAttentionStatus.awaitingRecovery => 'awaiting_recovery',
        RhythmDeviceAttentionStatus.unknown => 'unknown',
      };
}

class RhythmDeviceAttentionDevice {
  final String name;
  final String nativeId;
  final String hubType;
  final String hubAddress;
  final String deviceType;

  const RhythmDeviceAttentionDevice({
    required this.name,
    required this.nativeId,
    required this.hubType,
    required this.hubAddress,
    required this.deviceType,
  });

  factory RhythmDeviceAttentionDevice.fromJson(Map<String, dynamic> json) {
    return RhythmDeviceAttentionDevice(
      name: json['name'] as String? ?? 'Unknown device',
      nativeId: json['native_id'] as String? ?? '',
      hubType: json['hub_type'] as String? ?? '',
      hubAddress: json['hub_address'] as String? ?? '',
      deviceType: json['device_type'] as String? ?? 'light',
    );
  }
}

class RhythmDeviceAttentionEvidence {
  final int lastProofAt;
  final int firstFailureAt;
  final int lastFailureAt;
  final int failureCount;
  final List<String> failureClasses;
  final int createdAt;
  final int? snoozedUntil;

  const RhythmDeviceAttentionEvidence({
    required this.lastProofAt,
    required this.firstFailureAt,
    required this.lastFailureAt,
    required this.failureCount,
    this.failureClasses = const [],
    required this.createdAt,
    this.snoozedUntil,
  });

  factory RhythmDeviceAttentionEvidence.fromJson(Map<String, dynamic> json) {
    return RhythmDeviceAttentionEvidence(
      lastProofAt: jsonInt(json['last_proof_at']) ?? 0,
      firstFailureAt: jsonInt(json['first_failure_at']) ?? 0,
      lastFailureAt: jsonInt(json['last_failure_at']) ?? 0,
      failureCount: jsonInt(json['failure_count']) ?? 0,
      failureClasses: (json['failure_classes'] as List<dynamic>? ?? const [])
          .whereType<String>()
          .toList(growable: false),
      createdAt: jsonInt(json['created_at']) ?? 0,
      snoozedUntil: jsonInt(json['snoozed_until']),
    );
  }
}

class RhythmDeviceAttention {
  final String id;
  final String journeyId;
  final String kind;
  final RhythmDeviceAttentionStatus status;
  final RhythmDeviceAttentionDevice device;
  final RhythmDeviceAttentionEvidence evidence;
  final String guidance;

  const RhythmDeviceAttention({
    required this.id,
    required this.journeyId,
    required this.kind,
    required this.status,
    required this.device,
    required this.evidence,
    required this.guidance,
  });

  bool get isActionable =>
      id.isNotEmpty &&
      journeyId.isNotEmpty &&
      kind == 'unreachable_device' &&
      device.hubType == 'matter' &&
      device.nativeId.isNotEmpty &&
      status != RhythmDeviceAttentionStatus.unknown;

  factory RhythmDeviceAttention.fromJson(Map<String, dynamic> json) {
    return RhythmDeviceAttention(
      id: json['id'] as String? ?? '',
      journeyId: json['journey_id'] as String? ?? '',
      kind: json['kind'] as String? ?? '',
      status: RhythmDeviceAttentionStatus.fromWire(json['status']),
      device: RhythmDeviceAttentionDevice.fromJson(
        jsonMap(json['device']) ?? const {},
      ),
      evidence: RhythmDeviceAttentionEvidence.fromJson(
        jsonMap(json['evidence']) ?? const {},
      ),
      guidance: json['guidance'] as String? ?? '',
    );
  }
}
