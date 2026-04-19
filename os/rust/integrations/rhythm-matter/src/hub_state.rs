//! Matter hub-specific state stored in `ActiveHub::hub_data`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rhythm_devices::{DeviceQuirk, LightCapabilities};
use rhythm_os::hub::HubEvent;

use crate::controller::MatterDeviceRegistry;
#[cfg(feature = "desktop")]
use crate::transport::MatterTransport;
use crate::transport::{CommissionedDevice, MatterDeviceInfo};

/// Matter-specific state stored in `ActiveHub::hub_data`.
pub struct MatterHubData {
    /// Shared transport for all controller, commissioning, and probe paths.
    #[cfg(feature = "desktop")]
    pub transport: std::sync::OnceLock<Arc<dyn MatterTransport>>,
    /// Optional directory for raw probe captures.
    pub capture_dir: std::sync::OnceLock<String>,
    /// Device registry (shared with controller).
    pub registry: Arc<Mutex<MatterDeviceRegistry>>,
    /// Matter fabric identifier.
    pub fabric_id: String,
    /// Currently commissioned devices.
    pub commissioned: Mutex<Vec<MatterDeviceInfo>>,
    /// Next monotonic local node ID to assign.
    pub next_node_id: AtomicU64,
    /// Per-device capabilities keyed by device ID (for example `matter-42`).
    pub device_caps: Mutex<HashMap<String, LightCapabilities>>,
    /// Per-device Matter quirks from `rhythm-devices`.
    pub device_quirks: Mutex<HashMap<String, Vec<DeviceQuirk>>>,
    /// Event channel sender kept alive by the hub data.
    pub event_tx: std::sync::mpsc::Sender<HubEvent>,
}

impl MatterHubData {
    /// Reserve the next node ID.
    pub fn reserve_node_id(&self) -> u64 {
        self.next_node_id.fetch_add(1, Ordering::SeqCst)
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

        let next_after_device = device.node_id.saturating_add(1);
        let mut current = self.next_node_id.load(Ordering::SeqCst);
        while next_after_device > current {
            match self.next_node_id.compare_exchange(
                current,
                next_after_device,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
    }

    /// Remove a device from the commissioned list and capabilities cache.
    pub fn remove_device(&self, node_id: u64) {
        if let Ok(mut list) = self.commissioned.lock() {
            list.retain(|device| device.node_id != node_id);
        }

        let prefix = format!("matter-{}", node_id);
        if let Ok(mut caps) = self.device_caps.lock() {
            caps.retain(|key, _| key != &prefix && !key.starts_with(&format!("{}-", prefix)));
        }
        if let Ok(mut quirks) = self.device_quirks.lock() {
            quirks.retain(|key, _| key != &prefix && !key.starts_with(&format!("{}-", prefix)));
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
            capture_dir: std::sync::OnceLock::new(),
            registry: Arc::new(Mutex::new(MatterDeviceRegistry::new())),
            fabric_id: "default".to_string(),
            commissioned: Mutex::new(Vec::new()),
            next_node_id: AtomicU64::new(100),
            device_caps: Mutex::new(HashMap::new()),
            device_quirks: Mutex::new(HashMap::new()),
            event_tx,
        }
    }

    #[test]
    fn reserve_node_id_starts_at_100() {
        let hub_data = hub_data();
        assert_eq!(hub_data.reserve_node_id(), 100);
        assert_eq!(hub_data.reserve_node_id(), 101);
    }

    #[test]
    fn record_commissioned_device_updates_cache_and_counter() {
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

        assert_eq!(hub_data.reserve_node_id(), 102);
    }
}
