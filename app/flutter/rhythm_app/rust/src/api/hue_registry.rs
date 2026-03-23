//! FFI functions for Hue behavior tracking.
//!
//! These functions expose the HueBehaviorTracker to Flutter via
//! flutter_rust_bridge, using a functional pattern (take state, return new state).

use rhythm_hue::HueBehaviorTracker;

use super::dto::{
    BehaviorMappingDto, BehaviorRemoveResultDto, HueBehaviorTrackerDto, HueButtonDto, HueRoomDto,
    HueSwitchDeviceDto,
};

// ============================================================================
// Behavior Tracker Functions
// ============================================================================

/// Create a new empty behavior tracker.
///
/// # Example (Dart)
///
/// ```dart
/// final tracker = createBehaviorTracker();
/// ```
pub fn create_behavior_tracker() -> HueBehaviorTrackerDto {
    HueBehaviorTrackerDto::default()
}

/// Add a behavior_instance to device mapping.
///
/// Returns a new tracker with the behavior added.
///
/// # Arguments
///
/// * `tracker` - Current tracker state
/// * `behavior_id` - The behavior_instance resource ID
/// * `device_id` - The device resource ID this behavior configures
///
/// # Example (Dart)
///
/// ```dart
/// tracker = behaviorTrackerAdd(
///   tracker: tracker,
///   behaviorId: "behavior-uuid",
///   deviceId: "device-uuid",
/// );
/// ```
pub fn behavior_tracker_add(
    tracker: HueBehaviorTrackerDto,
    behavior_id: String,
    device_id: String,
) -> HueBehaviorTrackerDto {
    let mut rust_tracker: HueBehaviorTracker = tracker.into();
    rust_tracker.add_behavior(&behavior_id, &device_id);
    rust_tracker.into()
}

/// Remove a behavior_instance.
///
/// Returns the new tracker state and whether a device was unconfigured.
///
/// # Arguments
///
/// * `tracker` - Current tracker state
/// * `behavior_id` - The behavior_instance resource ID that was deleted
///
/// # Example (Dart)
///
/// ```dart
/// final result = behaviorTrackerRemove(
///   tracker: tracker,
///   behaviorId: "behavior-uuid",
/// );
/// tracker = result.tracker;
/// if (result.deviceUnconfigured) {
///   print("Device is now available for Rhythm");
/// }
/// ```
pub fn behavior_tracker_remove(
    tracker: HueBehaviorTrackerDto,
    behavior_id: String,
) -> BehaviorRemoveResultDto {
    let mut rust_tracker: HueBehaviorTracker = tracker.into();
    let device_unconfigured = rust_tracker.remove_behavior(&behavior_id);
    BehaviorRemoveResultDto {
        tracker: rust_tracker.into(),
        device_unconfigured,
    }
}

/// Check if a device is configured in the Hue app.
///
/// Returns `true` if the device has at least one behavior_instance,
/// meaning Hue handles its events and Rhythm should ignore them.
///
/// # Arguments
///
/// * `tracker` - Current tracker state
/// * `device_id` - The device resource ID to check
///
/// # Example (Dart)
///
/// ```dart
/// if (!behaviorTrackerIsConfigured(tracker: tracker, deviceId: deviceId)) {
///   // Process event with Rhythm
/// }
/// ```
pub fn behavior_tracker_is_configured(tracker: HueBehaviorTrackerDto, device_id: String) -> bool {
    let rust_tracker: HueBehaviorTracker = tracker.into();
    rust_tracker.is_device_configured(&device_id)
}

/// Get all configured device IDs.
///
/// # Arguments
///
/// * `tracker` - Current tracker state
///
/// # Example (Dart)
///
/// ```dart
/// final deviceIds = behaviorTrackerConfiguredDevices(tracker: tracker);
/// for (final id in deviceIds) {
///   print("Configured: $id");
/// }
/// ```
pub fn behavior_tracker_configured_devices(tracker: HueBehaviorTrackerDto) -> Vec<String> {
    tracker.configured_device_ids
}

/// Get the number of tracked behaviors.
pub fn behavior_tracker_behavior_count(tracker: HueBehaviorTrackerDto) -> i32 {
    tracker.behavior_mappings.len() as i32
}

/// Get the number of configured devices.
pub fn behavior_tracker_device_count(tracker: HueBehaviorTrackerDto) -> i32 {
    tracker.configured_device_ids.len() as i32
}

/// Clear all tracked data.
///
/// Returns an empty tracker.
pub fn behavior_tracker_clear(_tracker: HueBehaviorTrackerDto) -> HueBehaviorTrackerDto {
    HueBehaviorTrackerDto::default()
}

// ============================================================================
// Serialization Functions
// ============================================================================

/// Serialize behavior tracker to JSON string for persistence.
///
/// # Arguments
///
/// * `tracker` - Current tracker state
///
/// # Example (Dart)
///
/// ```dart
/// final json = behaviorTrackerToJson(tracker: tracker);
/// await prefs.setString('behavior_tracker', json);
/// ```
pub fn behavior_tracker_to_json(tracker: HueBehaviorTrackerDto) -> String {
    let mappings_json: Vec<String> = tracker
        .behavior_mappings
        .iter()
        .map(|m| {
            format!(
                r#"["{}", "{}"]"#,
                escape_json_string(&m.behavior_id),
                escape_json_string(&m.device_id)
            )
        })
        .collect();

    let devices_json: Vec<String> = tracker
        .configured_device_ids
        .iter()
        .map(|id| format!("\"{}\"", escape_json_string(id)))
        .collect();

    format!(
        r#"{{"behavior_mappings":[{}],"configured_device_ids":[{}]}}"#,
        mappings_json.join(","),
        devices_json.join(",")
    )
}

/// Deserialize behavior tracker from JSON string.
///
/// Returns None if the JSON is invalid.
///
/// # Arguments
///
/// * `json` - JSON string from `behavior_tracker_to_json`
///
/// # Example (Dart)
///
/// ```dart
/// final json = await prefs.getString('behavior_tracker');
/// if (json != null) {
///   final tracker = behaviorTrackerFromJson(json: json);
///   if (tracker != null) {
///     _behaviorTracker = tracker;
///   }
/// }
/// ```
pub fn behavior_tracker_from_json(json: String) -> Option<HueBehaviorTrackerDto> {
    parse_behavior_tracker_json(&json)
}

// ============================================================================
// Device/Button/Room Helper Functions (for completeness)
// ============================================================================

/// Create a new switch device DTO.
pub fn create_switch_device(
    id: String,
    name: String,
    product_name: Option<String>,
    button_ids: Vec<String>,
    room_id: Option<String>,
) -> HueSwitchDeviceDto {
    HueSwitchDeviceDto {
        id,
        name,
        product_name,
        button_ids,
        room_id,
    }
}

/// Create a new button DTO.
pub fn create_button(id: String, control_id: u8, owner_device_id: String) -> HueButtonDto {
    HueButtonDto {
        id,
        control_id,
        owner_device_id,
    }
}

/// Create a new room DTO.
pub fn create_room(id: String, name: String) -> HueRoomDto {
    HueRoomDto { id, name }
}

// ============================================================================
// JSON Helper Functions
// ============================================================================

fn escape_json_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn parse_behavior_tracker_json(json: &str) -> Option<HueBehaviorTrackerDto> {
    let json = json.trim();
    if !json.starts_with('{') || !json.ends_with('}') {
        return None;
    }

    let behavior_mappings = parse_behavior_mappings(json)?;
    let configured_device_ids = parse_string_array(json, "configured_device_ids")?;

    Some(HueBehaviorTrackerDto {
        behavior_mappings,
        configured_device_ids,
    })
}

fn parse_behavior_mappings(json: &str) -> Option<Vec<BehaviorMappingDto>> {
    let pattern = "\"behavior_mappings\"";
    let field_start = json.find(pattern)?;
    let array_start = json[field_start..].find('[')? + field_start;
    let array_end = find_matching_bracket(json, array_start)?;

    let array_content = &json[array_start + 1..array_end];
    let mut mappings = Vec::new();

    // Parse array of [behavior_id, device_id] tuples
    let mut i = 0;
    while i < array_content.len() {
        if let Some(tuple_start) = array_content[i..].find('[') {
            let tuple_start = i + tuple_start;
            if let Some(tuple_end) = find_matching_bracket(array_content, tuple_start) {
                let tuple_content = &array_content[tuple_start + 1..tuple_end];
                if let Some((behavior_id, device_id)) = parse_string_tuple(tuple_content) {
                    mappings.push(BehaviorMappingDto {
                        behavior_id,
                        device_id,
                    });
                }
                i = tuple_end + 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    Some(mappings)
}

fn parse_string_tuple(content: &str) -> Option<(String, String)> {
    // Parse ["string1", "string2"]
    let mut strings = Vec::new();
    let mut i = 0;

    while strings.len() < 2 && i < content.len() {
        if let Some(str_start) = content[i..].find('"') {
            let str_start = i + str_start + 1;
            if let Some(str_end) = find_string_end(content, str_start) {
                strings.push(unescape_json_string(&content[str_start..str_end]));
                i = str_end + 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    if strings.len() == 2 {
        Some((strings.remove(0), strings.remove(0)))
    } else {
        None
    }
}

fn parse_string_array(json: &str, field: &str) -> Option<Vec<String>> {
    let pattern = format!("\"{}\"", field);
    let field_start = json.find(&pattern)?;
    let array_start = json[field_start..].find('[')? + field_start;
    let array_end = find_matching_bracket(json, array_start)?;

    let array_content = &json[array_start + 1..array_end];
    let mut result = Vec::new();

    let mut i = 0;
    while i < array_content.len() {
        if let Some(str_start) = array_content[i..].find('"') {
            let str_start = i + str_start + 1;
            if let Some(str_end) = find_string_end(array_content, str_start) {
                result.push(unescape_json_string(&array_content[str_start..str_end]));
                i = str_end + 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    Some(result)
}

fn find_matching_bracket(s: &str, start: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let open = bytes[start];
    let close = match open {
        b'[' => b']',
        b'{' => b'}',
        _ => return None,
    };

    let mut depth = 1;
    let mut i = start + 1;
    let mut in_string = false;
    let mut escape = false;

    while i < bytes.len() && depth > 0 {
        if escape {
            escape = false;
        } else if bytes[i] == b'\\' {
            escape = true;
        } else if bytes[i] == b'"' {
            in_string = !in_string;
        } else if !in_string {
            if bytes[i] == open {
                depth += 1;
            } else if bytes[i] == close {
                depth -= 1;
            }
        }
        i += 1;
    }

    if depth == 0 {
        Some(i - 1)
    } else {
        None
    }
}

fn find_string_end(s: &str, start: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
        } else if bytes[i] == b'"' {
            return Some(i);
        } else {
            i += 1;
        }
    }
    None
}

fn unescape_json_string(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                chars.next();
                match next {
                    '"' => result.push('"'),
                    '\\' => result.push('\\'),
                    'n' => result.push('\n'),
                    't' => result.push('\t'),
                    'r' => result.push('\r'),
                    _ => {
                        result.push('\\');
                        result.push(next);
                    }
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_add_behavior() {
        let tracker = create_behavior_tracker();
        assert_eq!(behavior_tracker_behavior_count(tracker.clone()), 0);

        let tracker = behavior_tracker_add(tracker, "b1".to_string(), "d1".to_string());
        assert_eq!(behavior_tracker_behavior_count(tracker.clone()), 1);
        assert!(behavior_tracker_is_configured(tracker, "d1".to_string()));
    }

    #[test]
    fn test_remove_behavior() {
        let tracker = create_behavior_tracker();
        let tracker = behavior_tracker_add(tracker, "b1".to_string(), "d1".to_string());
        let tracker = behavior_tracker_add(tracker, "b2".to_string(), "d1".to_string());

        // Remove one behavior - device should still be configured
        let result = behavior_tracker_remove(tracker, "b1".to_string());
        assert!(!result.device_unconfigured);
        assert!(behavior_tracker_is_configured(
            result.tracker.clone(),
            "d1".to_string()
        ));

        // Remove second behavior - device should be unconfigured
        let result = behavior_tracker_remove(result.tracker, "b2".to_string());
        assert!(result.device_unconfigured);
        assert!(!behavior_tracker_is_configured(
            result.tracker,
            "d1".to_string()
        ));
    }

    #[test]
    fn test_json_roundtrip() {
        let tracker = create_behavior_tracker();
        let tracker = behavior_tracker_add(tracker, "behavior-1".to_string(), "device-1".to_string());
        let tracker = behavior_tracker_add(tracker, "behavior-2".to_string(), "device-2".to_string());
        let tracker = behavior_tracker_add(tracker, "behavior-3".to_string(), "device-1".to_string());

        let json = behavior_tracker_to_json(tracker.clone());
        let restored = behavior_tracker_from_json(json).expect("Should parse");

        assert_eq!(
            behavior_tracker_behavior_count(restored.clone()),
            behavior_tracker_behavior_count(tracker.clone())
        );
        assert_eq!(
            behavior_tracker_device_count(restored.clone()),
            behavior_tracker_device_count(tracker)
        );
        assert!(behavior_tracker_is_configured(
            restored.clone(),
            "device-1".to_string()
        ));
        assert!(behavior_tracker_is_configured(
            restored,
            "device-2".to_string()
        ));
    }

    #[test]
    fn test_configured_devices() {
        let tracker = create_behavior_tracker();
        let tracker = behavior_tracker_add(tracker, "b1".to_string(), "d1".to_string());
        let tracker = behavior_tracker_add(tracker, "b2".to_string(), "d2".to_string());

        let devices = behavior_tracker_configured_devices(tracker);
        assert_eq!(devices.len(), 2);
        assert!(devices.contains(&"d1".to_string()));
        assert!(devices.contains(&"d2".to_string()));
    }

    #[test]
    fn test_clear() {
        let tracker = create_behavior_tracker();
        let tracker = behavior_tracker_add(tracker, "b1".to_string(), "d1".to_string());
        let tracker = behavior_tracker_clear(tracker);

        assert_eq!(behavior_tracker_behavior_count(tracker.clone()), 0);
        assert_eq!(behavior_tracker_device_count(tracker), 0);
    }

    #[test]
    fn test_create_device() {
        let device = create_switch_device(
            "device-1".to_string(),
            "Living Room Switch".to_string(),
            Some("Hue dimmer switch".to_string()),
            vec!["btn-1".to_string(), "btn-2".to_string()],
            Some("room-1".to_string()),
        );

        assert_eq!(device.id, "device-1");
        assert_eq!(device.name, "Living Room Switch");
        assert_eq!(device.product_name, Some("Hue dimmer switch".to_string()));
        assert_eq!(device.button_ids.len(), 2);
        assert_eq!(device.room_id, Some("room-1".to_string()));
    }

    #[test]
    fn test_create_button() {
        let button = create_button("btn-1".to_string(), 1, "device-1".to_string());

        assert_eq!(button.id, "btn-1");
        assert_eq!(button.control_id, 1);
        assert_eq!(button.owner_device_id, "device-1");
    }

    #[test]
    fn test_create_room() {
        let room = create_room("room-1".to_string(), "Living Room".to_string());

        assert_eq!(room.id, "room-1");
        assert_eq!(room.name, "Living Room");
    }
}
