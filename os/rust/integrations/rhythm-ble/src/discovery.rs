//! Canonical discovery projection for local-BLE profiles.

use std::sync::Arc;

use anyhow::Result;
use rhythm_os::canonical::identity::{DiscoveredIdentity, HardwareId};
use rhythm_os::discovery::{DiscoveredDevice, DiscoveredRoom, HubDiscovery};

use crate::profile::profile_by_id;
use crate::store::LocalBleDeviceStore;

pub struct LocalBleDiscovery {
    store: Arc<LocalBleDeviceStore>,
}

impl LocalBleDiscovery {
    pub fn new(store: Arc<LocalBleDeviceStore>) -> Self {
        Self { store }
    }
}

impl HubDiscovery for LocalBleDiscovery {
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
            .filter_map(|device| {
                let projection = profile_by_id(&device.profile_id)?.projection();
                Some(DiscoveredIdentity {
                    native_id: device.id.clone(),
                    room_id: None,
                    room_name: None,
                    name: projection.display_name.to_string(),
                    device_type: projection.device_type,
                    // The persisted random public ID is stable on this
                    // appliance without exposing or cross-home correlating the
                    // private profile identity used to match advertisements.
                    hardware_ids: vec![HardwareId::serial(&device.id)],
                    manufacturer: projection.manufacturer.map(str::to_string),
                    model: projection.model.map(str::to_string),
                })
            })
            .collect())
    }
}
