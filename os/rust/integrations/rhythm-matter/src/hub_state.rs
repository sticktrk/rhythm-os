//! Matter hub-specific state stored in `ActiveHub::hub_data`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rhythm_devices::{DeviceQuirk, LightCapabilities};
use rhythm_os::hub::HubEvent;

use crate::cloud_profiles::CloudMatterProfileCatalog;
use crate::controller::MatterDeviceRegistry;
use crate::transport::MatterTransport;
use crate::transport::{CommissionedDevice, MatterAttributeReport, MatterDeviceInfo};

const DECOMMISSION_SUPPRESSION_WINDOW: Duration = Duration::from_secs(120);
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
    /// Event channel sender kept alive by the hub data.
    pub event_tx: std::sync::mpsc::Sender<HubEvent>,
}

impl MatterHubData {
    /// Reserve the next node ID.
    pub fn reserve_node_id(&self) -> u64 {
        self.next_node_id.fetch_add(1, Ordering::SeqCst)
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
        if let Ok(mut needs_audition) = self.needs_audition.lock() {
            needs_audition.retain(|(observed_node_id, _)| *observed_node_id != node_id);
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
        self.record_node_proof_of_life(node_id);
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
            device_quirks: Mutex::new(HashMap::new()),
            device_profiles: Mutex::new(HashMap::new()),
            pending_turn_on_plans: Arc::new(Mutex::new(HashMap::new())),
            needs_audition: Arc::new(Mutex::new(HashSet::new())),
            local_overrides: Mutex::new(crate::local_quirks::LocalMatterOverrides::default()),
            cloud_profiles: Mutex::new(CloudMatterProfileCatalog::default()),
            decommissioning: Mutex::new(HashSet::new()),
            recently_decommissioned: Mutex::new(HashMap::new()),
            node_proof_of_life: Arc::new(Mutex::new(HashMap::new())),
            on_off_observations: Arc::new(Mutex::new(HashMap::new())),
            attribute_report_history: Arc::new(Mutex::new(VecDeque::new())),
            event_tx,
        }
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
}
