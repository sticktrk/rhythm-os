//! Matter hub-specific state stored in `ActiveHub::hub_data`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rhythm_devices::LightCapabilities;
use rhythm_os::hub::HubEvent;

use crate::controller::MatterDeviceRegistry;
#[cfg(feature = "desktop")]
use crate::desktop_transport::MatcTransport;
use crate::transport::MatterDeviceInfo;

/// Matter-specific state stored in `ActiveHub::hub_data`.
///
/// Downcast via `active_hub.data::<Arc<MatterHubData>>()`.
pub struct MatterHubData {
    /// Shared transport — single `DeviceManager` instance for the fabric.
    /// All code paths (commands, commissioning, probing) share this to avoid
    /// port 5555 conflicts from multiple DMs.
    /// Initialized via `OnceLock::set()` after `connect_matter()` returns.
    #[cfg(feature = "desktop")]
    pub transport: std::sync::OnceLock<Arc<MatcTransport>>,
    /// Device registry (shared with controller).
    pub registry: Arc<Mutex<MatterDeviceRegistry>>,
    /// Matter fabric identifier.
    pub fabric_id: String,
    /// Currently commissioned devices.
    pub commissioned: Mutex<Vec<MatterDeviceInfo>>,
    /// Per-device capabilities, keyed by device ID (e.g., "matter-42").
    /// Populated during commissioning via `capabilities_from_commissioned()`.
    pub device_caps: Mutex<HashMap<String, LightCapabilities>>,
    /// Event channel sender — keeps the channel alive for the event loop.
    /// Subscription handling can later use this to emit real device events.
    pub event_tx: std::sync::mpsc::Sender<HubEvent>,
}

impl MatterHubData {
    /// Remove a device from the commissioned list and capabilities cache.
    ///
    /// Called during decommission — protocol + integration-specific cleanup.
    /// Does NOT touch the hub device registry or canonical registry (that's
    /// the rhythm-os handler's responsibility).
    pub fn remove_device(&self, node_id: u64) {
        if let Ok(mut list) = self.commissioned.lock() {
            list.retain(|d| d.node_id != node_id);
        }
        let prefix = format!("matter-{}", node_id);
        if let Ok(mut caps) = self.device_caps.lock() {
            caps.retain(|k, _| k != &prefix && !k.starts_with(&format!("{}-", prefix)));
        }
    }
}
