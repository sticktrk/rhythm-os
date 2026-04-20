//! Hue hub data.
//!
//! Platform-agnostic state for a connected Hue bridge, holding the device
//! registry and connection info. Stored as `Box<dyn Any>` in `ActiveHub`.

use std::sync::{Arc, Mutex};

use crate::registry::HueDeviceRegistry;

/// Hue-specific runtime data stored in `ActiveHub::hub_data`.
///
/// Holds the device registry (for room names, grouped_light IDs)
/// and bridge connection info.
pub struct HueHubData {
    /// Bridge IP address.
    pub bridge_ip: String,
    /// Application key (username).
    pub username: String,
    /// Shared device registry for room lookups and SSE routing.
    pub registry: Arc<Mutex<HueDeviceRegistry>>,
}
