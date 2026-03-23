//! Room/Area abstraction for adaptive lighting.
//!
//! This module provides the `Room` struct which represents a room or area
//! that can have Rhythm lighting enabled, along with `RoomManager` for
//! tracking multiple rooms.

use std::collections::HashMap;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::config::CurveConfig;

/// Source/provider for the room (where it was imported from).
///
/// When adding a new hub type, prefer using `Other(String)` to avoid
/// modifying this core enum. Only add a named variant if the source
/// is used extensively across multiple crates.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum RoomSource {
    /// Unknown source (backwards compatibility with existing stored rooms)
    #[default]
    Unknown,
    /// Philips Hue bridge
    Hue,
    /// Home Assistant via WebSocket
    HomeAssistant,
    /// ESP32 standalone controller
    Esp32,
    /// Other hub type (e.g., "lifx", "zigbee2mqtt")
    Other(String),
}

/// A room or area that can have Rhythm lighting enabled.
///
/// Each room tracks its Rhythm state and any time/brightness offsets
/// from manual adjustments (step up/down).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Room {
    /// Unique identifier for the room
    pub id: String,

    /// Human-readable name
    pub name: String,

    /// Source/provider for this room
    #[cfg_attr(feature = "serde", serde(default))]
    pub source: RoomSource,

    /// Whether Rhythm mode is enabled for this room
    pub rhythm_enabled: bool,

    /// Whether this room is disabled (excluded from kiosk mode)
    #[cfg_attr(feature = "serde", serde(default))]
    pub disabled: bool,

    /// Time offset in minutes from current solar time
    /// (positive = brighter/cooler, negative = dimmer/warmer)
    pub time_offset_minutes: f32,

    /// Direct brightness offset (for dim_up/dim_down)
    pub brightness_offset: f32,

    /// Per-room curve configuration (None uses global config)
    #[cfg_attr(feature = "serde", serde(default))]
    pub curve_config: Option<CurveConfig>,

    /// Whether this room is in "soft off" state (at soft-off brightness level).
    /// Used when power_save is disabled: lights dim to the configured
    /// soft-off brightness instead of turning fully off, maintaining
    /// color temperature readiness.
    #[cfg_attr(feature = "serde", serde(default))]
    pub soft_off: bool,
}

impl Room {
    /// Create a new room with default state (Rhythm disabled, no offsets).
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            source: RoomSource::Unknown,
            rhythm_enabled: false,
            disabled: false,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
            curve_config: None,
            soft_off: false,
        }
    }

    /// Create a new room with a specified source.
    pub fn with_source(id: impl Into<String>, name: impl Into<String>, source: RoomSource) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            source,
            rhythm_enabled: false,
            disabled: false,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
            curve_config: None,
            soft_off: false,
        }
    }

    /// Enable Rhythm mode for this room.
    pub fn enable_rhythm(&mut self) {
        self.rhythm_enabled = true;
    }

    /// Disable Rhythm mode for this room.
    pub fn disable_rhythm(&mut self) {
        self.rhythm_enabled = false;
    }

    /// Toggle Rhythm mode for this room.
    pub fn toggle_rhythm(&mut self) -> bool {
        self.rhythm_enabled = !self.rhythm_enabled;
        self.rhythm_enabled
    }

    /// Reset offsets to zero (return to current solar time).
    pub fn reset_offsets(&mut self) {
        self.time_offset_minutes = 0.0;
        self.brightness_offset = 0.0;
    }

    /// Apply a time offset (from step_up/step_down).
    pub fn apply_time_offset(&mut self, offset_minutes: f32) {
        self.time_offset_minutes += offset_minutes;
    }

    /// Set the time offset directly.
    pub fn set_time_offset(&mut self, offset_minutes: f32) {
        self.time_offset_minutes = offset_minutes;
    }

    /// Apply a brightness offset (from dim_up/dim_down).
    pub fn apply_brightness_offset(&mut self, offset: f32) {
        self.brightness_offset = (self.brightness_offset + offset).clamp(-100.0, 100.0);
    }

    /// Get the effective time offset including any adjustments.
    pub fn effective_time_offset(&self) -> f32 {
        self.time_offset_minutes
    }

    /// Get the effective brightness offset.
    pub fn effective_brightness_offset(&self) -> f32 {
        self.brightness_offset
    }

    /// Set the disabled state for this room.
    pub fn set_disabled(&mut self, disabled: bool) {
        self.disabled = disabled;
    }

    /// Set the per-room curve configuration.
    ///
    /// Pass `None` to use the global configuration.
    pub fn set_curve_config(&mut self, config: Option<CurveConfig>) {
        self.curve_config = config;
    }

    /// Get the source of this room.
    pub fn source(&self) -> &RoomSource {
        &self.source
    }

    /// Check if this room is disabled.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }
}

impl Default for Room {
    fn default() -> Self {
        Self {
            id: "default".to_string(),
            name: "Default Room".to_string(),
            source: RoomSource::Unknown,
            rhythm_enabled: false,
            disabled: false,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
            curve_config: None,
            soft_off: false,
        }
    }
}

/// Manager for tracking multiple rooms.
///
/// Provides methods to add, remove, and query rooms, as well as
/// bulk operations for Rhythm mode management.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct RoomManager {
    rooms: HashMap<String, Room>,
}

impl RoomManager {
    /// Create a new empty room manager.
    pub fn new() -> Self {
        Self {
            rooms: HashMap::new(),
        }
    }

    /// Add a room to the manager.
    ///
    /// If a room with the same ID already exists, it is replaced.
    pub fn add_room(&mut self, room: Room) {
        self.rooms.insert(room.id.clone(), room);
    }

    /// Add a room by ID and name.
    pub fn add(&mut self, id: impl Into<String>, name: impl Into<String>) {
        let room = Room::new(id, name);
        self.rooms.insert(room.id.clone(), room);
    }

    /// Remove a room by ID.
    pub fn remove(&mut self, id: &str) -> Option<Room> {
        self.rooms.remove(id)
    }

    /// Get a room by ID.
    pub fn get(&self, id: &str) -> Option<&Room> {
        self.rooms.get(id)
    }

    /// Get a mutable reference to a room by ID.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Room> {
        self.rooms.get_mut(id)
    }

    /// Get or create a room by ID.
    ///
    /// If the room doesn't exist, creates it with the given name.
    pub fn get_or_create(&mut self, id: impl Into<String>, name: impl Into<String>) -> &mut Room {
        let id = id.into();
        if !self.rooms.contains_key(&id) {
            let name = name.into();
            self.rooms.insert(id.clone(), Room::new(id.clone(), name));
        }
        self.rooms.get_mut(&id).unwrap()
    }

    /// Check if a room exists.
    pub fn contains(&self, id: &str) -> bool {
        self.rooms.contains_key(id)
    }

    /// Check if Rhythm is enabled for a room.
    pub fn is_rhythm_enabled(&self, id: &str) -> bool {
        self.rooms
            .get(id)
            .map(|r| r.rhythm_enabled)
            .unwrap_or(false)
    }

    /// Enable Rhythm for a room.
    pub fn enable_rhythm(&mut self, id: &str) -> bool {
        if let Some(room) = self.rooms.get_mut(id) {
            room.enable_rhythm();
            true
        } else {
            false
        }
    }

    /// Disable Rhythm for a room.
    pub fn disable_rhythm(&mut self, id: &str) -> bool {
        if let Some(room) = self.rooms.get_mut(id) {
            room.disable_rhythm();
            true
        } else {
            false
        }
    }

    /// Get all rooms.
    pub fn all_rooms(&self) -> Vec<&Room> {
        self.rooms.values().collect()
    }

    /// Get all room IDs.
    pub fn room_ids(&self) -> Vec<&str> {
        self.rooms.keys().map(|s| s.as_str()).collect()
    }

    /// Get all rooms with Rhythm enabled.
    pub fn rhythm_enabled_rooms(&self) -> Vec<&Room> {
        self.rooms.values().filter(|r| r.rhythm_enabled).collect()
    }

    /// Get the number of rooms.
    pub fn len(&self) -> usize {
        self.rooms.len()
    }

    /// Check if the manager is empty.
    pub fn is_empty(&self) -> bool {
        self.rooms.is_empty()
    }

    /// Get the number of rooms with Rhythm enabled.
    pub fn rhythm_enabled_count(&self) -> usize {
        self.rooms.values().filter(|r| r.rhythm_enabled).count()
    }

    /// Iterate over all rooms.
    pub fn iter(&self) -> impl Iterator<Item = &Room> {
        self.rooms.values()
    }

    /// Iterate over all rooms mutably.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Room> {
        self.rooms.values_mut()
    }

    /// Get all rooms that are not disabled.
    ///
    /// Useful for kiosk mode which should skip disabled rooms.
    pub fn enabled_rooms(&self) -> Vec<&Room> {
        self.rooms.values().filter(|r| !r.disabled).collect()
    }

    /// Get all rooms from a specific source.
    pub fn rooms_by_source(&self, source: RoomSource) -> Vec<&Room> {
        self.rooms.values().filter(|r| r.source == source).collect()
    }

    /// Remove all rooms from a specific source.
    ///
    /// Returns the number of rooms removed.
    pub fn remove_by_source(&mut self, source: RoomSource) -> usize {
        let ids_to_remove: Vec<String> = self
            .rooms
            .values()
            .filter(|r| r.source == source)
            .map(|r| r.id.clone())
            .collect();

        let count = ids_to_remove.len();
        for id in ids_to_remove {
            self.rooms.remove(&id);
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_room_new() {
        let room = Room::new("living_room", "Living Room");
        assert_eq!(room.id, "living_room");
        assert_eq!(room.name, "Living Room");
        assert_eq!(room.source, RoomSource::Unknown);
        assert!(!room.rhythm_enabled);
        assert!(!room.disabled);
        assert_eq!(room.time_offset_minutes, 0.0);
        assert_eq!(room.brightness_offset, 0.0);
        assert!(room.curve_config.is_none());
    }

    #[test]
    fn test_room_with_source() {
        let room = Room::with_source("hue_room", "Hue Living Room", RoomSource::Hue);
        assert_eq!(room.id, "hue_room");
        assert_eq!(room.name, "Hue Living Room");
        assert_eq!(room.source, RoomSource::Hue);
        assert!(!room.rhythm_enabled);
        assert!(!room.disabled);
    }

    #[test]
    fn test_room_disabled() {
        let mut room = Room::new("test", "Test");
        assert!(!room.is_disabled());

        room.set_disabled(true);
        assert!(room.is_disabled());

        room.set_disabled(false);
        assert!(!room.is_disabled());
    }

    #[test]
    fn test_room_rhythm_toggle() {
        let mut room = Room::new("test", "Test");
        assert!(!room.rhythm_enabled);

        room.enable_rhythm();
        assert!(room.rhythm_enabled);

        room.disable_rhythm();
        assert!(!room.rhythm_enabled);

        let result = room.toggle_rhythm();
        assert!(result);
        assert!(room.rhythm_enabled);
    }

    #[test]
    fn test_room_offsets() {
        let mut room = Room::new("test", "Test");

        room.apply_time_offset(30.0);
        assert_eq!(room.time_offset_minutes, 30.0);

        room.apply_time_offset(-10.0);
        assert_eq!(room.time_offset_minutes, 20.0);

        room.apply_brightness_offset(10.0);
        assert_eq!(room.brightness_offset, 10.0);

        room.reset_offsets();
        assert_eq!(room.time_offset_minutes, 0.0);
        assert_eq!(room.brightness_offset, 0.0);
    }

    #[test]
    fn test_room_manager_add_get() {
        let mut manager = RoomManager::new();

        manager.add("living_room", "Living Room");
        manager.add("bedroom", "Bedroom");

        assert_eq!(manager.len(), 2);
        assert!(manager.contains("living_room"));
        assert!(manager.contains("bedroom"));
        assert!(!manager.contains("kitchen"));

        let room = manager.get("living_room").unwrap();
        assert_eq!(room.name, "Living Room");
    }

    #[test]
    fn test_room_manager_rhythm() {
        let mut manager = RoomManager::new();
        manager.add("test", "Test Room");

        assert!(!manager.is_rhythm_enabled("test"));
        assert_eq!(manager.rhythm_enabled_count(), 0);

        manager.enable_rhythm("test");
        assert!(manager.is_rhythm_enabled("test"));
        assert_eq!(manager.rhythm_enabled_count(), 1);

        manager.disable_rhythm("test");
        assert!(!manager.is_rhythm_enabled("test"));
    }

    #[test]
    fn test_room_manager_get_or_create() {
        let mut manager = RoomManager::new();

        // First call creates the room
        let room = manager.get_or_create("new_room", "New Room");
        assert_eq!(room.name, "New Room");

        // Second call returns existing room
        room.enable_rhythm();
        let room2 = manager.get_or_create("new_room", "Different Name");
        assert_eq!(room2.name, "New Room"); // Original name kept
        assert!(room2.rhythm_enabled); // State preserved
    }

    #[test]
    fn test_room_manager_rhythm_enabled_rooms() {
        let mut manager = RoomManager::new();
        manager.add("room1", "Room 1");
        manager.add("room2", "Room 2");
        manager.add("room3", "Room 3");

        manager.enable_rhythm("room1");
        manager.enable_rhythm("room3");

        let enabled = manager.rhythm_enabled_rooms();
        assert_eq!(enabled.len(), 2);
    }

    #[test]
    fn test_room_manager_enabled_rooms() {
        let mut manager = RoomManager::new();
        manager.add("room1", "Room 1");
        manager.add("room2", "Room 2");
        manager.add("room3", "Room 3");

        // Disable room2
        manager.get_mut("room2").unwrap().set_disabled(true);

        let enabled = manager.enabled_rooms();
        assert_eq!(enabled.len(), 2);
        assert!(enabled.iter().all(|r| r.id != "room2"));
    }

    #[test]
    fn test_room_manager_rooms_by_source() {
        let mut manager = RoomManager::new();
        manager.add_room(Room::with_source("hue1", "Hue Room 1", RoomSource::Hue));
        manager.add_room(Room::with_source("hue2", "Hue Room 2", RoomSource::Hue));
        manager.add_room(Room::with_source(
            "ha1",
            "HA Room 1",
            RoomSource::HomeAssistant,
        ));

        let hue_rooms = manager.rooms_by_source(RoomSource::Hue);
        assert_eq!(hue_rooms.len(), 2);

        let ha_rooms = manager.rooms_by_source(RoomSource::HomeAssistant);
        assert_eq!(ha_rooms.len(), 1);

        let esp_rooms = manager.rooms_by_source(RoomSource::Esp32);
        assert_eq!(esp_rooms.len(), 0);
    }

    #[test]
    fn test_room_manager_remove_by_source() {
        let mut manager = RoomManager::new();
        manager.add_room(Room::with_source("hue1", "Hue Room 1", RoomSource::Hue));
        manager.add_room(Room::with_source("hue2", "Hue Room 2", RoomSource::Hue));
        manager.add_room(Room::with_source(
            "ha1",
            "HA Room 1",
            RoomSource::HomeAssistant,
        ));

        assert_eq!(manager.len(), 3);

        let removed = manager.remove_by_source(RoomSource::Hue);
        assert_eq!(removed, 2);
        assert_eq!(manager.len(), 1);
        assert!(manager.get("ha1").is_some());
    }

    // =========================================================================
    // Serialization Tests
    // =========================================================================

    #[cfg(feature = "serde")]
    mod serde_tests {
        use super::*;

        #[test]
        fn test_room_serialize_deserialize() {
            let room = Room::with_source("room1", "Living Room", RoomSource::Hue);
            let json = serde_json::to_string(&room).unwrap();
            let deserialized: Room = serde_json::from_str(&json).unwrap();

            assert_eq!(deserialized.id, "room1");
            assert_eq!(deserialized.name, "Living Room");
            assert_eq!(deserialized.source, RoomSource::Hue);
        }

        #[test]
        fn test_room_serialize_with_offsets() {
            let mut room = Room::new("room1", "Test Room");
            room.rhythm_enabled = true;
            room.time_offset_minutes = 30.5;
            room.brightness_offset = -10.0;

            let json = serde_json::to_string(&room).unwrap();
            let deserialized: Room = serde_json::from_str(&json).unwrap();

            assert!(deserialized.rhythm_enabled);
            assert_eq!(deserialized.time_offset_minutes, 30.5);
            assert_eq!(deserialized.brightness_offset, -10.0);
        }

        #[test]
        fn test_room_deserialize_missing_optional_fields() {
            // JSON without optional fields (disabled, source, curve_config)
            let json = r#"{
                "id": "room1",
                "name": "Test Room",
                "rhythm_enabled": false,
                "time_offset_minutes": 0.0,
                "brightness_offset": 0.0
            }"#;

            let room: Room = serde_json::from_str(json).unwrap();
            assert_eq!(room.id, "room1");
            assert!(!room.disabled); // default
            assert_eq!(room.source, RoomSource::Unknown); // default
            assert!(room.curve_config.is_none()); // default
        }

        #[test]
        fn test_room_source_serialize() {
            assert_eq!(serde_json::to_string(&RoomSource::Hue).unwrap(), "\"Hue\"");
            assert_eq!(
                serde_json::to_string(&RoomSource::HomeAssistant).unwrap(),
                "\"HomeAssistant\""
            );
            assert_eq!(
                serde_json::to_string(&RoomSource::Esp32).unwrap(),
                "\"Esp32\""
            );
        }

        #[test]
        fn test_room_manager_serialize_deserialize() {
            let mut manager = RoomManager::new();
            manager.add_room(Room::with_source("room1", "Room 1", RoomSource::Hue));
            manager.add_room(Room::with_source(
                "room2",
                "Room 2",
                RoomSource::HomeAssistant,
            ));
            manager.get_mut("room1").unwrap().enable_rhythm();

            let json = serde_json::to_string(&manager).unwrap();
            let deserialized: RoomManager = serde_json::from_str(&json).unwrap();

            assert_eq!(deserialized.len(), 2);
            assert!(deserialized.is_rhythm_enabled("room1"));
            assert!(!deserialized.is_rhythm_enabled("room2"));
        }

        #[test]
        fn test_room_roundtrip_preserves_all_fields() {
            let mut room = Room::with_source("room1", "Test", RoomSource::Esp32);
            room.rhythm_enabled = true;
            room.disabled = true;
            room.time_offset_minutes = -45.0;
            room.brightness_offset = 20.0;

            let json = serde_json::to_string(&room).unwrap();
            let roundtrip: Room = serde_json::from_str(&json).unwrap();

            assert_eq!(room, roundtrip);
        }
    }

    // =========================================================================
    // Edge Case Tests
    // =========================================================================

    #[test]
    fn test_room_offset_accumulation() {
        let mut room = Room::new("test", "Test");

        room.apply_time_offset(10.0);
        room.apply_time_offset(20.0);
        room.apply_time_offset(-5.0);

        assert_eq!(room.time_offset_minutes, 25.0);
    }

    #[test]
    fn test_room_brightness_offset_clamping() {
        let mut room = Room::new("test", "Test");

        // Should clamp to 100
        room.apply_brightness_offset(150.0);
        assert_eq!(room.brightness_offset, 100.0);

        room.reset_offsets();

        // Should clamp to -100
        room.apply_brightness_offset(-150.0);
        assert_eq!(room.brightness_offset, -100.0);
    }

    #[test]
    fn test_room_reset_preserves_identity() {
        let mut room = Room::with_source("room1", "Living Room", RoomSource::Hue);
        room.rhythm_enabled = true;
        room.time_offset_minutes = 60.0;
        room.brightness_offset = 20.0;

        room.reset_offsets();

        // Identity preserved
        assert_eq!(room.id, "room1");
        assert_eq!(room.name, "Living Room");
        assert_eq!(room.source, RoomSource::Hue);
        // Rhythm state preserved
        assert!(room.rhythm_enabled);
        // Offsets reset
        assert_eq!(room.time_offset_minutes, 0.0);
        assert_eq!(room.brightness_offset, 0.0);
    }

    #[test]
    fn test_room_manager_remove() {
        let mut manager = RoomManager::new();
        manager.add("room1", "Room 1");
        manager.add("room2", "Room 2");

        assert!(manager.remove("room1").is_some());
        assert!(manager.remove("room1").is_none()); // Already removed

        assert_eq!(manager.len(), 1);
        assert!(!manager.contains("room1"));
        assert!(manager.contains("room2"));
    }

    #[test]
    fn test_room_manager_get_or_create_preserves_existing() {
        let mut manager = RoomManager::new();

        // Create with initial name
        let room1 = manager.get_or_create("room1", "Original Name");
        room1.enable_rhythm();
        room1.apply_time_offset(30.0);

        // Get again with different name - should keep original
        let room2 = manager.get_or_create("room1", "Different Name");

        assert_eq!(room2.name, "Original Name");
        assert!(room2.rhythm_enabled);
        assert_eq!(room2.time_offset_minutes, 30.0);
    }

    #[test]
    fn test_room_manager_ids() {
        let mut manager = RoomManager::new();
        manager.add("z_room", "Z Room");
        manager.add("a_room", "A Room");
        manager.add("m_room", "M Room");

        let ids = manager.room_ids();

        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&"a_room"));
        assert!(ids.contains(&"m_room"));
        assert!(ids.contains(&"z_room"));
    }
}
