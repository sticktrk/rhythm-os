//! Hub discovery abstraction.
//!
//! Defines the `HubDiscovery` trait and discovery types that hub crates
//! implement to provide server-side room/device/sensor discovery.
//! The sync orchestrator (`room_sync`) uses these to diff discovered
//! topology against current state.

use anyhow::Result;
use rhythm_core::runtime::hub_registry::DeviceType;

use crate::canonical::identity::DiscoveredIdentity;
use crate::scenes::{LightSceneOutput, SceneDefinition};

/// One integration-native light action in a Rhythm-owned room scene.
#[derive(Clone, Debug, PartialEq)]
pub struct ManagedSceneProjectionTarget {
    /// Integration-native device identity, not a service or grouped-light ID.
    pub native_device_id: String,
    pub output: LightSceneOutput,
}

/// A room-scoped scene that an authoritative integration should materialize
/// inside its hidden controller topology and then recall atomically.
#[derive(Clone, Debug, PartialEq)]
pub struct ManagedSceneProjection {
    pub rhythm_room_id: String,
    pub hub_room_id: String,
    pub scene_id: String,
    pub targets: Vec<ManagedSceneProjectionTarget>,
}

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
    /// Hub-native room this device belongs to. `None` when the device exists on
    /// the hub but is not assigned to any hub room.
    pub room_id: Option<String>,
    /// Button mappings: (button_resource_id, control_id). Empty for motion sensors.
    pub buttons: Vec<(String, u8)>,
    /// Device type (Button, Motion, etc.).
    pub device_type: DeviceType,
}

/// A motion sensor with its current state, discovered at startup.
pub struct DiscoveredMotionState {
    /// Sensor entity/resource ID.
    pub sensor_id: String,
    /// Hub-native room this sensor belongs to. Roomless sensors remain
    /// first-class inputs and resolve their target through canonical topology.
    pub room_id: Option<String>,
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
    /// Whether a later source-room rename should replace an established
    /// canonical Rhythm room name. Integrations that expose generated or
    /// controller-facing names can opt out while still using those names to
    /// bootstrap newly discovered rooms.
    fn room_names_are_authoritative(&self) -> bool {
        true
    }

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
                room_id: d.room_id,
                room_name: None,
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

    /// Discover native scenes assigned to one hub-native room.
    ///
    /// Returned definitions are ephemeral projections for user-facing scene
    /// pickers and must use [`crate::scenes::native_scene_id`] for their stable
    /// public IDs. The native integration remains authoritative; callers must
    /// not persist these definitions as Rhythm-owned scenes.
    fn discover_scenes(&self, _room_id: &str) -> Result<Vec<SceneDefinition>> {
        Ok(vec![])
    }

    /// Recall a native scene by its integration-owned external ID.
    ///
    /// Integrations that expose native scenes should override this together
    /// with [`HubDiscovery::discover_scenes`].
    fn recall_scene(&self, _scene_id: &str, _transition_ms: Option<u32>) -> Result<()> {
        anyhow::bail!("Native scene recall is not supported by this integration")
    }

    /// Materialize and recall a Rhythm-owned room scene through the native
    /// controller. `ephemeral` projections are preview-only and must be
    /// removed after recall rather than persisted in integration ownership.
    ///
    /// Returns `true` only when this authoritative integration handled the
    /// projection. Required-group callers fail closed on `false`.
    fn apply_managed_scene_projection(
        &self,
        _projection: &ManagedSceneProjection,
        _transition_ms: Option<u32>,
        _ephemeral: bool,
    ) -> Result<bool> {
        Ok(false)
    }

    /// Delete every native projection owned for one Rhythm scene ID.
    /// Returns `true` when this authoritative integration handled the request,
    /// including when no matching projection remained.
    fn delete_managed_scene_projection(&self, _scene_id: &str) -> Result<bool> {
        Ok(false)
    }

    /// Discover devices with native hub automation configured.
    ///
    /// Returns `(behavior_id, device_id)` pairs for devices that have
    /// hub-side automations (e.g. Hue behavior_instances). These conflict
    /// with Rhythm and should be surfaced as triage items so the user can
    /// remove them in the hub's native app.
    ///
    /// Default returns empty — hubs without native automation support.
    fn discover_configured_devices(&self) -> Result<Vec<(String, String)>> {
        Ok(vec![])
    }

    /// Release cached network connections to free memory.
    ///
    /// Called after all discovery methods have been invoked so the
    /// transport's TLS session can be dropped before the runtime
    /// creates its own connection. Default is a no-op.
    fn release_resources(&self) {}
}
