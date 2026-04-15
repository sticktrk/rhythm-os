//! Matter hub-specific state stored in `ActiveHub::hub_data`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rhythm_devices::LightCapabilities;
use rhythm_os::hub::HubEvent;

use crate::controller::MatterDeviceRegistry;
#[cfg(feature = "desktop")]
use crate::desktop_transport::MatcTransport;
use crate::transport::{CommissionedDevice, MatterDeviceInfo};

/// Matter-specific state stored in `ActiveHub::hub_data`.
///
/// Downcast via `active_hub.data::<Arc<MatterHubData>>()`.
pub struct MatterHubData {
    /// Shared transport — single `DeviceManager` instance for the fabric.
    /// All code paths (commands, commissioning, probing) share this to avoid
    /// port 5555 conflicts from multiple DMs.
    /// Initialized via `OnceLock::set()` after `connect_matter()` returns.
    #[cfg(feature = "desktop")]
    pub transport: std::sync::OnceLock<Arc<MatcTransport>>,
    /// Device registry (shared with controller).
    pub registry: Arc<Mutex<MatterDeviceRegistry>>,
    /// Matter fabric identifier.
    pub fabric_id: String,
    /// Currently commissioned devices.
    pub commissioned: Mutex<Vec<MatterDeviceInfo>>,
    /// Per-device capabilities, keyed by device ID (e.g., "matter-42").
    /// Populated during commissioning via `capabilities_from_commissioned()`.
    pub device_caps: Mutex<HashMap<String, LightCapabilities>>,
    /// Event channel sender — keeps the channel alive for the event loop.
    /// Subscription handling can later use this to emit real device events.
    pub event_tx: std::sync::mpsc::Sender<HubEvent>,
}

impl MatterHubData {
    /// Allocate the next local node ID for a new commission.
    pub fn next_node_id(&self) -> u64 {
        self.commissioned
            .lock()
            .map(|c| c.iter().map(|d| d.node_id).max().unwrap_or(99) + 1)
            .unwrap_or(100)
    }

    /// Upsert a newly commissioned device into the in-memory fabric cache.
    pub fn record_commissioned_device(&self, device: &CommissionedDevice) {
        let info = MatterDeviceInfo {
            node_id: device.node_id,
            vendor_name: device.vendor_name.clone(),
            product_name: device.product_name.clone(),
            reachable: true,
        };

        if let Ok(mut list) = self.commissioned.lock() {
            if let Some(existing) = list.iter_mut().find(|d| d.node_id == device.node_id) {
                *existing = info;
            } else {
                list.push(info);
            }
        }
    }

    /// Remove a device from the commissioned list and capabilities cache.
    ///
    /// Called during decommission — protocol + integration-specific cleanup.
    /// Does NOT touch the hub device registry or canonical registry (that's
    /// the rhythm-os handler's responsibility).
    pub fn remove_device(&self, node_id: u64) {
        if let Ok(mut list) = self.commissioned.lock() {
            list.retain(|d| d.node_id != node_id);
        }
        let prefix = format!("matter-{}", node_id);
        if let Ok(mut caps) = self.device_caps.lock() {
            caps.retain(|k, _| k != &prefix && !k.starts_with(&format!("{}-", prefix)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commissioned_device(
        node_id: u64,
        vendor_name: &str,
        product_name: &str,
    ) -> CommissionedDevice {
        CommissionedDevice {
            node_id,
            vendor_name: vendor_name.to_string(),
            product_name: product_name.to_string(),
            vendor_id: 1,
            product_id: 2,
            serial_number: None,
            light_endpoint: 1,
            color_modes: Vec::new(),
            min_kelvin: None,
            max_kelvin: None,
        }
    }

    fn hub_data() -> MatterHubData {
        let (event_tx, _event_rx) = std::sync::mpsc::channel();
        MatterHubData {
            #[cfg(feature = "desktop")]
            transport: std::sync::OnceLock::new(),
            registry: Arc::new(Mutex::new(MatterDeviceRegistry::new())),
            fabric_id: "default".to_string(),
            commissioned: Mutex::new(Vec::new()),
            device_caps: Mutex::new(HashMap::new()),
            event_tx,
        }
    }

    #[test]
    fn next_node_id_defaults_to_100() {
        let hub_data = hub_data();
        assert_eq!(hub_data.next_node_id(), 100);
    }

    #[test]
    fn record_commissioned_device_updates_cache() {
        let hub_data = hub_data();

        hub_data.record_commissioned_device(&commissioned_device(100, "Vendor", "Lamp"));
        hub_data.record_commissioned_device(&commissioned_device(101, "Vendor", "Lamp 2"));

        let commissioned = hub_data.commissioned.lock().unwrap();
        assert_eq!(commissioned.len(), 2);
        assert_eq!(commissioned[0].node_id, 100);
        assert_eq!(commissioned[1].node_id, 101);
        drop(commissioned);

        hub_data.record_commissioned_device(&commissioned_device(100, "Updated", "Lamp"));

        let commissioned = hub_data.commissioned.lock().unwrap();
        assert_eq!(commissioned.len(), 2);
        assert_eq!(commissioned[0].vendor_name, "Updated");
        drop(commissioned);

        assert_eq!(hub_data.next_node_id(), 102);
    }
}
