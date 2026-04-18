//! Matter device discovery for room sync.

use std::sync::Arc;

use anyhow::Result;
use log::warn;
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_os::canonical::identity::{DiscoveredIdentity, HardwareId};
use rhythm_os::discovery::{DiscoveredDevice, DiscoveredRoom, HubDiscovery};

use crate::hub_state::MatterHubData;
use crate::transport::{CommissionedDevice, MatterDeviceInfo, MatterTransport};

/// Matter hub discovery.
///
/// Matter has no native room topology to mirror into Rhythm, but the server
/// still needs to surface already-commissioned bulbs as canonical devices so
/// macOS and restart flows can pick them up without re-commissioning.
pub struct MatterDiscovery {
    transport: Arc<dyn MatterTransport>,
    hub_data: Arc<MatterHubData>,
}

impl MatterDiscovery {
    pub fn new(transport: Arc<dyn MatterTransport>, hub_data: Arc<MatterHubData>) -> Self {
        Self {
            transport,
            hub_data,
        }
    }

    fn fallback_commissioned(info: &MatterDeviceInfo) -> CommissionedDevice {
        CommissionedDevice {
            node_id: info.node_id,
            vendor_name: info.vendor_name.clone(),
            product_name: info.product_name.clone(),
            vendor_id: 0,
            product_id: 0,
            serial_number: None,
            light_endpoint: 1,
            color_modes: Vec::new(),
            min_kelvin: None,
            max_kelvin: None,
        }
    }

    fn device_identity(device: &CommissionedDevice) -> DiscoveredIdentity {
        DiscoveredIdentity {
            native_id: crate::lifecycle::format_device_id(device.node_id, device.light_endpoint),
            room_id: String::new(),
            room_name: String::new(),
            name: format!("{} {}", device.vendor_name, device.product_name),
            device_type: DeviceType::Light,
            hardware_ids: vec![HardwareId::matter(&device.node_id.to_string())],
            manufacturer: Some(device.vendor_name.clone()),
            model: Some(device.product_name.clone()),
        }
    }
}

impl HubDiscovery for MatterDiscovery {
    fn discover_rooms(&self) -> Result<Vec<DiscoveredRoom>> {
        Ok(Vec::new())
    }

    fn discover_devices(&self) -> Result<Vec<DiscoveredDevice>> {
        Ok(Vec::new())
    }

    fn discover_identities(&self) -> Result<Vec<DiscoveredIdentity>> {
        let devices = self.transport.list_devices()?;
        let mut identities = Vec::with_capacity(devices.len());

        for info in devices {
            let commissioned = match self.transport.probe_light(info.node_id) {
                Ok(device) => device,
                Err(error) => {
                    warn!(
                        target: "room_sync",
                        "Matter: probe failed for node {} during sync, using basic device info: {}",
                        info.node_id,
                        error
                    );
                    Self::fallback_commissioned(&info)
                }
            };

            self.hub_data.record_commissioned_device(&commissioned);

            let device_id = crate::lifecycle::format_device_id(
                commissioned.node_id,
                commissioned.light_endpoint,
            );
            crate::commissioning::store_device_capabilities(
                &self.hub_data,
                &commissioned,
                &device_id,
            );

            identities.push(Self::device_identity(&commissioned));
        }

        Ok(identities)
    }
}
