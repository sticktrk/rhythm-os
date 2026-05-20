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
