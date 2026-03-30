//! Matter device discovery for room sync.
//!
//! Implements `HubDiscovery` by enumerating commissioned devices in the
//! local Matter fabric. Unlike hub integrations (Hue, HA), Matter devices
//! don't come pre-grouped into rooms — the user assigns them via the app.

use std::sync::Arc;

use anyhow::Result;

use rhythm_os::discovery::{DiscoveredDevice, DiscoveredRoom, HubDiscovery};

use crate::transport::MatterTransport;

/// Matter hub discovery — returns empty.
///
/// Matter devices are tracked via the canonical registry (created during
/// pairing), not via hub sync discovery. Both rooms and devices return
/// empty to prevent phantom self-roomed entries.
pub struct MatterDiscovery<T: MatterTransport> {
    _transport: Arc<T>,
}

impl<T: MatterTransport> MatterDiscovery<T> {
    pub fn new(transport: Arc<T>) -> Self {
        Self { _transport: transport }
    }
}

impl<T: MatterTransport + 'static> HubDiscovery for MatterDiscovery<T> {
    fn discover_rooms(&self) -> Result<Vec<DiscoveredRoom>> {
        // Matter has no native room concept. Rooms are created during
        // explicit pairing via start_pairing(), not during sync discovery.
        // Returning empty here prevents phantom rooms from appearing for
        // every commissioned device in the fabric.
        Ok(Vec::new())
    }

    fn discover_devices(&self) -> Result<Vec<DiscoveredDevice>> {
        // Matter devices are tracked via the canonical registry (created during
        // pairing), not via hub sync discovery. Returning empty here prevents
        // phantom self-roomed entries (device_id == room_id) in the hub registry.
        // The user assigns Matter devices to rooms via the canonical assign_room API.
        Ok(Vec::new())
    }
}
