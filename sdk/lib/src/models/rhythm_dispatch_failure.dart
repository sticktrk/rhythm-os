import '../json_parsing.dart';

/// A hub light command that failed, timed out, or was dropped.
///
/// Dispatch is fire-and-forget server-side — commands are queued and paced
/// per hub — so physical delivery problems surface only through this event,
/// never as an error on the originating API call.
class RhythmDispatchFailure {
  final String hubType;
  final String hubKey;

  /// Topology node the command addressed.
  final String nodeId;

  /// Canonical light-device node whose physical endpoint failed, when the
  /// appliance could resolve the hub-native target exactly.
  ///
  /// Older appliances omit this additive field. Clients must not infer the
  /// device from [target], which is scoped to one integration and is not a
  /// durable identity.
  final String? targetNodeId;

  /// Hub-native dispatch target label.
  final String target;

  /// Command kind: "turn_on" or "turn_off".
  final String kind;

  /// Failure class: "failed", "timed_out", "skipped_cooldown", "dropped".
  final String status;

  /// Human-readable failure detail, when available.
  final String? detail;

  /// Time the command waited in the hub queue.
  final int queuedMs;

  /// Time spent in the physical dispatch attempt.
  final int dispatchMs;

  final int epochMs;

  const RhythmDispatchFailure({
    required this.hubType,
    required this.hubKey,
    required this.nodeId,
    this.targetNodeId,
    required this.target,
    required this.kind,
    required this.status,
    this.detail,
    this.queuedMs = 0,
    this.dispatchMs = 0,
    this.epochMs = 0,
  });

  factory RhythmDispatchFailure.fromJson(Map<String, dynamic> json) {
    return RhythmDispatchFailure(
      hubType: json['hub_type'] as String? ?? '',
      hubKey: json['hub_key'] as String? ?? '',
      nodeId: json['node_id'] as String? ?? '',
      targetNodeId: json['target_node_id'] as String?,
      target: json['target'] as String? ?? '',
      kind: json['kind'] as String? ?? '',
      status: json['status'] as String? ?? '',
      detail: json['detail'] as String?,
      queuedMs:
          jsonInt(json['queued_ms'], preferredKeys: const ['queued_ms']) ?? 0,
      dispatchMs:
          jsonInt(json['dispatch_ms'], preferredKeys: const ['dispatch_ms']) ??
              0,
      epochMs:
          jsonInt(json['epoch_ms'], preferredKeys: const ['epoch_ms']) ?? 0,
    );
  }

  @override
  String toString() =>
      'RhythmDispatchFailure($kind $target via $hubType: $status'
      '${detail != null ? ' — $detail' : ''})';
}
