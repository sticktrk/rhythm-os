//! Hue behavior instance tracking.
//!
//! Tracks which Hue devices have behavior_instances configured in the Hue app.
//! Devices with behavior_instances are handled by Hue, so Rhythm should ignore
//! their button events.

use std::collections::{HashMap, HashSet};

/// Tracks Hue behavior_instance to device mappings.
///
/// The Hue bridge has a concept of "behavior_instances" which are automations
/// configured in the Hue app (like "switch controls living room lights").
/// When a device has a behavior_instance configured, the Hue bridge handles
/// its button events, so Rhythm should ignore them.
///
/// This tracker maintains:
/// - A mapping from behavior_instance IDs to device IDs
/// - A set of device IDs that have at least one behavior_instance
///
/// ## Thread Safety
///
/// This type is not thread-safe. For concurrent access, wrap in a Mutex.
///
/// ## Example
///
/// ```
/// use rhythm_hue::HueBehaviorTracker;
///
/// let mut tracker = HueBehaviorTracker::new();
///
/// // Device gets configured in Hue app
/// tracker.add_behavior("behavior-1", "device-1");
/// assert!(tracker.is_device_configured("device-1"));
///
/// // Device gets unconfigured
/// tracker.remove_behavior("behavior-1");
/// assert!(!tracker.is_device_configured("device-1"));
/// ```
#[derive(Debug, Clone, Default)]
pub struct HueBehaviorTracker {
    /// Mapping from behavior_instance ID to device ID.
    behavior_to_device: HashMap<String, String>,

    /// Set of device IDs that have at least one behavior_instance.
    configured_device_ids: HashSet<String>,
}

impl HueBehaviorTracker {
    /// Create a new empty behavior tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a behavior_instance to device mapping.
    ///
    /// Call this when:
    /// - Initially fetching behavior_instances from the Hue API
    /// - Receiving an "add" or "update" SSE event for behavior_instance
    ///
    /// # Arguments
    ///
    /// * `behavior_id` - The behavior_instance resource ID
    /// * `device_id` - The device resource ID this behavior configures
    pub fn add_behavior(&mut self, behavior_id: &str, device_id: &str) {
        self.behavior_to_device
            .insert(behavior_id.to_string(), device_id.to_string());
        self.configured_device_ids.insert(device_id.to_string());
    }

    /// Remove a behavior_instance.
    ///
    /// Call this when receiving a "delete" SSE event for behavior_instance.
    /// Only removes the device from the configured set if no other behaviors
    /// reference it.
    ///
    /// # Arguments
    ///
    /// * `behavior_id` - The behavior_instance resource ID that was deleted
    ///
    /// # Returns
    ///
    /// `true` if a device was unconfigured (no more behaviors), `false` otherwise.
    pub fn remove_behavior(&mut self, behavior_id: &str) -> bool {
        if let Some(device_id) = self.behavior_to_device.remove(behavior_id) {
            // Check if any other behaviors reference this device
            let still_configured = self.behavior_to_device.values().any(|d| d == &device_id);

            if !still_configured {
                self.configured_device_ids.remove(&device_id);
                return true;
            }
        }
        false
    }

    /// Check if a device is configured in the Hue app.
    ///
    /// Returns `true` if the device has at least one behavior_instance,
    /// meaning Hue handles its events and Rhythm should ignore them.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The device resource ID to check
    pub fn is_device_configured(&self, device_id: &str) -> bool {
        self.configured_device_ids.contains(device_id)
    }

    /// Get the set of all configured device IDs.
    ///
    /// Useful for debugging or displaying status.
    pub fn configured_device_ids(&self) -> &HashSet<String> {
        &self.configured_device_ids
    }

    /// Get the number of tracked behavior_instances.
    pub fn behavior_count(&self) -> usize {
        self.behavior_to_device.len()
    }

    /// Get the number of configured devices.
    pub fn configured_device_count(&self) -> usize {
        self.configured_device_ids.len()
    }

    /// Clear all tracked data.
    ///
    /// Call this before re-fetching behavior_instances from the API.
    pub fn clear(&mut self) {
        self.behavior_to_device.clear();
        self.configured_device_ids.clear();
    }

    /// Get all behavior mappings as a vector of (behavior_id, device_id) tuples.
    ///
    /// Useful for serialization (FFI doesn't support HashMap directly).
    pub fn behavior_mappings(&self) -> Vec<(String, String)> {
        self.behavior_to_device
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Restore from serialized data.
    ///
    /// # Arguments
    ///
    /// * `mappings` - Vector of (behavior_id, device_id) tuples
    pub fn restore_from_mappings(&mut self, mappings: &[(String, String)]) {
        self.clear();
        for (behavior_id, device_id) in mappings {
            self.add_behavior(behavior_id, device_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_tracker_is_empty() {
        let tracker = HueBehaviorTracker::new();
        assert_eq!(tracker.behavior_count(), 0);
        assert_eq!(tracker.configured_device_count(), 0);
    }

    #[test]
    fn test_add_behavior_marks_device_configured() {
        let mut tracker = HueBehaviorTracker::new();

        tracker.add_behavior("behavior-1", "device-1");

        assert!(tracker.is_device_configured("device-1"));
        assert!(!tracker.is_device_configured("device-2"));
        assert_eq!(tracker.behavior_count(), 1);
        assert_eq!(tracker.configured_device_count(), 1);
    }

    #[test]
    fn test_multiple_behaviors_same_device() {
        let mut tracker = HueBehaviorTracker::new();

        tracker.add_behavior("behavior-1", "device-1");
        tracker.add_behavior("behavior-2", "device-1");

        assert!(tracker.is_device_configured("device-1"));
        assert_eq!(tracker.behavior_count(), 2);
        assert_eq!(tracker.configured_device_count(), 1); // Same device
    }

    #[test]
    fn test_remove_behavior_keeps_device_if_other_behaviors() {
        let mut tracker = HueBehaviorTracker::new();

        tracker.add_behavior("behavior-1", "device-1");
        tracker.add_behavior("behavior-2", "device-1");

        let unconfigured = tracker.remove_behavior("behavior-1");
        assert!(!unconfigured); // Device still has behavior-2
        assert!(tracker.is_device_configured("device-1"));
        assert_eq!(tracker.behavior_count(), 1);
    }

    #[test]
    fn test_remove_last_behavior_unconfigures_device() {
        let mut tracker = HueBehaviorTracker::new();

        tracker.add_behavior("behavior-1", "device-1");

        let unconfigured = tracker.remove_behavior("behavior-1");
        assert!(unconfigured);
        assert!(!tracker.is_device_configured("device-1"));
        assert_eq!(tracker.behavior_count(), 0);
        assert_eq!(tracker.configured_device_count(), 0);
    }

    #[test]
    fn test_remove_nonexistent_behavior() {
        let mut tracker = HueBehaviorTracker::new();

        tracker.add_behavior("behavior-1", "device-1");

        let unconfigured = tracker.remove_behavior("behavior-999");
        assert!(!unconfigured);
        assert!(tracker.is_device_configured("device-1"));
    }

    #[test]
    fn test_clear() {
        let mut tracker = HueBehaviorTracker::new();

        tracker.add_behavior("behavior-1", "device-1");
        tracker.add_behavior("behavior-2", "device-2");
        tracker.clear();

        assert_eq!(tracker.behavior_count(), 0);
        assert_eq!(tracker.configured_device_count(), 0);
        assert!(!tracker.is_device_configured("device-1"));
    }

    #[test]
    fn test_behavior_mappings_roundtrip() {
        let mut tracker = HueBehaviorTracker::new();

        tracker.add_behavior("behavior-1", "device-1");
        tracker.add_behavior("behavior-2", "device-2");
        tracker.add_behavior("behavior-3", "device-1");

        let mappings = tracker.behavior_mappings();
        assert_eq!(mappings.len(), 3);

        let mut tracker2 = HueBehaviorTracker::new();
        tracker2.restore_from_mappings(&mappings);

        assert_eq!(tracker2.behavior_count(), 3);
        assert_eq!(tracker2.configured_device_count(), 2);
        assert!(tracker2.is_device_configured("device-1"));
        assert!(tracker2.is_device_configured("device-2"));
    }

    #[test]
    fn test_configured_device_ids() {
        let mut tracker = HueBehaviorTracker::new();

        tracker.add_behavior("behavior-1", "device-1");
        tracker.add_behavior("behavior-2", "device-2");

        let ids = tracker.configured_device_ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains("device-1"));
        assert!(ids.contains("device-2"));
    }
}
