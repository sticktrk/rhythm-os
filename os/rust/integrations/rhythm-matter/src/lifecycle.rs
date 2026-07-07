//! Matter hub lifecycle — connect, disconnect, runtime creation.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use log::{info, warn};
use rhythm_devices::{DeviceQuirk, LightCapabilities};
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::{ActiveHub, HubEvent, HubType};
use rhythm_os::registry::HubDeviceRegistry;
use rhythm_os::state::SharedState;

use crate::hub_state::MatterHubData;
use crate::transport::{
    MatterDeviceInfo, MatterSubscriptionTarget, MatterTransport,
    DEFAULT_SUBSCRIPTION_MAX_INTERVAL_SECS, DEFAULT_SUBSCRIPTION_MIN_INTERVAL_SECS,
};

/// How long a single device probe may run during hub bootstrap before we give
/// up and fall back to basic device info. Reachable devices respond in well
/// under a second; an unreachable one would otherwise block the connect path on
/// a ~35s CHIP timeout, delaying the whole Matter integration from loading.
/// A later successful rediscovery fills in real capabilities once the device
/// becomes reachable. See #169.
const CONNECT_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Connect to the local Matter fabric.
pub fn connect_matter(
    state: &SharedState,
    transport: Arc<dyn MatterTransport>,
) -> Result<(ActiveHub, Receiver<HubEvent>)> {
    let hub_key = HubKey::new(HubType::new("matter"), "local");
    let commissioned = transport.list_devices().unwrap_or_default();
    let cloud_profiles = crate::cloud_profiles::load_or_sync_for_state(state);
    let initial_metadata = fallback_initial_device_metadata(state, &commissioned, &hub_key);
    let next_node_id = next_node_id_seed(&commissioned);
    let fabric_id = configured_fabric_id(state, &hub_key);

    info!(target: "sys", "Matter: {} commissioned devices", commissioned.len());

    let snapshot = {
        let state = state
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock state"))?;
        state
            .storage
            .as_ref()
            .and_then(|storage| storage.load_hub_registry_for(&hub_key).ok().flatten())
            .and_then(|value| {
                serde_json::from_value::<rhythm_os::registry::RegistrySnapshot>(value).ok()
            })
    };

    let commissioned_for_closure = commissioned.clone();
    let transport_for_closure = transport.clone();
    let fabric_id_for_closure = fabric_id.clone();
    let cloud_profiles_for_hub_data = cloud_profiles.clone();
    let hub_key_for_refresh = hub_key.clone();
    let node_proof_of_life = Arc::new(Mutex::new(HashMap::new()));
    let node_proof_of_life_for_closure = node_proof_of_life.clone();

    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let _ = event_tx.send(HubEvent::Connected {
        hub_key: Some(hub_key.clone()),
    });

    let (hub, event_rx) = rhythm_os::lifecycle::connect_hub(
        state,
        HubType::new("matter"),
        hub_key,
        true,
        snapshot,
        move |registry: Arc<Mutex<HubDeviceRegistry>>| -> Box<dyn std::any::Any + Send + Sync> {
            let transport_cell = {
                let cell = std::sync::OnceLock::new();
                let _ = cell.set(transport_for_closure.clone());
                cell
            };

            Box::new(Arc::new(MatterHubData {
                transport: transport_cell,
                capture_dir: std::sync::OnceLock::new(),
                registry,
                fabric_id: fabric_id_for_closure.clone(),
                commissioned: std::sync::Mutex::new(commissioned_for_closure.clone()),
                next_node_id: std::sync::atomic::AtomicU64::new(next_node_id),
                device_caps: std::sync::Mutex::new(initial_metadata.device_caps.clone()),
                device_quirks: std::sync::Mutex::new(initial_metadata.device_quirks.clone()),
                cloud_profiles: std::sync::Mutex::new(cloud_profiles_for_hub_data.clone()),
                decommissioning: std::sync::Mutex::new(std::collections::HashSet::new()),
                recently_decommissioned: std::sync::Mutex::new(std::collections::HashMap::new()),
                node_proof_of_life: node_proof_of_life_for_closure.clone(),
                event_tx,
            }))
        },
        move |_registry, _shutdown| event_rx,
    )?;

    if let Some(hub_data) = hub.data::<Arc<MatterHubData>>().cloned() {
        start_initial_metadata_refresh(
            state.clone(),
            transport,
            commissioned,
            cloud_profiles,
            hub_key_for_refresh,
            hub_data,
            CONNECT_PROBE_TIMEOUT,
        );
    }

    Ok((hub, event_rx))
}

fn configured_fabric_id(state: &SharedState, hub_key: &HubKey) -> String {
    state
        .lock()
        .ok()
        .and_then(|state| state.hub_credentials.get(hub_key).cloned())
        .and_then(|creds| crate::provider::matter_fabric_id(&creds).map(ToOwned::to_owned))
        .unwrap_or_else(|| "default".to_string())
}

fn next_node_id_seed(commissioned: &[MatterDeviceInfo]) -> u64 {
    commissioned
        .iter()
        .map(|device| device.node_id)
        .max()
        .unwrap_or(99)
        .saturating_add(1)
}

#[derive(Clone)]
struct InitialDeviceMetadata {
    device_caps: HashMap<String, LightCapabilities>,
    device_quirks: HashMap<String, Vec<DeviceQuirk>>,
    subscription_targets: Vec<MatterSubscriptionTarget>,
    unreachable_nodes: HashSet<u64>,
}

fn fallback_initial_device_metadata(
    state: &SharedState,
    commissioned: &[MatterDeviceInfo],
    hub_key: &HubKey,
) -> InitialDeviceMetadata {
    let mut device_caps = HashMap::new();
    let mut device_quirks = HashMap::new();
    let commissioned_nodes = commissioned
        .iter()
        .map(|info| info.node_id)
        .collect::<HashSet<_>>();

    let mut insert_fallback = |device_id: String| {
        device_caps
            .entry(device_id.clone())
            .or_insert_with(crate::commissioning::fallback_device_capabilities);
        device_quirks.entry(device_id).or_default();
    };

    for info in commissioned {
        insert_fallback(format_device_id(info.node_id, 1));
    }

    if let Ok(state) = state.lock() {
        for device in state.canonical_registry.devices() {
            for endpoint in device.active_endpoints() {
                if &endpoint.hub_key != hub_key {
                    continue;
                }
                let Some((node_id, _endpoint_id)) = parse_device_id(&endpoint.native_id) else {
                    continue;
                };
                if commissioned_nodes.contains(&node_id) {
                    insert_fallback(endpoint.native_id.clone());
                }
            }
        }
    }

    InitialDeviceMetadata {
        device_caps,
        device_quirks,
        subscription_targets: Vec::new(),
        unreachable_nodes: HashSet::new(),
    }
}

/// Probe a node but give up after `timeout`, so a single unreachable device
/// cannot stall the whole hub bootstrap on a serial ~35s CHIP timeout. The
/// probe runs on a detached thread; on timeout we return an error and let the
/// caller fall back to basic device info until rediscovery can recover real
/// capabilities later. See #169.
pub(crate) fn probe_light_with_deadline(
    transport: &Arc<dyn MatterTransport>,
    node_id: u64,
    timeout: Duration,
) -> Result<crate::transport::CommissionedDevice> {
    let (tx, rx) = std::sync::mpsc::channel();
    let probe_transport = transport.clone();
    std::thread::Builder::new()
        .name(format!("matter-probe-{node_id}"))
        .spawn(move || {
            let _ = tx.send(probe_transport.probe_light(node_id));
        })?;

    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => Err(anyhow::anyhow!(
            "probe timed out after {}ms (node {} unreachable)",
            timeout.as_millis(),
            node_id
        )),
    }
}

fn load_initial_device_metadata(
    state: &SharedState,
    transport: &Arc<dyn MatterTransport>,
    commissioned: &[MatterDeviceInfo],
    cloud_profiles: &crate::cloud_profiles::CloudMatterProfileCatalog,
    probe_timeout: Duration,
) -> InitialDeviceMetadata {
    let mut device_caps = HashMap::new();
    let mut device_quirks = HashMap::new();
    let mut subscription_targets = Vec::new();
    let mut unreachable_nodes = HashSet::new();
    let local_overrides = crate::local_quirks::load_overrides_for_state(state);

    for info in commissioned {
        match probe_light_with_deadline(transport, info.node_id, probe_timeout) {
            Ok(device) => {
                let device_id = format_device_id(device.node_id, device.light_endpoint);
                subscription_targets.push(MatterSubscriptionTarget {
                    node_id: device.node_id,
                    endpoint: device.light_endpoint,
                });
                let mut caps = crate::commissioning::build_device_capabilities(&device);
                let mut quirks = crate::commissioning::build_device_quirks(&device);
                cloud_profiles.apply_to_device(&device, &mut caps, &mut quirks);
                if let Some(override_caps) = local_overrides.capabilities.get(&device_id) {
                    crate::local_quirks::apply_capability_override(&mut caps, override_caps);
                }
                device_caps.insert(device_id.clone(), caps);
                if let Some(local_quirks) = local_overrides.quirks.get(&device_id) {
                    quirks = local_quirks.clone();
                }
                device_quirks.insert(device_id, quirks);
            }
            Err(error) => {
                warn!(
                    target: "sys",
                    "Matter: failed to probe node {} during connect, using fallback metadata and skipping live subscriptions until rediscovery: {}",
                    info.node_id,
                    error
                );
                let device_id = format_device_id(info.node_id, 1);
                device_caps
                    .entry(device_id.clone())
                    .or_insert_with(crate::commissioning::fallback_device_capabilities);
                device_quirks.entry(device_id).or_default();
                unreachable_nodes.insert(info.node_id);
            }
        }
    }

    InitialDeviceMetadata {
        device_caps,
        device_quirks,
        subscription_targets,
        unreachable_nodes,
    }
}

fn commissioned_with_reachability(
    commissioned: Vec<MatterDeviceInfo>,
    metadata: &InitialDeviceMetadata,
) -> Vec<MatterDeviceInfo> {
    commissioned
        .into_iter()
        .map(|mut device| {
            if metadata.unreachable_nodes.contains(&device.node_id) {
                device.reachable = false;
            }
            device
        })
        .collect()
}

fn start_initial_metadata_refresh(
    state: SharedState,
    transport: Arc<dyn MatterTransport>,
    commissioned: Vec<MatterDeviceInfo>,
    cloud_profiles: crate::cloud_profiles::CloudMatterProfileCatalog,
    hub_key: HubKey,
    hub_data: Arc<MatterHubData>,
    probe_timeout: Duration,
) {
    if commissioned.is_empty() {
        return;
    }

    let spawn_result = std::thread::Builder::new()
        .name("matter-initial-metadata".to_string())
        .spawn(move || {
            let device_count = commissioned.len();
            let metadata = load_initial_device_metadata(
                &state,
                &transport,
                &commissioned,
                &cloud_profiles,
                probe_timeout,
            );
            let subscription_targets = metadata.subscription_targets.clone();
            let subscription_count = subscription_targets.len();
            let refreshed = commissioned_with_reachability(commissioned, &metadata);

            if let Ok(mut list) = hub_data.commissioned.lock() {
                for device in refreshed {
                    if let Some(existing) = list
                        .iter_mut()
                        .find(|existing| existing.node_id == device.node_id)
                    {
                        *existing = device;
                    } else {
                        list.push(device);
                    }
                }
            }

            if let Ok(mut device_caps) = hub_data.device_caps.lock() {
                device_caps.extend(metadata.device_caps);
            }
            if let Ok(mut device_quirks) = hub_data.device_quirks.lock() {
                device_quirks.extend(metadata.device_quirks);
            }

            info!(
                target: "sys",
                "Matter: initial metadata refresh complete for {} device(s), live subscriptions={}",
                device_count,
                subscription_count
            );
            start_attribute_report_loop(
                hub_key,
                transport,
                subscription_targets,
                hub_data.node_proof_of_life.clone(),
                hub_data.event_tx.clone(),
            );
        });

    if let Err(error) = spawn_result {
        warn!(
            target: "sys",
            "Matter: failed to spawn initial metadata refresh: {}",
            error
        );
    }
}

fn start_attribute_report_loop(
    hub_key: HubKey,
    transport: Arc<dyn MatterTransport>,
    targets: Vec<MatterSubscriptionTarget>,
    node_proof_of_life: Arc<Mutex<HashMap<u64, Instant>>>,
    event_tx: Sender<HubEvent>,
) {
    if targets.is_empty() {
        return;
    }

    let _ = std::thread::Builder::new()
        .name("matter-attr-sub".to_string())
        .spawn(move || {
            if let Err(error) = transport.subscribe_on_off(
                &targets,
                DEFAULT_SUBSCRIPTION_MIN_INTERVAL_SECS,
                DEFAULT_SUBSCRIPTION_MAX_INTERVAL_SECS,
            ) {
                warn!(
                    target: "evt",
                    "Matter: failed to start On/Off attribute subscriptions: {}",
                    error
                );
                return;
            }

            info!(
                target: "evt",
                "Matter: subscribed to On/Off reports for {} endpoint(s)",
                targets.len()
            );

            loop {
                match transport.drain_attribute_reports() {
                    Ok(reports) => {
                        for report in reports {
                            if let Ok(mut proof) = node_proof_of_life.lock() {
                                proof.insert(report.node_id, Instant::now());
                            }
                            if let Some(event) = crate::events::translate_report(&report) {
                                if event_tx.send(event.with_hub_key(hub_key.clone())).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                    Err(error) => {
                        warn!(
                            target: "evt",
                            "Matter: failed to drain attribute reports: {}",
                            error
                        );
                        std::thread::sleep(Duration::from_secs(2));
                        continue;
                    }
                }

                std::thread::sleep(Duration::from_millis(250));
            }
        });
}

/// Format a Matter node ID as a device ID string.
pub fn format_device_id(node_id: u64, endpoint: u16) -> String {
    if endpoint == 1 {
        format!("matter-{}", node_id)
    } else {
        format!("matter-{}-{}", node_id, endpoint)
    }
}

/// Parse a device ID string back into `(node_id, endpoint)`.
pub fn parse_device_id(device_id: &str) -> Option<(u64, u16)> {
    let rest = device_id.strip_prefix("matter-")?;
    match rest.split_once('-') {
        Some((node_str, endpoint_str)) => {
            let node_id = node_str.parse::<u64>().ok()?;
            let endpoint = endpoint_str.parse::<u16>().ok()?;
            Some((node_id, endpoint))
        }
        None => {
            let node_id = rest.parse::<u64>().ok()?;
            Some((node_id, 1))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    use rhythm_core::runtime::hub_registry::DeviceType;
    use rhythm_os::canonical::identity::{DiscoveredIdentity, HardwareId};
    use rhythm_os::state::AppState;

    use crate::provider::matter_credentials;
    use crate::transport::{
        CommissionedDevice, MatterAttributeReport, MatterColorMode, MatterCommissionRequest,
        MatterGroup, MatterGroupMember,
    };

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct FakeMatterTransport {
        devices: Vec<MatterDeviceInfo>,
        probe_failures: HashMap<u64, String>,
        probe_delays: HashMap<u64, Duration>,
        subscribe_calls: AtomicUsize,
    }

    impl FakeMatterTransport {
        fn new(devices: Vec<MatterDeviceInfo>, probe_failures: HashMap<u64, String>) -> Self {
            Self {
                devices,
                probe_failures,
                probe_delays: HashMap::new(),
                subscribe_calls: AtomicUsize::new(0),
            }
        }

        /// Make `probe_light` for `node_id` block for `delay`, emulating an
        /// unreachable device that only fails after the CHIP timeout.
        fn with_probe_delay(mut self, node_id: u64, delay: Duration) -> Self {
            self.probe_delays.insert(node_id, delay);
            self
        }
    }

    impl MatterTransport for FakeMatterTransport {
        fn commission_light(
            &self,
            _request: &MatterCommissionRequest,
        ) -> Result<CommissionedDevice> {
            anyhow::bail!("not used")
        }

        fn decommission_device(&self, _node_id: u64, _force: bool) -> Result<()> {
            Ok(())
        }

        fn list_devices(&self) -> Result<Vec<MatterDeviceInfo>> {
            Ok(self.devices.clone())
        }

        fn probe_light(&self, node_id: u64) -> Result<CommissionedDevice> {
            if let Some(delay) = self.probe_delays.get(&node_id) {
                std::thread::sleep(*delay);
            }
            if let Some(error) = self.probe_failures.get(&node_id) {
                anyhow::bail!("{}", error);
            }
            Ok(commissioned_device(node_id, 2))
        }

        fn set_on_off(&self, _node_id: u64, _endpoint: u16, _on: bool) -> Result<()> {
            Ok(())
        }

        fn configure_group(&self, _group: &MatterGroup) -> Result<()> {
            Ok(())
        }

        fn remove_group(&self, _group_id: u16, _members: &[MatterGroupMember]) -> Result<()> {
            Ok(())
        }

        fn identify_light(&self, _node_id: u64, _endpoint: u16, _duration_secs: u16) -> Result<()> {
            Ok(())
        }

        fn set_brightness(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _level: u8,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn set_color_temperature(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _kelvin: u16,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn set_xy(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _x: f32,
            _y: f32,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn set_hue_saturation(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _hue: u8,
            _saturation: u8,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn read_on_off(&self, _node_id: u64, _endpoint: u16) -> Result<bool> {
            Ok(false)
        }

        fn subscribe_on_off(
            &self,
            _targets: &[MatterSubscriptionTarget],
            _min_interval_secs: u16,
            _max_interval_secs: u16,
        ) -> Result<()> {
            self.subscribe_calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            anyhow::bail!("subscriptions disabled in test")
        }

        fn drain_attribute_reports(&self) -> Result<Vec<MatterAttributeReport>> {
            Ok(Vec::new())
        }
    }

    fn shared_state(prefix: &str) -> SharedState {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "rhythm-matter-lifecycle-{}-{}-{}",
            prefix,
            std::process::id(),
            id
        ));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        let state = Arc::new(Mutex::new(AppState::default()));
        state.lock().unwrap().data_dir = dir.to_string_lossy().to_string();
        state
    }

    fn device_info(node_id: u64) -> MatterDeviceInfo {
        MatterDeviceInfo {
            node_id,
            vendor_name: format!("Vendor {node_id}"),
            product_name: format!("Lamp {node_id}"),
            reachable: true,
        }
    }

    fn commissioned_device(node_id: u64, endpoint: u16) -> CommissionedDevice {
        CommissionedDevice {
            node_id,
            vendor_name: format!("Vendor {node_id}"),
            product_name: format!("Lamp {node_id}"),
            vendor_id: 100,
            product_id: 200,
            serial_number: Some(format!("serial-{node_id}")),
            light_endpoint: endpoint,
            color_modes: vec![MatterColorMode::ColorTemperature],
            min_kelvin: Some(2700),
            max_kelvin: Some(5000),
        }
    }

    fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
        let started = std::time::Instant::now();
        while started.elapsed() < timeout {
            if condition() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        condition()
    }

    #[test]
    fn parse_simple_device_id() {
        assert_eq!(parse_device_id("matter-100"), Some((100, 1)));
    }

    #[test]
    fn parse_device_id_with_endpoint() {
        assert_eq!(parse_device_id("matter-42-2"), Some((42, 2)));
    }

    #[test]
    fn parse_invalid_prefix() {
        assert_eq!(parse_device_id("hue-abc123"), None);
    }

    #[test]
    fn parse_invalid_node_id() {
        assert_eq!(parse_device_id("matter-abc"), None);
    }

    #[test]
    fn roundtrip() {
        assert_eq!(parse_device_id(&format_device_id(55, 1)), Some((55, 1)));
        assert_eq!(parse_device_id(&format_device_id(55, 3)), Some((55, 3)));
    }

    #[test]
    fn next_node_id_seed_uses_max_commissioned_node() {
        let commissioned = vec![
            MatterDeviceInfo {
                node_id: 100,
                vendor_name: "A".to_string(),
                product_name: "Light".to_string(),
                reachable: true,
            },
            MatterDeviceInfo {
                node_id: 105,
                vendor_name: "B".to_string(),
                product_name: "Lamp".to_string(),
                reachable: true,
            },
        ];

        assert_eq!(next_node_id_seed(&commissioned), 106);
    }

    #[test]
    fn fallback_initial_metadata_includes_persisted_endpoint_ids() {
        let state = shared_state("fallback-endpoints");
        let key = HubKey::new(HubType::new("matter"), "local");
        let identity = DiscoveredIdentity {
            native_id: "matter-12-2".to_string(),
            room_id: None,
            room_name: None,
            name: "Endpoint two bulb".to_string(),
            device_type: DeviceType::Light,
            hardware_ids: vec![HardwareId::matter("12")],
            manufacturer: None,
            model: None,
        };
        state
            .lock()
            .unwrap()
            .canonical_registry
            .resolve(&identity, &key, 100);

        let metadata = fallback_initial_device_metadata(&state, &[device_info(12)], &key);

        assert!(metadata.device_caps.contains_key("matter-12"));
        assert!(metadata.device_caps.contains_key("matter-12-2"));
    }

    #[test]
    fn connect_matter_builds_hub_data_from_commissioned_devices_and_tags_events() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RHYTHM_MATTER_PROFILE_SYNC", "disabled");
        let state = shared_state("connect");
        let key = HubKey::new(HubType::new("matter"), "local");
        state
            .lock()
            .unwrap()
            .hub_credentials
            .insert(key.clone(), matter_credentials("local", "fabric-test"));

        let mut probe_failures = HashMap::new();
        probe_failures.insert(12, "probe failed".to_string());
        let transport = Arc::new(FakeMatterTransport::new(
            vec![device_info(10), device_info(12)],
            probe_failures,
        ));

        let (hub, event_rx) = connect_matter(&state, transport.clone()).unwrap();

        assert_eq!(hub.hub_key, key);
        let data = hub.data::<Arc<MatterHubData>>().unwrap();
        assert_eq!(data.fabric_id, "fabric-test");
        assert_eq!(data.commissioned.lock().unwrap().len(), 2);
        assert!(data.device_caps.lock().unwrap().contains_key("matter-10"));
        assert!(data.device_caps.lock().unwrap().contains_key("matter-12"));

        assert!(
            wait_until(Duration::from_secs(1), || {
                let caps_ready = data.device_caps.lock().unwrap().contains_key("matter-10-2");
                let unreachable_marked = !data
                    .commissioned
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|device| device.node_id == 12)
                    .unwrap()
                    .reachable;
                caps_ready && unreachable_marked
            }),
            "initial Matter metadata refresh did not complete"
        );
        assert_eq!(
            data.next_node_id.load(std::sync::atomic::Ordering::SeqCst),
            13
        );
        assert!(data.device_caps.lock().unwrap().contains_key("matter-10-2"));
        assert!(data.device_caps.lock().unwrap().contains_key("matter-12"));
        assert!(!data.device_caps.lock().unwrap().contains_key("matter-12-2"));

        let event = event_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert_eq!(event.hub_key(), Some(&key));
        for _ in 0..20 {
            if transport
                .subscribe_calls
                .load(std::sync::atomic::Ordering::Relaxed)
                > 0
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            transport
                .subscribe_calls
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        std::env::remove_var("RHYTHM_MATTER_PROFILE_SYNC");
    }

    #[test]
    fn connect_matter_returns_before_initial_probe_refresh_finishes() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RHYTHM_MATTER_PROFILE_SYNC", "disabled");
        let state = shared_state("connect-async-probe");
        let mut probe_failures = HashMap::new();
        probe_failures.insert(12, "probe failed".to_string());
        let transport = Arc::new(
            FakeMatterTransport::new(vec![device_info(12)], probe_failures)
                .with_probe_delay(12, Duration::from_millis(800)),
        );

        let started = std::time::Instant::now();
        let (hub, _event_rx) = connect_matter(&state, transport).unwrap();
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_millis(250),
            "Matter connect waited for initial probe refresh: {:?}",
            elapsed
        );

        let data = hub.data::<Arc<MatterHubData>>().unwrap();
        assert!(data.device_caps.lock().unwrap().contains_key("matter-12"));
        assert!(
            data.commissioned
                .lock()
                .unwrap()
                .iter()
                .find(|device| device.node_id == 12)
                .unwrap()
                .reachable,
            "connect should expose cached commissioned devices before live reachability probing"
        );

        assert!(
            wait_until(Duration::from_secs(2), || {
                !data
                    .commissioned
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|device| device.node_id == 12)
                    .unwrap()
                    .reachable
            }),
            "background metadata refresh did not mark the failed node unreachable"
        );
        std::env::remove_var("RHYTHM_MATTER_PROFILE_SYNC");
    }

    #[test]
    fn load_initial_device_metadata_bounds_unreachable_probe() {
        // A device that hangs (unreachable) must not block the connect path:
        // the probe is bounded by a deadline and the device falls back to basic
        // info, so Matter still loads promptly. Regression for #169.
        let state = shared_state("probe-deadline");
        let transport: Arc<dyn MatterTransport> = Arc::new(
            FakeMatterTransport::new(vec![device_info(10), device_info(12)], HashMap::new())
                .with_probe_delay(12, Duration::from_secs(5)),
        );
        let commissioned = transport.list_devices().unwrap();
        let cloud = crate::cloud_profiles::CloudMatterProfileCatalog::default();

        let started = std::time::Instant::now();
        let metadata = load_initial_device_metadata(
            &state,
            &transport,
            &commissioned,
            &cloud,
            Duration::from_millis(200),
        );
        let elapsed = started.elapsed();

        // Unpatched code probes serially with no deadline and would block ~5s
        // on node 12. With the deadline it returns in ~200ms.
        assert!(
            elapsed < Duration::from_secs(2),
            "bootstrap probe blocked on unreachable device: {:?}",
            elapsed
        );
        // Reachable device pre-warmed; unreachable device gets fallback
        // metadata but no subscription target, so background subscriptions
        // cannot occupy chipd on a known-bad node.
        assert!(metadata.device_caps.contains_key("matter-10-2"));
        assert!(metadata.device_caps.contains_key("matter-12"));
        assert!(!metadata.device_caps.contains_key("matter-12-2"));
        let fallback_caps = metadata.device_caps.get("matter-12").unwrap();
        assert!(fallback_caps.supports_hue_saturation());
        assert!(!fallback_caps.supports_xy_color());
        assert!(metadata.unreachable_nodes.contains(&12));
        assert!(!metadata
            .subscription_targets
            .iter()
            .any(|target| target.node_id == 12 && target.endpoint == 1));
    }
}
