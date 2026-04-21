//! Complete Hue lifecycle using reqwest transport (desktop/server targets).
//!
//! Provides everything a binary crate needs to run Hue as a plugin:
//! `connect_and_start`, `ensure_runtime`, `get_hub_provider`.
//! No platform-specific code needed in the consuming crate.

use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use log::warn;

use crate::hub_state::HueHubData;
use crate::registry::{HueDeviceRegistry, HueRegistrySnapshot};
use crate::reqwest_sse::start_reqwest_sse;
use crate::reqwest_transport::ReqwestHueTransport;
use crate::sse::HueSseConfig;

use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::{ActiveHub, ExternalLightHubIntegration, HubEvent, HubProvider, HubType};
use rhythm_os::state::SharedState;

// ============================================================================
// Boot-time connection
// ============================================================================

/// Connect to the Hue bridge and store the hub in state.
///
/// Call this at startup when hub credentials are already configured.
/// Loads the registry snapshot from storage, connects SSE, and stores the hub
/// in state. Runtime creation is deferred to room sync.
///
/// Returns the event receiver for the main event loop.
pub fn connect_and_start(state: SharedState, key: &HubKey) -> Result<Receiver<HubEvent>> {
    let snapshot = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.storage
            .as_ref()
            .and_then(|st| st.load_hub_registry_for(key).ok().flatten())
            .and_then(|v| serde_json::from_value::<HueRegistrySnapshot>(v).ok())
    };

    let (hub, event_rx) = connect_hue_sse(&state, key, snapshot)?;

    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let key = hub.hub_key.clone();
        s.hubs.insert(key.clone(), hub);
        s.set_hub_connected(&key, false);
    }

    // Runtime creation is deferred to room sync (do_room_set → commands::ensure_runtime),
    // which uses ensure_composite_runtime on desktop. This ensures the CompositeController
    // is created with controllers for ALL connected hubs, enabling proper topology ID
    // → hub-native ID translation.

    Ok(event_rx)
}

// ============================================================================
// Runtime creation
// ============================================================================

/// Create the RhythmRuntime for the Hue hub using reqwest transport.
pub fn ensure_runtime(state: &SharedState) -> Result<()> {
    // Check early: skip if any runtime already exists (avoids creating a
    // ReqwestHueTransport whose reqwest::blocking::Client would panic on
    // drop if we're on a tokio worker thread and the runtime was a no-op).
    {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        if s.hubs.values().any(|h| h.runtime.is_some()) {
            return Ok(());
        }
    }

    let bridge_ip = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hub_credentials
            .values()
            .find(|c| c.hub_type.as_ref().is_some_and(|t| t.as_str() == "hue"))
            .map(|c| c.address.clone())
            .unwrap_or_default()
    };

    let transport = ReqwestHueTransport::new(&bridge_ip)?;
    crate::hue_lifecycle::ensure_hue_runtime(state, transport)
}

// ============================================================================
// Controller creation (for CompositeController)
// ============================================================================

/// Create a type-erased Hue light controller for a specific hub key.
///
/// Extracts credentials and registry from state, builds a reqwest transport,
/// and returns the controller as `Arc<dyn HubLightController>`.
pub fn create_hue_controller(
    state: &SharedState,
    key: &HubKey,
) -> Result<std::sync::Arc<dyn rhythm_core::HubLightController>> {
    use crate::controller::HueLightController;
    use crate::hub_state::HueHubData;

    let (bridge_ip, username, registry) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

        let creds = s
            .hub_credentials
            .get(key)
            .ok_or_else(|| anyhow::anyhow!("No Hue credentials for {}", key))?;
        let username = crate::provider::hue_username(creds)
            .ok_or_else(|| anyhow::anyhow!("Hue credentials missing username"))?
            .to_string();
        let bridge_ip = creds.address.clone();

        let hue_data = s.hubs.get(key).and_then(|h| h.data::<HueHubData>());
        let reg = hue_data
            .map(|hue| hue.registry.clone())
            .ok_or_else(|| anyhow::anyhow!("Hue hub not active for {}", key))?;

        (bridge_ip, username, reg)
    };

    let transport = ReqwestHueTransport::new(&bridge_ip)?;
    let controller = HueLightController::new(transport, username, registry)
        .with_capability_source(state.clone(), key.clone());
    Ok(std::sync::Arc::new(controller))
}

// ============================================================================
// Hub provider
// ============================================================================

/// Get the static Hue hub provider (reqwest transport).
pub fn get_hub_provider() -> &'static dyn HubProvider {
    static HUE: ReqwestHueHubProvider = ReqwestHueHubProvider;
    &HUE
}

/// Hub provider for Hue bridges using reqwest transport.
pub struct ReqwestHueHubProvider;

impl HubProvider for ReqwestHueHubProvider {
    fn hub_type(&self) -> HubType {
        HubType::new(HubType::HUE)
    }

    fn configure(&self, address: &str, credentials_json: &str, state: &SharedState) -> Result<()> {
        let configure_key = HubKey::new(HubType::new(HubType::HUE), address);
        crate::provider::configure_hue_hub(address, credentials_json, state, |state| {
            let snapshot = {
                let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
                s.storage
                    .as_ref()
                    .and_then(|st| st.load_hub_registry_for(&configure_key).ok().flatten())
                    .and_then(|v| serde_json::from_value::<HueRegistrySnapshot>(v).ok())
            };
            connect_hue_sse(state, &configure_key, snapshot)
        })?;

        // Runtime creation is deferred to room sync (do_room_set → commands::ensure_runtime).
        // do_configure_hub triggers auto-sync after this returns.

        Ok(())
    }
}

// ============================================================================
// ExternalLightHubIntegration — static integration for platform crate registries
// ============================================================================

/// Static integration instance for platform crate registries.
pub static INTEGRATION: HueIntegration = HueIntegration;

/// Hue integration using reqwest transport (desktop/server targets).
pub struct HueIntegration;

impl ExternalLightHubIntegration for HueIntegration {
    fn hub_type(&self) -> &'static str {
        HubType::HUE
    }
    fn provider(&self) -> &'static dyn HubProvider {
        get_hub_provider()
    }
    fn connect_and_start(&self, state: SharedState, key: &HubKey) -> Result<Receiver<HubEvent>> {
        connect_and_start(state, key)
    }
    fn ensure_runtime(&self, state: &SharedState) -> Result<()> {
        ensure_runtime(state)
    }
    fn create_controller(
        &self,
        state: &SharedState,
        key: &HubKey,
    ) -> Result<std::sync::Arc<dyn rhythm_core::HubLightController>> {
        create_hue_controller(state, key)
    }
}

// ============================================================================
// Internal helpers
// ============================================================================

/// Connect Hue SSE with reqwest transport, including discovery.
fn connect_hue_sse(
    state: &SharedState,
    key: &HubKey,
    snapshot: Option<HueRegistrySnapshot>,
) -> Result<(ActiveHub, Receiver<HubEvent>)> {
    let (mut hub, event_rx) = crate::hue_lifecycle::connect_hue_sse(
        state,
        key.clone(),
        snapshot,
        |config, registry, shutdown| {
            start_event_stream(config.bridge_ip, config.username, registry, shutdown)
        },
    )?;

    // Attach Hue discovery to the hub
    if let Some(hue_data) = hub.data::<HueHubData>() {
        let bridge_ip = hue_data.bridge_ip.clone();
        let username = hue_data.username.clone();
        match ReqwestHueTransport::new(&bridge_ip) {
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

/// Start the SSE event stream using reqwest + a shared translator thread.
fn start_event_stream(
    bridge_ip: String,
    username: String,
    registry: Arc<Mutex<HueDeviceRegistry>>,
    shutdown: Arc<AtomicBool>,
) -> Receiver<HubEvent> {
    let sse_config = HueSseConfig {
        bridge_ip,
        username,
    };

    let sse_rx = start_reqwest_sse(sse_config, shutdown.clone());

    crate::events::start_event_translator(sse_rx, registry, shutdown, None, None, None)
}
