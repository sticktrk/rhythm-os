//! HA hub data.
//!
//! Platform-agnostic state for a connected Home Assistant instance,
//! holding the device registry and connection info. Stored as
//! `Box<dyn Any>` in `ActiveHub`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::registry::HaDeviceRegistry;
use crate::transport::HaConnectionConfig;

/// HA-specific runtime data stored in `ActiveHub::hub_data`.
pub struct HaHubData {
    /// HA connection configuration.
    pub config: HaConnectionConfig,
    /// Shared device registry for room lookups and event routing.
    pub registry: Arc<Mutex<HaDeviceRegistry>>,
    /// Shared cache for on-demand event discovery, mapping IDs to area_ids.
    ///
    /// Populated after area sync with two kinds of entries:
    /// - HA device_id → area_id (for `hue_event` button auto-registration)
    /// - entity_id → area_id (for `state_changed` motion sensor auto-registration)
    ///
    /// No collision risk: device IDs are UUIDs, entity IDs are `domain.name`.
    pub device_area_cache: Arc<Mutex<HashMap<String, String>>>,
}
