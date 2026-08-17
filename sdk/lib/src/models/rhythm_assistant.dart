import '../json_parsing.dart';
import 'rhythm_room.dart';

abstract final class RhythmAssistantOperationId {
  static const String getTopologySnapshot = 'get_topology_snapshot';
  static const String identifyDevice = 'identify_device';
  static const String planMoveDeviceRoom = 'plan_move_device_room';
  static const String applyMoveDeviceRoomPlan = 'apply_move_device_room_plan';
}

class RhythmAssistantOperation {
  final String id;
  final String description;
  final String method;
  final String path;
  final String effect;
  final String confirmation;
  final String freshnessPrecondition;
  final String verification;
  final bool physicalConfirmationRequired;
  final Map<String, dynamic> inputSchema;
  final Map<String, dynamic> resultSchema;

  const RhythmAssistantOperation({
    required this.id,
    required this.description,
    required this.method,
    required this.path,
    required this.effect,
    required this.confirmation,
    required this.freshnessPrecondition,
    required this.verification,
    required this.physicalConfirmationRequired,
    this.inputSchema = const {},
    this.resultSchema = const {},
  });

  factory RhythmAssistantOperation.fromJson(Map<String, dynamic> json) {
    return RhythmAssistantOperation(
      id: json['id'] as String? ?? '',
      description: json['description'] as String? ?? '',
      method: json['method'] as String? ?? '',
      path: json['path'] as String? ?? '',
      effect: json['effect'] as String? ?? '',
      confirmation: json['confirmation'] as String? ?? '',
      freshnessPrecondition: json['freshness_precondition'] as String? ?? '',
      verification: json['verification'] as String? ?? '',
      physicalConfirmationRequired:
          json['physical_confirmation_required'] as bool? ?? false,
      inputSchema: jsonMap(json['input_schema']) ?? const {},
      resultSchema: jsonMap(json['result_schema']) ?? const {},
    );
  }
}

class RhythmAssistantContract {
  final int schemaVersion;
  final String contractSha256;
  final String serverVersion;
  final String topologySnapshotPath;
  final List<RhythmAssistantOperation> operations;

  const RhythmAssistantContract({
    required this.schemaVersion,
    required this.contractSha256,
    required this.serverVersion,
    required this.topologySnapshotPath,
    this.operations = const [],
  });

  factory RhythmAssistantContract.fromJson(Map<String, dynamic> json) {
    return RhythmAssistantContract(
      schemaVersion: jsonInt(
            json['schema_version'],
            preferredKeys: const ['schema_version'],
          ) ??
          0,
      contractSha256: json['contract_sha256'] as String? ?? '',
      serverVersion: json['server_version'] as String? ?? '',
      topologySnapshotPath: json['topology_snapshot_path'] as String? ?? '',
      operations: ((json['operations'] as List<dynamic>?) ?? const [])
          .map(jsonMap)
          .nonNulls
          .map(RhythmAssistantOperation.fromJson)
          .where((operation) => operation.id.isNotEmpty)
          .toList(),
    );
  }

  RhythmAssistantOperation? operation(String id) {
    for (final operation in operations) {
      if (operation.id == id) return operation;
    }
    return null;
  }

  bool supportsOperation(String id) => operation(id) != null;
}

class RhythmAssistantTopologySnapshot {
  final int schemaVersion;
  final String contractSha256;
  final String serverInstanceId;
  final String serverVersion;
  final int observedAtEpochMs;
  final String topologyResourceSha256;
  final List<RhythmTopologyNode> nodes;

  const RhythmAssistantTopologySnapshot({
    required this.schemaVersion,
    required this.contractSha256,
    required this.serverInstanceId,
    required this.serverVersion,
    required this.observedAtEpochMs,
    required this.topologyResourceSha256,
    this.nodes = const [],
  });

  factory RhythmAssistantTopologySnapshot.fromJson(
    Map<String, dynamic> json,
  ) {
    return RhythmAssistantTopologySnapshot(
      schemaVersion: jsonInt(
            json['schema_version'],
            preferredKeys: const ['schema_version'],
          ) ??
          0,
      contractSha256: json['contract_sha256'] as String? ?? '',
      serverInstanceId: json['server_instance_id'] as String? ?? '',
      serverVersion: json['server_version'] as String? ?? '',
      observedAtEpochMs: jsonInt(
            json['observed_at_epoch_ms'],
            preferredKeys: const ['observed_at_epoch_ms'],
          ) ??
          0,
      topologyResourceSha256: json['topology_resource_sha256'] as String? ?? '',
      nodes: ((json['nodes'] as List<dynamic>?) ?? const [])
          .map(jsonMap)
          .nonNulls
          .map(RhythmTopologyNode.fromJson)
          .toList(),
    );
  }
}

class RhythmAssistantNodeRef {
  final String id;
  final String name;

  const RhythmAssistantNodeRef({required this.id, required this.name});

  factory RhythmAssistantNodeRef.fromJson(Map<String, dynamic> json) {
    return RhythmAssistantNodeRef(
      id: json['id'] as String? ?? '',
      name: json['name'] as String? ?? '',
    );
  }
}

class RhythmAssistantMovePlan {
  final int schemaVersion;
  final String planId;
  final String correlationId;
  final String operation;
  final String contractSha256;
  final String serverInstanceId;
  final String topologyResourceSha256;
  final RhythmAssistantNodeRef device;
  final RhythmAssistantNodeRef? fromRoom;
  final RhythmAssistantNodeRef toRoom;
  final String resultingPlacement;
  final bool requiresConfirmation;
  final List<String> warnings;

  const RhythmAssistantMovePlan({
    required this.schemaVersion,
    required this.planId,
    required this.correlationId,
    required this.operation,
    required this.contractSha256,
    required this.serverInstanceId,
    required this.topologyResourceSha256,
    required this.device,
    required this.fromRoom,
    required this.toRoom,
    required this.resultingPlacement,
    required this.requiresConfirmation,
    this.warnings = const [],
  });

  factory RhythmAssistantMovePlan.fromJson(Map<String, dynamic> json) {
    final device = jsonMap(json['device']) ?? const <String, dynamic>{};
    final toRoom = jsonMap(json['to_room']) ?? const <String, dynamic>{};
    final fromRoom = jsonMap(json['from_room']);
    return RhythmAssistantMovePlan(
      schemaVersion: jsonInt(
            json['schema_version'],
            preferredKeys: const ['schema_version'],
          ) ??
          0,
      planId: json['plan_id'] as String? ?? '',
      correlationId: json['correlation_id'] as String? ?? '',
      operation: json['operation'] as String? ?? '',
      contractSha256: json['contract_sha256'] as String? ?? '',
      serverInstanceId: json['server_instance_id'] as String? ?? '',
      topologyResourceSha256: json['topology_resource_sha256'] as String? ?? '',
      device: RhythmAssistantNodeRef.fromJson(device),
      fromRoom:
          fromRoom == null ? null : RhythmAssistantNodeRef.fromJson(fromRoom),
      toRoom: RhythmAssistantNodeRef.fromJson(toRoom),
      resultingPlacement: json['resulting_placement'] as String? ?? '',
      requiresConfirmation: json['requires_confirmation'] as bool? ?? false,
      warnings: ((json['warnings'] as List<dynamic>?) ?? const [])
          .whereType<String>()
          .toList(),
    );
  }

  Map<String, dynamic> toApplyJson() {
    return {
      'plan_id': planId,
      'correlation_id': correlationId,
      'operation': operation,
      'contract_sha256': contractSha256,
      'server_instance_id': serverInstanceId,
      'topology_resource_sha256': topologyResourceSha256,
      'device_id': device.id,
      'from_room_id': fromRoom?.id,
      'to_room_id': toRoom.id,
    };
  }
}

class RhythmAssistantExecutionReceipt {
  final int schemaVersion;
  final String planId;
  final String correlationId;
  final String operation;
  final String status;
  final String contractSha256;
  final String serverInstanceId;
  final String topologyResourceSha256;
  final String deviceId;
  final String? previousParentId;
  final String currentParentId;
  final String resultingPlacement;
  final bool serverAcknowledged;
  final bool canonicalReadbackVerified;
  final String physicalVerification;
  final int completedAtEpochMs;

  const RhythmAssistantExecutionReceipt({
    required this.schemaVersion,
    required this.planId,
    required this.correlationId,
    required this.operation,
    required this.status,
    required this.contractSha256,
    required this.serverInstanceId,
    required this.topologyResourceSha256,
    required this.deviceId,
    required this.previousParentId,
    required this.currentParentId,
    required this.resultingPlacement,
    required this.serverAcknowledged,
    required this.canonicalReadbackVerified,
    required this.physicalVerification,
    required this.completedAtEpochMs,
  });

  factory RhythmAssistantExecutionReceipt.fromJson(
    Map<String, dynamic> json,
  ) {
    return RhythmAssistantExecutionReceipt(
      schemaVersion: jsonInt(
            json['schema_version'],
            preferredKeys: const ['schema_version'],
          ) ??
          0,
      planId: json['plan_id'] as String? ?? '',
      correlationId: json['correlation_id'] as String? ?? '',
      operation: json['operation'] as String? ?? '',
      status: json['status'] as String? ?? '',
      contractSha256: json['contract_sha256'] as String? ?? '',
      serverInstanceId: json['server_instance_id'] as String? ?? '',
      topologyResourceSha256: json['topology_resource_sha256'] as String? ?? '',
      deviceId: json['device_id'] as String? ?? '',
      previousParentId: json['previous_parent_id'] as String?,
      currentParentId: json['current_parent_id'] as String? ?? '',
      resultingPlacement: json['resulting_placement'] as String? ?? '',
      serverAcknowledged: json['server_acknowledged'] as bool? ?? false,
      canonicalReadbackVerified:
          json['canonical_readback_verified'] as bool? ?? false,
      physicalVerification: json['physical_verification'] as String? ?? '',
      completedAtEpochMs: jsonInt(
            json['completed_at_epoch_ms'],
            preferredKeys: const ['completed_at_epoch_ms'],
          ) ??
          0,
    );
  }
}

class RhythmAssistantError {
  final String code;
  final String message;
  final bool retryable;
  final bool mutationMayHaveApplied;

  const RhythmAssistantError({
    required this.code,
    required this.message,
    required this.retryable,
    required this.mutationMayHaveApplied,
  });

  factory RhythmAssistantError.fromJson(Map<String, dynamic> json) {
    final error = jsonMap(json['error']) ?? json;
    return RhythmAssistantError(
      code: error['code'] as String? ?? '',
      message: error['message'] as String? ?? '',
      retryable: error['retryable'] as bool? ?? false,
      mutationMayHaveApplied:
          error['mutation_may_have_applied'] as bool? ?? false,
    );
  }
}
