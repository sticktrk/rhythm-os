//! Matter hub lifecycle — connect, disconnect, runtime creation.
//!
//! Thin wrappers around `rhythm_os::lifecycle` helpers with Matter-specific
//! configuration. The Matter fabric is the "hub" — `HubKey("matter", "local")`.

use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use log::info;
use rhythm_core::room::RoomSource;

use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::{ActiveHub, HubEvent, HubType};
use rhythm_os::registry::HubDeviceRegistry;
use rhythm_os::state::SharedState;

use crate::hub_state::MatterHubData;
use crate::transport::MatterTransport;

/// Connect to the local Matter fabric.
///
/// Creates an `ActiveHub` with a `MatterHubData` containing the device
/// registry and commissioned device list.
pub fn connect_matter<T: MatterTransport + 'static>(
    state: &SharedState,
    transport: Arc<T>,
) -> Result<(ActiveHub, Receiver<HubEvent>)> {
    let hub_key = HubKey::new(HubType::new("matter"), "local");

    // Discover commissioned devices
    let commissioned = transport.commissioned_devices().unwrap_or_default();
    info!(target: "sys", "Matter: {} commissioned devices", commissioned.len());

    // Load persisted registry snapshot if available
    let snapshot = {
        let s = state
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock state"))?;
        s.storage
            .as_ref()
            .and_then(|st| st.load_hub_registry_for(&hub_key).ok().flatten())
            .and_then(|v| {
                serde_json::from_value::<rhythm_os::registry::RegistrySnapshot>(v).ok()
            })
    };

    let commissioned_for_closure = commissioned.clone();

    // Create event channel — tx is held by MatterHubData (keeps channel alive),
    // rx goes to the event loop. Later, subscription handling can send real events.
    let (event_tx, event_rx) = std::sync::mpsc::channel();

    rhythm_os::lifecycle::connect_hub(
        state,
        HubType::new("matter"),
        hub_key,
        RoomSource::Other("matter".to_string()),
        true, // default_grouped_light_to_room_id: room_id IS the control target
        snapshot,
        // hub_data_builder: receives the registry Arc from connect_hub
        move |registry: Arc<Mutex<HubDeviceRegistry>>| -> Box<dyn std::any::Any + Send + Sync> {
            Box::new(Arc::new(MatterHubData {
                registry,
                fabric_id: "default".to_string(),
                commissioned: commissioned_for_closure,
                device_caps: std::sync::Mutex::new(HashMap::new()),
                event_tx,
            }))
        },
        // start_event_stream: return pre-created rx (tx lives in MatterHubData)
        move |_registry, _shutdown| event_rx,
    )
}

/// Format a Matter node ID as a device ID string.
///
/// Format: `"matter-{node_id}"` (endpoint appended if non-default).
pub fn format_device_id(node_id: u64, endpoint: u16) -> String {
    if endpoint == 1 {
        format!("matter-{}", node_id)
    } else {
        format!("matter-{}-{}", node_id, endpoint)
    }
}
