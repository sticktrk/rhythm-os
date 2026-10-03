//! HA hub data.
//!
//! Platform-agnostic state for a connected Home Assistant instance,
//! holding the device registry and connection info. Stored as
//! `Box<dyn Any>` in `ActiveHub`.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex};

use crate::registry::HaDeviceRegistry;
use crate::transport::HaConnectionConfig;

/// Discovery-backed routing metadata shared with the live event translator.
#[derive(Debug, Default)]
pub struct HaEventRoutingCache {
    /// Last complete light snapshot. Invalidation blocks output until discovery.
    pub lights: std::collections::BTreeMap<String, crate::light::HaLightCatalogEntry>,
    pub lights_ready: bool,
    pub stream_ready: bool,
    /// Prefer event entities when HA also emits a native Hue/ZHA event for the same control.
    pub button_event_device_ids: std::collections::HashSet<String>,
    pub generation: u64,
    pub observation_epoch: Arc<std::sync::atomic::AtomicU64>,
    pub snapshot_revision: String,
    pub ownership_revision: u64,
    pub own_contexts: std::collections::VecDeque<String>,
    pub writes_in_flight: HashMap<String, usize>,
    pub pending_manual: HashMap<String, (rhythm_os::hub::LightObservation, serde_json::Value, u64)>,
    pub deferred_tx: Option<std::sync::mpsc::SyncSender<rhythm_os::hub::HubEvent>>,
    pub reviewed: std::collections::BTreeMap<String, crate::light::HaLightIdentity>,
    /// HA device/entity ID → area ID for on-demand registration and admission.
    pub(crate) device_areas: HashMap<String, String>,
    /// Camera event entity ID → its sole sibling motion binary sensor ID.
    pub(crate) motion_subevents: HashMap<String, String>,
}

impl HaEventRoutingCache {
    pub fn invalidate(&mut self) {
        self.lights_ready = false;
        self.generation = self.generation.wrapping_add(1);
        self.observation_epoch
            .store(self.generation, std::sync::atomic::Ordering::Release);
        self.pending_manual.clear();
    }
}

impl Deref for HaEventRoutingCache {
    type Target = HashMap<String, String>;

    fn deref(&self) -> &Self::Target {
        &self.device_areas
    }
}

impl DerefMut for HaEventRoutingCache {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.device_areas
    }
}

/// HA-specific runtime data stored in `ActiveHub::hub_data`.
pub struct HaHubData {
    /// HA connection configuration.
    pub config: HaConnectionConfig,
    /// Shared device registry for room lookups and event routing.
    pub registry: Arc<Mutex<HaDeviceRegistry>>,
    /// Shared discovery metadata for on-demand registration and event routing.
    ///
    /// Populated after area sync with two kinds of entries:
    /// - HA device_id → area_id (for `hue_event` button auto-registration)
    /// - entity_id → area_id (for `state_changed` motion sensor auto-registration)
    ///
    /// No collision risk: device IDs are UUIDs, entity IDs are `domain.name`.
    pub event_routing_cache: Arc<Mutex<HaEventRoutingCache>>,
}
