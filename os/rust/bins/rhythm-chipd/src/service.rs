use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use rhythm_matter::chip_rpc::{
    ChipInitControllerRequest, ChipInitControllerResponse, ChipRpcAttributeReportsResponse,
    ChipRpcCommissionLightResponse, ChipRpcEmpty, ChipRpcJsonValueResponse,
    ChipRpcListDevicesResponse, ChipRpcProbeLightResponse, ChipRpcReadOnOffResponse,
    ChipRpcRequest,
};
use rhythm_matter::transport::{CommissionedDevice, MatterDeviceInfo};

use crate::backend::ChipControllerBackend;

#[derive(Debug, Clone)]
pub struct CommissioningState {
    pub fabric_id: String,
    pub operational_fabric_id: u64,
    pub ipk_hex: String,
    pub storage_path: PathBuf,
}

impl CommissioningState {
    pub fn devices_path(&self) -> PathBuf {
        self.storage_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("devices.json")
    }
}

pub struct ChipControllerService {
    backend: Box<dyn ChipControllerBackend>,
    device_store: DeviceStore,
    state: Option<CommissioningState>,
}

impl ChipControllerService {
    pub fn new(backend: Box<dyn ChipControllerBackend>) -> Self {
        Self {
            backend,
            device_store: DeviceStore::default(),
            state: None,
        }
    }

    pub fn handle(&mut self, request: ChipRpcRequest) -> Result<serde_json::Value> {
        match request {
            ChipRpcRequest::InitController(request) => {
                let result = self.init_controller(request)?;
                Ok(serde_json::to_value(result)?)
            }
            ChipRpcRequest::CommissionLight(request) => {
                self.require_initialized()?;
                let device = self.backend.commission_light(&request)?;
                self.device_store.upsert(device.clone())?;
                Ok(serde_json::to_value(ChipRpcCommissionLightResponse {
                    device,
                })?)
            }
            ChipRpcRequest::ListDevices => Ok(serde_json::to_value(ChipRpcListDevicesResponse {
                devices: self.device_store.list_devices(),
            })?),
            ChipRpcRequest::ProbeLight { node_id } => {
                self.require_initialized()?;
                let device = self.backend.probe_light(node_id)?;
                self.device_store.upsert(device.clone())?;
                Ok(serde_json::to_value(ChipRpcProbeLightResponse { device })?)
            }
            ChipRpcRequest::DecommissionDevice { node_id, force } => {
                self.require_initialized()?;
                self.backend.decommission_device(node_id, force)?;
                self.device_store.remove(node_id)?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::SetOnOff {
                node_id,
                endpoint,
                on,
            } => {
                self.require_initialized()?;
                self.backend.set_on_off(node_id, endpoint, on)?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::ConfigureGroup { group } => {
                self.require_initialized()?;
                self.backend.configure_group(&group)?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::RemoveGroup { group_id, members } => {
                self.require_initialized()?;
                self.backend.remove_group(group_id, &members)?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::SetGroupOnOff { group_id, on } => {
                self.require_initialized()?;
                self.backend.set_group_on_off(group_id, on)?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::IdentifyGroup {
                group_id,
                duration_secs,
            } => {
                self.require_initialized()?;
                self.backend.identify_group(group_id, duration_secs)?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::SetGroupBrightness {
                group_id,
                level,
                transition_ms,
            } => {
                self.require_initialized()?;
                self.backend
                    .set_group_brightness(group_id, level, transition_ms)?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::SetGroupColorTemperature {
                group_id,
                kelvin,
                transition_ms,
            } => {
                self.require_initialized()?;
                self.backend
                    .set_group_color_temperature(group_id, kelvin, transition_ms)?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::SetGroupXy {
                group_id,
                x,
                y,
                transition_ms,
            } => {
                self.require_initialized()?;
                self.backend.set_group_xy(group_id, x, y, transition_ms)?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::SetGroupHueSaturation {
                group_id,
                hue,
                saturation,
                transition_ms,
            } => {
                self.require_initialized()?;
                self.backend
                    .set_group_hue_saturation(group_id, hue, saturation, transition_ms)?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::IdentifyLight {
                node_id,
                endpoint,
                duration_secs,
            } => {
                self.require_initialized()?;
                self.backend
                    .identify_light(node_id, endpoint, duration_secs)?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::SetBrightness {
                node_id,
                endpoint,
                level,
                transition_ms,
            } => {
                self.require_initialized()?;
                self.backend
                    .set_brightness(node_id, endpoint, level, transition_ms)?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::RunLevelCommand {
                node_id,
                endpoint,
                command,
                level_or_step,
                step_mode,
                transition_ms,
            } => {
                self.require_initialized()?;
                self.backend.run_level_command(
                    node_id,
                    endpoint,
                    command,
                    level_or_step,
                    step_mode,
                    transition_ms,
                )?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::SetColorTemperature {
                node_id,
                endpoint,
                kelvin,
                transition_ms,
            } => {
                self.require_initialized()?;
                self.backend
                    .set_color_temperature(node_id, endpoint, kelvin, transition_ms)?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::SetXy {
                node_id,
                endpoint,
                x,
                y,
                transition_ms,
            } => {
                self.require_initialized()?;
                self.backend
                    .set_xy(node_id, endpoint, x, y, transition_ms)?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::SetHueSaturation {
                node_id,
                endpoint,
                hue,
                saturation,
                transition_ms,
            } => {
                self.require_initialized()?;
                self.backend.set_hue_saturation(
                    node_id,
                    endpoint,
                    hue,
                    saturation,
                    transition_ms,
                )?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::ReadOnOff { node_id, endpoint } => {
                self.require_initialized()?;
                let on = self.backend.read_on_off(node_id, endpoint)?;
                Ok(serde_json::to_value(ChipRpcReadOnOffResponse { on })?)
            }
            ChipRpcRequest::ReadLightCapabilitySnapshot { node_id, endpoint } => {
                self.require_initialized()?;
                let value = self
                    .backend
                    .read_light_capability_snapshot(node_id, endpoint)?;
                Ok(serde_json::to_value(ChipRpcJsonValueResponse { value })?)
            }
            ChipRpcRequest::ReadLightState { node_id, endpoint } => {
                self.require_initialized()?;
                let value = self.backend.read_light_state(node_id, endpoint)?;
                Ok(serde_json::to_value(ChipRpcJsonValueResponse { value })?)
            }
            ChipRpcRequest::SubscribeOnOff {
                targets,
                min_interval_secs,
                max_interval_secs,
            } => {
                self.require_initialized()?;
                self.backend
                    .subscribe_on_off(&targets, min_interval_secs, max_interval_secs)?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::DrainAttributeReports => {
                self.require_initialized()?;
                let reports = self.backend.drain_attribute_reports()?;
                Ok(serde_json::to_value(ChipRpcAttributeReportsResponse {
                    reports,
                })?)
            }
        }
    }

    fn init_controller(
        &mut self,
        request: ChipInitControllerRequest,
    ) -> Result<ChipInitControllerResponse> {
        let ChipInitControllerRequest {
            fabric_id,
            operational_fabric_id,
            ipk_hex,
            storage_path,
            ble_controller,
        } = request;
        let state = CommissioningState {
            fabric_id,
            operational_fabric_id,
            ipk_hex,
            storage_path: PathBuf::from(storage_path),
        };

        self.device_store.configure(state.devices_path())?;
        let existing_devices = self.device_store.devices();
        let response = self
            .backend
            .init_controller(&state, ble_controller, &existing_devices)?;
        self.state = Some(state);
        Ok(response)
    }

    fn require_initialized(&self) -> Result<&CommissioningState> {
        self.state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Controller not initialized"))
    }
}

#[derive(Default)]
struct DeviceStore {
    path: Option<PathBuf>,
    devices: BTreeMap<u64, CommissionedDevice>,
}

impl DeviceStore {
    fn configure(&mut self, path: PathBuf) -> Result<()> {
        self.path = Some(path.clone());
        let loaded = match fs::read_to_string(&path) {
            Ok(json) => serde_json::from_str::<Vec<CommissionedDevice>>(&json)
                .with_context(|| format!("decoding {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                return Err(error).with_context(|| format!("reading {}", path.display()));
            }
        };
        self.devices = loaded
            .into_iter()
            .map(|device| (device.node_id, device))
            .collect();
        Ok(())
    }

    fn devices(&self) -> Vec<CommissionedDevice> {
        self.devices.values().cloned().collect()
    }

    fn list_devices(&self) -> Vec<MatterDeviceInfo> {
        self.devices
            .values()
            .map(|device| MatterDeviceInfo {
                node_id: device.node_id,
                vendor_name: device.vendor_name.clone(),
                product_name: device.product_name.clone(),
                reachable: true,
            })
            .collect()
    }

    fn upsert(&mut self, device: CommissionedDevice) -> Result<()> {
        self.devices.insert(device.node_id, device);
        self.flush()
    }

    fn remove(&mut self, node_id: u64) -> Result<()> {
        self.devices.remove(&node_id);
        self.flush()
    }

    fn flush(&self) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(&self.devices())?;
        fs::write(path, json).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    use rhythm_matter::chip_rpc::{
        ChipRpcAttributeReportsResponse, ChipRpcCommissionLightResponse, ChipRpcEmpty,
        ChipRpcJsonValueResponse, ChipRpcListDevicesResponse, ChipRpcProbeLightResponse,
        ChipRpcReadOnOffResponse,
    };
    use rhythm_matter::transport::{
        MatterColorMode, MatterCommissionRequest, MatterCommissioningNetwork,
        MatterCommissioningRendezvous, MatterCommissioningWifiCredentials, MatterGroup,
        MatterGroupMember, MatterLevelCommandVariant, MatterLevelStepMode,
        MatterSubscriptionTarget,
    };

    use crate::backend::FakeChipBackend;

    fn unique_test_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rhythm-chipd-{}-{}", name, nanos));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn init_request(storage_path: &Path) -> ChipInitControllerRequest {
        ChipInitControllerRequest {
            fabric_id: "fabric-test".to_string(),
            operational_fabric_id: 0x1234,
            ipk_hex: "00112233445566778899aabbccddeeff".to_string(),
            storage_path: storage_path.display().to_string(),
            ble_controller: Some(1),
        }
    }

    fn commission_request(node_id: u64) -> MatterCommissionRequest {
        MatterCommissionRequest {
            setup_payload: format!("MT:payload-{}", node_id),
            node_id,
            network: MatterCommissioningNetwork::Wifi,
            rendezvous: MatterCommissioningRendezvous::Auto,
            wifi_credentials: MatterCommissioningWifiCredentials {
                ssid: "Rhythm".to_string(),
                password: "secret".to_string(),
            },
        }
    }

    fn commissioned_device(node_id: u64, endpoint: u16) -> CommissionedDevice {
        CommissionedDevice {
            node_id,
            vendor_name: "Vendor".to_string(),
            product_name: format!("Lamp {}", node_id),
            vendor_id: 1,
            product_id: 2,
            serial_number: Some(format!("serial-{}", node_id)),
            light_endpoint: endpoint,
            color_modes: vec![MatterColorMode::ColorTemperature],
            min_kelvin: Some(2700),
            max_kelvin: Some(5000),
        }
    }

    fn string_error<T>(result: Result<T>) -> String {
        match result {
            Ok(_) => panic!("expected error"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn service_routes_rpc_requests_and_persists_device_store() {
        let dir = unique_test_dir("service-flow");
        let storage_path = dir.join("chip.json");
        let devices_path = dir.join("devices.json");
        fs::write(
            &devices_path,
            serde_json::to_string_pretty(&vec![commissioned_device(10, 2)]).unwrap(),
        )
        .unwrap();

        let mut service = ChipControllerService::new(Box::new(FakeChipBackend::default()));
        let initial_list: ChipRpcListDevicesResponse =
            serde_json::from_value(service.handle(ChipRpcRequest::ListDevices).unwrap()).unwrap();
        assert!(initial_list.devices.is_empty());
        assert_eq!(
            string_error(service.handle(ChipRpcRequest::ProbeLight { node_id: 10 })),
            "Controller not initialized"
        );

        let init: ChipInitControllerResponse = serde_json::from_value(
            service
                .handle(ChipRpcRequest::InitController(init_request(&storage_path)))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(init.fabric_id, "fabric-test");
        assert_eq!(init.operational_fabric_id, 0x1234);

        let list: ChipRpcListDevicesResponse =
            serde_json::from_value(service.handle(ChipRpcRequest::ListDevices).unwrap()).unwrap();
        assert_eq!(list.devices.len(), 1);
        assert_eq!(list.devices[0].node_id, 10);

        let probed: ChipRpcProbeLightResponse = serde_json::from_value(
            service
                .handle(ChipRpcRequest::ProbeLight { node_id: 10 })
                .unwrap(),
        )
        .unwrap();
        assert_eq!(probed.device.light_endpoint, 2);

        let commissioned: ChipRpcCommissionLightResponse = serde_json::from_value(
            service
                .handle(ChipRpcRequest::CommissionLight(commission_request(42)))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(commissioned.device.node_id, 42);
        assert!(fs::read_to_string(&devices_path)
            .unwrap()
            .contains("\"node_id\": 42"));

        let _: ChipRpcEmpty = serde_json::from_value(
            service
                .handle(ChipRpcRequest::SetOnOff {
                    node_id: 42,
                    endpoint: 1,
                    on: true,
                })
                .unwrap(),
        )
        .unwrap();
        let on: ChipRpcReadOnOffResponse = serde_json::from_value(
            service
                .handle(ChipRpcRequest::ReadOnOff {
                    node_id: 42,
                    endpoint: 1,
                })
                .unwrap(),
        )
        .unwrap();
        assert!(on.on);

        let _: ChipRpcEmpty = serde_json::from_value(
            service
                .handle(ChipRpcRequest::SetBrightness {
                    node_id: 42,
                    endpoint: 1,
                    level: 0,
                    transition_ms: Some(100),
                })
                .unwrap(),
        )
        .unwrap();
        let _: ChipRpcEmpty = serde_json::from_value(
            service
                .handle(ChipRpcRequest::RunLevelCommand {
                    node_id: 42,
                    endpoint: 1,
                    command: MatterLevelCommandVariant::StepWithOnOff,
                    level_or_step: 12,
                    step_mode: Some(MatterLevelStepMode::Up),
                    transition_ms: Some(50),
                })
                .unwrap(),
        )
        .unwrap();
        let _: ChipRpcEmpty = serde_json::from_value(
            service
                .handle(ChipRpcRequest::SetColorTemperature {
                    node_id: 42,
                    endpoint: 1,
                    kelvin: 1800,
                    transition_ms: None,
                })
                .unwrap(),
        )
        .unwrap();
        let _: ChipRpcEmpty = serde_json::from_value(
            service
                .handle(ChipRpcRequest::SetXy {
                    node_id: 42,
                    endpoint: 1,
                    x: 0.31,
                    y: 0.32,
                    transition_ms: Some(250),
                })
                .unwrap(),
        )
        .unwrap();
        let _: ChipRpcEmpty = serde_json::from_value(
            service
                .handle(ChipRpcRequest::SetHueSaturation {
                    node_id: 42,
                    endpoint: 1,
                    hue: 20,
                    saturation: 200,
                    transition_ms: None,
                })
                .unwrap(),
        )
        .unwrap();
        let _: ChipRpcEmpty = serde_json::from_value(
            service
                .handle(ChipRpcRequest::IdentifyLight {
                    node_id: 42,
                    endpoint: 1,
                    duration_secs: 5,
                })
                .unwrap(),
        )
        .unwrap();

        let group = MatterGroup {
            group_id: 7,
            name: "Kitchen".to_string(),
            members: vec![
                MatterGroupMember {
                    node_id: 10,
                    endpoint: 2,
                },
                MatterGroupMember {
                    node_id: 42,
                    endpoint: 1,
                },
            ],
        };
        let _: ChipRpcEmpty = serde_json::from_value(
            service
                .handle(ChipRpcRequest::ConfigureGroup {
                    group: group.clone(),
                })
                .unwrap(),
        )
        .unwrap();
        let _: ChipRpcEmpty = serde_json::from_value(
            service
                .handle(ChipRpcRequest::IdentifyGroup {
                    group_id: 7,
                    duration_secs: 2,
                })
                .unwrap(),
        )
        .unwrap();
        let _: ChipRpcEmpty = serde_json::from_value(
            service
                .handle(ChipRpcRequest::SetGroupOnOff {
                    group_id: 7,
                    on: true,
                })
                .unwrap(),
        )
        .unwrap();
        let _: ChipRpcEmpty = serde_json::from_value(
            service
                .handle(ChipRpcRequest::SetGroupBrightness {
                    group_id: 7,
                    level: 0,
                    transition_ms: Some(100),
                })
                .unwrap(),
        )
        .unwrap();
        let _: ChipRpcEmpty = serde_json::from_value(
            service
                .handle(ChipRpcRequest::SetGroupColorTemperature {
                    group_id: 7,
                    kelvin: 2700,
                    transition_ms: None,
                })
                .unwrap(),
        )
        .unwrap();
        let _: ChipRpcEmpty = serde_json::from_value(
            service
                .handle(ChipRpcRequest::SetGroupXy {
                    group_id: 7,
                    x: 0.1,
                    y: 0.2,
                    transition_ms: None,
                })
                .unwrap(),
        )
        .unwrap();
        let _: ChipRpcEmpty = serde_json::from_value(
            service
                .handle(ChipRpcRequest::SetGroupHueSaturation {
                    group_id: 7,
                    hue: 10,
                    saturation: 20,
                    transition_ms: None,
                })
                .unwrap(),
        )
        .unwrap();

        let capability: ChipRpcJsonValueResponse = serde_json::from_value(
            service
                .handle(ChipRpcRequest::ReadLightCapabilitySnapshot {
                    node_id: 42,
                    endpoint: 1,
                })
                .unwrap(),
        )
        .unwrap();
        assert_eq!(capability.value["node_id"], 42);

        let light_state: ChipRpcJsonValueResponse = serde_json::from_value(
            service
                .handle(ChipRpcRequest::ReadLightState {
                    node_id: 42,
                    endpoint: 1,
                })
                .unwrap(),
        )
        .unwrap();
        assert_eq!(light_state.value["onoff"]["ok"], true);

        let _: ChipRpcEmpty = serde_json::from_value(
            service
                .handle(ChipRpcRequest::SubscribeOnOff {
                    targets: vec![MatterSubscriptionTarget {
                        node_id: 42,
                        endpoint: 1,
                    }],
                    min_interval_secs: 1,
                    max_interval_secs: 60,
                })
                .unwrap(),
        )
        .unwrap();
        let reports: ChipRpcAttributeReportsResponse = serde_json::from_value(
            service
                .handle(ChipRpcRequest::DrainAttributeReports)
                .unwrap(),
        )
        .unwrap();
        assert!(reports.reports.is_empty());

        let _: ChipRpcEmpty = serde_json::from_value(
            service
                .handle(ChipRpcRequest::RemoveGroup {
                    group_id: 7,
                    members: group.members,
                })
                .unwrap(),
        )
        .unwrap();
        let _: ChipRpcEmpty = serde_json::from_value(
            service
                .handle(ChipRpcRequest::DecommissionDevice {
                    node_id: 42,
                    force: true,
                })
                .unwrap(),
        )
        .unwrap();
        assert!(!fs::read_to_string(&devices_path)
            .unwrap()
            .contains("\"node_id\": 42"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn device_store_handles_missing_path_noop_and_decode_errors() {
        let mut store = DeviceStore::default();
        store.upsert(commissioned_device(1, 1)).unwrap();
        assert_eq!(store.devices().len(), 1);

        let dir = unique_test_dir("store-errors");
        let devices_path = dir.join("devices.json");
        fs::write(&devices_path, "{ not valid json").unwrap();

        let error = string_error(store.configure(devices_path.clone()));
        assert!(error.contains(&format!("decoding {}", devices_path.display())));

        fs::write(
            &devices_path,
            serde_json::to_string(&vec![commissioned_device(2, 3)]).unwrap(),
        )
        .unwrap();
        store.configure(devices_path.clone()).unwrap();
        assert_eq!(store.devices().len(), 1);
        assert_eq!(store.list_devices()[0].node_id, 2);

        store.remove(2).unwrap();
        assert_eq!(store.devices().len(), 0);
        assert_eq!(fs::read_to_string(&devices_path).unwrap(), "[]");

        let _ = fs::remove_dir_all(dir);
    }
}
