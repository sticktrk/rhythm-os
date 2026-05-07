//! Unified hub device registry.
//!
//! Maps hub resources (rooms, devices, buttons, sensors) into structures
//! needed for event routing and light control. Implements `HubRegistry` +
//! `DeviceRegistry` traits from rhythm-core.
//!
//! Replaces the per-hub registries (`HueDeviceRegistry`, `HaDeviceRegistry`)
//! with a single concrete struct.

use std::collections::{HashMap, HashSet};

use log::info;
use rhythm_core::room::Room;
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_core::{DeviceRegistry, HubRegistry};
use serde::{Deserialize, Serialize};

/// Compact room entry for persistence.
#[derive(Serialize, Deserialize)]
pub struct SnapshotRoom {
    /// Room UUID
    #[serde(rename = "i")]
    pub id: String,
    /// Room name
    #[serde(rename = "n")]
    pub name: String,
    /// Grouped light resource ID
    #[serde(rename = "g")]
    pub grouped_light_id: String,
}

/// A typed device entry for the new `"dv"` snapshot field.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SnapshotDevice {
    /// Device ID
    #[serde(rename = "i")]
    pub id: String,
    /// Hub-native room this device belongs to. `None`/missing for devices
    /// that exist on the hub but aren't yet assigned to a hub room.
    #[serde(
        rename = "r",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_room_id"
    )]
    pub room_id: Option<String>,
    /// Device type (button or motion)
    #[serde(rename = "t")]
    pub device_type: DeviceType,
}

/// Deserialize the snapshot `room_id` accepting either `null`, missing, or a
/// (possibly empty) string. Empty strings round-trip to `None` so older
/// snapshots that wrote `"r": ""` are interpreted as roomless.
fn deserialize_optional_room_id<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt: Option<String> = serde::Deserialize::deserialize(deserializer)?;
    Ok(opt.filter(|s| !s.is_empty()))
}

/// Serializable registry snapshot for persistence.
///
/// Compact format: rooms are stored once in an array (no duplicate UUID keys),
/// typed devices in `"dv"`, and field names are shortened for minimal JSON size.
#[derive(Serialize, Deserialize)]
pub struct RegistrySnapshot {
    /// Rooms (id, name, grouped_light_id)
    #[serde(rename = "r")]
    pub rooms: Vec<SnapshotRoom>,
    /// Typed devices (button + motion, not lights).
    #[serde(rename = "dv", default)]
    pub devices: Vec<SnapshotDevice>,
    /// button_id -> (device_id, control_id)
    #[serde(rename = "b")]
    pub buttons: HashMap<String, (String, u8)>,
    /// area_id -> list of light entity_ids (HA-specific, empty on Hue)
    #[serde(rename = "l", default, skip_serializing_if = "HashMap::is_empty")]
    pub area_lights: HashMap<String, Vec<String>>,
}

/// Unified registry mapping hub resources for event routing and light control.
///
/// Works for all hub types (Hue, HA, etc.). `default_grouped_light_to_room_id`
/// controls whether `upsert_room` defaults the grouped_light_id to the room_id
/// (true for HA where area_id serves as grouped_light_id).
pub struct HubDeviceRegistry {
    /// button_id -> (owner_device_id, control_id)
    buttons: HashMap<String, (String, u8)>,
    /// device_id -> room_id
    device_rooms: HashMap<String, String>,
    /// room_id -> grouped_light_id
    room_to_grouped_light: HashMap<String, String>,
    /// room_id -> room_name
    room_names: HashMap<String, String>,
    /// device_id -> DeviceType (Button or Motion; lights not tracked here)
    device_types: HashMap<String, DeviceType>,
    /// area_id -> list of light entity_ids (HA-specific, empty on Hue)
    area_lights: HashMap<String, Vec<String>>,
    /// When true, `upsert_room` defaults grouped_light_id to room_id (HA behavior).
    /// When false, defaults to empty string (Hue behavior).
    default_grouped_light_to_room_id: bool,
    /// True when in-memory state has changed since last persist (on-demand discovery).
    dirty: bool,
}

impl HubDeviceRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            buttons: HashMap::new(),
            device_rooms: HashMap::new(),
            room_to_grouped_light: HashMap::new(),
            room_names: HashMap::new(),
            device_types: HashMap::new(),
            area_lights: HashMap::new(),
            default_grouped_light_to_room_id: false,
            dirty: false,
        }
    }

    /// Create a new registry with options.
    pub fn with_options(default_grouped_light_to_room_id: bool) -> Self {
        let mut reg = Self::new();
        reg.default_grouped_light_to_room_id = default_grouped_light_to_room_id;
        reg
    }

    // =========================================================================
    // Light entity management (HA-specific, harmless no-ops on other hubs)
    // =========================================================================

    /// Set the light entities for an area (used for any_lights_on checks).
    pub fn set_area_lights(&mut self, area_id: &str, entities: Vec<String>) {
        self.area_lights.insert(area_id.to_string(), entities);
    }

    /// Get light entity IDs for a room/area.
    pub fn get_light_entities(&self, room_id: &str) -> Vec<String> {
        self.area_lights.get(room_id).cloned().unwrap_or_default()
    }

    /// Find the room whose light membership exactly matches the provided IDs.
    ///
    /// This is used by integrations like Hue where some live-state queries are
    /// only available at the native room/group level. Returns the lexicographically
    /// first match to keep the result deterministic if duplicate memberships exist.
    pub fn find_room_for_exact_light_entities(&self, device_ids: &[String]) -> Option<String> {
        if device_ids.is_empty() {
            return None;
        }

        let wanted: HashSet<&str> = device_ids.iter().map(String::as_str).collect();
        let mut matches: Vec<String> = self
            .area_lights
            .iter()
            .filter_map(|(room_id, lights)| {
                let existing: HashSet<&str> = lights.iter().map(String::as_str).collect();
                (existing == wanted).then(|| room_id.clone())
            })
            .collect();
        matches.sort();
        matches.into_iter().next()
    }

    // =========================================================================
    // Incremental upsert methods (called from HTTP/command handlers)
    // =========================================================================

    /// Check if a room already matches the incoming data (dedup for persistence writes).
    ///
    /// Compares name, grouped_light_id (only if incoming is non-empty), and
    /// light device_ids (excluding button-owning devices).
    pub fn room_matches(
        &self,
        room_id: &str,
        name: &str,
        grouped_light_id: &str,
        device_ids: &[String],
    ) -> bool {
        // Name must match
        if self.room_names.get(room_id).map(|n| n.as_str()) != Some(name) {
            return false;
        }

        // Grouped light ID: only compare if incoming is non-empty
        if !grouped_light_id.is_empty()
            && self.room_to_grouped_light.get(room_id).map(|g| g.as_str()) != Some(grouped_light_id)
        {
            return false;
        }

        // Compare light device IDs (excluding managed devices: Button/Motion)
        let managed_device_ids: std::collections::HashSet<&str> =
            self.device_types.keys().map(|id| id.as_str()).collect();

        let existing_device_ids: std::collections::HashSet<&str> = self
            .device_rooms
            .iter()
            .filter(|(dev_id, r)| {
                r.as_str() == room_id && !managed_device_ids.contains(dev_id.as_str())
            })
            .map(|(dev_id, _)| dev_id.as_str())
            .collect();

        let incoming_device_ids: std::collections::HashSet<&str> =
            device_ids.iter().map(|s| s.as_str()).collect();

        existing_device_ids == incoming_device_ids
    }

    /// Check if a device already matches the incoming data (dedup for persistence writes).
    ///
    /// Compares room mapping, device type, and button set (same IDs, same control_ids, same count).
    /// `room_id == None` matches a device that has no `device_rooms` entry (roomless).
    pub fn device_matches(
        &self,
        device_id: &str,
        room_id: Option<&str>,
        buttons: &[(String, u8)],
        device_type: &DeviceType,
    ) -> bool {
        // Room mapping must match (Option-equality so roomless dedups cleanly)
        if self.device_rooms.get(device_id).map(|r| r.as_str()) != room_id {
            return false;
        }

        // Device type must match
        if self.device_types.get(device_id) != Some(device_type) {
            return false;
        }

        // Collect existing buttons for this device
        let existing_buttons: std::collections::HashSet<(&str, u8)> = self
            .buttons
            .iter()
            .filter(|(_, (owner, _))| owner == device_id)
            .map(|(btn_id, (_, ctrl))| (btn_id.as_str(), *ctrl))
            .collect();

        let incoming_buttons: std::collections::HashSet<(&str, u8)> = buttons
            .iter()
            .map(|(btn_id, ctrl)| (btn_id.as_str(), *ctrl))
            .collect();

        existing_buttons == incoming_buttons
    }

    /// Upsert a room into the registry.
    ///
    /// Updates room name and grouped_light mapping. Also associates
    /// the given device_ids with this room.
    pub fn upsert_room(
        &mut self,
        room_id: &str,
        name: &str,
        grouped_light_id: &str,
        device_ids: &[String],
    ) {
        self.room_names
            .insert(room_id.to_string(), name.to_string());

        // Preserve existing valid grouped_light_id when incoming is empty
        if !grouped_light_id.is_empty() {
            self.room_to_grouped_light
                .insert(room_id.to_string(), grouped_light_id.to_string());
        } else if !self.room_to_grouped_light.contains_key(room_id) {
            if self.default_grouped_light_to_room_id {
                // HA: area_id itself is the grouped_light_id
                self.room_to_grouped_light
                    .insert(room_id.to_string(), room_id.to_string());
            } else {
                // Hue: empty until the Hue service provides it
                self.room_to_grouped_light
                    .insert(room_id.to_string(), String::new());
            }
        }

        // Collect device IDs managed by `device_set` (Button/Motion) — these are
        // not room children (lights), so must be preserved across room re-upserts.
        let managed_device_ids: std::collections::HashSet<&str> =
            self.device_types.keys().map(|id| id.as_str()).collect();

        // Remove stale device mappings for this room, but preserve managed devices
        self.device_rooms
            .retain(|dev_id, r| r != room_id || managed_device_ids.contains(dev_id.as_str()));

        // Add current device -> room mappings
        for device_id in device_ids {
            self.device_rooms
                .insert(device_id.clone(), room_id.to_string());
        }

        // Keep area_lights in sync so any_lights_on() can query per-room entities
        if !device_ids.is_empty() {
            self.area_lights.insert(
                room_id.to_string(),
                device_ids.iter().map(|d| d.to_string()).collect(),
            );
        }

        info!(
            "Registry: upserted room '{}' ({}) gl={} devices={}",
            name,
            room_id,
            grouped_light_id,
            device_ids.len()
        );
    }

    /// Remove a room and its associated device mappings from the registry.
    pub fn remove_room(&mut self, room_id: &str) {
        self.room_names.remove(room_id);
        self.room_to_grouped_light.remove(room_id);
        // Collect device IDs being removed so we can clean device_types
        let removed_devices: Vec<String> = self
            .device_rooms
            .iter()
            .filter(|(_, r)| r.as_str() == room_id)
            .map(|(d, _)| d.clone())
            .collect();
        self.device_rooms.retain(|_, r| r != room_id);
        for dev_id in &removed_devices {
            self.device_types.remove(dev_id);
        }
        self.area_lights.remove(room_id);
        info!("Registry: removed room {}", room_id);
    }

    /// Upsert a device with its button mappings and type.
    ///
    /// `room_id == None` registers the device as roomless: button/type mappings
    /// are still recorded (so SSE events resolve to a known device) but no
    /// `device_rooms` entry is created, and any pre-existing entry is removed.
    /// `get_room_for_button` / `get_room_for_motion_sensor` will return `None`
    /// for roomless devices, so events go unrouted until the user assigns a
    /// Rhythm room via the canonical assign-room endpoint.
    pub fn upsert_device(
        &mut self,
        device_id: &str,
        room_id: Option<&str>,
        buttons: &[(String, u8)],
        device_type: DeviceType,
    ) {
        match room_id {
            Some(rid) => {
                self.device_rooms
                    .insert(device_id.to_string(), rid.to_string());
            }
            None => {
                self.device_rooms.remove(device_id);
            }
        }
        self.device_types
            .insert(device_id.to_string(), device_type.clone());

        for (button_id, control_id) in buttons {
            self.buttons
                .insert(button_id.clone(), (device_id.to_string(), *control_id));
        }

        self.dirty = true;
        match room_id {
            Some(rid) => info!(
                "Registry: upserted device {} ({:?}) -> room {} ({} buttons)",
                device_id,
                device_type,
                rid,
                buttons.len()
            ),
            None => info!(
                "Registry: upserted roomless device {} ({:?}) ({} buttons)",
                device_id,
                device_type,
                buttons.len()
            ),
        }
    }

    /// Set or clear the room mapping for an already-known device.
    ///
    /// Used after the canonical room-assignment endpoint propagates a Rhythm
    /// room down to the hub registry. Only mutates `device_rooms`; preserves
    /// existing button/type entries.
    pub fn set_device_room(&mut self, device_id: &str, room_id: &str) {
        self.device_rooms
            .insert(device_id.to_string(), room_id.to_string());
        self.dirty = true;
    }

    /// Remove only the room mapping for a device, preserving button/type entries.
    pub fn clear_device_room(&mut self, device_id: &str) {
        if self.device_rooms.remove(device_id).is_some() {
            self.dirty = true;
        }
    }

    /// Check whether the registry has seen a device with this ID, regardless
    /// of whether it has a room assignment.
    pub fn has_device(&self, device_id: &str) -> bool {
        self.device_types.contains_key(device_id)
    }

    /// Remove a device and its button mappings from the registry.
    pub fn remove_device(&mut self, device_id: &str) {
        self.device_rooms.remove(device_id);
        self.device_types.remove(device_id);
        self.buttons.retain(|_, (owner, _)| owner != device_id);
        info!("Registry: removed device {}", device_id);
    }

    // =========================================================================
    // Query methods
    // =========================================================================

    /// Get the room ID for a button (for event routing).
    pub fn get_room_for_button(&self, button_id: &str) -> Option<String> {
        let (device_id, _) = self.buttons.get(button_id)?;
        self.device_rooms.get(device_id).cloned()
    }

    /// Get the control ID for a button (button index 1-4).
    pub fn get_control_id(&self, button_id: &str) -> Option<u8> {
        self.buttons
            .get(button_id)
            .map(|(_, control_id)| *control_id)
    }

    /// Get the device ID that owns a button.
    pub fn get_device_for_button(&self, button_id: &str) -> Option<String> {
        self.buttons
            .get(button_id)
            .map(|(device_id, _)| device_id.clone())
    }

    /// Get the grouped_light ID for a room (for light control).
    ///
    /// Returns `None` if the room has no grouped_light mapping or if the
    /// stored ID is empty.
    pub fn get_grouped_light_id(&self, room_id: &str) -> Option<String> {
        self.room_to_grouped_light
            .get(room_id)
            .filter(|id| !id.is_empty())
            .cloned()
    }

    /// Get the room ID that owns a grouped_light resource (reverse lookup).
    pub fn get_room_for_grouped_light(&self, grouped_light_id: &str) -> Option<String> {
        self.room_to_grouped_light
            .iter()
            .find(|(_, gl_id)| gl_id.as_str() == grouped_light_id)
            .map(|(room_id, _)| room_id.clone())
    }

    // =========================================================================
    // Motion sensor methods (derived from device_types + device_rooms)
    // =========================================================================

    /// Get the room ID for a motion sensor (for event routing).
    ///
    /// Motion sensors are now stored as regular devices with `DeviceType::Motion`.
    pub fn get_room_for_motion_sensor(&self, sensor_id: &str) -> Option<String> {
        if self.device_types.get(sensor_id) == Some(&DeviceType::Motion) {
            self.device_rooms.get(sensor_id).cloned()
        } else {
            None
        }
    }

    /// Get room IDs that have at least one motion sensor mapped.
    pub fn rooms_with_motion_sensors(&self) -> Vec<String> {
        let set: std::collections::HashSet<&str> = self
            .device_types
            .iter()
            .filter(|(_, dt)| **dt == DeviceType::Motion)
            .filter_map(|(id, _)| self.device_rooms.get(id).map(|r| r.as_str()))
            .collect();
        set.into_iter().map(|s| s.to_string()).collect()
    }

    /// Get all devices for a room with their types.
    pub fn devices_for_room_typed(&self, room_id: &str) -> Vec<(String, DeviceType)> {
        self.device_rooms
            .iter()
            .filter(|(_, r)| r.as_str() == room_id)
            .filter_map(|(dev_id, _)| {
                self.device_types
                    .get(dev_id)
                    .map(|dt| (dev_id.clone(), dt.clone()))
            })
            .collect()
    }

    /// Check if the registry has any rooms.
    pub fn has_rooms(&self) -> bool {
        !self.room_names.is_empty()
    }

    /// Look up the display name for a room ID.
    pub fn room_name(&self, room_id: &str) -> Option<&str> {
        self.room_names.get(room_id).map(String::as_str)
    }

    /// Convert to rhythm-core Room objects.
    pub fn rooms(&self) -> Vec<Room> {
        self.room_names
            .iter()
            .map(|(id, name)| Room::new(id.clone(), name.clone()))
            .collect()
    }

    // =========================================================================
    // Persistence
    // =========================================================================

    /// Create a compact snapshot for persistence.
    ///
    /// Includes Button and Motion devices in the `"dv"` array (not Light
    /// devices — those are re-pushed by room sync). Roomless devices are
    /// preserved with `room_id: None` so they survive across restarts.
    pub fn snapshot(&self) -> RegistrySnapshot {
        // Build typed device entries from device_types (Button + Motion only)
        let devices: Vec<SnapshotDevice> = self
            .device_types
            .iter()
            .map(|(dev_id, dt)| SnapshotDevice {
                id: dev_id.clone(),
                room_id: self.device_rooms.get(dev_id).cloned(),
                device_type: dt.clone(),
            })
            .collect();

        // Merge room_to_grouped_light + room_names into compact array
        let rooms: Vec<SnapshotRoom> = self
            .room_names
            .iter()
            .map(|(id, name)| SnapshotRoom {
                id: id.clone(),
                name: name.clone(),
                grouped_light_id: self
                    .room_to_grouped_light
                    .get(id)
                    .cloned()
                    .unwrap_or_default(),
            })
            .collect();

        RegistrySnapshot {
            rooms,
            devices,
            buttons: self.buttons.clone(),
            area_lights: self.area_lights.clone(),
        }
    }

    /// Restore registry from a persisted snapshot.
    ///
    /// Reconstructs `room_to_grouped_light` and `room_names` from the compact
    /// rooms array. Typed devices are restored from the `"dv"` field.
    pub fn restore_from_snapshot(&mut self, snapshot: RegistrySnapshot) {
        self.room_to_grouped_light.clear();
        self.room_names.clear();
        for room in &snapshot.rooms {
            self.room_names.insert(room.id.clone(), room.name.clone());
            self.room_to_grouped_light
                .insert(room.id.clone(), room.grouped_light_id.clone());
        }

        self.buttons = snapshot.buttons;
        self.area_lights = snapshot.area_lights;

        self.device_rooms.clear();
        self.device_types.clear();
        for dev in &snapshot.devices {
            if let Some(rid) = &dev.room_id {
                self.device_rooms.insert(dev.id.clone(), rid.clone());
            }
            self.device_types
                .insert(dev.id.clone(), dev.device_type.clone());
        }

        self.dirty = false;
        info!(
            "Registry restored from snapshot: {} rooms, {} buttons, {} devices",
            self.room_names.len(),
            self.buttons.len(),
            self.device_rooms.len(),
        );
    }
}

impl Default for HubDeviceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// =========================================================================
// HubRegistry trait implementation
// =========================================================================

impl HubRegistry for HubDeviceRegistry {
    fn rooms(&self) -> Vec<Room> {
        self.rooms()
    }

    fn get_grouped_light_id(&self, room_id: &str) -> Option<String> {
        self.get_grouped_light_id(room_id)
    }

    fn devices_for_room(&self, room_id: &str) -> Vec<String> {
        DeviceRegistry::devices_for_room(self, room_id)
    }

    fn room_matches(
        &self,
        id: &str,
        name: &str,
        grouped_light_id: &str,
        device_ids: &[String],
    ) -> bool {
        self.room_matches(id, name, grouped_light_id, device_ids)
    }

    fn upsert_room(&mut self, id: &str, name: &str, grouped_light_id: &str, device_ids: &[String]) {
        self.upsert_room(id, name, grouped_light_id, device_ids);
    }

    fn remove_room(&mut self, room_id: &str) {
        self.remove_room(room_id);
    }

    fn device_matches(
        &self,
        device_id: &str,
        room_id: Option<&str>,
        buttons: &[(String, u8)],
        device_type: &DeviceType,
    ) -> bool {
        self.device_matches(device_id, room_id, buttons, device_type)
    }

    fn upsert_device(
        &mut self,
        device_id: &str,
        room_id: Option<&str>,
        buttons: &[(String, u8)],
        device_type: DeviceType,
    ) {
        self.upsert_device(device_id, room_id, buttons, device_type);
    }

    fn remove_device(&mut self, device_id: &str) {
        self.remove_device(device_id);
    }

    fn set_device_room(&mut self, device_id: &str, room_id: &str) {
        self.set_device_room(device_id, room_id);
    }

    fn clear_device_room(&mut self, device_id: &str) {
        self.clear_device_room(device_id);
    }

    fn devices_for_room_typed(&self, room_id: &str) -> Vec<(String, DeviceType)> {
        self.devices_for_room_typed(room_id)
    }

    fn rooms_with_motion_sensors(&self) -> Vec<String> {
        self.rooms_with_motion_sensors()
    }

    fn snapshot_json(&self) -> serde_json::Value {
        serde_json::to_value(self.snapshot()).unwrap_or_default()
    }

    fn take_dirty(&mut self) -> bool {
        let was = self.dirty;
        self.dirty = false;
        was
    }
}

// =========================================================================
// DeviceRegistry trait implementation
// =========================================================================

impl DeviceRegistry for HubDeviceRegistry {
    fn get_room_for_device(&self, device_id: &str) -> Option<String> {
        self.device_rooms.get(device_id).cloned()
    }

    fn register_device(&mut self, device_id: &str, room_id: &str) {
        self.device_rooms
            .insert(device_id.to_string(), room_id.to_string());
        // Ensure dynamically registered rooms appear in room_names/list_rooms
        self.room_names
            .entry(room_id.to_string())
            .or_insert_with(|| room_id.to_string());
    }

    fn unregister_device(&mut self, device_id: &str) {
        self.device_rooms.remove(device_id);
    }

    fn list_rooms(&self) -> Vec<String> {
        let mut rooms: Vec<String> = self.room_names.keys().cloned().collect();
        rooms.sort();
        rooms
    }

    fn devices_for_room(&self, room_id: &str) -> Vec<String> {
        self.device_rooms
            .iter()
            .filter(|(_, room)| room.as_str() == room_id)
            .map(|(device, _)| device.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhythm_core::{DeviceRegistry, HubRegistry};

    // =========================================================================
    // Construction
    // =========================================================================

    #[test]
    fn new_creates_empty_registry() {
        let reg = HubDeviceRegistry::new();
        assert!(!reg.has_rooms());
        assert!(reg.rooms().is_empty());
        assert_eq!(reg.buttons.len(), 0);
        assert_eq!(reg.device_rooms.len(), 0);
    }

    #[test]
    fn default_creates_empty_registry() {
        let reg = HubDeviceRegistry::default();
        let mut reg = reg;
        reg.upsert_room("r1", "Room 1", "gl1", &[]);
        let rooms = reg.rooms();
        assert_eq!(rooms.len(), 1);
        assert_eq!(rooms[0].id, "r1");
    }

    #[test]
    fn with_options_ha_defaults_gl_to_room_id() {
        let mut reg = HubDeviceRegistry::with_options(true);
        reg.upsert_room("area_kitchen", "Kitchen", "", &[]);
        // With default_grouped_light_to_room_id=true, empty gl should default to room_id
        assert_eq!(
            reg.get_grouped_light_id("area_kitchen"),
            Some("area_kitchen".to_string())
        );
    }

    // =========================================================================
    // Room upsert
    // =========================================================================

    #[test]
    fn upsert_room_basic() {
        let mut reg = HubDeviceRegistry::new();
        let devices = vec!["light1".to_string(), "light2".to_string()];
        reg.upsert_room("r1", "Living Room", "gl_r1", &devices);

        assert!(reg.has_rooms());
        assert_eq!(reg.room_names.get("r1").unwrap(), "Living Room");
        assert_eq!(reg.get_grouped_light_id("r1"), Some("gl_r1".to_string()));
        assert_eq!(reg.device_rooms.get("light1").unwrap(), "r1");
        assert_eq!(reg.device_rooms.get("light2").unwrap(), "r1");
    }

    #[test]
    fn upsert_room_update_name() {
        let mut reg = HubDeviceRegistry::new();
        reg.upsert_room("r1", "Old Name", "gl1", &[]);
        reg.upsert_room("r1", "New Name", "gl1", &[]);
        assert_eq!(reg.room_names.get("r1").unwrap(), "New Name");
    }

    #[test]
    fn upsert_room_empty_gl_hue() {
        let mut reg = HubDeviceRegistry::new();
        reg.upsert_room("r1", "Room", "", &[]);
        // Hue mode: empty gl defaults to empty string, so get_grouped_light_id returns None
        assert_eq!(reg.get_grouped_light_id("r1"), None);
        // But the entry exists in the map
        assert_eq!(reg.room_to_grouped_light.get("r1").unwrap(), "");
    }

    #[test]
    fn upsert_room_empty_gl_ha() {
        let mut reg = HubDeviceRegistry::with_options(true);
        reg.upsert_room("area1", "Area 1", "", &[]);
        // HA mode: empty gl defaults to room_id
        assert_eq!(reg.get_grouped_light_id("area1"), Some("area1".to_string()));
    }

    #[test]
    fn upsert_room_preserves_existing_gl() {
        let mut reg = HubDeviceRegistry::new();
        reg.upsert_room("r1", "Room", "gl_original", &[]);
        // Re-upsert with empty gl should preserve existing
        reg.upsert_room("r1", "Room", "", &[]);
        assert_eq!(
            reg.get_grouped_light_id("r1"),
            Some("gl_original".to_string())
        );
    }

    #[test]
    fn upsert_room_replaces_device_mappings() {
        let mut reg = HubDeviceRegistry::new();
        let devices_v1 = vec!["light_a".to_string(), "light_b".to_string()];
        reg.upsert_room("r1", "Room", "gl1", &devices_v1);

        // Re-upsert with different devices
        let devices_v2 = vec!["light_c".to_string()];
        reg.upsert_room("r1", "Room", "gl1", &devices_v2);

        // Old light devices should be gone, new one present
        assert!(!reg.device_rooms.contains_key("light_a"));
        assert!(!reg.device_rooms.contains_key("light_b"));
        assert_eq!(reg.device_rooms.get("light_c").unwrap(), "r1");
    }

    #[test]
    fn upsert_room_preserves_button_devices() {
        let mut reg = HubDeviceRegistry::new();
        let lights = vec!["light1".to_string()];
        reg.upsert_room("r1", "Room", "gl1", &lights);

        // Add a switch device with buttons
        let buttons = vec![("btn1".to_string(), 1u8)];
        reg.upsert_device("switch1", Some("r1"), &buttons, DeviceType::Button);

        // Re-upsert room with different lights — switch1 should survive
        let lights_v2 = vec!["light2".to_string()];
        reg.upsert_room("r1", "Room", "gl1", &lights_v2);

        assert_eq!(reg.device_rooms.get("switch1").unwrap(), "r1");
        assert!(!reg.device_rooms.contains_key("light1"));
        assert_eq!(reg.device_rooms.get("light2").unwrap(), "r1");
    }

    #[test]
    fn upsert_room_preserves_motion_devices() {
        let mut reg = HubDeviceRegistry::new();
        let lights = vec!["light1".to_string()];
        reg.upsert_room("r1", "Room", "gl1", &lights);

        // Add a motion sensor as a device
        reg.upsert_device("ms1", Some("r1"), &[], DeviceType::Motion);

        // Re-upsert room with different lights — motion sensor should survive
        let lights_v2 = vec!["light2".to_string()];
        reg.upsert_room("r1", "Room", "gl1", &lights_v2);

        assert_eq!(reg.device_rooms.get("ms1").unwrap(), "r1");
        assert_eq!(reg.device_types.get("ms1"), Some(&DeviceType::Motion));
    }

    #[test]
    fn upsert_room_syncs_area_lights() {
        let mut reg = HubDeviceRegistry::new();
        let devices = vec!["light1".to_string(), "light2".to_string()];
        reg.upsert_room("r1", "Room", "gl1", &devices);

        let entities = reg.get_light_entities("r1");
        assert_eq!(entities.len(), 2);
        assert!(entities.contains(&"light1".to_string()));
        assert!(entities.contains(&"light2".to_string()));
    }

    #[test]
    fn find_room_for_exact_light_entities_matches_ignoring_order() {
        let mut reg = HubDeviceRegistry::new();
        reg.upsert_room(
            "r1",
            "Room",
            "gl1",
            &["light1".to_string(), "light2".to_string()],
        );
        reg.upsert_room("r2", "Other", "gl2", &["light3".to_string()]);

        let room_id =
            reg.find_room_for_exact_light_entities(&["light2".to_string(), "light1".to_string()]);

        assert_eq!(room_id.as_deref(), Some("r1"));
    }

    #[test]
    fn find_room_for_exact_light_entities_requires_exact_set() {
        let mut reg = HubDeviceRegistry::new();
        reg.upsert_room(
            "r1",
            "Room",
            "gl1",
            &["light1".to_string(), "light2".to_string()],
        );

        assert_eq!(
            reg.find_room_for_exact_light_entities(&["light1".to_string()]),
            None
        );
    }

    // =========================================================================
    // Room removal
    // =========================================================================

    #[test]
    fn remove_room_clears_all() {
        let mut reg = HubDeviceRegistry::new();
        let devices = vec!["light1".to_string()];
        reg.upsert_room("r1", "Room", "gl1", &devices);
        reg.upsert_device("ms1", Some("r1"), &[], DeviceType::Motion);

        reg.remove_room("r1");

        assert!(!reg.has_rooms());
        assert!(!reg.room_names.contains_key("r1"));
        assert!(!reg.room_to_grouped_light.contains_key("r1"));
        assert!(!reg.device_rooms.contains_key("light1"));
        assert!(!reg.device_rooms.contains_key("ms1"));
        assert!(!reg.device_types.contains_key("ms1"));
        assert!(reg.get_light_entities("r1").is_empty());
    }

    // =========================================================================
    // Device upsert / remove
    // =========================================================================

    #[test]
    fn upsert_device_with_buttons() {
        let mut reg = HubDeviceRegistry::new();
        let buttons = vec![("btn1".to_string(), 1u8), ("btn2".to_string(), 2u8)];
        reg.upsert_device("dev1", Some("r1"), &buttons, DeviceType::Button);

        assert_eq!(reg.device_rooms.get("dev1").unwrap(), "r1");
        assert_eq!(reg.device_types.get("dev1"), Some(&DeviceType::Button));
        assert_eq!(reg.buttons.get("btn1").unwrap(), &("dev1".to_string(), 1));
        assert_eq!(reg.buttons.get("btn2").unwrap(), &("dev1".to_string(), 2));
    }

    #[test]
    fn remove_device_clears_buttons_and_type() {
        let mut reg = HubDeviceRegistry::new();
        let buttons = vec![("btn1".to_string(), 1u8), ("btn2".to_string(), 2u8)];
        reg.upsert_device("dev1", Some("r1"), &buttons, DeviceType::Button);

        reg.remove_device("dev1");

        assert!(!reg.device_rooms.contains_key("dev1"));
        assert!(!reg.device_types.contains_key("dev1"));
        assert!(!reg.buttons.contains_key("btn1"));
        assert!(!reg.buttons.contains_key("btn2"));
    }

    #[test]
    fn upsert_device_replaces_buttons() {
        let mut reg = HubDeviceRegistry::new();
        let buttons_v1 = vec![("btn_old".to_string(), 1u8)];
        reg.upsert_device("dev1", Some("r1"), &buttons_v1, DeviceType::Button);

        let buttons_v2 = vec![("btn_new".to_string(), 3u8)];
        reg.upsert_device("dev1", Some("r1"), &buttons_v2, DeviceType::Button);

        // New button present
        assert_eq!(
            reg.buttons.get("btn_new").unwrap(),
            &("dev1".to_string(), 3)
        );
        // Note: upsert_device does NOT remove old buttons — it only inserts.
        // Old buttons remain unless remove_device is called first.
        // This is the actual behavior: btn_old still exists.
        assert!(reg.buttons.contains_key("btn_old"));
    }

    // =========================================================================
    // Query methods
    // =========================================================================

    #[test]
    fn get_room_for_button_found() {
        let mut reg = HubDeviceRegistry::new();
        reg.upsert_device(
            "dev1",
            Some("r1"),
            &[("btn1".to_string(), 1u8)],
            DeviceType::Button,
        );

        assert_eq!(reg.get_room_for_button("btn1"), Some("r1".to_string()));
    }

    #[test]
    fn get_room_for_button_not_found() {
        let reg = HubDeviceRegistry::new();
        assert_eq!(reg.get_room_for_button("nonexistent"), None);
    }

    #[test]
    fn get_room_for_grouped_light_forward_reverse() {
        let mut reg = HubDeviceRegistry::new();
        reg.upsert_room("r1", "Room 1", "gl_abc", &[]);

        // Forward: room -> gl
        assert_eq!(reg.get_grouped_light_id("r1"), Some("gl_abc".to_string()));
        // Reverse: gl -> room
        assert_eq!(
            reg.get_room_for_grouped_light("gl_abc"),
            Some("r1".to_string())
        );
    }

    #[test]
    fn get_control_id() {
        let mut reg = HubDeviceRegistry::new();
        reg.upsert_device(
            "dev1",
            Some("r1"),
            &[("btn1".to_string(), 3u8)],
            DeviceType::Button,
        );

        assert_eq!(reg.get_control_id("btn1"), Some(3));
        assert_eq!(reg.get_control_id("unknown"), None);
    }

    #[test]
    fn get_device_for_button() {
        let mut reg = HubDeviceRegistry::new();
        reg.upsert_device(
            "dev_xyz",
            Some("r1"),
            &[("btn_a".to_string(), 1u8)],
            DeviceType::Button,
        );

        assert_eq!(
            reg.get_device_for_button("btn_a"),
            Some("dev_xyz".to_string())
        );
    }

    // =========================================================================
    // Motion sensors (via typed devices)
    // =========================================================================

    #[test]
    fn motion_sensor_crud() {
        let mut reg = HubDeviceRegistry::new();

        // Upsert as Motion device
        reg.upsert_device("ms1", Some("r1"), &[], DeviceType::Motion);
        assert_eq!(
            reg.get_room_for_motion_sensor("ms1"),
            Some("r1".to_string())
        );

        // rooms_with_motion_sensors
        let rooms = reg.rooms_with_motion_sensors();
        assert_eq!(rooms.len(), 1);
        assert!(rooms.contains(&"r1".to_string()));

        // Remove
        reg.remove_device("ms1");
        assert_eq!(reg.get_room_for_motion_sensor("ms1"), None);
        assert!(reg.rooms_with_motion_sensors().is_empty());
    }

    #[test]
    fn rooms_with_motion_sensors_deduplicates() {
        let mut reg = HubDeviceRegistry::new();
        reg.upsert_device("ms1", Some("r1"), &[], DeviceType::Motion);
        reg.upsert_device("ms2", Some("r1"), &[], DeviceType::Motion);
        reg.upsert_device("ms3", Some("r2"), &[], DeviceType::Motion);

        let rooms = reg.rooms_with_motion_sensors();
        assert_eq!(rooms.len(), 2);
        assert!(rooms.contains(&"r1".to_string()));
        assert!(rooms.contains(&"r2".to_string()));
    }

    #[test]
    fn devices_for_room_typed() {
        let mut reg = HubDeviceRegistry::new();
        reg.upsert_device(
            "btn1",
            Some("r1"),
            &[("b1".to_string(), 1)],
            DeviceType::Button,
        );
        reg.upsert_device("ms1", Some("r1"), &[], DeviceType::Motion);
        reg.upsert_room("r1", "Room", "gl1", &["light1".to_string()]);

        let typed = reg.devices_for_room_typed("r1");
        assert_eq!(typed.len(), 2);
        assert!(typed
            .iter()
            .any(|(id, dt)| id == "btn1" && *dt == DeviceType::Button));
        assert!(typed
            .iter()
            .any(|(id, dt)| id == "ms1" && *dt == DeviceType::Motion));
    }

    #[test]
    fn room_matches_exact() {
        let mut reg = HubDeviceRegistry::new();
        let devices = vec!["light1".to_string(), "light2".to_string()];
        reg.upsert_room("r1", "Living Room", "gl1", &devices);

        assert!(reg.room_matches("r1", "Living Room", "gl1", &devices));
    }

    #[test]
    fn room_matches_different_name() {
        let mut reg = HubDeviceRegistry::new();
        let devices = vec!["light1".to_string()];
        reg.upsert_room("r1", "Living Room", "gl1", &devices);

        assert!(!reg.room_matches("r1", "Kitchen", "gl1", &devices));
    }

    #[test]
    fn room_matches_excludes_managed_devices() {
        let mut reg = HubDeviceRegistry::new();
        let lights = vec!["light1".to_string()];
        reg.upsert_room("r1", "Room", "gl1", &lights);
        // Add a switch device with buttons
        reg.upsert_device(
            "switch1",
            Some("r1"),
            &[("btn1".to_string(), 1u8)],
            DeviceType::Button,
        );
        // Add a motion device
        reg.upsert_device("ms1", Some("r1"), &[], DeviceType::Motion);

        // room_matches should match with only light devices, ignoring managed devices
        assert!(reg.room_matches("r1", "Room", "gl1", &lights));
    }

    #[test]
    fn device_matches_correct() {
        let mut reg = HubDeviceRegistry::new();
        let buttons = vec![("btn1".to_string(), 1u8), ("btn2".to_string(), 2u8)];
        reg.upsert_device("dev1", Some("r1"), &buttons, DeviceType::Button);

        assert!(reg.device_matches("dev1", Some("r1"), &buttons, &DeviceType::Button));
    }

    #[test]
    fn device_matches_wrong_room() {
        let mut reg = HubDeviceRegistry::new();
        let buttons = vec![("btn1".to_string(), 1u8)];
        reg.upsert_device("dev1", Some("r1"), &buttons, DeviceType::Button);

        assert!(!reg.device_matches("dev1", Some("r2"), &buttons, &DeviceType::Button));
    }

    #[test]
    fn device_matches_wrong_type() {
        let mut reg = HubDeviceRegistry::new();
        reg.upsert_device("dev1", Some("r1"), &[], DeviceType::Motion);

        assert!(!reg.device_matches("dev1", Some("r1"), &[], &DeviceType::Button));
        assert!(reg.device_matches("dev1", Some("r1"), &[], &DeviceType::Motion));
    }

    // =========================================================================
    // Snapshot / restore
    // =========================================================================

    #[test]
    fn snapshot_restore_roundtrip() {
        let mut reg = HubDeviceRegistry::new();
        reg.upsert_room("r1", "Living Room", "gl1", &["light1".to_string()]);
        reg.upsert_room("r2", "Bedroom", "gl2", &["light2".to_string()]);
        reg.upsert_device(
            "switch1",
            Some("r1"),
            &[("btn1".to_string(), 1), ("btn2".to_string(), 2)],
            DeviceType::Button,
        );
        reg.upsert_device("ms1", Some("r1"), &[], DeviceType::Motion);
        reg.set_area_lights("r1", vec!["entity1".to_string()]);

        let snapshot = reg.snapshot();

        let mut restored = HubDeviceRegistry::new();
        restored.restore_from_snapshot(snapshot);

        // Rooms
        assert_eq!(restored.room_names.get("r1").unwrap(), "Living Room");
        assert_eq!(restored.room_names.get("r2").unwrap(), "Bedroom");
        assert_eq!(restored.get_grouped_light_id("r1"), Some("gl1".to_string()));
        assert_eq!(restored.get_grouped_light_id("r2"), Some("gl2".to_string()));

        // Buttons
        assert_eq!(restored.get_control_id("btn1"), Some(1));
        assert_eq!(restored.get_control_id("btn2"), Some(2));

        // Device rooms (switch + motion in snapshot)
        assert_eq!(restored.device_rooms.get("switch1").unwrap(), "r1");
        assert_eq!(
            restored.device_types.get("switch1"),
            Some(&DeviceType::Button)
        );

        // Motion (via typed device)
        assert_eq!(
            restored.get_room_for_motion_sensor("ms1"),
            Some("r1".to_string())
        );
        assert_eq!(restored.device_types.get("ms1"), Some(&DeviceType::Motion));

        // Area lights
        assert_eq!(
            restored.get_light_entities("r1"),
            vec!["entity1".to_string()]
        );
    }

    #[test]
    fn snapshot_only_includes_typed_devices() {
        let mut reg = HubDeviceRegistry::new();
        reg.upsert_room(
            "r1",
            "Room",
            "gl1",
            &["light1".to_string(), "light2".to_string()],
        );
        reg.upsert_device(
            "switch1",
            Some("r1"),
            &[("btn1".to_string(), 1)],
            DeviceType::Button,
        );
        reg.upsert_device("ms1", Some("r1"), &[], DeviceType::Motion);

        let snapshot = reg.snapshot();

        // devices should contain both switch and motion, but not lights
        assert_eq!(snapshot.devices.len(), 2);
        assert!(snapshot
            .devices
            .iter()
            .any(|d| d.id == "switch1" && d.device_type == DeviceType::Button));
        assert!(snapshot
            .devices
            .iter()
            .any(|d| d.id == "ms1" && d.device_type == DeviceType::Motion));
    }

    #[test]
    fn snapshot_json_via_hub_registry() {
        let mut reg = HubDeviceRegistry::new();
        reg.upsert_room("r1", "Test Room", "gl1", &[]);

        let json = HubRegistry::snapshot_json(&reg);
        assert!(json.is_object());
        let obj = json.as_object().unwrap();
        assert!(obj.contains_key("r"));
        assert!(obj.contains_key("dv"));
        assert!(obj.contains_key("b"));

        // Verify it round-trips through serde
        let snapshot: RegistrySnapshot = serde_json::from_value(json).unwrap();
        assert_eq!(snapshot.rooms.len(), 1);
        assert_eq!(snapshot.rooms[0].name, "Test Room");
    }

    // =========================================================================
    // DeviceRegistry trait
    // =========================================================================

    #[test]
    fn device_registry_register_auto_creates_room() {
        let mut reg = HubDeviceRegistry::new();
        DeviceRegistry::register_device(&mut reg, "dev1", "auto_room");

        // register_device auto-creates room_names entry with room_id as name
        assert_eq!(reg.room_names.get("auto_room").unwrap(), "auto_room");
        assert_eq!(
            DeviceRegistry::get_room_for_device(&reg, "dev1"),
            Some("auto_room".to_string())
        );
    }

    #[test]
    fn device_registry_unregister() {
        let mut reg = HubDeviceRegistry::new();
        DeviceRegistry::register_device(&mut reg, "dev1", "r1");
        assert!(DeviceRegistry::get_room_for_device(&reg, "dev1").is_some());

        DeviceRegistry::unregister_device(&mut reg, "dev1");
        assert!(DeviceRegistry::get_room_for_device(&reg, "dev1").is_none());
    }

    #[test]
    fn device_registry_devices_for_room() {
        let mut reg = HubDeviceRegistry::new();
        reg.upsert_room(
            "r1",
            "Room",
            "gl1",
            &["light1".to_string(), "light2".to_string()],
        );
        reg.upsert_device(
            "switch1",
            Some("r1"),
            &[("btn1".to_string(), 1)],
            DeviceType::Button,
        );

        let devices = DeviceRegistry::devices_for_room(&reg, "r1");
        assert_eq!(devices.len(), 3);
        assert!(devices.contains(&"light1".to_string()));
        assert!(devices.contains(&"light2".to_string()));
        assert!(devices.contains(&"switch1".to_string()));
    }

    #[test]
    fn has_rooms_and_rooms_list() {
        let mut reg = HubDeviceRegistry::new();

        assert!(!reg.has_rooms());
        assert!(DeviceRegistry::list_rooms(&reg).is_empty());

        reg.upsert_room("r_beta", "Beta", "gl1", &[]);
        reg.upsert_room("r_alpha", "Alpha", "gl2", &[]);

        assert!(reg.has_rooms());

        let rooms = reg.rooms();
        assert_eq!(rooms.len(), 2);

        // list_rooms returns sorted room IDs
        let listed = DeviceRegistry::list_rooms(&reg);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0], "r_alpha");
        assert_eq!(listed[1], "r_beta");
    }
}
