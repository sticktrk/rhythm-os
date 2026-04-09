//! HA hub lifecycle management (generic over transport).
//!
//! Platform-agnostic lifecycle functions for connecting to Home Assistant,
//! translating WS events, and creating the runtime. The actual WS
//! transport and concrete types are provided by the platform crate
//! via closures/callbacks.
//!
//! Connect, runtime creation, and event translation are delegated to
//! `rhythm_os::lifecycle`. This module keeps only HA-specific logic:
//! `HaWsEvent` enum, `connect_ha` wrapper, and `ensure_ha_runtime`.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use log::info;

use crate::hub_state::HaHubData;
use crate::registry::HaDeviceRegistry;
use crate::transport::HaConnectionConfig;

use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::{ActiveHub, HubEvent, HubType};
use rhythm_os::registry::RegistrySnapshot;
use rhythm_os::state::SharedState;

/// HA hub type constant.
pub const HA_HUB_TYPE: &str = "homeassistant";

/// Connect to HA and set up the hub (generic over transport).
///
/// Creates a `device_area_cache` (initially empty) and shares it with
/// both `HaHubData` (for later population) and the event translator
/// (for on-demand button discovery).
///
/// The `start_event_stream` closure is provided by the platform crate to
/// handle the actual WS transport. It now receives the device_area_cache
/// so the event translator can use it for button resolution.
///
/// Returns `(ActiveHub, Receiver<HubEvent>)`. The runtime inside ActiveHub
/// is `None` — call the platform's `ensure_runtime()` when the first room arrives.
pub fn connect_ha<F>(
    state: &SharedState,
    hub_key: HubKey,
    config: HaConnectionConfig,
    load_registry_snapshot: Option<RegistrySnapshot>,
    start_event_stream: F,
) -> Result<(ActiveHub, Receiver<HubEvent>)>
where
    F: FnOnce(
        &HaConnectionConfig,
        Arc<Mutex<HaDeviceRegistry>>,
        Arc<AtomicBool>,
        Arc<Mutex<HashMap<String, String>>>,
    ) -> Receiver<HubEvent>,
{
    info!(target: "sys", "Connecting to HA at {}:{}...", config.host, config.port);

    let config_clone = config.clone();
    let device_area_cache = Arc::new(Mutex::new(HashMap::new()));
    let cache_for_hub = device_area_cache.clone();

    rhythm_os::lifecycle::connect_hub(
        state,
        HubType::new(HA_HUB_TYPE),
        hub_key,
        true, // HA: grouped_light_id defaults to room_id (area_id)
        load_registry_snapshot,
        // hub_data_builder: create HaHubData from registry + cache
        move |registry| {
            Box::new(HaHubData {
                config: config_clone,
                registry,
                device_area_cache: cache_for_hub,
            })
        },
        // start_event_stream: wrap the platform closure with HA config + cache
        move |registry, shutdown| {
            start_event_stream(&config, registry, shutdown, device_area_cache)
        },
    )
}

/// Create the RhythmRuntime for an HA hub, generic over transport.
///
/// Reads credentials, registry, and config from AppState.
/// Creates `HaLightController<H>`, `BlockingTimeProvider`, `BlockingScheduler`,
/// and `RhythmRuntime`. Restores persisted room state from storage.
#[cfg(feature = "blocking")]
pub fn ensure_ha_runtime<H: crate::transport::HaTransport + 'static>(
    state: &SharedState,
    transport: H,
    _scheduler_stack_size: Option<usize>,
) -> Result<()> {
    use crate::controller::HaLightController;

    let (hub_key, registry) = {
        let s = state
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock state"))?;

        s.hubs
            .iter()
            .find_map(|(key, hub)| {
                hub.data::<HaHubData>()
                    .map(|ha| (key.clone(), ha.registry.clone()))
            })
            .ok_or_else(|| anyhow::anyhow!("HA hub not active (call connect first)"))?
    };

    let controller = HaLightController::new(transport, registry.clone())
        .with_capability_source(state.clone(), hub_key);

    rhythm_os::lifecycle::ensure_hub_runtime(
        state, controller, registry, None, // No TLS warmup for HA
    )
}

/// Spawn a translator thread that converts raw HA WS events to hub-agnostic `HubEvent`s.
///
/// The `device_area_cache` is threaded through to `translate_ws_event` so that
/// `hue_event` button events can use on-demand discovery.
pub fn start_event_translator(
    ws_rx: Receiver<HaWsEvent>,
    registry: Arc<Mutex<HaDeviceRegistry>>,
    device_area_cache: Arc<Mutex<HashMap<String, String>>>,
    shutdown: Arc<AtomicBool>,
    stack_size: Option<usize>,
) -> Receiver<HubEvent> {
    rhythm_os::lifecycle::start_event_translator(
        ws_rx,
        move |event| match event {
            HaWsEvent::Connected => vec![HubEvent::Connected { hub_key: None }],
            HaWsEvent::ServiceEvent { event_type, data } => {
                crate::events::translate_ws_event(event_type, data, &registry, &device_area_cache)
            }
            HaWsEvent::Heartbeat => vec![HubEvent::Heartbeat { hub_key: None }],
            HaWsEvent::Disconnected(reason) => vec![HubEvent::Disconnected {
                hub_key: None,
                reason: reason.clone(),
            }],
        },
        shutdown,
        "ha-evt",
        stack_size,
        None,
    )
}

/// Raw event from the HA WebSocket connection.
#[derive(Debug, Clone)]
pub enum HaWsEvent {
    /// WebSocket authenticated and ready for event subscriptions.
    Connected,
    /// A subscribed event was received.
    ServiceEvent {
        event_type: String,
        data: serde_json::Value,
    },
    /// Connection heartbeat.
    Heartbeat,
    /// Connection lost.
    Disconnected(String),
}
