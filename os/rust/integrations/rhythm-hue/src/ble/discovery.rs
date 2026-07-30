//! Projection of bonded Hue BLE bulbs into Rhythm discovery.

use std::sync::Arc;

use anyhow::Result;
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_os::canonical::identity::{DiscoveredIdentity, HardwareId};
use rhythm_os::discovery::{DiscoveredDevice, DiscoveredRoom, HubDiscovery};

use super::store::HueBleDeviceStore;

pub struct HueBleDiscovery {
    store: Arc<HueBleDeviceStore>,
}

impl HueBleDiscovery {
    pub fn new(store: Arc<HueBleDeviceStore>) -> Self {
        Self { store }
    }
}

impl HubDiscovery for HueBleDiscovery {
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
            .map(|device| DiscoveredIdentity {
                native_id: device.id.clone(),
                room_id: None,
                room_name: None,
                name: device.display_name(),
                device_type: DeviceType::Light,
                // The EUI-64 is the stable identity shared with Hue Bridge
                // discovery. A printed six-character serial is not part of
                // direct BLE pairing and must not cause an unsafe merge.
                hardware_ids: vec![HardwareId::mac(&device.eui64)],
                manufacturer: Some(device.manufacturer),
                model: Some(device.model),
            })
            .collect())
    }
}
