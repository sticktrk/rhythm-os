//! Hue hub lifecycle management (generic over transport).
//!
//! Platform-agnostic lifecycle functions for connecting to a Hue bridge
//! and creating the runtime. The actual SSE transport and concrete types
//! are provided by the platform crate via closures/callbacks.
//!
//! Connect, runtime creation, and configuration are delegated to
//! `rhythm_os::lifecycle`. Event translation lives in `events.rs`.

use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use log::info;

use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::{ActiveHub, HubEvent, HubType};
use rhythm_os::registry::{HubDeviceRegistry, RegistrySnapshot};
use rhythm_os::state::SharedState;

use crate::hub_state::HueHubData;

/// Configuration for connecting the Hue SSE event stream.
pub struct HueSseConnectConfig {
    pub bridge_ip: String,
    pub username: String,
}

/// Connect to the Hue bridge SSE event stream (generic over transport).
///
/// The `start_event_stream` closure is provided by the platform crate to
/// handle the actual SSE transport (for example a blocking TLS client or
/// reqwest on Linux).
///
/// Returns `(ActiveHub, Receiver<HubEvent>)`. The runtime inside ActiveHub
/// is `None` — call the platform's `ensure_runtime()` when the first room arrives.
pub fn connect_hue_sse<F>(
    state: &SharedState,
    hub_key: HubKey,
    load_registry_snapshot: Option<RegistrySnapshot>,
    start_event_stream: F,
) -> Result<(ActiveHub, Receiver<HubEvent>)>
where
    F: FnOnce(
        HueSseConnectConfig,
        Arc<Mutex<HubDeviceRegistry>>,
        Arc<AtomicBool>,
    ) -> Receiver<HubEvent>,
{
    let (bridge_ip, username) = {
        let s = state
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock state"))?;
        let creds = s
            .hub_credentials
            .get(&hub_key)
            .ok_or_else(|| anyhow::anyhow!("No hub credentials configured for {}", hub_key))?;
        let username = crate::provider::hue_username(creds)
            .ok_or_else(|| anyhow::anyhow!("Hue credentials not configured"))?
            .to_string();
        (creds.address.clone(), username)
    };

    info!(target: "sys", "Connecting Hue SSE for bridge {}...", bridge_ip);

    let bridge_ip_clone = bridge_ip.clone();
    let username_clone = username.clone();

    rhythm_os::lifecycle::connect_hub(
        state,
        HubType::new(HubType::HUE),
        hub_key,
        false, // Hue: grouped_light_id does NOT default to room_id
        load_registry_snapshot,
        // hub_data_builder: create HueHubData from registry
        move |registry| {
            Box::new(HueHubData {
                bridge_ip: bridge_ip_clone,
                username: username_clone,
                registry,
            })
        },
        // start_event_stream: wrap the platform closure with SSE config
        move |registry, shutdown| {
            let sse_config = HueSseConnectConfig {
                bridge_ip,
                username,
            };
            start_event_stream(sse_config, registry, shutdown)
        },
    )
}

/// Create the RhythmRuntime for a Hue hub, generic over transport.
///
/// Reads credentials, registry, and config from AppState.
/// Creates `HueLightController<H>`, `BlockingTimeProvider`, `BlockingScheduler`,
/// and `RhythmRuntime`. Restores persisted room state from storage.
#[cfg(feature = "blocking")]
pub fn ensure_hue_runtime<H: crate::transport::HueTransport + 'static>(
    state: &SharedState,
    transport: H,
    _scheduler_stack_size: Option<usize>,
) -> Result<()> {
    use crate::controller::HueLightController;
    use log::warn;

    let (hub_key, username, registry) = {
        let s = state
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock state"))?;

        let (hub_key, registry) = s
            .hubs
            .iter()
            .find_map(|(key, hub)| {
                hub.data::<HueHubData>()
                    .map(|hue| (key.clone(), hue.registry.clone()))
            })
            .ok_or_else(|| anyhow::anyhow!("Hue hub not active (call connect_sse first)"))?;

        let hue_creds = s
            .hub_credentials
            .get(&hub_key)
            .ok_or_else(|| anyhow::anyhow!("No hub credentials configured"))?;
        let user = crate::provider::hue_username(hue_creds)
            .ok_or_else(|| anyhow::anyhow!("Hue credentials not configured"))?
            .to_string();

        (hub_key, user, registry)
    };

    let controller = HueLightController::new(transport, username, registry.clone())
        .with_capability_source(state.clone(), hub_key);

    rhythm_os::lifecycle::ensure_hub_runtime(
        state,
        controller,
        registry,
        Some(Box::new(|c: &HueLightController<H>| {
            if let Err(e) = c.warmup_tls() {
                warn!(target: "sys", "TLS warmup failed (will retry on first command): {}", e);
            }
        })),
    )
}
