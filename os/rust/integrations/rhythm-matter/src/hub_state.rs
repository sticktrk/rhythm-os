//! Matter hub-specific state stored in `ActiveHub::hub_data`.

use std::sync::{Arc, Mutex};

use crate::controller::MatterDeviceRegistry;
use crate::transport::MatterDeviceInfo;

/// Matter-specific state stored in `ActiveHub::hub_data`.
///
/// Downcast via `active_hub.data::<MatterHubData>()`.
pub struct MatterHubData {
    /// Device registry (shared with controller).
    pub registry: Arc<Mutex<MatterDeviceRegistry>>,
    /// Matter fabric identifier.
    pub fabric_id: String,
    /// Currently commissioned devices.
    pub commissioned: Vec<MatterDeviceInfo>,
}
