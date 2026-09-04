//! Matter hub-specific state stored in `ActiveHub::hub_data`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rhythm_devices::{DeviceQuirk, LightCapabilities};
use rhythm_os::hub::{DeviceReachabilityEvidence, DeviceReachabilityFailureClass, HubEvent};

use crate::cloud_profiles::CloudMatterProfileCatalog;
use crate::controller::MatterDeviceRegistry;
use crate::transport::MatterTransport;
use crate::transport::{
    CommissionedDevice, MatterAttributeReport, MatterAttributeValue, MatterDeviceInfo,
    MatterEndpointCommandPlan,
};

const DECOMMISSION_SUPPRESSION_WINDOW: Duration = Duration::from_secs(120);

#[derive(Debug)]
struct ScheduledReadback {
    due_at: Instant,
    plan: MatterEndpointCommandPlan,
}

/// One scheduler and worker for all delayed authoritative command readbacks.
pub(crate) struct MatterReadbackCoordinator {
    latest_command_ids: Arc<Mutex<HashMap<(u64, u16), u64>>>,
    sender: OnceLock<mpsc::Sender<ScheduledReadback>>,
    store_path: Option<PathBuf>,
}

impl MatterReadbackCoordinator {
    pub(crate) fn new(store_path: Option<PathBuf>) -> Self {
        Self {
            latest_command_ids: Arc::new(Mutex::new(HashMap::new())),
            sender: OnceLock::new(),
            store_path,
        }
    }

    pub(crate) fn record_submitted(&self, plans: &[MatterEndpointCommandPlan]) {
        if let Ok(mut latest) = self.latest_command_ids.lock() {
            for plan in plans {
                latest.insert((plan.node_id, plan.endpoint), plan.command_id);
            }
        }
    }

    pub(crate) fn schedule(
        &self,
        transport: Arc<dyn MatterTransport>,
        plan: MatterEndpointCommandPlan,
        needs_audition: Arc<Mutex<HashSet<(u64, u16)>>>,
        delay: Duration,
    ) {
        let latest_command_ids = self.latest_command_ids.clone();
        let store_path = self.store_path.clone();
        let sender = self.sender.get_or_init(|| {
            let (sender, receiver) = mpsc::channel();
            let spawn_result = std::thread::Builder::new()
                .name("matter-readback".to_string())
                .spawn(move || {
                    readback_worker(
                        receiver,
                        transport,
                        latest_command_ids,
                        needs_audition,
                        store_path,
                    )
                });
            if let Err(error) = spawn_result {
                log::warn!(target: "cmd", "Failed to start Matter readback worker: {error}");
            }
            sender
        });
        if sender
            .send(ScheduledReadback {
                due_at: Instant::now() + delay,
                plan,
            })
            .is_err()
        {
            log::warn!(target: "cmd", "Matter readback worker is unavailable");
        }
    }

    fn store_path(&self) -> Option<&PathBuf> {
        self.store_path.as_ref()
    }
}

impl Default for MatterReadbackCoordinator {
    fn default() -> Self {
        Self::new(None)
    }
}

fn readback_worker(
    receiver: mpsc::Receiver<ScheduledReadback>,
    transport: Arc<dyn MatterTransport>,
    latest_command_ids: Arc<Mutex<HashMap<(u64, u16), u64>>>,
    needs_audition: Arc<Mutex<HashSet<(u64, u16)>>>,
    store_path: Option<PathBuf>,
) {
    let mut scheduled = Vec::<ScheduledReadback>::new();
    loop {
        if scheduled.is_empty() {
            let Ok(request) = receiver.recv() else {
                return;
            };
            scheduled.push(request);
        }
        scheduled.sort_by_key(|request| request.due_at);
        let wait = scheduled[0]
            .due_at
            .saturating_duration_since(Instant::now());
        match receiver.recv_timeout(wait) {
            Ok(request) => {
                scheduled.push(request);
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }

        let request = scheduled.remove(0);
        let key = (request.plan.node_id, request.plan.endpoint);
        if !is_latest_command(&latest_command_ids, key, request.plan.command_id) {
            continue;
        }
        let Ok(reported) = transport.read_light_state(request.plan.node_id, request.plan.endpoint)
        else {
            continue;
        };
        if !is_latest_command(&latest_command_ids, key, request.plan.command_id) {
            continue;
        }
        let mismatch = crate::controller::turn_on_readback_mismatch(&request.plan, &reported);
        if let Some(mismatch_field) = mismatch {
            tracing::warn!(
                target: "cmd",
                event = "matter_command_ack_without_effect",
                node_id = request.plan.node_id,
                endpoint = request.plan.endpoint,
                command_id = request.plan.command_id,
                mismatch_field,
                "Matter command completed but authoritative readback did not show the requested effect"
            );
        }
        persist_needs_audition_change(
            &needs_audition,
            store_path.as_ref(),
            key,
            mismatch.is_some(),
        );
    }
}

fn is_latest_command(
    latest_command_ids: &Mutex<HashMap<(u64, u16), u64>>,
    key: (u64, u16),
    command_id: u64,
) -> bool {
    latest_command_ids
        .lock()
        .ok()
        .and_then(|latest| latest.get(&key).copied())
        == Some(command_id)
}

fn persist_needs_audition_change(
    needs_audition: &Mutex<HashSet<(u64, u16)>>,
    store_path: Option<&PathBuf>,
    key: (u64, u16),
    value: bool,
) {
    let changed = needs_audition
        .lock()
        .map(|mut devices| {
            if value {
                devices.insert(key)
            } else {
                devices.remove(&key)
            }
        })
        .unwrap_or(false);
    if !changed {
        return;
    }
    if let Some(path) = store_path {
        let device_id = crate::lifecycle::format_device_id(key.0, key.1);
        if let Err(error) =
            crate::local_quirks::save_needs_audition_at_path(path, &device_id, value)
        {
            log::warn!(
                target: "cmd",
                "Failed to persist Matter needs-audition state for {device_id}: {error:#}"
            );
        }
    }
}

/// Matter-specific state stored in `ActiveHub::hub_data`.
pub struct MatterHubData {
    /// Shared transport for all controller, commissioning, and probe paths.
    pub transport: std::sync::OnceLock<Arc<dyn MatterTransport>>,
    /// Optional directory for raw probe captures.
    pub capture_dir: std::sync::OnceLock<String>,
    /// Device registry (shared with controller).
    pub registry: Arc<Mutex<MatterDeviceRegistry>>,
    /// Matter fabric identifier.
    pub fabric_id: String,
    /// Currently commissioned devices.
    pub commissioned: Mutex<Vec<MatterDeviceInfo>>,
    /// Next monotonic local node ID to assign.
    pub next_node_id: AtomicU64,
    /// Per-device capabilities keyed by device ID (for example `matter-42`).
    pub device_caps: Mutex<HashMap<String, LightCapabilities>>,
    /// Device IDs whose cached capabilities are a fallback rather than a probe result.
    pub fallback_caps: Mutex<HashSet<String>>,
    /// Per-device Matter quirks from `rhythm-devices`.
    pub device_quirks: Mutex<HashMap<String, Vec<DeviceQuirk>>>,
    /// Per-device effective typed control profiles consumed by runtime plans.
    pub device_profiles: Mutex<HashMap<String, crate::control_profile::MatterControlProfile>>,
    /// Runtime turn-on plans awaiting terminal acknowledgement and readback.
    pub pending_turn_on_plans:
        Arc<Mutex<HashMap<u64, crate::transport::MatterEndpointCommandPlan>>>,
    /// Endpoints whose successful command acknowledgement did not produce the
    /// intended reported state. The app reads this through Audition status.
    pub needs_audition: Arc<Mutex<HashSet<(u64, u16)>>>,
    /// Shared latest-command tracker and single delayed readback worker.
    pub(crate) readback: Arc<MatterReadbackCoordinator>,
    /// Local audition inputs cached for startup, pairing, discovery, and
    /// on-demand probes so all metadata paths use one resolver.
    pub local_overrides: Mutex<crate::local_quirks::LocalMatterOverrides>,
    /// Approved cloud profile overlay catalog cached at hub startup.
    pub cloud_profiles: Mutex<CloudMatterProfileCatalog>,
    /// Matter nodes currently being decommissioned.
    pub decommissioning: Mutex<HashSet<u64>>,
    /// Matter nodes recently decommissioned and temporarily ignored by sync.
    pub recently_decommissioned: Mutex<HashMap<u64, Instant>>,
    /// Last time each node produced any successful Matter interaction.
    pub node_proof_of_life: Arc<Mutex<HashMap<u64, Instant>>>,
    /// Latest authoritative On/Off subscription report by node and endpoint.
    pub on_off_observations: Arc<Mutex<HashMap<(u64, u16), (bool, Instant)>>>,
    /// Recent reports from the controller event stream. Audition reads this
    /// shared history instead of racing chipd's native report drain.
    pub attribute_report_history: Arc<Mutex<VecDeque<MatterAttributeReport>>>,
    /// Last time Rhythm dispatched a turn-on plan by node and endpoint.
    pub last_turn_on_dispatch: Mutex<HashMap<(u64, u16), Instant>>,
    /// Event channel sender kept alive by the hub data.
    pub event_tx: std::sync::mpsc::Sender<HubEvent>,
}

impl MatterHubData {
    /// Reserve the next node ID.
    pub fn reserve_node_id(&self) -> u64 {
        self.next_node_id.fetch_add(1, Ordering::SeqCst)
    }

    pub(crate) fn set_needs_audition(&self, node_id: u64, endpoint: u16, value: bool) {
        persist_needs_audition_change(
            self.needs_audition.as_ref(),
            self.readback.store_path(),
            (node_id, endpoint),
            value,
        );
    }

    pub(crate) fn observed_current_level(&self, node_id: u64, endpoint: u16) -> Option<u8> {
        self.attribute_report_history
            .lock()
            .ok()?
            .iter()
            .rev()
            .find_map(|report| {
                (report.node_id == node_id
                    && report.endpoint == endpoint
                    && report.cluster == 0x0008
                    && report.attr_id == 0x0000)
                    .then(|| match &report.value {
                        MatterAttributeValue::U8(value) => Some(*value),
                        _ => None,
                    })
                    .flatten()
            })
    }

    /// Upsert a newly commissioned device into the in-memory fabric cache.
    pub fn record_commissioned_device(&self, device: &CommissionedDevice) {
        self.clear_decommission_suppression(device.node_id);

        let info = MatterDeviceInfo {
            node_id: device.node_id,
            vendor_name: device.vendor_name.clone(),
            product_name: device.product_name.clone(),
            reachable: true,
        };

        if let Ok(mut list) = self.commissioned.lock() {
            if let Some(existing) = list.iter_mut().find(|d| d.node_id == device.node_id) {
                *existing = info;
            } else {
                list.push(info);
            }
        }

        let next_after_device = device.node_id.saturating_add(1);
        let mut current = self.next_node_id.load(Ordering::SeqCst);
        while next_after_device > current {
            match self.next_node_id.compare_exchange(
                current,
                next_after_device,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
    }

    /// Remove a device from the commissioned list and capabilities cache.
    pub fn remove_device(&self, node_id: u64) {
        if let Ok(mut list) = self.commissioned.lock() {
            list.retain(|device| device.node_id != node_id);
        }

        let prefix = format!("matter-{}", node_id);
        if let Ok(mut caps) = self.device_caps.lock() {
            caps.retain(|key, _| key != &prefix && !key.starts_with(&format!("{}-", prefix)));
        }
        if let Ok(mut quirks) = self.device_quirks.lock() {
            quirks.retain(|key, _| key != &prefix && !key.starts_with(&format!("{}-", prefix)));
        }
        if let Ok(mut profiles) = self.device_profiles.lock() {
            profiles.retain(|key, _| key != &prefix && !key.starts_with(&format!("{}-", prefix)));
        }
        if let Ok(mut pending) = self.pending_turn_on_plans.lock() {
            pending.retain(|_, plan| plan.node_id != node_id);
        }
        let audition_endpoints = self
            .needs_audition
            .lock()
            .map(|needs_audition| {
                needs_audition
                    .iter()
                    .filter_map(|(observed_node_id, endpoint)| {
                        (*observed_node_id == node_id).then_some(*endpoint)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for endpoint in audition_endpoints {
            self.set_needs_audition(node_id, endpoint, false);
        }
        if let Ok(mut observations) = self.on_off_observations.lock() {
            observations.retain(|(observed_node_id, _), _| *observed_node_id != node_id);
        }
        if let Ok(mut reports) = self.attribute_report_history.lock() {
            reports.retain(|report| report.node_id != node_id);
        }
    }

    /// Mark whether a cached Matter node is currently reachable.
    pub fn mark_node_reachable(&self, node_id: u64, reachable: bool) {
        if let Ok(mut list) = self.commissioned.lock() {
            if let Some(device) = list.iter_mut().find(|device| device.node_id == node_id) {
                device.reachable = reachable;
            }
        }
    }

    /// Record that a node responded or emitted a live report.
    pub fn record_node_proof_of_life(&self, node_id: u64) {
        if let Ok(mut proof) = self.node_proof_of_life.lock() {
            proof.insert(node_id, Instant::now());
        }
        self.mark_node_reachable(node_id, true);
    }

    /// Record endpoint-scoped physical proof and publish it to the shared
    /// durable health tracker. Command intent never calls this method.
    pub fn record_endpoint_proof_of_life(&self, node_id: u64, endpoint: u16) {
        self.record_node_proof_of_life(node_id);
        let _ = self.event_tx.send(HubEvent::DeviceReachability {
            hub_key: None,
            device_id: crate::lifecycle::format_device_id(node_id, endpoint),
            fabric_id: self.fabric_id.clone(),
            controller_stream_id: None,
            evidence: DeviceReachabilityEvidence::Proof,
        });
    }

    /// Publish a bounded connectivity failure class without retaining the raw
    /// transport error, address, or fabric credentials.
    pub fn record_endpoint_failure(
        &self,
        node_id: u64,
        endpoint: u16,
        class: DeviceReachabilityFailureClass,
    ) {
        let _ = self.event_tx.send(HubEvent::DeviceReachability {
            hub_key: None,
            device_id: crate::lifecycle::format_device_id(node_id, endpoint),
            fabric_id: self.fabric_id.clone(),
            controller_stream_id: None,
            evidence: DeviceReachabilityEvidence::Failure(class),
        });
    }

    /// Whether a node has proven alive after a specific failure/cooldown mark.
    pub fn has_node_proof_of_life_after(&self, node_id: u64, marked_at: Instant) -> bool {
        self.node_proof_of_life
            .lock()
            .ok()
            .and_then(|proof| proof.get(&node_id).copied())
            .is_some_and(|proof_at| proof_at > marked_at)
    }

    /// Record an authoritative On/Off subscription report.
    pub fn record_on_off_observation(&self, node_id: u64, endpoint: u16, lights_on: bool) {
        if let Ok(mut observations) = self.on_off_observations.lock() {
            observations.insert((node_id, endpoint), (lights_on, Instant::now()));
        }
        self.record_endpoint_proof_of_life(node_id, endpoint);
    }

    /// Return the authoritative On/Off value from the current controller
    /// stream for periodic work.
    ///
    /// Matter may send an empty ReportData at the negotiated maximum interval
    /// when the subscribed value has not changed. The CHIP stack uses that
    /// report to refresh subscription liveness, but it does not invoke the
    /// attribute callback. Consequently, elapsed wall time cannot make the
    /// last reported value stale. Lifecycle code clears these observations
    /// when controller stream continuity is lost.
    pub fn observed_on_off(&self, node_id: u64, endpoint: u16) -> Option<bool> {
        self.on_off_observations
            .lock()
            .ok()
            .and_then(|observations| observations.get(&(node_id, endpoint)).copied())
            .map(|(lights_on, _)| lights_on)
    }

    /// Latest On/Off observation with the instant it was received.
    pub fn observed_on_off_at(&self, node_id: u64, endpoint: u16) -> Option<(bool, Instant)> {
        self.on_off_observations
            .lock()
            .ok()
            .and_then(|observations| observations.get(&(node_id, endpoint)).copied())
    }

    /// Remember that Rhythm dispatched a turn-on to this endpoint, so an
    /// off observation received before it is no longer trusted for plan
    /// shaping until the subscription reports again.
    pub fn record_turn_on_dispatch(&self, node_id: u64, endpoint: u16) {
        if let Ok(mut sent) = self.last_turn_on_dispatch.lock() {
            sent.insert((node_id, endpoint), Instant::now());
        }
    }

    /// True when the newest observation says off and nothing we sent since
    /// could have turned the light on.
    pub fn observed_off_since_last_turn_on(&self, node_id: u64, endpoint: u16) -> bool {
        let Some((false, seen_at)) = self.observed_on_off_at(node_id, endpoint) else {
            return false;
        };
        let last_sent = self
            .last_turn_on_dispatch
            .lock()
            .ok()
            .and_then(|sent| sent.get(&(node_id, endpoint)).copied());
        last_sent.is_none_or(|sent| seen_at > sent)
    }

    /// Upsert basic cached Matter device info without treating it as a live
    /// probe success.
    pub fn record_device_info(&self, info: &MatterDeviceInfo, reachable: bool) {
        let mut updated = info.clone();
        updated.reachable = reachable;

        if let Ok(mut list) = self.commissioned.lock() {
            if let Some(existing) = list
                .iter_mut()
                .find(|device| device.node_id == info.node_id)
            {
                *existing = updated;
            } else {
                list.push(updated);
            }
        }
    }

    /// Mark a Matter node as actively decommissioning.
    ///
    /// Returns `false` if another unpair request is already in progress for the
    /// same node.
    pub fn begin_decommission(&self, node_id: u64) -> bool {
        let Ok(mut decommissioning) = self.decommissioning.lock() else {
            return true;
        };
        decommissioning.insert(node_id)
    }

    /// Finish a Matter node decommission attempt.
    pub fn finish_decommission(&self, node_id: u64, succeeded: bool) {
        if let Ok(mut decommissioning) = self.decommissioning.lock() {
            decommissioning.remove(&node_id);
        }

        if succeeded {
            self.mark_recently_decommissioned(node_id);
            self.remove_device(node_id);
        } else if let Ok(mut recent) = self.recently_decommissioned.lock() {
            recent.remove(&node_id);
        }
    }

    /// Whether discovery should ignore a node because it is being removed.
    pub fn is_decommission_suppressed(&self, node_id: u64) -> bool {
        if self
            .decommissioning
            .lock()
            .map(|nodes| nodes.contains(&node_id))
            .unwrap_or(false)
        {
            return true;
        }

        self.is_recently_decommissioned(node_id)
    }

    /// Whether the node was successfully decommissioned recently.
    pub fn is_recently_decommissioned(&self, node_id: u64) -> bool {
        let Ok(mut recent) = self.recently_decommissioned.lock() else {
            return false;
        };
        Self::prune_recent_decommissions(&mut recent);
        recent.contains_key(&node_id)
    }

    fn mark_recently_decommissioned(&self, node_id: u64) {
        if let Ok(mut recent) = self.recently_decommissioned.lock() {
            Self::prune_recent_decommissions(&mut recent);
            recent.insert(node_id, Instant::now());
        }
    }

    fn clear_decommission_suppression(&self, node_id: u64) {
        if let Ok(mut decommissioning) = self.decommissioning.lock() {
            decommissioning.remove(&node_id);
        }
        if let Ok(mut recent) = self.recently_decommissioned.lock() {
            recent.remove(&node_id);
        }
    }

    fn prune_recent_decommissions(recent: &mut HashMap<u64, Instant>) {
        let now = Instant::now();
        recent.retain(|_, marked_at| {
            now.duration_since(*marked_at) < DECOMMISSION_SUPPRESSION_WINDOW
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::SpyTransport;
    use crate::transport::MatterCommandStep;

    fn commissioned_device(
        node_id: u64,
        vendor_name: &str,
        product_name: &str,
    ) -> CommissionedDevice {
        CommissionedDevice {
            node_id,
            vendor_name: vendor_name.to_string(),
            product_name: product_name.to_string(),
            vendor_id: 1,
            product_id: 2,
            serial_number: None,
            light_endpoint: 1,
            color_modes: Vec::new(),
            min_kelvin: None,
            max_kelvin: None,
        }
    }

    fn hub_data() -> MatterHubData {
        let (event_tx, _event_rx) = std::sync::mpsc::channel();
        MatterHubData {
            transport: std::sync::OnceLock::new(),
            capture_dir: std::sync::OnceLock::new(),
            registry: Arc::new(Mutex::new(MatterDeviceRegistry::new())),
            fabric_id: "default".to_string(),
            commissioned: Mutex::new(Vec::new()),
            next_node_id: AtomicU64::new(100),
            device_caps: Mutex::new(HashMap::new()),
            fallback_caps: Mutex::new(HashSet::new()),
            device_quirks: Mutex::new(HashMap::new()),
            device_profiles: Mutex::new(HashMap::new()),
            pending_turn_on_plans: Arc::new(Mutex::new(HashMap::new())),
            needs_audition: Arc::new(Mutex::new(HashSet::new())),
            readback: Arc::new(MatterReadbackCoordinator::default()),
            local_overrides: Mutex::new(crate::local_quirks::LocalMatterOverrides::default()),
            cloud_profiles: Mutex::new(CloudMatterProfileCatalog::default()),
            decommissioning: Mutex::new(HashSet::new()),
            recently_decommissioned: Mutex::new(HashMap::new()),
            node_proof_of_life: Arc::new(Mutex::new(HashMap::new())),
            on_off_observations: Arc::new(Mutex::new(HashMap::new())),
            attribute_report_history: Arc::new(Mutex::new(VecDeque::new())),
            last_turn_on_dispatch: Mutex::new(HashMap::new()),
            event_tx,
        }
    }

    fn level_plan(command_id: u64, level: u8) -> MatterEndpointCommandPlan {
        MatterEndpointCommandPlan {
            command_id,
            node_id: 42,
            endpoint: 1,
            steps: vec![MatterCommandStep::SetBrightness {
                level,
                transition_ms: None,
            }],
            inter_step_delay_ms: None,
        }
    }

    fn wait_for_reads(transport: &SpyTransport, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while transport.light_state_read_count() < expected && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert_eq!(transport.light_state_read_count(), expected);
    }

    #[test]
    fn superseded_readback_ignores_the_older_completed_plan() {
        let coordinator = MatterReadbackCoordinator::default();
        let transport = Arc::new(SpyTransport::default());
        transport.set_light_state(
            42,
            1,
            serde_json::json!({
                "onoff": {"ok": true, "value": true},
                "current_level": {"ok": true, "value": 203}
            }),
        );
        let needs_audition = Arc::new(Mutex::new(HashSet::new()));
        let plan_a = level_plan(1, 76);
        let plan_b = level_plan(2, 203);

        coordinator.record_submitted(std::slice::from_ref(&plan_a));
        coordinator.schedule(
            transport.clone(),
            plan_a,
            needs_audition.clone(),
            Duration::from_millis(20),
        );
        coordinator.record_submitted(std::slice::from_ref(&plan_b));
        coordinator.schedule(
            transport.clone(),
            plan_b,
            needs_audition.clone(),
            Duration::ZERO,
        );

        wait_for_reads(transport.as_ref(), 1);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(transport.light_state_read_count(), 1);
        assert!(needs_audition.lock().unwrap().is_empty());
    }

    #[test]
    fn matching_readback_clears_needs_audition() {
        let coordinator = MatterReadbackCoordinator::default();
        let transport = Arc::new(SpyTransport::default());
        transport.set_light_state(
            42,
            1,
            serde_json::json!({
                "onoff": {"ok": true, "value": true},
                "current_level": {"ok": true, "value": 203}
            }),
        );
        let needs_audition = Arc::new(Mutex::new(HashSet::from([(42, 1)])));
        let plan = level_plan(2, 203);

        coordinator.record_submitted(std::slice::from_ref(&plan));
        coordinator.schedule(
            transport.clone(),
            plan,
            needs_audition.clone(),
            Duration::ZERO,
        );

        wait_for_reads(transport.as_ref(), 1);
        let deadline = Instant::now() + Duration::from_secs(1);
        while !needs_audition.lock().unwrap().is_empty() && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(needs_audition.lock().unwrap().is_empty());
    }

    #[test]
    fn reserve_node_id_starts_at_100() {
        let hub_data = hub_data();
        assert_eq!(hub_data.reserve_node_id(), 100);
        assert_eq!(hub_data.reserve_node_id(), 101);
    }

    #[test]
    fn record_commissioned_device_updates_cache_and_counter() {
        let hub_data = hub_data();

        hub_data.record_commissioned_device(&commissioned_device(100, "Vendor", "Lamp"));
        hub_data.record_commissioned_device(&commissioned_device(101, "Vendor", "Lamp 2"));

        let commissioned = hub_data.commissioned.lock().unwrap();
        assert_eq!(commissioned.len(), 2);
        assert_eq!(commissioned[0].node_id, 100);
        assert_eq!(commissioned[1].node_id, 101);
        drop(commissioned);

        hub_data.record_commissioned_device(&commissioned_device(100, "Updated", "Lamp"));

        let commissioned = hub_data.commissioned.lock().unwrap();
        assert_eq!(commissioned.len(), 2);
        assert_eq!(commissioned[0].vendor_name, "Updated");
        drop(commissioned);

        assert_eq!(hub_data.reserve_node_id(), 102);
    }

    #[test]
    fn decommission_suppression_tracks_inflight_and_recent_nodes() {
        let hub_data = hub_data();

        assert!(!hub_data.is_decommission_suppressed(101));
        assert!(hub_data.begin_decommission(101));
        assert!(!hub_data.begin_decommission(101));
        assert!(hub_data.is_decommission_suppressed(101));

        hub_data.finish_decommission(101, true);
        assert!(hub_data.is_decommission_suppressed(101));

        hub_data.record_commissioned_device(&commissioned_device(101, "Vendor", "Lamp"));
        assert!(!hub_data.is_decommission_suppressed(101));
    }

    #[test]
    fn observed_off_since_last_turn_on_tracks_observation_order() {
        let hub_data = hub_data();

        assert!(!hub_data.observed_off_since_last_turn_on(42, 1));

        hub_data.record_on_off_observation(42, 1, false);
        assert!(hub_data.observed_off_since_last_turn_on(42, 1));

        hub_data.record_turn_on_dispatch(42, 1);
        assert!(!hub_data.observed_off_since_last_turn_on(42, 1));

        hub_data.record_on_off_observation(42, 1, false);
        assert!(hub_data.observed_off_since_last_turn_on(42, 1));

        hub_data.record_on_off_observation(42, 1, true);
        assert!(!hub_data.observed_off_since_last_turn_on(42, 1));
    }
}
