import '../json_parsing.dart';

/// Startup retry state for a hub in `/api/state.hubs[].startup_retry`.
enum RhythmHubStartupRetryStatus {
  scheduled('scheduled'),
  manualRetryRequired('manual_retry_required');

  final String wireValue;

  const RhythmHubStartupRetryStatus(this.wireValue);

  static RhythmHubStartupRetryStatus? fromWireValue(String? value) {
    for (final status in values) {
      if (status.wireValue == value) return status;
    }
    return null;
  }
}

/// Backoff/retry metadata for a configured hub.
class RhythmHubStartupRetry {
  final RhythmHubStartupRetryStatus status;
  final int attemptCount;
  final int? firstFailureEpochMs;
  final int? lastFailureEpochMs;
  final int? nextRetryEpochMs;
  final String? lastError;

  const RhythmHubStartupRetry({
    required this.status,
    this.attemptCount = 0,
    this.firstFailureEpochMs,
    this.lastFailureEpochMs,
    this.nextRetryEpochMs,
    this.lastError,
  });

  bool get isScheduled => status == RhythmHubStartupRetryStatus.scheduled;
  bool get isManualRetryRequired =>
      status == RhythmHubStartupRetryStatus.manualRetryRequired;

  DateTime? get firstFailureAt => firstFailureEpochMs == null
      ? null
      : DateTime.fromMillisecondsSinceEpoch(firstFailureEpochMs!);

  DateTime? get lastFailureAt => lastFailureEpochMs == null
      ? null
      : DateTime.fromMillisecondsSinceEpoch(lastFailureEpochMs!);

  DateTime? get nextRetryAt => nextRetryEpochMs == null
      ? null
      : DateTime.fromMillisecondsSinceEpoch(nextRetryEpochMs!);

  factory RhythmHubStartupRetry.fromJson(Map<String, dynamic> json) {
    final status = RhythmHubStartupRetryStatus.fromWireValue(
      json['status'] as String?,
    );
    if (status == null) {
      throw ArgumentError.value(
        json['status'],
        'status',
        'Unsupported hub startup retry status',
      );
    }
    return RhythmHubStartupRetry(
      status: status,
      attemptCount: jsonInt(
            json['attempt_count'],
            preferredKeys: const ['attempt_count'],
          ) ??
          0,
      firstFailureEpochMs: jsonInt(
        json['first_failure_epoch_ms'],
        preferredKeys: const ['first_failure_epoch_ms'],
      ),
      lastFailureEpochMs: jsonInt(
        json['last_failure_epoch_ms'],
        preferredKeys: const ['last_failure_epoch_ms'],
      ),
      nextRetryEpochMs: jsonInt(
        json['next_retry_epoch_ms'],
        preferredKeys: const ['next_retry_epoch_ms'],
      ),
      lastError: json['last_error'] as String?,
    );
  }

  Map<String, dynamic> toJson() {
    return {
      'status': status.wireValue,
      'attempt_count': attemptCount,
      if (firstFailureEpochMs != null)
        'first_failure_epoch_ms': firstFailureEpochMs,
      if (lastFailureEpochMs != null)
        'last_failure_epoch_ms': lastFailureEpochMs,
      if (nextRetryEpochMs != null) 'next_retry_epoch_ms': nextRetryEpochMs,
      if (lastError != null) 'last_error': lastError,
    };
  }
}

/// A configured hub entry from `/api/state.hubs`.
class RhythmHubInfo {
  final String type;
  final String? address;
  final bool connected;
  final RhythmHubStartupRetry? startupRetry;

  const RhythmHubInfo({
    required this.type,
    this.address,
    required this.connected,
    this.startupRetry,
  });

  bool get configured => type.isNotEmpty && type != 'none';

  factory RhythmHubInfo.fromJson(Map<String, dynamic> json) {
    final startupRetryJson = jsonMap(json['startup_retry']);
    return RhythmHubInfo(
      type: json['type'] as String? ?? '',
      address: json['address'] as String?,
      connected: json['connected'] as bool? ?? false,
      startupRetry: startupRetryJson == null
          ? null
          : RhythmHubStartupRetry.fromJson(startupRetryJson),
    );
  }

  Map<String, dynamic> toJson() {
    return {
      'type': type,
      if (address != null) 'address': address,
      'connected': connected,
      if (startupRetry != null) 'startup_retry': startupRetry!.toJson(),
    };
  }
}
