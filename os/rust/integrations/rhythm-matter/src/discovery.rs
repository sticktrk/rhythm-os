//! Matter device discovery for room sync.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use log::warn;
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_os::canonical::identity::{DiscoveredIdentity, HardwareId};
use rhythm_os::discovery::{DiscoveredDevice, DiscoveredRoom, HubDiscovery};

use crate::hub_state::MatterHubData;
use crate::transport::{CommissionedDevice, MatterDeviceInfo, MatterTransport};

const SYNC_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

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
            room_id: None,
            room_name: None,
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
            if self.hub_data.is_decommission_suppressed(info.node_id) {
                continue;
            }

            let (commissioned, used_fallback) = match crate::lifecycle::probe_light_with_deadline(
                &self.transport,
                info.node_id,
                SYNC_PROBE_TIMEOUT,
            ) {
                Ok(device) => (device, false),
                Err(error) => {
                    warn!(
                        target: "room_sync",
                        "Matter: probe failed for node {} during sync, using basic device info: {}",
                        info.node_id,
                        error
                    );
                    (Self::fallback_commissioned(&info), true)
                }
            };

            if self
                .hub_data
                .is_decommission_suppressed(commissioned.node_id)
            {
                continue;
            }

            if used_fallback {
                self.hub_data.record_device_info(&info, false);
            } else {
                self.hub_data.record_commissioned_device(&commissioned);
            }

            let device_id = crate::lifecycle::format_device_id(
                commissioned.node_id,
                commissioned.light_endpoint,
            );
            if used_fallback {
                crate::commissioning::store_fallback_device_metadata(&self.hub_data, &device_id);
            } else {
                crate::commissioning::store_device_metadata(
                    &self.hub_data,
                    &commissioned,
                    &device_id,
                );
                if let Err(error) = crate::capture::persist_device_capture(
                    &self.hub_data,
                    &commissioned,
                    "sync_probe",
                ) {
                    warn!(
                        target: "room_sync",
                        "Matter: failed to persist sync probe capture for {}: {}",
                        device_id,
                        error
                    );
                }
            }

            identities.push(Self::device_identity(&commissioned));
        }

        Ok(identities)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::AtomicU64;
    use std::sync::{Mutex, OnceLock};

    use rhythm_os::hub::HubEvent;

    use crate::cloud_profiles::CloudMatterProfileCatalog;
    use crate::controller::MatterDeviceRegistry;
    use crate::transport::{
        MatterColorMode, MatterCommissionRequest, MatterGroup, MatterGroupMember,
        MatterLevelCommandVariant, MatterLevelStepMode, MatterSubscriptionTarget,
    };

    struct FakeTransport {
        devices: Vec<MatterDeviceInfo>,
        probes: HashMap<u64, Result<CommissionedDevice, String>>,
    }

    impl MatterTransport for FakeTransport {
        fn commission_light(
            &self,
            _request: &MatterCommissionRequest,
        ) -> anyhow::Result<CommissionedDevice> {
            anyhow::bail!("not used")
        }

        fn decommission_device(&self, _node_id: u64, _force: bool) -> anyhow::Result<()> {
            Ok(())
        }

        fn list_devices(&self) -> anyhow::Result<Vec<MatterDeviceInfo>> {
            Ok(self.devices.clone())
        }

        fn probe_light(&self, node_id: u64) -> anyhow::Result<CommissionedDevice> {
            match self.probes.get(&node_id) {
                Some(Ok(device)) => Ok(device.clone()),
                Some(Err(error)) => anyhow::bail!("{}", error),
                None => anyhow::bail!("unknown node"),
            }
        }

        fn set_on_off(&self, _node_id: u64, _endpoint: u16, _on: bool) -> anyhow::Result<()> {
            Ok(())
        }

        fn configure_group(&self, _group: &MatterGroup) -> anyhow::Result<()> {
            Ok(())
        }

        fn remove_group(
            &self,
            _group_id: u16,
            _members: &[MatterGroupMember],
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_group_on_off(&self, _group_id: u16, _on: bool) -> anyhow::Result<()> {
            Ok(())
        }

        fn identify_group(&self, _group_id: u16, _duration_secs: u16) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_group_brightness(
            &self,
            _group_id: u16,
            _level: u8,
            _transition_ms: Option<u32>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_group_color_temperature(
            &self,
            _group_id: u16,
            _kelvin: u16,
            _transition_ms: Option<u32>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_group_xy(
            &self,
            _group_id: u16,
            _x: f32,
            _y: f32,
            _transition_ms: Option<u32>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_group_hue_saturation(
            &self,
            _group_id: u16,
            _hue: u8,
            _saturation: u8,
            _transition_ms: Option<u32>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn identify_light(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _duration_secs: u16,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_brightness(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _level: u8,
            _transition_ms: Option<u32>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn run_level_command(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _command: MatterLevelCommandVariant,
            _level_or_step: u8,
            _step_mode: Option<MatterLevelStepMode>,
            _transition_ms: Option<u32>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_color_temperature(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _kelvin: u16,
            _transition_ms: Option<u32>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_xy(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _x: f32,
            _y: f32,
            _transition_ms: Option<u32>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_hue_saturation(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _hue: u8,
            _saturation: u8,
            _transition_ms: Option<u32>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn read_on_off(&self, _node_id: u64, _endpoint: u16) -> anyhow::Result<bool> {
            Ok(false)
        }

        fn subscribe_on_off(
            &self,
            _targets: &[MatterSubscriptionTarget],
            _min_interval_secs: u16,
            _max_interval_secs: u16,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn device_info(node_id: u64) -> MatterDeviceInfo {
        MatterDeviceInfo {
            node_id,
            vendor_name: format!("Vendor {}", node_id),
            product_name: format!("Lamp {}", node_id),
            reachable: true,
        }
    }

    fn commissioned_device(node_id: u64, endpoint: u16) -> CommissionedDevice {
        CommissionedDevice {
            node_id,
            vendor_name: format!("Vendor {}", node_id),
            product_name: format!("Lamp {}", node_id),
            vendor_id: 1,
            product_id: 2,
            serial_number: None,
            light_endpoint: endpoint,
            color_modes: vec![MatterColorMode::ColorTemperature],
            min_kelvin: Some(2700),
            max_kelvin: Some(5000),
        }
    }

    fn hub_data() -> Arc<MatterHubData> {
        let (event_tx, _event_rx) = std::sync::mpsc::channel::<HubEvent>();
        Arc::new(MatterHubData {
            transport: OnceLock::new(),
            capture_dir: OnceLock::new(),
            registry: Arc::new(Mutex::new(MatterDeviceRegistry::new())),
            fabric_id: "test".to_string(),
            commissioned: Mutex::new(Vec::new()),
            next_node_id: AtomicU64::new(100),
            device_caps: Mutex::new(HashMap::new()),
            device_quirks: Mutex::new(HashMap::new()),
            cloud_profiles: Mutex::new(CloudMatterProfileCatalog::default()),
            decommissioning: Mutex::new(HashSet::new()),
            recently_decommissioned: Mutex::new(HashMap::new()),
            event_tx,
        })
    }

    #[test]
    fn discovery_emits_identities_with_probe_fallback_and_suppression() {
        let hub_data = hub_data();
        assert!(hub_data.begin_decommission(3));
        assert!(hub_data.begin_decommission(5));

        let transport = Arc::new(FakeTransport {
            devices: vec![
                device_info(1),
                device_info(2),
                device_info(3),
                device_info(4),
            ],
            probes: HashMap::from([
                (1, Ok(commissioned_device(1, 2))),
                (2, Err("probe failed".to_string())),
                (4, Ok(commissioned_device(5, 1))),
            ]),
        });
        let discovery = MatterDiscovery::new(transport, hub_data.clone());

        assert!(discovery.discover_rooms().unwrap().is_empty());
        assert!(discovery.discover_devices().unwrap().is_empty());
        let identities = discovery.discover_identities().unwrap();

        assert_eq!(identities.len(), 2);
        assert_eq!(identities[0].native_id, "matter-1-2");
        assert_eq!(identities[0].name, "Vendor 1 Lamp 1");
        assert_eq!(identities[0].manufacturer.as_deref(), Some("Vendor 1"));
        assert_eq!(identities[0].model.as_deref(), Some("Lamp 1"));
        assert_eq!(identities[0].hardware_ids, vec![HardwareId::matter("1")]);
        assert_eq!(identities[1].native_id, "matter-2");
        assert_eq!(identities[1].name, "Vendor 2 Lamp 2");

        let commissioned = hub_data.commissioned.lock().unwrap();
        assert_eq!(commissioned.len(), 2);
        assert_eq!(commissioned[0].node_id, 1);
        assert_eq!(commissioned[1].node_id, 2);
        assert!(hub_data
            .device_caps
            .lock()
            .unwrap()
            .contains_key("matter-1-2"));
        assert!(hub_data
            .device_caps
            .lock()
            .unwrap()
            .contains_key("matter-2"));
    }
}
