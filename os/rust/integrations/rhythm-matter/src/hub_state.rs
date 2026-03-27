//! Matter hub-specific state stored in `ActiveHub::hub_data`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rhythm_devices::LightCapabilities;
use rhythm_os::hub::HubEvent;

use crate::controller::MatterDeviceRegistry;
use crate::transport::MatterDeviceInfo;

/// Matter-specific state stored in `ActiveHub::hub_data`.
///
/// Downcast via `active_hub.data::<Arc<MatterHubData>>()`.
pub struct MatterHubData {
    /// Device registry (shared with controller).
    pub registry: Arc<Mutex<MatterDeviceRegistry>>,
    /// Matter fabric identifier.
    pub fabric_id: String,
    /// Currently commissioned devices.
    pub commissioned: Vec<MatterDeviceInfo>,
    /// Per-device capabilities, keyed by device ID (e.g., "matter-42").
    /// Populated during commissioning via `capabilities_from_commissioned()`.
    pub device_caps: Mutex<HashMap<String, LightCapabilities>>,
    /// Event channel sender — keeps the channel alive for the event loop.
    /// Subscription handling can later use this to emit real device events.
    pub event_tx: std::sync::mpsc::Sender<HubEvent>,
}
