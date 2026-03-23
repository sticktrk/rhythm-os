//! Hue lifecycle for embedded targets (ESP32, etc.).
//!
//! Provides the same lifecycle pattern as `reqwest_lifecycle` but parameterized
//! over transport and SSE implementations. Platform crates provide:
//! - A `HueTransport` impl (e.g., ESP-IDF HTTP client)
//! - An SSE event stream factory (closure that spawns the SSE reader)
//!
//! This module provides: `connect_and_start`, `ensure_runtime`, `connect_hue_sse`.

use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use log::{info, warn};

use crate::hub_state::HueHubData;
use crate::hue_lifecycle::HueSseConnectConfig;
use crate::registry::{HueDeviceRegistry, HueRegistrySnapshot};
use crate::transport::HueTransport;

use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::{ActiveHub, HubEvent, HubType};
use rhythm_os::state::SharedState;

// ============================================================================
// Boot-time connection
// ============================================================================

/// Connect to the Hue bridge and optionally start the runtime.
///
/// Full boot-time lifecycle:
/// 1. Load registry snapshot from storage
/// 2. Connect SSE via platform closure (with discovery support)
/// 3. Store hub in state
/// 4. Room sync (rooms only, no full device discovery) — populates device_rooms
///    with all room children so on-demand button discovery can map devices to rooms
/// 5. Start runtime if rooms exist
///
/// Generic over transport factory and SSE event stream factory.
/// The transport factory is called twice if rooms exist: once for discovery
/// and once for the runtime's light controller.
pub fn connect_and_start<T, F, G>(
    state: SharedState,
    create_transport: G,
    start_event_stream: F,
) -> Result<Receiver<HubEvent>>
where
    T: HueTransport + 'static,
    F: FnOnce(
        HueSseConnectConfig,
        Arc<Mutex<HueDeviceRegistry>>,
        Arc<AtomicBool>,
    ) -> Receiver<HubEvent>,
    G: Fn(&str) -> Result<T>,
{
    let (hub, event_rx) =
        connect_hue_sse_with_discovery(&state, |ip| create_transport(ip), start_event_stream)?;

    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let key = hub.hub_key.clone();
        s.hubs.insert(key, hub);
    }

    // Room sync: fetch rooms from the bridge to populate device_rooms with all
    // room children (lights + switches). The NVS snapshot only persists button-
    // owning device mappings, so without this, undiscovered switches wouldn't
    // have a device→room entry for on-demand button discovery.
    // Devices are NOT fetched (false) — too large for ESP32 memory.
    let discover_devices = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.platform.full_device_discovery
    };
    if let Err(e) = rhythm_os::room_sync::sync_from_hub_with_options(&state, discover_devices) {
        warn!(target: "sys", "Room sync at boot failed: {}", e);
    }

    // Start runtime if rooms exist (room sync may have already created it
    // via do_room_set, but ensure_runtime is a no-op if already running)
    let has_rooms = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hubs
            .values()
            .find_map(|h| h.data::<HueHubData>())
            .and_then(|hue| hue.registry.lock().ok().map(|r| r.has_rooms()))
            .unwrap_or(false)
    };

    if has_rooms {
        // Check if runtime was already created by room sync
        let needs_runtime = {
            let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            !s.hubs.values().any(|h| h.runtime.is_some())
        };
        if needs_runtime {
            info!(target: "sys", "Rooms found, starting runtime...");
            let bridge_ip = {
                let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
                s.first_hub_credentials()
                    .map(|c| c.address.clone())
                    .unwrap_or_default()
            };
            let transport = create_transport(&bridge_ip)?;
            if let Err(e) = ensure_runtime(&state, transport) {
                warn!(target: "sys", "Failed to start runtime: {}", e);
            }
        }
    }

    Ok(event_rx)
}

// ============================================================================
// Runtime creation
// ============================================================================

/// Create the RhythmRuntime for the Hue hub with a platform-provided transport.
pub fn ensure_runtime<T: HueTransport + 'static>(state: &SharedState, transport: T) -> Result<()> {
    let scheduler_stack = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.platform.scheduler_stack
    };
    crate::hue_lifecycle::ensure_hue_runtime(state, transport, scheduler_stack)
}

// ============================================================================
// SSE connection (for use with configure_hue_hub)
// ============================================================================

/// Connect to the Hue bridge SSE stream, loading the registry snapshot from storage.
///
/// Returns `(ActiveHub, Receiver<HubEvent>)` — suitable for use as the
/// `connect_fn` closure in `provider::configure_hue_hub`.
pub fn connect_hue_sse<F>(
    state: &SharedState,
    start_event_stream: F,
) -> Result<(ActiveHub, Receiver<HubEvent>)>
where
    F: FnOnce(
        HueSseConnectConfig,
        Arc<Mutex<HueDeviceRegistry>>,
        Arc<AtomicBool>,
    ) -> Receiver<HubEvent>,
{
    let (hub_key, snapshot) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let key = s
            .first_hub_credentials()
            .and_then(|c| c.hub_key())
            .unwrap_or_else(|| HubKey::new(HubType::new(HubType::HUE), "unknown"));
        let snap = s
            .storage
            .as_ref()
            .and_then(|st| st.load_hub_registry_for(&key).ok().flatten())
            .and_then(|v| serde_json::from_value::<HueRegistrySnapshot>(v).ok());
        (key, snap)
    };

    crate::hue_lifecycle::connect_hue_sse(state, hub_key, snapshot, start_event_stream)
}

/// Connect to the Hue bridge SSE stream with discovery support.
///
/// Like `connect_hue_sse` but also creates a `HueDiscovery` using a
/// platform-provided transport factory, and attaches it to the hub.
pub fn connect_hue_sse_with_discovery<T, F>(
    state: &SharedState,
    create_transport: impl FnOnce(&str) -> Result<T>,
    start_event_stream: F,
) -> Result<(ActiveHub, Receiver<HubEvent>)>
where
    T: HueTransport + 'static,
    F: FnOnce(
        HueSseConnectConfig,
        Arc<Mutex<HueDeviceRegistry>>,
        Arc<AtomicBool>,
    ) -> Receiver<HubEvent>,
{
    // connect_hue_sse internally derives the HubKey from credentials
    let (mut hub, event_rx) = connect_hue_sse(state, start_event_stream)?;

    // Attach discovery if we can create a transport
    if let Some(hue_data) = hub.data::<HueHubData>() {
        let bridge_ip = hue_data.bridge_ip.clone();
        let username = hue_data.username.clone();
        match create_transport(&bridge_ip) {
            Ok(transport) => {
                let discovery = crate::discovery::HueDiscovery::new(Arc::new(transport), username);
                hub.discovery = Some(Arc::new(discovery));
            }
            Err(e) => {
                warn!(target: "sys", "Failed to create discovery transport: {}", e);
            }
        }
    }

    Ok((hub, event_rx))
}
