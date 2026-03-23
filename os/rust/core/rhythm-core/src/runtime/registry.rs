//! Device registry trait for mapping devices to rooms.
//!
//! This module defines the `DeviceRegistry` trait which abstracts how devices
//! (switches, remotes) are mapped to rooms/areas. Different platforms implement this:
//! - Home Assistant addon uses the HA device registry
//! - ESP32 uses config-based mapping stored in NVS

use std::collections::HashMap;

/// Trait for mapping devices to rooms/areas.
///
/// Implementations handle the device-to-room mapping in platform-specific ways:
/// - Home Assistant: queries the device registry via WebSocket
/// - ESP32: uses a simple HashMap persisted to NVS
pub trait DeviceRegistry: Send + Sync {
    /// Get the room ID for a device.
    ///
    /// Returns `None` if the device is not mapped to any room.
    fn get_room_for_device(&self, device_id: &str) -> Option<String>;

    /// Register a device to a room.
    ///
    /// This creates or updates the mapping.
    fn register_device(&mut self, device_id: &str, room_id: &str);

    /// Remove a device mapping.
    fn unregister_device(&mut self, device_id: &str);

    /// List all registered room IDs.
    fn list_rooms(&self) -> Vec<String>;

    /// List all devices for a room.
    fn devices_for_room(&self, room_id: &str) -> Vec<String>;
}

/// A simple in-memory device registry.
///
/// Useful for testing and as a base for persistent implementations.
#[derive(Debug, Clone, Default)]
pub struct SimpleDeviceRegistry {
    /// Map from device_id -> room_id
    device_to_room: HashMap<String, String>,
}

impl SimpleDeviceRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            device_to_room: HashMap::new(),
        }
    }

    /// Create from an existing map.
    pub fn from_map(map: HashMap<String, String>) -> Self {
        Self {
            device_to_room: map,
        }
    }

    /// Get the underlying map (for serialization).
    pub fn as_map(&self) -> &HashMap<String, String> {
        &self.device_to_room
    }

    /// Take the underlying map.
    pub fn into_map(self) -> HashMap<String, String> {
        self.device_to_room
    }
}

impl DeviceRegistry for SimpleDeviceRegistry {
    fn get_room_for_device(&self, device_id: &str) -> Option<String> {
        self.device_to_room.get(device_id).cloned()
    }

    fn register_device(&mut self, device_id: &str, room_id: &str) {
        self.device_to_room
            .insert(device_id.to_string(), room_id.to_string());
    }

    fn unregister_device(&mut self, device_id: &str) {
        self.device_to_room.remove(device_id);
    }

    fn list_rooms(&self) -> Vec<String> {
        let mut rooms: Vec<String> = self.device_to_room.values().cloned().collect();
        rooms.sort();
        rooms.dedup();
        rooms
    }

    fn devices_for_room(&self, room_id: &str) -> Vec<String> {
        self.device_to_room
            .iter()
            .filter(|(_, room)| room.as_str() == room_id)
            .map(|(device, _)| device.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_registry() {
        let mut registry = SimpleDeviceRegistry::new();

        // Register devices
        registry.register_device("switch1", "living_room");
        registry.register_device("switch2", "bedroom");
        registry.register_device("switch3", "living_room");

        // Lookup
        assert_eq!(
            registry.get_room_for_device("switch1"),
            Some("living_room".to_string())
        );
        assert_eq!(
            registry.get_room_for_device("switch2"),
            Some("bedroom".to_string())
        );
        assert_eq!(registry.get_room_for_device("unknown"), None);

        // List rooms
        let rooms = registry.list_rooms();
        assert!(rooms.contains(&"living_room".to_string()));
        assert!(rooms.contains(&"bedroom".to_string()));

        // Devices for room
        let living_room_devices = registry.devices_for_room("living_room");
        assert!(living_room_devices.contains(&"switch1".to_string()));
        assert!(living_room_devices.contains(&"switch3".to_string()));
        assert_eq!(living_room_devices.len(), 2);

        // Unregister
        registry.unregister_device("switch1");
        assert_eq!(registry.get_room_for_device("switch1"), None);
    }
}
