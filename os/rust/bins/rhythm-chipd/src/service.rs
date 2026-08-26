use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use rhythm_matter::chip_rpc::{
    ChipInitControllerRequest, ChipInitControllerResponse, ChipRpcAttributeReportsResponse,
    ChipRpcCommissionLightResponse, ChipRpcControllerEventsResponse, ChipRpcEmpty,
    ChipRpcJsonValueResponse, ChipRpcListDevicesResponse, ChipRpcOperationalDiscoveryResponse,
    ChipRpcProbeLightResponse, ChipRpcReadOnOffResponse, ChipRpcRequest,
    ChipRpcSubmitEndpointPlansResponse,
};
use rhythm_matter::transport::{CommissionedDevice, MatterControllerEvent};
use rhythm_matter::transport::{MatterDeviceInfo, MatterSubscriptionTarget};

use crate::backend::ChipControllerBackend;
use crate::command_dispatch::{
    CommandDispatcher, ControllerEventBroker, ControllerWorkBudget, MAX_CONCURRENT_CONTROLLER_WORK,
};

const DEVICE_STORE_SCHEMA_VERSION: u32 = 1;
const MATTER_OPERATIONAL_SERVICE_TYPE: &str = "_matter._tcp.local.";
#[cfg(not(test))]
const LEGACY_DEVICE_STORE_MDNS_SCAN_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(test)]
const LEGACY_DEVICE_STORE_MDNS_SCAN_TIMEOUT: Duration = Duration::from_secs(0);

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

/// Thread-safe RPC service.
///
/// Connection threads call [`ChipControllerService::handle`] concurrently:
/// device/group control and reads run in parallel (the CHIP bridge blocks
/// each caller on per-operation state while the Matter thread multiplexes),
/// so one lagging bulb no longer stalls every other Matter command. Only
/// controller (re)initialization is exclusive, and lifecycle operations that
/// mutate shared fabric/group/device tables serialize among themselves.
pub struct ChipControllerService {
    backend: Arc<RwLock<Box<dyn ChipControllerBackend>>>,
    command_dispatcher: Arc<CommandDispatcher>,
    controller_work_budget: Arc<ControllerWorkBudget>,
    event_broker: Arc<ControllerEventBroker>,
    device_store: Mutex<DeviceStore>,
    state: RwLock<Option<CommissioningState>>,
    /// Serializes commissioning/decommissioning/probing/group config —
    /// operations that touch shared fabric tables and the device store.
    lifecycle_lock: Mutex<()>,
}

impl ChipControllerService {
    pub fn new(backend: Box<dyn ChipControllerBackend>) -> Self {
        let backend = Arc::new(RwLock::new(backend));
        let event_broker = Arc::new(ControllerEventBroker::new());
        let controller_work_budget =
            Arc::new(ControllerWorkBudget::new(MAX_CONCURRENT_CONTROLLER_WORK));
        let command_dispatcher = CommandDispatcher::with_work_budget(
            backend.clone(),
            event_broker.clone(),
            controller_work_budget.clone(),
        );
        Self {
            backend,
            command_dispatcher,
            controller_work_budget,
            event_broker,
            device_store: Mutex::new(DeviceStore::default()),
            state: RwLock::new(None),
            lifecycle_lock: Mutex::new(()),
        }
    }

    fn backend(&self) -> RwLockReadGuard<'_, Box<dyn ChipControllerBackend>> {
        self.backend.read().expect("chipd backend lock poisoned")
    }

    fn device_store(&self) -> std::sync::MutexGuard<'_, DeviceStore> {
        self.device_store
            .lock()
            .expect("chipd device store lock poisoned")
    }

    pub fn handle(&self, request: ChipRpcRequest) -> Result<serde_json::Value> {
        match request {
            ChipRpcRequest::InitController(request) => {
                let result = self.init_controller(request)?;
                Ok(serde_json::to_value(result)?)
            }
            ChipRpcRequest::CommissionLight(request) => {
                self.require_initialized()?;
                let _lifecycle = self.lifecycle_lock.lock();
                let device = self.backend().commission_light(&request)?;
                self.device_store().upsert(device.clone())?;
                // Keep newly commissioned endpoints inside the same
                // authoritative observed-state stream as restored devices.
                let _permit = self.controller_work_budget.acquire();
                if let Err(error) = self.backend().subscribe_on_off(
                    &[MatterSubscriptionTarget {
                        node_id: device.node_id,
                        endpoint: device.light_endpoint,
                    }],
                    rhythm_matter::transport::DEFAULT_SUBSCRIPTION_MIN_INTERVAL_SECS,
                    rhythm_matter::transport::DEFAULT_SUBSCRIPTION_MAX_INTERVAL_SECS,
                ) {
                    eprintln!(
                        "rhythm-chipd could not establish the initial subscription for node {} endpoint {}: {error:#}",
                        device.node_id, device.light_endpoint
                    );
                }
                Ok(serde_json::to_value(ChipRpcCommissionLightResponse {
                    device,
                })?)
            }
            ChipRpcRequest::ListDevices => {
                let store = self.device_store();
                Ok(serde_json::to_value(ChipRpcListDevicesResponse {
                    devices: store.list_devices(),
                    commissioned_devices: store.devices(),
                })?)
            }
            ChipRpcRequest::ProbeLight { node_id } => {
                self.require_initialized()?;
                let _lifecycle = self.lifecycle_lock.lock();
                let device = self.backend().probe_light(node_id)?;
                self.device_store().upsert(device.clone())?;
                Ok(serde_json::to_value(ChipRpcProbeLightResponse { device })?)
            }
            ChipRpcRequest::ScanOperationalNode {
                node_id,
                timeout_ms,
            } => {
                self.require_initialized()?;
                let timeout = Duration::from_millis(timeout_ms.min(60_000));
                let observed = scan_matter_operational_fabrics(HashSet::from([node_id]), timeout)?;
                let mut fabrics: Vec<String> = observed
                    .get(&node_id)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                fabrics.sort();
                Ok(serde_json::to_value(ChipRpcOperationalDiscoveryResponse {
                    node_id,
                    fabrics,
                })?)
            }
            ChipRpcRequest::DecommissionDevice { node_id, force } => {
                self.require_initialized()?;
                let _lifecycle = self.lifecycle_lock.lock();
                self.backend().decommission_device(node_id, force)?;
                self.device_store().remove(node_id)?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::SetOnOff {
                node_id,
                endpoint,
                on,
            } => {
                self.require_initialized()?;
                self.backend().set_on_off(node_id, endpoint, on)?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::ConfigureGroup { group } => {
                self.require_initialized()?;
                let _lifecycle = self.lifecycle_lock.lock();
                self.backend().configure_group(&group)?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::RemoveGroup { group_id, members } => {
                self.require_initialized()?;
                let _lifecycle = self.lifecycle_lock.lock();
                self.backend().remove_group(group_id, &members)?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::SetGroupOnOff { group_id, on } => {
                self.require_initialized()?;
                self.backend().set_group_on_off(group_id, on)?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::IdentifyGroup {
                group_id,
                duration_secs,
            } => {
                self.require_initialized()?;
                self.backend().identify_group(group_id, duration_secs)?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::SetGroupBrightness {
                group_id,
                level,
                transition_ms,
            } => {
                self.require_initialized()?;
                self.backend()
                    .set_group_brightness(group_id, level, transition_ms)?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::SetGroupColorTemperature {
                group_id,
                kelvin,
                transition_ms,
            } => {
                self.require_initialized()?;
                self.backend()
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
                self.backend().set_group_xy(group_id, x, y, transition_ms)?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::SetGroupHueSaturation {
                group_id,
                hue,
                saturation,
                transition_ms,
            } => {
                self.require_initialized()?;
                self.backend().set_group_hue_saturation(
                    group_id,
                    hue,
                    saturation,
                    transition_ms,
                )?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::IdentifyLight {
                node_id,
                endpoint,
                duration_secs,
            } => {
                self.require_initialized()?;
                self.backend()
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
                self.backend()
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
                self.backend().run_level_command(
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
                self.backend()
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
                self.backend()
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
                self.backend().set_hue_saturation(
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
                let on = self.backend().read_on_off(node_id, endpoint)?;
                Ok(serde_json::to_value(ChipRpcReadOnOffResponse { on })?)
            }
            ChipRpcRequest::ReadLightCapabilitySnapshot { node_id, endpoint } => {
                self.require_initialized()?;
                let value = self
                    .backend()
                    .read_light_capability_snapshot(node_id, endpoint)?;
                Ok(serde_json::to_value(ChipRpcJsonValueResponse { value })?)
            }
            ChipRpcRequest::ReadLightState { node_id, endpoint } => {
                self.require_initialized()?;
                let value = self.backend().read_light_state(node_id, endpoint)?;
                Ok(serde_json::to_value(ChipRpcJsonValueResponse { value })?)
            }
            ChipRpcRequest::SubscribeOnOff {
                targets,
                min_interval_secs,
                max_interval_secs,
            } => {
                self.require_initialized()?;
                // No lifecycle lock: subscribing is ordinary controller work,
                // and the native subscription table is owned by the Matter
                // thread, so it cannot race a concurrent decommission. Holding
                // the lifecycle lock here made one slow subscribe stall every
                // commission/decommission and turned into a false-failure
                // cascade. The shared work budget is the only bound.
                let _permit = self.controller_work_budget.acquire();
                self.backend()
                    .subscribe_on_off(&targets, min_interval_secs, max_interval_secs)?;
                Ok(serde_json::to_value(ChipRpcEmpty::new())?)
            }
            ChipRpcRequest::DrainAttributeReports => {
                self.require_initialized()?;
                let reports = self.backend().drain_attribute_reports()?;
                Ok(serde_json::to_value(ChipRpcAttributeReportsResponse {
                    reports,
                })?)
            }
            ChipRpcRequest::SubmitEndpointPlans { plans } => {
                self.require_initialized()?;
                let submissions = self.command_dispatcher.submit(plans)?;
                Ok(serde_json::to_value(ChipRpcSubmitEndpointPlansResponse {
                    submissions,
                })?)
            }
            ChipRpcRequest::WaitControllerEvents {
                cursor,
                max_wait_ms,
            } => {
                self.require_initialized()?;
                let max_wait = Duration::from_millis(max_wait_ms.min(30_000));
                let deadline = Instant::now() + max_wait;
                loop {
                    for report in self.backend().drain_attribute_reports()? {
                        self.event_broker
                            .publish(MatterControllerEvent::AttributeReport(report));
                    }
                    // Terminal subscription failures are reported once by the
                    // native bridge; rhythm-matter owns the retry timing.
                    for termination in self.backend().drain_subscription_terminations()? {
                        self.event_broker
                            .publish(MatterControllerEvent::SubscriptionTerminated(termination));
                    }

                    let remaining = deadline.saturating_duration_since(Instant::now());
                    let batch = self
                        .event_broker
                        .wait(cursor.as_ref(), remaining.min(Duration::from_millis(500)));
                    if !batch.events.is_empty() || remaining.is_zero() {
                        break Ok(serde_json::to_value(ChipRpcControllerEventsResponse {
                            batch,
                        })?);
                    }
                }
            }
        }
    }

    fn init_controller(
        &self,
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

        let _lifecycle = self.lifecycle_lock.lock();
        let response = {
            let mut device_store = self.device_store();
            device_store.configure(state.devices_path())?;
            let existing_devices = device_store.devices();
            // Initialization is the one exclusive backend operation.
            drop(device_store);
            let mut backend = self.backend.write().expect("chipd backend lock poisoned");
            backend.init_controller(&state, ble_controller, &existing_devices)?
        };
        self.device_store()
            .ensure_controller_fabric(response.compressed_fabric_id.as_deref())?;
        *self.state.write().expect("chipd state lock poisoned") = Some(state);
        Ok(response)
    }

    fn require_initialized(&self) -> Result<()> {
        if self
            .state
            .read()
            .expect("chipd state lock poisoned")
            .is_some()
        {
            Ok(())
        } else {
            Err(anyhow::anyhow!("Controller not initialized"))
        }
    }
}

#[derive(Default)]
struct DeviceStore {
    path: Option<PathBuf>,
    compressed_fabric_id: Option<String>,
    devices: BTreeMap<u64, CommissionedDevice>,
}

#[derive(Serialize, Deserialize)]
struct DeviceStoreFile {
    schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    compressed_fabric_id: Option<String>,
    devices: Vec<CommissionedDevice>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum DeviceStoreOnDisk {
    V1(DeviceStoreFile),
    Legacy(Vec<CommissionedDevice>),
}

impl DeviceStore {
    fn configure(&mut self, path: PathBuf) -> Result<()> {
        self.path = Some(path.clone());
        let (compressed_fabric_id, loaded) = match fs::read_to_string(&path) {
            Ok(json) => match serde_json::from_str::<DeviceStoreOnDisk>(&json)
                .with_context(|| format!("decoding {}", path.display()))?
            {
                DeviceStoreOnDisk::V1(file) => {
                    if file.schema_version != DEVICE_STORE_SCHEMA_VERSION {
                        anyhow::bail!(
                            "Unsupported Matter device store schema version {} in {}",
                            file.schema_version,
                            path.display()
                        );
                    }
                    (
                        file.compressed_fabric_id
                            .as_deref()
                            .map(normalize_compressed_fabric_id)
                            .transpose()?,
                        file.devices,
                    )
                }
                DeviceStoreOnDisk::Legacy(devices) => (None, devices),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (None, Vec::new()),
            Err(error) => {
                return Err(error).with_context(|| format!("reading {}", path.display()));
            }
        };
        self.compressed_fabric_id = compressed_fabric_id;
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

    fn ensure_controller_fabric(
        &mut self,
        controller_compressed_fabric_id: Option<&str>,
    ) -> Result<()> {
        let Some(controller_id) = controller_compressed_fabric_id else {
            return Ok(());
        };
        let controller_id = normalize_compressed_fabric_id(controller_id)?;

        if let Some(stored_id) = &self.compressed_fabric_id {
            if stored_id != &controller_id {
                anyhow::bail!(
                    "Matter device store belongs to compressed fabric {}, but current controller initialized fabric {}; clear Matter devices and recommission them onto the current fabric",
                    stored_id,
                    controller_id
                );
            }
            return Ok(());
        }

        if self.ensure_legacy_devices_do_not_advertise_other_fabric(&controller_id)? {
            self.compressed_fabric_id = Some(controller_id);
            self.flush()
        } else {
            Ok(())
        }
    }

    fn ensure_legacy_devices_do_not_advertise_other_fabric(
        &self,
        controller_id: &str,
    ) -> Result<bool> {
        if self.devices.is_empty() {
            return Ok(true);
        }

        match scan_matter_operational_fabrics(
            self.devices.keys().copied().collect(),
            LEGACY_DEVICE_STORE_MDNS_SCAN_TIMEOUT,
        ) {
            Ok(observed) => {
                let mut saw_current_fabric = false;
                for (node_id, fabrics) in observed {
                    if fabrics.contains(controller_id) {
                        saw_current_fabric = true;
                        continue;
                    }
                    if let Some(other_id) = fabrics.iter().next() {
                        anyhow::bail!(
                            "Matter cached node {} is advertising on compressed fabric {}, but current controller initialized fabric {}; clear Matter devices and recommission them onto the current fabric",
                            node_id,
                            other_id,
                            controller_id
                        );
                    }
                }
                Ok(saw_current_fabric)
            }
            Err(error) => {
                eprintln!(
                    "rhythm-chipd warning: skipped legacy Matter fabric mDNS check: {error:#}"
                );
                Ok(false)
            }
        }
    }

    fn flush(&self) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let file = DeviceStoreFile {
            schema_version: DEVICE_STORE_SCHEMA_VERSION,
            compressed_fabric_id: self.compressed_fabric_id.clone(),
            devices: self.devices(),
        };
        let json = serde_json::to_string_pretty(&file)?;
        fs::write(path, json).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }
}

fn normalize_compressed_fabric_id(value: &str) -> Result<String> {
    let trimmed = value
        .trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X");
    if trimmed.len() > 16 || trimmed.is_empty() || !trimmed.chars().all(|ch| ch.is_ascii_hexdigit())
    {
        anyhow::bail!("Invalid Matter compressed fabric id '{}'", value);
    }
    Ok(format!("{:0>16}", trimmed).to_ascii_uppercase())
}

fn scan_matter_operational_fabrics(
    node_ids: HashSet<u64>,
    timeout: Duration,
) -> Result<HashMap<u64, HashSet<String>>> {
    if node_ids.is_empty() {
        return Ok(HashMap::new());
    }
    if timeout.is_zero() {
        return Ok(HashMap::new());
    }

    let daemon = mdns_sd::ServiceDaemon::new().context("creating Matter mDNS daemon")?;
    let receiver = daemon
        .browse(MATTER_OPERATIONAL_SERVICE_TYPE)
        .context("browsing Matter operational mDNS service")?;
    let deadline = Instant::now() + timeout;
    let mut observed: HashMap<u64, HashSet<String>> = HashMap::new();

    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let poll = remaining.min(Duration::from_millis(100));
        match receiver.recv_timeout(poll) {
            Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) => {
                if let Some((fabric_id, node_id)) =
                    parse_matter_operational_instance(info.get_fullname())?
                {
                    if node_ids.contains(&node_id) {
                        observed.entry(node_id).or_default().insert(fabric_id);
                    }
                }
            }
            Ok(_) => {}
            Err(flume::RecvTimeoutError::Timeout) => {}
            Err(flume::RecvTimeoutError::Disconnected) => break,
        }
    }

    let _ = daemon.stop_browse(MATTER_OPERATIONAL_SERVICE_TYPE);
    Ok(observed)
}

fn parse_matter_operational_instance(fullname: &str) -> Result<Option<(String, u64)>> {
    let Some(instance) = fullname.split("._matter._tcp").next() else {
        return Ok(None);
    };
    let Some((fabric, node)) = instance.rsplit_once('-') else {
        return Ok(None);
    };
    let fabric = normalize_compressed_fabric_id(fabric)?;
    let node = normalize_compressed_fabric_id(node)?;
    let node_id = u64::from_str_radix(&node, 16)
        .with_context(|| format!("parsing Matter operational node id from {}", fullname))?;
    Ok(Some((fabric, node_id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    use rhythm_matter::chip_rpc::{
        ChipRpcAttributeReportsResponse, ChipRpcCommissionLightResponse,
        ChipRpcControllerEventsResponse, ChipRpcEmpty, ChipRpcJsonValueResponse,
        ChipRpcListDevicesResponse, ChipRpcOperationalDiscoveryResponse, ChipRpcProbeLightResponse,
        ChipRpcReadOnOffResponse,
    };
    use rhythm_matter::transport::{
        MatterColorMode, MatterCommissionRequest, MatterCommissioningNetwork,
        MatterCommissioningRendezvous, MatterCommissioningWifiCredentials,
        MatterControllerEventCursor, MatterGroup, MatterGroupMember, MatterLevelCommandVariant,
        MatterLevelStepMode, MatterSubscriptionFailureClass, MatterSubscriptionTarget,
        MatterSubscriptionTermination,
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

        let service = ChipControllerService::new(Box::new(FakeChipBackend::default()));
        let initial_list: ChipRpcListDevicesResponse =
            serde_json::from_value(service.handle(ChipRpcRequest::ListDevices).unwrap()).unwrap();
        assert!(initial_list.devices.is_empty());
        assert!(initial_list.commissioned_devices.is_empty());
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
        assert_eq!(list.commissioned_devices.len(), 1);
        assert_eq!(list.commissioned_devices[0].light_endpoint, 2);

        let probed: ChipRpcProbeLightResponse = serde_json::from_value(
            service
                .handle(ChipRpcRequest::ProbeLight { node_id: 10 })
                .unwrap(),
        )
        .unwrap();
        assert_eq!(probed.device.light_endpoint, 2);

        let discovery: ChipRpcOperationalDiscoveryResponse = serde_json::from_value(
            service
                .handle(ChipRpcRequest::ScanOperationalNode {
                    node_id: 10,
                    timeout_ms: 0,
                })
                .unwrap(),
        )
        .unwrap();
        assert_eq!(discovery.node_id, 10);
        assert!(discovery.fabrics.is_empty());

        let commissioned: ChipRpcCommissionLightResponse = serde_json::from_value(
            service
                .handle(ChipRpcRequest::CommissionLight(commission_request(42)))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(commissioned.device.node_id, 42);
        let persisted: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&devices_path).unwrap()).unwrap();
        assert!(persisted["devices"]
            .as_array()
            .unwrap()
            .iter()
            .any(|device| device["node_id"] == 42));

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
        let persisted: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&devices_path).unwrap()).unwrap();
        assert!(!persisted["devices"]
            .as_array()
            .unwrap()
            .iter()
            .any(|device| device["node_id"] == 42));

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
        let persisted: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&devices_path).unwrap()).unwrap();
        assert_eq!(persisted["devices"].as_array().unwrap().len(), 0);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn device_store_rejects_mismatched_compressed_fabric() {
        let dir = unique_test_dir("store-fabric-mismatch");
        let devices_path = dir.join("devices.json");
        fs::write(
            &devices_path,
            serde_json::to_string(&DeviceStoreFile {
                schema_version: DEVICE_STORE_SCHEMA_VERSION,
                compressed_fabric_id: Some("D6B252ACB7133A7E".to_string()),
                devices: vec![commissioned_device(100, 1)],
            })
            .unwrap(),
        )
        .unwrap();

        let mut store = DeviceStore::default();
        store.configure(devices_path).unwrap();
        let error = string_error(store.ensure_controller_fabric(Some("399026E03C18B2D2")));
        assert!(error.contains("device store belongs to compressed fabric D6B252ACB7133A7E"));
        assert!(error.contains("current controller initialized fabric 399026E03C18B2D2"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn wait_controller_events_publishes_native_subscription_terminations() {
        let dir = unique_test_dir("service-subscription-termination");
        let storage_path = dir.join("chip.json");

        let backend = FakeChipBackend::default();
        let terminations = backend.termination_queue();
        let service = ChipControllerService::new(Box::new(backend));
        let _: ChipInitControllerResponse = serde_json::from_value(
            service
                .handle(ChipRpcRequest::InitController(init_request(&storage_path)))
                .unwrap(),
        )
        .unwrap();

        terminations
            .lock()
            .unwrap()
            .push(MatterSubscriptionTermination {
                node_id: 42,
                endpoint: 1,
                failure_class: MatterSubscriptionFailureClass::PeerClosed,
                chip_error: 0x0000_002e,
                detail: None,
            });

        let events: ChipRpcControllerEventsResponse = serde_json::from_value(
            service
                .handle(ChipRpcRequest::WaitControllerEvents {
                    cursor: None,
                    max_wait_ms: 500,
                })
                .unwrap(),
        )
        .unwrap();

        let last_sequence = events
            .batch
            .events
            .iter()
            .map(|envelope| envelope.sequence)
            .max()
            .unwrap_or(0);
        let published: Vec<_> = events
            .batch
            .events
            .into_iter()
            .filter_map(|envelope| match envelope.event {
                MatterControllerEvent::SubscriptionTerminated(termination) => Some(termination),
                _ => None,
            })
            .collect();
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].node_id, 42);
        assert_eq!(published[0].endpoint, 1);
        assert_eq!(
            published[0].failure_class,
            MatterSubscriptionFailureClass::PeerClosed
        );

        // Drained exactly once: a second wait must not replay it.
        let repeat: ChipRpcControllerEventsResponse = serde_json::from_value(
            service
                .handle(ChipRpcRequest::WaitControllerEvents {
                    cursor: Some(MatterControllerEventCursor {
                        stream_id: events.batch.stream_id.clone(),
                        sequence: last_sequence,
                    }),
                    max_wait_ms: 0,
                })
                .unwrap(),
        )
        .unwrap();
        assert!(repeat.batch.events.is_empty());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn parses_matter_operational_instance_names() {
        let parsed = parse_matter_operational_instance(
            "D6B252ACB7133A7E-0000000000000067._matter._tcp.local.",
        )
        .unwrap()
        .unwrap();
        assert_eq!(parsed, ("D6B252ACB7133A7E".to_string(), 103));
    }
}
