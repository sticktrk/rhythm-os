//! ESP32-specific Hue transport implementations.
//!
//! Provides ESP-IDF based transport (`HueClient`) and SSE reader (`run_hue_sse`).
//! All lifecycle logic lives in `rhythm_hue::embedded_lifecycle`.

pub mod client;
pub mod sse;

use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use log::{info, warn};
use rhythm_hue::hue_lifecycle::HueSseConnectConfig;
use rhythm_hue::registry::HueDeviceRegistry;

use rhythm_os::hub::{HubEvent, HubProvider, HubType};
use rhythm_os::state::SharedState;

use self::client::HueClient;

// ============================================================================
// SSE helper — starts ESP32 SSE reader + shared translator
// ============================================================================

/// Start the Hue SSE event stream with ESP32 transport.
///
/// Spawns the EspTls SSE reader thread and delegates event translation
/// to `rhythm_hue::hue_lifecycle::start_event_translator`.
///
/// When an unknown button_id arrives via SSE, the translator thread will
/// fetch the individual button and device resources from the Hue bridge
/// to auto-register the switch (on-demand discovery).
pub fn start_event_stream(
    config: HueSseConnectConfig,
    registry: Arc<Mutex<HueDeviceRegistry>>,
    shutdown: Arc<AtomicBool>,
    state: SharedState,
) -> Receiver<HubEvent> {
    let (sse_tx, sse_rx) = std::sync::mpsc::sync_channel::<sse::HueSseEvent>(32);

    let sse_config = sse::HueSseConfig {
        bridge_ip: config.bridge_ip.clone(),
        username: config.username.clone(),
    };

    // SSE reader thread (ESP32-specific: EspTls, small stack)
    let sse_shutdown = shutdown.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("hue-sse".to_string())
        .stack_size(16 * 1024)
        .spawn(move || {
            sse::run_hue_sse(sse_config, sse_tx, &sse_shutdown);
        })
    {
        log::warn!("Failed to spawn Hue SSE thread: {} — events will not stream", e);
    }

    // Build on-demand discovery closures for the translator thread.
    // Creates a persistent HueClient that reuses TLS across discoveries.
    // Shared by both button and motion discovery closures.
    let discovery_transport: Arc<Mutex<Option<HueClient>>> = Arc::new(Mutex::new(None));
    let discovery_bridge_ip = config.bridge_ip.clone();
    let discovery_username = config.username.clone();
    let discovery_state = state;

    // Helper: ensure the shared transport is initialized, returning whether it's ready.
    let ensure_transport = {
        let transport = discovery_transport.clone();
        let bridge_ip = discovery_bridge_ip.clone();
        move || -> bool {
            let mut guard = match transport.lock() {
                Ok(g) => g,
                Err(_) => return false,
            };
            if guard.is_none() {
                *guard = Some(HueClient::new(bridge_ip.clone()));
            }
            guard.is_some()
        }
    };

    // On-demand button discovery closure
    let btn_transport = discovery_transport.clone();
    let btn_registry = registry.clone();
    let btn_username = discovery_username.clone();
    let btn_state = discovery_state.clone();
    let btn_ensure = ensure_transport.clone();

    let on_unknown_button: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |button_id: &str| {
        info!(target: "evt", "SSE: Unknown button {}, attempting on-demand discovery...", button_id);

        if !btn_ensure() {
            return;
        }

        let result = {
            let guard = match btn_transport.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            let transport = match guard.as_ref() {
                Some(t) => t,
                None => return,
            };
            rhythm_hue::discovery::discover_device(
                transport,
                &btn_username,
                button_id,
                "button",
                &btn_registry,
            )
        };

        match result {
            Ok(true) => {
                info!(target: "evt", "SSE: On-demand discovery succeeded for button {}", button_id);
                rhythm_os::commands::persist_registry(&btn_state);
            }
            Ok(false) => {
                info!(target: "evt", "SSE: Button {} device not in any known room", button_id);
            }
            Err(e) => {
                warn!(target: "evt", "SSE: On-demand discovery failed for button {}: {}", button_id, e);
            }
        }
    });

    // On-demand motion sensor discovery closure
    let motion_transport = discovery_transport;
    let motion_registry = registry.clone();
    let motion_username = discovery_username;
    let motion_state = discovery_state;
    let motion_ensure = ensure_transport;

    let on_unknown_motion: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |motion_id: &str| {
        info!(target: "evt", "SSE: Unknown motion sensor {}, attempting on-demand discovery...", motion_id);

        if !motion_ensure() {
            return;
        }

        let result = {
            let guard = match motion_transport.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            let transport = match guard.as_ref() {
                Some(t) => t,
                None => return,
            };
            rhythm_hue::discovery::discover_device(
                transport,
                &motion_username,
                motion_id,
                "motion",
                &motion_registry,
            )
        };

        match result {
            Ok(true) => {
                info!(target: "evt", "SSE: On-demand discovery succeeded for motion sensor {}", motion_id);
                rhythm_os::commands::persist_registry(&motion_state);
            }
            Ok(false) => {
                info!(target: "evt", "SSE: Motion sensor {} device not in any known room", motion_id);
            }
            Err(e) => {
                warn!(target: "evt", "SSE: On-demand discovery failed for motion sensor {}: {}", motion_id, e);
            }
        }
    });

    // Shared translator thread (16KB stack for TLS handshake in discovery callback)
    rhythm_hue::hue_lifecycle::start_event_translator(
        sse_rx,
        registry,
        shutdown,
        Some(Arc::new(|| crate::led::led_activity())),
        Some(on_unknown_button),
        Some(on_unknown_motion),
        Some(16 * 1024),
    )
}

// ============================================================================
// HueHubProvider — implements HubProvider for HTTP credential push
// ============================================================================

/// Static hub provider for Hue bridges (ESP32 transport).
pub struct HueHubProvider;

impl HubProvider for HueHubProvider {
    fn hub_type(&self) -> HubType {
        HubType::new(HubType::HUE)
    }

    fn configure(&self, address: &str, credentials_json: &str, state: &SharedState) -> Result<()> {
        rhythm_hue::provider::configure_hue_hub(address, credentials_json, state, |state| {
            // ESP32: spawn SSE connect in a thread with adequate stack for HTTPS
            let init_state = state.clone();
            let event_state = state.clone();
            std::thread::Builder::new()
                .name("hub-init".to_string())
                .stack_size(16 * 1024)
                .spawn(move || {
                    rhythm_hue::embedded_lifecycle::connect_hue_sse_with_discovery(
                        &init_state,
                        |ip| Ok(HueClient::new(ip.to_string())),
                        |config, registry, shutdown| {
                            start_event_stream(config, registry, shutdown, event_state)
                        },
                    )
                })
                .map_err(|e| anyhow::anyhow!("Failed to spawn init thread: {}", e))?
                .join()
                .map_err(|_| anyhow::anyhow!("Hub init thread panicked"))?
        })?;

        // Try starting runtime if rooms exist
        let has_rooms = {
            let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            s.all_hub_registries()
                .iter()
                .any(|reg| reg.lock().ok().map(|r| !r.rooms().is_empty()).unwrap_or(false))
        };

        if has_rooms {
            let bridge_ip = {
                let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
                s.hub_credentials.values().next()
                    .map(|c| c.address.clone())
                    .ok_or_else(|| anyhow::anyhow!("No hub credentials"))?
            };
            let transport = HueClient::new(bridge_ip);
            if let Err(e) = rhythm_hue::embedded_lifecycle::ensure_runtime(
                state,
                transport,
            ) {
                warn!(target: "sys", "Failed to start runtime on reconfigure: {}", e);
            }
        }

        Ok(())
    }
}
