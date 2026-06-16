//! Hub-agnostic device registry trait.
//!
//! Defines a common interface for device/room registries used by command
//! handlers. Hub-specific implementations (e.g., `HueDeviceRegistry`) implement
//! this trait so that `commands.rs` can work without knowing the concrete hub type.

use crate::room::Room;
use serde::{Deserialize, Serialize};

/// The type of a device in the registry.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceType {
    /// A light bulb or luminaire (not persisted in snapshot — re-pushed by room sync).
    Light,
    /// A button/switch/remote control.
    Button,
    /// A motion/occupancy sensor.
    Motion,
}

/// Hub-agnostic registry for rooms, devices, buttons, and motion sensors.
///
/// Provides the query and mutation methods that `commands.rs` needs, without
/// coupling to any specific hub implementation.
pub trait HubRegistry: Send + Sync {
    // =========================================================================
    // Room queries
    // =========================================================================

    /// List all rooms in the registry as `Room` objects.
    fn rooms(&self) -> Vec<Room>;

    /// Get the grouped light resource ID for a room.
    fn get_grouped_light_id(&self, room_id: &str) -> Option<String>;

    /// Get all device IDs associated with a room.
    fn devices_for_room(&self, room_id: &str) -> Vec<String>;

    // =========================================================================
    // Room mutations
    // =========================================================================

    /// Check if a room already matches the incoming data (dedup check).
    fn room_matches(
        &self,
        id: &str,
        name: &str,
        grouped_light_id: &str,
        device_ids: &[String],
    ) -> bool;

    /// Upsert a room into the registry.
    fn upsert_room(&mut self, id: &str, name: &str, grouped_light_id: &str, device_ids: &[String]);

    /// Remove a room and its associated mappings.
    fn remove_room(&mut self, room_id: &str);

    // =========================================================================
    // Device mutations
    // =========================================================================

    /// Check if a device already matches the incoming data (dedup check).
    ///
    /// `room_id == None` matches a roomless device — one that exists on the
    /// hub but isn't yet assigned to a room.
    fn device_matches(
        &self,
        device_id: &str,
        room_id: Option<&str>,
        buttons: &[(String, u8)],
        device_type: &DeviceType,
    ) -> bool;

    /// Upsert a device with button mappings and type.
    ///
    /// `room_id == None` registers the device as roomless: button/type
    /// mappings are recorded but no room mapping is created.
    fn upsert_device(
        &mut self,
        device_id: &str,
        room_id: Option<&str>,
        buttons: &[(String, u8)],
        device_type: DeviceType,
    );

    /// Remove a device and its button mappings.
    fn remove_device(&mut self, device_id: &str);

    /// Set the room mapping for an already-known device. Preserves
    /// button/type entries; only mutates the device→room map.
    ///
    /// Used after a canonical room-assignment is propagated down to the hub
    /// registry so subsequent button/motion events route correctly.
    fn set_device_room(&mut self, device_id: &str, room_id: &str);

    /// Clear the room mapping for a device, preserving button/type entries.
    fn clear_device_room(&mut self, device_id: &str);

    /// Get all devices for a room with their types.
    fn devices_for_room_typed(&self, _room_id: &str) -> Vec<(String, DeviceType)> {
        Vec::new()
    }

    // =========================================================================
    // Motion sensors
    // =========================================================================

    /// Get room IDs that have at least one motion sensor mapped.
    ///
    /// Default returns empty — override if the registry tracks motion sensors.
    fn rooms_with_motion_sensors(&self) -> Vec<String> {
        Vec::new()
    }

    // =========================================================================
    // Persistence
    // =========================================================================

    /// Serialize the registry state for storage.
    fn snapshot_json(&self) -> serde_json::Value;

    /// Check and clear the dirty flag (registry was modified since last persist).
    fn take_dirty(&mut self) -> bool {
        false
    }
}
