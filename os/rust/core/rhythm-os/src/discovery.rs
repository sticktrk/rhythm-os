//! Hub discovery abstraction.
//!
//! Defines the `HubDiscovery` trait and discovery types that hub crates
//! implement to provide server-side room/device/sensor discovery.
//! The sync orchestrator (`room_sync`) uses these to diff discovered
//! topology against current state.

use anyhow::Result;
use rhythm_core::runtime::hub_registry::DeviceType;

use crate::canonical::identity::DiscoveredIdentity;

/// A room discovered from the hub (Hue room, HA area, etc.).
pub struct DiscoveredRoom {
    /// Unique room identifier (Hue room UUID, HA area_id).
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Grouped light resource ID for room-level control.
    pub grouped_light_id: String,
    /// Device IDs associated with this room.
    pub device_ids: Vec<String>,
}

/// A typed device discovered from the hub.
pub struct DiscoveredDevice {
    /// Unique device identifier.
    pub device_id: String,
    /// Room this device belongs to.
    pub room_id: String,
    /// Button mappings: (button_resource_id, control_id). Empty for motion sensors.
    pub buttons: Vec<(String, u8)>,
    /// Device type (Button, Motion, etc.).
    pub device_type: DeviceType,
}

/// A motion sensor with its current state, discovered at startup.
pub struct DiscoveredMotionState {
    /// Sensor entity/resource ID.
    pub sensor_id: String,
    /// Room this sensor belongs to.
    pub room_id: String,
    /// Whether the sensor is currently detecting motion.
    pub is_active: bool,
}

/// Hub-agnostic discovery interface.
///
/// Implemented by hub crates (rhythm-hue, rhythm-ha) to discover rooms
/// and devices from the hub's own topology. Called by
/// `room_sync::sync_from_hub()` to keep the server's state in sync
/// with the hub without requiring app-side pushes.
pub trait HubDiscovery: Send + Sync {
    /// Discover all rooms from the hub.
    fn discover_rooms(&self) -> Result<Vec<DiscoveredRoom>>;

    /// Discover all typed devices (buttons + motion sensors) from the hub.
    fn discover_devices(&self) -> Result<Vec<DiscoveredDevice>>;

    /// Discover devices with hardware identity information.
    ///
    /// Extracts hardware IDs (MAC, IEEE, serial), manufacturer/model, and
    /// room names for cross-hub deduplication. Default wraps `discover_devices()`
    /// with empty hardware IDs.
    fn discover_identities(&self) -> Result<Vec<DiscoveredIdentity>> {
        // Default: convert DiscoveredDevice to DiscoveredIdentity with empty HW IDs
        let devices = self.discover_devices()?;
        Ok(devices
            .into_iter()
            .map(|d| DiscoveredIdentity {
                native_id: d.device_id,
                room_id: d.room_id.clone(),
                room_name: String::new(), // No room name available in basic discovery
                name: String::new(),
                device_type: d.device_type,
                hardware_ids: vec![],
                manufacturer: None,
                model: None,
            })
            .collect())
    }

    /// Discover current motion sensor states from the hub.
    ///
    /// Used at startup to seed the motion timer system with sensors
    /// that are already active. Default returns empty (hubs where
    /// the event stream replays current state don't need this).
    fn discover_motion_state(&self) -> Result<Vec<DiscoveredMotionState>> {
        Ok(vec![])
    }

    /// Release cached network connections to free memory.
    ///
    /// Called after all discovery methods have been invoked so the
    /// transport's TLS session can be dropped before the runtime
    /// creates its own connection. Default is a no-op.
    fn release_resources(&self) {}
}
