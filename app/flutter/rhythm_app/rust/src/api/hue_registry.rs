//! FFI functions for Hue behavior tracking.
//!
//! These functions expose the HueBehaviorTracker to Flutter via
//! flutter_rust_bridge, using a functional pattern (take state, return new state).

use rhythm_hue::HueBehaviorTracker;

use super::dto::{
    BehaviorRemoveResultDto, HueBehaviorTrackerDto, HueButtonDto, HueRoomDto,
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
// Device/Button/Room Helper Functions
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
