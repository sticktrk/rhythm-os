//! Matter device discovery for room sync.

use std::sync::Arc;

use anyhow::Result;
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
        let persisted_devices = self
            .transport
            .list_commissioned_devices()
            .unwrap_or_default();
        let devices = if persisted_devices.is_empty() {
            self.transport
                .list_devices()?
                .into_iter()
                .map(|info| (Self::fallback_commissioned(&info), Some(info)))
                .collect::<Vec<_>>()
        } else {
            persisted_devices
                .into_iter()
                .map(|device| (device, None))
                .collect::<Vec<_>>()
        };
        let mut identities = Vec::with_capacity(devices.len());

        for (commissioned, fallback_info) in devices {
            if self
                .hub_data
                .is_decommission_suppressed(commissioned.node_id)
            {
                continue;
            }

            let used_fallback = fallback_info.is_some();
            if let Some(info) = fallback_info {
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
            } else if !self
                .hub_data
                .device_caps
                .lock()
                .is_ok_and(|caps| caps.contains_key(&device_id))
                || !self
                    .hub_data
                    .device_quirks
                    .lock()
                    .is_ok_and(|quirks| quirks.contains_key(&device_id))
            {
                crate::commissioning::store_device_metadata(
                    &self.hub_data,
                    &commissioned,
                    &device_id,
                );
            }

            identities.push(Self::device_identity(&commissioned));
        }

        Ok(identities)
    }

    fn endpoint_capabilities(&self, native_id: &str) -> Option<serde_json::Value> {
        let capabilities = self
            .hub_data
            .device_caps
            .lock()
            .ok()?
            .get(native_id)
            .cloned()?;
        crate::lifecycle::normalized_endpoint_capabilities(&capabilities)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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
        persisted_devices: Vec<CommissionedDevice>,
        probe_calls: AtomicUsize,
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

        fn list_commissioned_devices(&self) -> anyhow::Result<Vec<CommissionedDevice>> {
            Ok(self.persisted_devices.clone())
        }

        fn probe_light(&self, _node_id: u64) -> anyhow::Result<CommissionedDevice> {
            self.probe_calls.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("discovery must not probe persisted devices")
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
            device_profiles: Mutex::new(HashMap::new()),
            pending_turn_on_plans: Arc::new(Mutex::new(HashMap::new())),
            needs_audition: Arc::new(Mutex::new(HashSet::new())),
            readback: Arc::new(crate::hub_state::MatterReadbackCoordinator::default()),
            local_overrides: Mutex::new(crate::local_quirks::LocalMatterOverrides::default()),
            cloud_profiles: Mutex::new(CloudMatterProfileCatalog::default()),
            decommissioning: Mutex::new(HashSet::new()),
            recently_decommissioned: Mutex::new(HashMap::new()),
            node_proof_of_life: Arc::new(Mutex::new(HashMap::new())),
            on_off_observations: Arc::new(Mutex::new(HashMap::new())),
            attribute_report_history: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            event_tx,
        })
    }

    #[test]
    fn discovery_uses_persisted_records_without_probing_and_honors_suppression() {
        let hub_data = hub_data();
        assert!(hub_data.begin_decommission(3));
        assert!(hub_data.begin_decommission(5));

        let transport = Arc::new(FakeTransport {
            devices: Vec::new(),
            persisted_devices: vec![
                commissioned_device(1, 2),
                commissioned_device(2, 1),
                commissioned_device(3, 1),
                commissioned_device(5, 1),
            ],
            probe_calls: AtomicUsize::new(0),
        });
        let discovery = MatterDiscovery::new(transport.clone(), hub_data.clone());

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
        assert_eq!(transport.probe_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn discovery_falls_back_to_basic_identity_without_probing_old_transports() {
        let hub_data = hub_data();
        let transport = Arc::new(FakeTransport {
            devices: vec![device_info(7)],
            persisted_devices: Vec::new(),
            probe_calls: AtomicUsize::new(0),
        });
        let discovery = MatterDiscovery::new(transport.clone(), hub_data);

        let identities = discovery.discover_identities().unwrap();

        assert_eq!(identities.len(), 1);
        assert_eq!(identities[0].native_id, "matter-7");
        assert_eq!(identities[0].name, "Vendor 7 Lamp 7");
        assert_eq!(transport.probe_calls.load(Ordering::SeqCst), 0);
    }
}
