/// Dart-side JSON serialization for HueBehaviorTrackerDto.
///
/// Replaces the hand-rolled Rust JSON serializer/parser that was in
/// hue_registry.rs. JSON format uses the same keys for backwards
/// compatibility with existing persisted data.
///
/// Format:
/// ```json
/// {
///   "behavior_mappings": [["behavior-id", "device-id"], ...],
///   "configured_device_ids": ["device-id", ...]
/// }
/// ```
library;

import 'dart:convert';

import '../src/rust/api/dto/hue_registry.dart';

/// Serialize HueBehaviorTrackerDto to a JSON string.
String behaviorTrackerToJson(HueBehaviorTrackerDto tracker) {
  return jsonEncode({
    'behavior_mappings': tracker.behaviorMappings
        .map((m) => [m.behaviorId, m.deviceId])
        .toList(),
    'configured_device_ids': tracker.configuredDeviceIds,
  });
}

/// Deserialize a JSON string to HueBehaviorTrackerDto.
/// Returns null if the JSON is invalid.
HueBehaviorTrackerDto? behaviorTrackerFromJson(String json) {
  try {
    final map = jsonDecode(json) as Map<String, dynamic>;

    final mappings = (map['behavior_mappings'] as List<dynamic>? ?? [])
        .map((tuple) {
          final list = tuple as List<dynamic>;
          return BehaviorMappingDto(
            behaviorId: list[0] as String,
            deviceId: list[1] as String,
          );
        })
        .toList();

    final deviceIds =
        (map['configured_device_ids'] as List<dynamic>?)?.cast<String>() ?? [];

    return HueBehaviorTrackerDto(
      behaviorMappings: mappings,
      configuredDeviceIds: deviceIds,
    );
  } catch (_) {
    return null;
  }
}
