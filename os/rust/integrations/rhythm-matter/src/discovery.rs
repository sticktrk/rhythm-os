//! Matter device discovery for room sync.
//!
//! Implements `HubDiscovery` by enumerating commissioned devices in the
//! local Matter fabric. Unlike hub integrations (Hue, HA), Matter devices
//! don't come pre-grouped into rooms — the user assigns them via the app.

use std::sync::Arc;

use anyhow::Result;
use rhythm_core::runtime::hub_registry::DeviceType;

use rhythm_os::discovery::{DiscoveredDevice, DiscoveredRoom, HubDiscovery};

use crate::lifecycle::format_device_id;
use crate::transport::MatterTransport;

/// Matter hub discovery — enumerates commissioned devices.
pub struct MatterDiscovery<T: MatterTransport> {
    transport: Arc<T>,
}

impl<T: MatterTransport> MatterDiscovery<T> {
    pub fn new(transport: Arc<T>) -> Self {
        Self { transport }
    }
}

impl<T: MatterTransport + 'static> HubDiscovery for MatterDiscovery<T> {
    fn discover_rooms(&self) -> Result<Vec<DiscoveredRoom>> {
        // Matter has no native room concept. Each commissioned device
        // is returned as its own synthetic "room" (named after the device).
        // The user organizes them into Rhythm rooms via the topology system.
        let devices = self.transport.commissioned_devices()?;
        let rooms: Vec<DiscoveredRoom> = devices
            .iter()
            .map(|d| {
                let device_id = format_device_id(d.node_id, 1);
                DiscoveredRoom {
                    id: device_id.clone(),
                    name: format!("{} {}", d.vendor_name, d.product_name),
                    // For Matter, grouped_light_id == room_id (per-device addressing)
                    grouped_light_id: device_id.clone(),
                    device_ids: vec![device_id],
                }
            })
            .collect();

        Ok(rooms)
    }

    fn discover_devices(&self) -> Result<Vec<DiscoveredDevice>> {
        let devices = self.transport.commissioned_devices()?;
        let discovered: Vec<DiscoveredDevice> = devices
            .iter()
            .map(|d| {
                let device_id = format_device_id(d.node_id, 1);
                DiscoveredDevice {
                    device_id: device_id.clone(),
                    room_id: device_id, // Self-roomed until user assigns
                    buttons: Vec::new(),
                    device_type: DeviceType::Light,
                }
            })
            .collect();

        Ok(discovered)
    }

}
