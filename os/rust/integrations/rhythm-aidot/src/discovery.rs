use std::sync::Arc;

use anyhow::Result;
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_os::canonical::identity::{DiscoveredIdentity, HardwareId};
use rhythm_os::discovery::{DiscoveredDevice, DiscoveredRoom, HubDiscovery};

use crate::store::AidotButtonStore;

pub struct AidotButtonDiscovery {
    store: Arc<AidotButtonStore>,
}

impl AidotButtonDiscovery {
    pub fn new(store: Arc<AidotButtonStore>) -> Self {
        Self { store }
    }
}

impl HubDiscovery for AidotButtonDiscovery {
    fn discover_rooms(&self) -> Result<Vec<DiscoveredRoom>> {
        Ok(Vec::new())
    }

    fn discover_devices(&self) -> Result<Vec<DiscoveredDevice>> {
        Ok(Vec::new())
    }

    fn discover_identities(&self) -> Result<Vec<DiscoveredIdentity>> {
        Ok(self
            .store
            .all()
            .into_iter()
            .map(|device| {
                let name = device.display_name().to_string();
                DiscoveredIdentity {
                    native_id: device.id,
                    room_id: None,
                    room_name: None,
                    name,
                    device_type: DeviceType::Button,
                    hardware_ids: vec![HardwareId::mac(&device.ble_identity)],
                    manufacturer: Some("Orein/AiDot".to_string()),
                    model: Some("OC02001-CR-B".to_string()),
                }
            })
            .collect())
    }
}
