//! Matter hub lifecycle — connect, disconnect, runtime creation.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use log::{info, warn};
use rhythm_devices::{DeviceQuirk, LightCapabilities};
use rhythm_os::api_types::{LightCapabilitiesDto, LightColorTemperatureCapabilitiesDto};
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::{ActiveHub, HubEvent, HubType};
use rhythm_os::registry::HubDeviceRegistry;
use rhythm_os::state::SharedState;

use crate::hub_state::MatterHubData;
use crate::transport::{
    CommissionedDevice, MatterAttributeValue, MatterControllerEvent, MatterControllerEventCursor,
    MatterDeviceInfo, MatterSubscriptionTarget, MatterTransport,
    DEFAULT_SUBSCRIPTION_MAX_INTERVAL_SECS, DEFAULT_SUBSCRIPTION_MIN_INTERVAL_SECS,
};

const MATTER_EVENT_LONG_POLL: Duration = Duration::from_secs(1);
const MATTER_EVENT_RETRY_INITIAL: Duration = Duration::from_millis(250);
const MATTER_EVENT_RETRY_MAX: Duration = Duration::from_secs(5);
const MATTER_EVENT_SHUTDOWN_POLL: Duration = Duration::from_millis(50);
const MATTER_SUBSCRIPTION_RETRY_INTERVAL: Duration = Duration::from_secs(30);

fn next_controller_event_retry_delay(current: Duration) -> Duration {
    current.saturating_mul(2).min(MATTER_EVENT_RETRY_MAX)
}

fn wait_for_controller_event_retry(
    shutdown: &std::sync::atomic::AtomicBool,
    delay: Duration,
) -> bool {
    let deadline = std::time::Instant::now() + delay;
    while !shutdown.load(Ordering::Relaxed) {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return false;
        }
        std::thread::sleep(remaining.min(MATTER_EVENT_SHUTDOWN_POLL));
    }
    true
}

fn subscription_targets(transport: &dyn MatterTransport) -> Vec<MatterSubscriptionTarget> {
    match transport.list_commissioned_devices() {
        Ok(devices) if !devices.is_empty() => devices
            .into_iter()
            .map(|device| MatterSubscriptionTarget {
                node_id: device.node_id,
                endpoint: device.light_endpoint,
            })
            .collect(),
        _ => transport
            .list_devices()
            .unwrap_or_default()
            .into_iter()
            .map(|device| MatterSubscriptionTarget {
                node_id: device.node_id,
                endpoint: 1,
            })
            .collect(),
    }
}

struct MatterSubscriptionFailure {
    target: MatterSubscriptionTarget,
    error: String,
}

fn attempt_observed_state_subscriptions(
    transport: &dyn MatterTransport,
    mut should_stop: impl FnMut() -> bool,
) -> Vec<MatterSubscriptionFailure> {
    let targets = subscription_targets(transport);
    let mut failures = Vec::new();
    for target in targets {
        if should_stop() {
            break;
        }
        if let Err(error) = transport.subscribe_on_off(
            std::slice::from_ref(&target),
            DEFAULT_SUBSCRIPTION_MIN_INTERVAL_SECS,
            DEFAULT_SUBSCRIPTION_MAX_INTERVAL_SECS,
        ) {
            failures.push(MatterSubscriptionFailure {
                target,
                error: format!("{error:#}"),
            });
        }
    }
    failures
}

fn start_observed_state_subscription_worker(
    transport: Arc<dyn MatterTransport>,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
) -> SyncSender<()> {
    let (refresh_tx, refresh_rx) = sync_channel(1);
    let spawn_result = std::thread::Builder::new()
        .name("matter-observed-subscriptions".to_string())
        .spawn(move || {
            let mut previous_failures = HashSet::new();
            loop {
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                let failures = attempt_observed_state_subscriptions(transport.as_ref(), || {
                    shutdown.load(Ordering::Relaxed)
                });
                let current_failures: HashSet<(u64, u16)> = failures
                    .iter()
                    .map(|failure| (failure.target.node_id, failure.target.endpoint))
                    .collect();

                for failure in &failures {
                    let key = (failure.target.node_id, failure.target.endpoint);
                    if !previous_failures.contains(&key) {
                        warn!(
                            target: "evt",
                            "Matter observed-state subscription unavailable for node {} endpoint {}: {}",
                            failure.target.node_id,
                            failure.target.endpoint,
                            failure.error
                        );
                    }
                }
                for (node_id, endpoint) in previous_failures.difference(&current_failures) {
                    info!(
                        target: "evt",
                        "Matter observed-state subscription recovered for node {} endpoint {}",
                        node_id,
                        endpoint
                    );
                }
                previous_failures = current_failures;

                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                match refresh_rx.recv_timeout(MATTER_SUBSCRIPTION_RETRY_INTERVAL) {
                    Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        });
    if let Err(error) = spawn_result {
        warn!(
            target: "evt",
            "Failed to start Matter observed-state subscription worker: {error}"
        );
    }
    refresh_tx
}

fn cache_on_off_observation(
    report: &crate::transport::MatterAttributeReport,
    observations: &Mutex<HashMap<(u64, u16), (bool, std::time::Instant)>>,
) {
    if report.cluster != crate::clusters::CLUSTER_ON_OFF_U32
        || report.attr_id != crate::clusters::ATTR_ON_OFF_U32
    {
        return;
    }

    let MatterAttributeValue::Bool(lights_on) = &report.value;
    if let Ok(mut observations) = observations.lock() {
        observations.insert(
            (report.node_id, report.endpoint),
            (*lights_on, std::time::Instant::now()),
        );
    }
}

fn invalidate_on_off_observations(
    observations: &Mutex<HashMap<(u64, u16), (bool, std::time::Instant)>>,
) {
    if let Ok(mut observations) = observations.lock() {
        observations.clear();
    }
}

fn start_controller_event_stream(
    transport: Arc<dyn MatterTransport>,
    event_tx: std::sync::mpsc::Sender<HubEvent>,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
    node_proof_of_life: Arc<Mutex<HashMap<u64, std::time::Instant>>>,
    on_off_observations: Arc<Mutex<HashMap<(u64, u16), (bool, std::time::Instant)>>>,
) {
    let subscription_refresh =
        start_observed_state_subscription_worker(transport.clone(), shutdown.clone());
    let spawn_result = std::thread::Builder::new()
        .name("matter-controller-events".to_string())
        .spawn(move || {
            let mut cursor: Option<MatterControllerEventCursor> = None;
            let mut connection_state: Option<bool> = None;
            let mut retry_delay = MATTER_EVENT_RETRY_INITIAL;

            while !shutdown.load(Ordering::Relaxed) {
                // Bootstrap without a long poll so the stream identity is known
                // before Connected allows callers to admit controller-owned work.
                let max_wait = if cursor.is_some() {
                    MATTER_EVENT_LONG_POLL
                } else {
                    Duration::ZERO
                };
                let batch = match transport.wait_controller_events(cursor.as_ref(), max_wait) {
                    Ok(batch) => batch,
                    Err(error) => {
                        if connection_state != Some(false) {
                            let _ = event_tx.send(HubEvent::Disconnected {
                                hub_key: None,
                                reason: format!("Matter controller event stream: {error:#}"),
                            });
                        }
                        connection_state = Some(false);
                        if wait_for_controller_event_retry(shutdown.as_ref(), retry_delay) {
                            break;
                        }
                        retry_delay = next_controller_event_retry_delay(retry_delay);
                        continue;
                    }
                };
                retry_delay = MATTER_EVENT_RETRY_INITIAL;

                let stream_changed = cursor
                    .as_ref()
                    .is_some_and(|cursor| cursor.stream_id != batch.stream_id);
                let history_gap = cursor.as_ref().is_some_and(|cursor| {
                    cursor.stream_id == batch.stream_id
                        && cursor.sequence.saturating_add(1) < batch.oldest_sequence
                });
                if stream_changed || history_gap {
                    let reason = if stream_changed {
                        "Matter controller sidecar restarted"
                    } else {
                        "Matter controller event history gap"
                    };
                    let _ = event_tx.send(HubEvent::CommandStreamReset {
                        hub_key: None,
                        stream_id: batch.stream_id.clone(),
                        history_gap,
                        reason: reason.to_string(),
                    });
                    // Values are authoritative only within one contiguous
                    // controller event stream. Re-subscription produces a new
                    // initial attribute report for every reachable endpoint.
                    invalidate_on_off_observations(on_off_observations.as_ref());
                    let _ = subscription_refresh.try_send(());
                }
                let mut last_sequence = cursor
                    .as_ref()
                    .filter(|cursor| cursor.stream_id == batch.stream_id)
                    .map(|cursor| cursor.sequence)
                    .unwrap_or(0);
                let event_stream_id = batch.stream_id.clone();
                for envelope in batch.events {
                    last_sequence = last_sequence.max(envelope.sequence);
                    let event = match envelope.event {
                        MatterControllerEvent::CommandOutcome(outcome) => {
                            if matches!(
                                outcome.status,
                                crate::transport::MatterCommandOutcomeStatus::Succeeded
                            ) {
                                if let Ok(mut proof) = node_proof_of_life.lock() {
                                    proof.insert(outcome.node_id, std::time::Instant::now());
                                }
                            }
                            Some(crate::events::translate_command_outcome(
                                event_stream_id.clone(),
                                outcome,
                            ))
                        }
                        MatterControllerEvent::AttributeReport(report) => {
                            cache_on_off_observation(&report, on_off_observations.as_ref());
                            if let Ok(mut proof) = node_proof_of_life.lock() {
                                proof.insert(report.node_id, std::time::Instant::now());
                            }
                            crate::events::translate_report(&report)
                        }
                    };
                    if let Some(event) = event {
                        if event_tx.send(event).is_err() {
                            return;
                        }
                    }
                }
                cursor = Some(MatterControllerEventCursor {
                    stream_id: batch.stream_id,
                    sequence: last_sequence,
                });
                if connection_state != Some(true) {
                    let _ = event_tx.send(HubEvent::Connected { hub_key: None });
                    connection_state = Some(true);
                }
            }
        });
    if let Err(error) = spawn_result {
        warn!(target: "evt", "Failed to start Matter controller event stream: {error}");
    }
}

/// Connect to the local Matter fabric.
pub fn connect_matter(
    state: &SharedState,
    transport: Arc<dyn MatterTransport>,
) -> Result<(ActiveHub, Receiver<HubEvent>)> {
    let hub_key = HubKey::new(HubType::new("matter"), "local");
    let persisted_devices = transport.list_commissioned_devices().unwrap_or_default();
    let commissioned = if persisted_devices.is_empty() {
        transport.list_devices().unwrap_or_default()
    } else {
        persisted_devices
            .iter()
            .map(device_info_from_record)
            .collect()
    };
    let cloud_profiles = crate::cloud_profiles::load_or_sync_for_state(state);
    let initial_metadata = initial_device_metadata(
        state,
        &commissioned,
        &persisted_devices,
        &cloud_profiles,
        &hub_key,
    );
    publish_endpoint_capabilities(state, &hub_key, &initial_metadata.device_caps)?;
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
    let node_proof_of_life = Arc::new(Mutex::new(HashMap::new()));
    let node_proof_of_life_for_closure = node_proof_of_life.clone();
    let node_proof_of_life_for_events = node_proof_of_life.clone();
    let on_off_observations = Arc::new(Mutex::new(HashMap::new()));
    let on_off_observations_for_closure = on_off_observations.clone();
    let on_off_observations_for_events = on_off_observations.clone();

    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let hub_data_event_tx = event_tx.clone();
    let event_transport = transport.clone();

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
                on_off_observations: on_off_observations_for_closure.clone(),
                event_tx: hub_data_event_tx,
            }))
        },
        move |_registry, shutdown| {
            start_controller_event_stream(
                event_transport,
                event_tx,
                shutdown,
                node_proof_of_life_for_events,
                on_off_observations_for_events,
            );
            event_rx
        },
    )?;

    Ok((hub, event_rx))
}

fn device_info_from_record(device: &CommissionedDevice) -> MatterDeviceInfo {
    MatterDeviceInfo {
        node_id: device.node_id,
        vendor_name: device.vendor_name.clone(),
        product_name: device.product_name.clone(),
        // Persisted metadata proves identity and capabilities, not current
        // liveness. Keep the neutral startup behavior; command/read outcomes
        // update reachability after connect without starting background work.
        reachable: true,
    }
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
}

pub(crate) fn normalized_endpoint_capabilities(
    capabilities: &LightCapabilities,
) -> Option<serde_json::Value> {
    let color_temperature = if capabilities.supports_color_temp() {
        let min_kelvin = capabilities.min_kelvin?;
        let max_kelvin = capabilities.max_kelvin?;
        if min_kelvin == 0 || min_kelvin > max_kelvin {
            return None;
        }
        Some(LightColorTemperatureCapabilitiesDto {
            min_kelvin,
            max_kelvin,
        })
    } else {
        None
    };

    Some(serde_json::json!({
        "light_capabilities": LightCapabilitiesDto {
            color_temperature,
            individual_profile_overrides: None,
        },
        "automatic_naming": {
            "color_kind": if capabilities.light_type == rhythm_devices::LightType::ExtendedColor {
                "color"
            } else {
                "white"
            }
        },
    }))
}

fn publish_endpoint_capabilities(
    state: &SharedState,
    hub_key: &HubKey,
    device_capabilities: &HashMap<String, LightCapabilities>,
) -> Result<()> {
    let mut state = state
        .lock()
        .map_err(|_| anyhow::anyhow!("Failed to lock state"))?;
    let mut changed = false;

    let canonical_ids = state
        .canonical_registry
        .devices()
        .map(|device| device.id.clone())
        .collect::<Vec<_>>();
    for canonical_id in canonical_ids {
        let Some(device) = state.canonical_registry.get_mut(&canonical_id) else {
            continue;
        };
        for endpoint in &mut device.endpoints {
            if &endpoint.hub_key != hub_key || !endpoint.active {
                continue;
            }
            let Some(capabilities) = device_capabilities.get(&endpoint.native_id) else {
                continue;
            };
            let Some(normalized) = normalized_endpoint_capabilities(capabilities) else {
                continue;
            };
            if endpoint.capabilities.as_ref() != Some(&normalized) {
                endpoint.capabilities = Some(normalized);
                changed = true;
            }
        }
    }

    if changed {
        rhythm_os::commands::save_authority_state(&state)?;
    }
    Ok(())
}

/// Build the best metadata available without talking to the device.
///
/// `list_devices` is backed by chipd's persisted device store, so its vendor
/// and product names survive a temporary connectivity failure. Preserve any
/// matching built-in profile here instead of replacing it with an anonymous
/// extended-color fallback. In particular, command quirks such as
/// `NeedsExplicitOn` remain necessary while the device is unreachable to
/// probes but reachable again by the time a light command is dispatched.
fn fallback_device_metadata(info: &MatterDeviceInfo) -> (LightCapabilities, Vec<DeviceQuirk>) {
    let Some(entry) =
        rhythm_devices::builtin_db().lookup(info.vendor_name.as_str(), info.product_name.as_str())
    else {
        return (
            crate::commissioning::fallback_device_capabilities(),
            Vec::new(),
        );
    };

    let quirks = entry
        .matter
        .as_ref()
        .map(|matter| matter.quirks.clone())
        .unwrap_or_default();
    (entry.capabilities(), quirks)
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

    let mut insert_fallback = |device_id: String, info: Option<&MatterDeviceInfo>| {
        let (caps, quirks) = info.map(fallback_device_metadata).unwrap_or_else(|| {
            (
                crate::commissioning::fallback_device_capabilities(),
                Vec::new(),
            )
        });
        device_caps.entry(device_id.clone()).or_insert(caps);
        device_quirks.entry(device_id).or_insert(quirks);
    };

    for info in commissioned {
        insert_fallback(format_device_id(info.node_id, 1), Some(info));
    }

    if let Ok(state) = state.lock() {
        for device in state.canonical_registry.devices() {
            for endpoint in device.active_endpoints() {
                if &endpoint.hub_key != hub_key {
                    continue;
                }
                let Some((node_id, _)) = parse_device_id(&endpoint.native_id) else {
                    continue;
                };
                if commissioned_nodes.contains(&node_id) {
                    let info = commissioned.iter().find(|info| info.node_id == node_id);
                    insert_fallback(endpoint.native_id.clone(), info);
                }
            }
        }
    }

    InitialDeviceMetadata {
        device_caps,
        device_quirks,
    }
}

fn initial_device_metadata(
    state: &SharedState,
    commissioned: &[MatterDeviceInfo],
    persisted_devices: &[CommissionedDevice],
    cloud_profiles: &crate::cloud_profiles::CloudMatterProfileCatalog,
    hub_key: &HubKey,
) -> InitialDeviceMetadata {
    let mut metadata = fallback_initial_device_metadata(state, commissioned, hub_key);
    let local_overrides = crate::local_quirks::load_overrides_for_state(state);
    let aliases_by_node = state
        .lock()
        .ok()
        .map(|state| {
            state
                .canonical_registry
                .devices()
                .flat_map(|device| device.active_endpoints())
                .filter(|endpoint| &endpoint.hub_key == hub_key)
                .filter_map(|endpoint| {
                    parse_device_id(&endpoint.native_id)
                        .map(|(node_id, _)| (node_id, endpoint.native_id.clone()))
                })
                .fold(
                    HashMap::<u64, Vec<String>>::new(),
                    |mut aliases, (node_id, id)| {
                        aliases.entry(node_id).or_default().push(id);
                        aliases
                    },
                )
        })
        .unwrap_or_default();
    for device in persisted_devices {
        let device_id = format_device_id(device.node_id, device.light_endpoint);
        let mut caps = crate::commissioning::build_device_capabilities(device);
        let mut quirks = crate::commissioning::build_device_quirks(device);
        cloud_profiles.apply_to_device(device, &mut caps, &mut quirks);
        if let Some(override_caps) = local_overrides.capabilities.get(&device_id) {
            crate::local_quirks::apply_capability_override(&mut caps, override_caps);
        }
        if let Some(local_quirks) = local_overrides.quirks.get(&device_id) {
            quirks = local_quirks.clone();
        }

        metadata.device_caps.insert(device_id.clone(), caps.clone());
        metadata
            .device_quirks
            .insert(device_id.clone(), quirks.clone());
        if let Some(aliases) = aliases_by_node.get(&device.node_id) {
            for alias in aliases {
                metadata.device_caps.insert(alias.clone(), caps.clone());
                metadata.device_quirks.insert(alias.clone(), quirks.clone());
            }
        }
    }
    metadata
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use rhythm_core::runtime::hub_registry::DeviceType;
    use rhythm_os::canonical::identity::{DiscoveredIdentity, HardwareId};
    use rhythm_os::state::AppState;

    use crate::provider::matter_credentials;
    use crate::transport::{
        CommissionedDevice, MatterColorMode, MatterCommissionRequest, MatterGroup,
        MatterGroupMember, MatterSubscriptionTarget,
    };

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct FakeMatterTransport {
        devices: Vec<MatterDeviceInfo>,
        persisted_devices: Vec<CommissionedDevice>,
        probe_calls: AtomicUsize,
        subscribe_calls: AtomicUsize,
        subscription_attempts: Mutex<Vec<MatterSubscriptionTarget>>,
        subscription_failures_remaining: Mutex<HashMap<(u64, u16), usize>>,
        event_wait_calls: AtomicUsize,
        block_initial_event_wait: AtomicBool,
        release_initial_event_wait: AtomicBool,
    }

    impl FakeMatterTransport {
        fn new(devices: Vec<MatterDeviceInfo>, persisted_devices: Vec<CommissionedDevice>) -> Self {
            Self {
                devices,
                persisted_devices,
                probe_calls: AtomicUsize::new(0),
                subscribe_calls: AtomicUsize::new(0),
                subscription_attempts: Mutex::new(Vec::new()),
                subscription_failures_remaining: Mutex::new(HashMap::new()),
                event_wait_calls: AtomicUsize::new(0),
                block_initial_event_wait: AtomicBool::new(false),
                release_initial_event_wait: AtomicBool::new(false),
            }
        }

        fn fail_subscription_attempts(&self, node_id: u64, endpoint: u16, attempt_count: usize) {
            self.subscription_failures_remaining
                .lock()
                .unwrap()
                .insert((node_id, endpoint), attempt_count);
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

        fn list_commissioned_devices(&self) -> Result<Vec<CommissionedDevice>> {
            Ok(self.persisted_devices.clone())
        }

        fn probe_light(&self, _node_id: u64) -> Result<CommissionedDevice> {
            self.probe_calls.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("startup must not probe persisted Matter devices")
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
            targets: &[MatterSubscriptionTarget],
            _min_interval_secs: u16,
            _max_interval_secs: u16,
        ) -> Result<()> {
            assert_eq!(
                targets.len(),
                1,
                "observed-state subscriptions must be isolated per endpoint"
            );
            let target = targets[0].clone();
            self.subscription_attempts
                .lock()
                .unwrap()
                .push(target.clone());
            let mut failures = self.subscription_failures_remaining.lock().unwrap();
            let should_fail =
                if let Some(remaining) = failures.get_mut(&(target.node_id, target.endpoint)) {
                    if *remaining > 0 {
                        *remaining -= 1;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };
            self.subscribe_calls.fetch_add(1, Ordering::SeqCst);
            if should_fail {
                anyhow::bail!("simulated operational discovery timeout")
            }
            Ok(())
        }

        fn wait_controller_events(
            &self,
            cursor: Option<&MatterControllerEventCursor>,
            max_wait: Duration,
        ) -> Result<crate::transport::MatterControllerEventBatch> {
            let call_index = self.event_wait_calls.fetch_add(1, Ordering::SeqCst);
            if call_index == 0 && self.block_initial_event_wait.load(Ordering::SeqCst) {
                while !self.release_initial_event_wait.load(Ordering::SeqCst) {
                    std::thread::yield_now();
                }
            }
            if !max_wait.is_zero() {
                std::thread::sleep(max_wait);
            }
            Ok(crate::transport::MatterControllerEventBatch {
                stream_id: cursor
                    .map(|cursor| cursor.stream_id.clone())
                    .unwrap_or_else(|| "fake-controller".to_string()),
                oldest_sequence: cursor.map(|cursor| cursor.sequence + 1).unwrap_or(1),
                events: Vec::new(),
            })
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

    fn wait_for_atomic_at_least(value: &AtomicUsize, expected: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while value.load(Ordering::SeqCst) < expected && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(
            value.load(Ordering::SeqCst) >= expected,
            "timed out waiting for {expected} calls"
        );
    }

    fn device_info(node_id: u64) -> MatterDeviceInfo {
        MatterDeviceInfo {
            node_id,
            vendor_name: format!("Vendor {node_id}"),
            product_name: format!("Lamp {node_id}"),
            reachable: true,
        }
    }

    fn h6004_device_info(node_id: u64) -> MatterDeviceInfo {
        MatterDeviceInfo {
            node_id,
            vendor_name: "Shenzhen Qianyan Technology".to_string(),
            product_name: "H6004".to_string(),
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

    fn h6004_device(node_id: u64) -> CommissionedDevice {
        CommissionedDevice {
            node_id,
            vendor_name: "Shenzhen Qianyan Technology".to_string(),
            product_name: "H6004".to_string(),
            vendor_id: 4999,
            product_id: 24580,
            serial_number: Some(format!("h6004-{node_id}")),
            light_endpoint: 1,
            color_modes: vec![
                MatterColorMode::HueSaturation,
                MatterColorMode::Xy,
                MatterColorMode::ColorTemperature,
            ],
            min_kelvin: Some(2000),
            max_kelvin: Some(6500),
        }
    }

    fn sengled_w41_device(node_id: u64) -> CommissionedDevice {
        CommissionedDevice {
            node_id,
            vendor_name: "Sengled".to_string(),
            product_name: "W41-N15A".to_string(),
            vendor_id: 4448,
            product_id: 36866,
            serial_number: Some(format!("sengled-{node_id}")),
            light_endpoint: 1,
            color_modes: vec![
                MatterColorMode::HueSaturation,
                MatterColorMode::ColorTemperature,
            ],
            min_kelvin: None,
            max_kelvin: None,
        }
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
    fn startup_publishes_persisted_matter_capabilities_to_canonical_endpoint() {
        let state = shared_state("publish-endpoint-capabilities");
        let key = HubKey::new(HubType::new("matter"), "local");
        let identity = DiscoveredIdentity {
            native_id: "matter-101".to_string(),
            room_id: None,
            room_name: None,
            name: "AiDot Smart RGBTW Bulb".to_string(),
            device_type: DeviceType::Light,
            hardware_ids: vec![HardwareId::matter("101")],
            manufacturer: Some("AiDot".to_string()),
            model: Some("Smart RGBTW Bulb".to_string()),
        };
        state
            .lock()
            .unwrap()
            .canonical_registry
            .resolve(&identity, &key, 100);
        let capabilities = HashMap::from([(
            "matter-101".to_string(),
            LightCapabilities {
                color_modes: vec![
                    rhythm_devices::ColorMode::HueSaturation,
                    rhythm_devices::ColorMode::ColorTemperature,
                ],
                min_kelvin: Some(2702),
                max_kelvin: Some(6535),
                ..LightCapabilities::defaults_for(rhythm_devices::LightType::ExtendedColor)
            },
        )]);

        publish_endpoint_capabilities(&state, &key, &capabilities).unwrap();

        let state = state.lock().unwrap();
        let endpoint = state
            .canonical_registry
            .find_by_native_id(&key, "matter-101")
            .and_then(|device| device.endpoint_by_native_id("matter-101"))
            .unwrap();
        assert_eq!(
            endpoint
                .capabilities
                .as_ref()
                .and_then(|value| value.pointer("/light_capabilities/color_temperature")),
            Some(&serde_json::json!({
                "min_kelvin": 2702,
                "max_kelvin": 6535,
            }))
        );
    }

    #[test]
    fn fallback_initial_metadata_preserves_known_device_quirks() {
        let state = shared_state("fallback-known-quirks");
        let key = HubKey::new(HubType::new("matter"), "local");

        let metadata = fallback_initial_device_metadata(&state, &[h6004_device_info(107)], &key);

        assert_eq!(
            metadata.device_quirks.get("matter-107"),
            Some(&vec![
                DeviceQuirk::NeedsExplicitOn,
                DeviceQuirk::NeedsHueSaturationNotCt,
                DeviceQuirk::CommandThrottleMs(250),
            ])
        );
        assert!(metadata
            .device_caps
            .get("matter-107")
            .is_some_and(LightCapabilities::supports_hue_saturation));
        assert!(!metadata
            .device_caps
            .get("matter-107")
            .is_some_and(LightCapabilities::supports_xy_color));
    }

    #[test]
    fn fallback_initial_metadata_preserves_sengled_hue_saturation_profile() {
        let state = shared_state("fallback-sengled-profile");
        let key = HubKey::new(HubType::new("matter"), "local");
        let info = MatterDeviceInfo {
            node_id: 104,
            vendor_name: "Sengled".to_string(),
            product_name: "W41-N15A".to_string(),
            reachable: false,
        };

        let metadata = fallback_initial_device_metadata(&state, &[info], &key);
        let caps = metadata.device_caps.get("matter-104").unwrap();

        assert!(caps.supports_hue_saturation());
        assert!(!caps.supports_xy_color());
        assert_eq!(
            metadata.device_quirks.get("matter-104"),
            Some(&vec![
                DeviceQuirk::NeedsExplicitOn,
                DeviceQuirk::NeedsHueSaturationNotCt,
            ])
        );
    }

    #[test]
    fn persisted_metadata_applies_local_overrides_after_builtin_and_cloud_profiles() {
        let state = shared_state("persisted-overrides");
        let key = HubKey::new(HubType::new("matter"), "local");
        let device = h6004_device(107);
        crate::local_quirks::save_device_profile_override(
            &state,
            "matter-107",
            Some(vec![DeviceQuirk::NeedsXyNotCt]),
            Some(crate::local_quirks::LocalCapabilityOverride {
                min_brightness: Some(17),
                supports_transition: Some(false),
            }),
            Some("test-report".to_string()),
        )
        .unwrap();

        let metadata = initial_device_metadata(
            &state,
            &[device_info_from_record(&device)],
            &[device],
            &crate::cloud_profiles::CloudMatterProfileCatalog::default(),
            &key,
        );

        let caps = metadata.device_caps.get("matter-107").unwrap();
        assert_eq!(caps.min_brightness, Some(17));
        assert!(!caps.supports_transition);
        assert_eq!(
            metadata.device_quirks.get("matter-107"),
            Some(&vec![DeviceQuirk::NeedsXyNotCt])
        );
    }

    #[test]
    fn persisted_sengled_metadata_keeps_profile_hue_saturation_and_explicit_on() {
        let state = shared_state("persisted-sengled-profile");
        let key = HubKey::new(HubType::new("matter"), "local");
        let device = sengled_w41_device(104);

        let metadata = initial_device_metadata(
            &state,
            &[device_info_from_record(&device)],
            &[device],
            &crate::cloud_profiles::CloudMatterProfileCatalog::default(),
            &key,
        );

        let caps = metadata.device_caps.get("matter-104").unwrap();
        assert!(caps.supports_hue_saturation());
        assert!(!caps.supports_xy_color());
        assert_eq!(
            metadata.device_quirks.get("matter-104"),
            Some(&vec![
                DeviceQuirk::NeedsExplicitOn,
                DeviceQuirk::NeedsHueSaturationNotCt,
            ])
        );
    }

    #[test]
    fn connect_matter_uses_persisted_records_without_probing_and_starts_subscription_stream() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RHYTHM_MATTER_PROFILE_SYNC", "disabled");
        let state = shared_state("connect");
        let key = HubKey::new(HubType::new("matter"), "local");
        state
            .lock()
            .unwrap()
            .hub_credentials
            .insert(key.clone(), matter_credentials("local", "fabric-test"));

        let transport = Arc::new(FakeMatterTransport::new(
            Vec::new(),
            vec![commissioned_device(10, 2), h6004_device(107)],
        ));

        let (hub, event_rx) = connect_matter(&state, transport.clone()).unwrap();

        assert_eq!(hub.hub_key, key);
        let data = hub.data::<Arc<MatterHubData>>().unwrap();
        assert_eq!(data.fabric_id, "fabric-test");
        assert_eq!(data.commissioned.lock().unwrap().len(), 2);
        assert!(data.device_caps.lock().unwrap().contains_key("matter-10-2"));
        assert!(data.device_caps.lock().unwrap().contains_key("matter-107"));
        assert_eq!(
            data.next_node_id.load(std::sync::atomic::Ordering::SeqCst),
            108
        );
        assert_eq!(
            data.device_quirks.lock().unwrap().get("matter-107"),
            Some(&vec![
                DeviceQuirk::NeedsExplicitOn,
                DeviceQuirk::NeedsHueSaturationNotCt,
                DeviceQuirk::CommandThrottleMs(250),
            ])
        );
        assert_eq!(transport.probe_calls.load(Ordering::SeqCst), 0);

        let event = event_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert_eq!(event.hub_key(), Some(&key));
        wait_for_atomic_at_least(&transport.subscribe_calls, 2);
        assert_eq!(transport.subscribe_calls.load(Ordering::SeqCst), 2);
        std::env::remove_var("RHYTHM_MATTER_PROFILE_SYNC");
    }

    #[test]
    fn observed_state_subscriptions_isolate_offline_endpoint_and_retry_recovery() {
        let transport = Arc::new(FakeMatterTransport::new(
            Vec::new(),
            vec![commissioned_device(103, 1), commissioned_device(110, 1)],
        ));
        transport.fail_subscription_attempts(103, 1, 1);
        let shutdown = Arc::new(AtomicBool::new(false));

        let refresh = start_observed_state_subscription_worker(transport.clone(), shutdown.clone());
        wait_for_atomic_at_least(&transport.subscribe_calls, 2);
        assert_eq!(
            transport.subscription_attempts.lock().unwrap().as_slice(),
            &[
                MatterSubscriptionTarget {
                    node_id: 103,
                    endpoint: 1,
                },
                MatterSubscriptionTarget {
                    node_id: 110,
                    endpoint: 1,
                },
            ],
            "the healthy endpoint must still be attempted after its peer fails"
        );

        refresh.try_send(()).unwrap();
        wait_for_atomic_at_least(&transport.subscribe_calls, 4);
        assert_eq!(
            transport.subscription_attempts.lock().unwrap().as_slice(),
            &[
                MatterSubscriptionTarget {
                    node_id: 103,
                    endpoint: 1,
                },
                MatterSubscriptionTarget {
                    node_id: 110,
                    endpoint: 1,
                },
                MatterSubscriptionTarget {
                    node_id: 103,
                    endpoint: 1,
                },
                MatterSubscriptionTarget {
                    node_id: 110,
                    endpoint: 1,
                },
            ],
            "the next refresh must retry the failed endpoint without losing healthy subscriptions"
        );

        shutdown.store(true, Ordering::SeqCst);
        let _ = refresh.try_send(());
    }

    #[test]
    fn controller_stream_identity_is_known_before_connected_is_emitted() {
        let transport = Arc::new(FakeMatterTransport::new(Vec::new(), Vec::new()));
        transport
            .block_initial_event_wait
            .store(true, Ordering::SeqCst);
        let shutdown = Arc::new(AtomicBool::new(false));
        let (event_tx, event_rx) = std::sync::mpsc::channel();

        start_controller_event_stream(
            transport.clone(),
            event_tx,
            shutdown.clone(),
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(Mutex::new(HashMap::new())),
        );

        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while transport.event_wait_calls.load(Ordering::SeqCst) == 0
            && std::time::Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        assert_eq!(transport.event_wait_calls.load(Ordering::SeqCst), 1);
        assert!(event_rx.recv_timeout(Duration::from_millis(50)).is_err());

        transport
            .release_initial_event_wait
            .store(true, Ordering::SeqCst);
        assert!(matches!(
            event_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            HubEvent::Connected { .. }
        ));
        shutdown.store(true, Ordering::SeqCst);
    }

    #[test]
    fn on_off_subscription_reports_update_periodic_observation_cache() {
        let observations = Mutex::new(HashMap::new());
        let report = crate::transport::MatterAttributeReport {
            node_id: 42,
            endpoint: 2,
            cluster: crate::clusters::CLUSTER_ON_OFF_U32,
            attr_id: crate::clusters::ATTR_ON_OFF_U32,
            value: MatterAttributeValue::Bool(true),
        };

        cache_on_off_observation(&report, &observations);

        assert_eq!(
            observations
                .lock()
                .unwrap()
                .get(&(42, 2))
                .map(|(lights_on, _)| *lights_on),
            Some(true)
        );
    }

    #[test]
    fn controller_stream_reset_invalidates_subscription_observations() {
        let observations = Mutex::new(HashMap::from([(
            (42, 2),
            (true, std::time::Instant::now()),
        )]));

        invalidate_on_off_observations(&observations);

        assert!(observations.lock().unwrap().is_empty());
    }

    #[test]
    fn controller_event_retry_is_bounded_and_shutdown_aware() {
        let mut delay = MATTER_EVENT_RETRY_INITIAL;
        assert_eq!(delay, Duration::from_millis(250));
        delay = next_controller_event_retry_delay(delay);
        assert_eq!(delay, Duration::from_millis(500));
        delay = next_controller_event_retry_delay(delay);
        assert_eq!(delay, Duration::from_secs(1));
        for _ in 0..8 {
            delay = next_controller_event_retry_delay(delay);
        }
        assert_eq!(delay, MATTER_EVENT_RETRY_MAX);

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_for_thread = shutdown.clone();
        let setter = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            shutdown_for_thread.store(true, Ordering::SeqCst);
        });
        assert!(wait_for_controller_event_retry(
            shutdown.as_ref(),
            MATTER_EVENT_RETRY_INITIAL
        ));
        setter.join().unwrap();
    }

    #[test]
    fn connect_matter_falls_back_to_basic_list_without_probing_old_transports() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RHYTHM_MATTER_PROFILE_SYNC", "disabled");
        let state = shared_state("connect-old-transport");
        let transport = Arc::new(FakeMatterTransport::new(
            vec![h6004_device_info(107)],
            Vec::new(),
        ));

        let (hub, event_rx) = connect_matter(&state, transport.clone()).unwrap();
        let data = hub.data::<Arc<MatterHubData>>().unwrap();
        assert_eq!(
            data.device_quirks.lock().unwrap().get("matter-107"),
            Some(&vec![
                DeviceQuirk::NeedsExplicitOn,
                DeviceQuirk::NeedsHueSaturationNotCt,
                DeviceQuirk::CommandThrottleMs(250),
            ])
        );
        assert_eq!(transport.probe_calls.load(Ordering::SeqCst), 0);

        let event = event_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert_eq!(event.hub_key(), Some(&hub.hub_key));
        wait_for_atomic_at_least(&transport.subscribe_calls, 1);
        assert_eq!(transport.subscribe_calls.load(Ordering::SeqCst), 1);
        std::env::remove_var("RHYTHM_MATTER_PROFILE_SYNC");
    }
}
