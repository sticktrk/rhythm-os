//! Complete HA lifecycle using reqwest transport (desktop/server targets).
//!
//! Provides everything a binary crate needs to run HA as a plugin:
//! `connect_and_start`, `ensure_runtime`, `get_hub_provider`.
//! No platform-specific code needed in the consuming crate.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::registry::{HaDeviceRegistry, HaRegistrySnapshot};
use crate::reqwest_transport::ReqwestHaTransport;
use crate::transport::HaConnectionConfig;
use crate::ws_client::start_ha_ws;

use rhythm_os::button_resolve::RawButtonEvent;
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::{ActiveHub, ExternalLightHubIntegration, HubEvent, HubProvider, HubType};
use rhythm_os::state::SharedState;

// ============================================================================
// Boot-time connection
// ============================================================================

/// Connect to HA and store the hub in state.
///
/// Call this at startup when hub credentials are already configured.
/// Loads the registry snapshot from storage, connects WS, and stores the hub
/// in state. Runtime creation is deferred to room sync.
///
/// Returns the event receiver for the main event loop.
pub fn connect_and_start(state: SharedState, key: &HubKey) -> Result<Receiver<HubEvent>> {
    let (snapshot, config) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let snapshot = s
            .storage
            .as_ref()
            .and_then(|st| st.load_hub_registry_for(key).ok().flatten())
            .and_then(|v| serde_json::from_value::<HaRegistrySnapshot>(v).ok());

        let ha_creds = s
            .hub_credentials
            .get(key)
            .or_else(|| s.first_hub_credentials())
            .ok_or_else(|| anyhow::anyhow!("No HA credentials configured for {}", key))?;
        let config = crate::provider::config_from_credentials(&ha_creds.address, ha_creds)?;

        (snapshot, config)
    };

    let (hub, event_rx) = connect_ha_ws(&state, key, config, snapshot)?;

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

/// Create the RhythmRuntime for the HA hub using reqwest transport.
pub fn ensure_runtime(state: &SharedState) -> Result<()> {
    // Check early: skip if any runtime already exists
    {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        if s.hubs.values().any(|h| h.runtime.is_some()) {
            return Ok(());
        }
    }

    let config = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let ha_creds = s
            .hub_credentials
            .values()
            .find(|c| {
                c.hub_type
                    .as_ref()
                    .is_some_and(|t| t.as_str() == "homeassistant")
            })
            .or_else(|| s.first_hub_credentials())
            .ok_or_else(|| anyhow::anyhow!("No HA credentials configured"))?;
        crate::provider::config_from_credentials(&ha_creds.address, ha_creds)?
    };

    let scheduler_stack = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.platform.scheduler_stack
    };

    let transport = ReqwestHaTransport::new(config)?;
    crate::ha_lifecycle::ensure_ha_runtime(state, transport, scheduler_stack)
}

// ============================================================================
// Hub provider
// ============================================================================

/// Get the static HA hub provider (reqwest transport).
pub fn get_hub_provider() -> &'static dyn HubProvider {
    static HA: ReqwestHaHubProvider = ReqwestHaHubProvider;
    &HA
}

/// Hub provider for HA using reqwest transport.
pub struct ReqwestHaHubProvider;

impl HubProvider for ReqwestHaHubProvider {
    fn hub_type(&self) -> HubType {
        HubType::new(crate::ha_lifecycle::HA_HUB_TYPE)
    }

    fn configure(&self, address: &str, credentials_json: &str, state: &SharedState) -> Result<()> {
        let configure_key = HubKey::new(HubType::new(crate::ha_lifecycle::HA_HUB_TYPE), address);
        crate::provider::configure_ha_hub(address, credentials_json, state, |state| {
            let (snapshot, config) = {
                let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
                let snapshot = s
                    .storage
                    .as_ref()
                    .and_then(|st| st.load_hub_registry_for(&configure_key).ok().flatten())
                    .and_then(|v| serde_json::from_value::<HaRegistrySnapshot>(v).ok());

                let ha_creds = s
                    .hub_credentials
                    .get(&configure_key)
                    .or_else(|| s.first_hub_credentials())
                    .ok_or_else(|| {
                        anyhow::anyhow!("No HA credentials configured for {}", configure_key)
                    })?;
                let config = crate::provider::config_from_credentials(&ha_creds.address, ha_creds)?;

                (snapshot, config)
            };
            connect_ha_ws(state, &configure_key, config, snapshot)
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
pub static INTEGRATION: HaIntegration = HaIntegration;

/// HA integration using reqwest transport (desktop/server targets).
pub struct HaIntegration;

impl ExternalLightHubIntegration for HaIntegration {
    fn hub_type(&self) -> &'static str {
        crate::ha_lifecycle::HA_HUB_TYPE
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
        create_ha_controller(state, key)
    }

    fn post_connect(&self, state: &SharedState, _key: &HubKey) {
        if let Some(transport) = transport_from_state(state) {
            crate::post_connect::fetch_ha_config(state, &transport);
        }
        crate::post_connect::populate_device_area_cache(state);
    }

    fn credentials_interceptor(
        &self,
        state: &SharedState,
        body: &serde_json::Value,
    ) -> Option<Result<String, String>> {
        let hub_type = body.get("hub_type").and_then(|v| v.as_str())?;
        if hub_type != crate::ha_lifecycle::HA_HUB_TYPE {
            return None;
        }

        let creds_empty = body
            .get("credentials")
            .map(|v| v.is_null() || v.as_object().map(|o| o.is_empty()).unwrap_or(false))
            .unwrap_or(true);
        if !creds_empty {
            return None;
        }

        let token = std::env::var("SUPERVISOR_TOKEN").ok()?;
        let credentials = serde_json::json!({ "token": token });

        match rhythm_os::commands::do_hub_credentials(
            state,
            crate::ha_lifecycle::HA_HUB_TYPE,
            "supervisor:80",
            &credentials,
        ) {
            Ok(()) => {
                let hub_connected = state
                    .lock()
                    .map(|s| s.has_any_connected_hub())
                    .unwrap_or(false);

                // Run HA post-connect (device cache + config import)
                let ha_key = HubKey::new(
                    HubType::new(crate::ha_lifecycle::HA_HUB_TYPE),
                    "supervisor:80",
                );
                self.post_connect(state, &ha_key);

                Some(Ok(format!(r#"{{"hub_connected":{}}}"#, hub_connected)))
            }
            Err(e) => Some(Err(e.to_string())),
        }
    }

    fn refresh_credentials(&self, state: &SharedState, key: &HubKey) {
        let token = match std::env::var("SUPERVISOR_TOKEN") {
            Ok(t) => t,
            Err(_) => return,
        };

        let address = {
            let s = match state.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            s.hub_credentials
                .get(key)
                .map(|c| c.address.clone())
                .unwrap_or_default()
        };

        let new_creds = crate::provider::ha_credentials(&address, &token);
        {
            let mut s = match state.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            if let Some(cred_key) = new_creds.hub_key() {
                s.hub_credentials.insert(cred_key, new_creds.clone());
            }
            if let Some(ref storage) = s.storage {
                let all: Vec<_> = s.hub_credentials.values().cloned().collect();
                let _ = storage.save_all_hub_credentials(&all);
            }
        }

        // Refresh timezone (DST may have changed across restarts)
        if let Ok(config) = crate::provider::config_from_credentials(&address, &new_creds) {
            if let Ok(transport) = ReqwestHaTransport::new(config) {
                crate::post_connect::fetch_ha_config(state, &transport);
            }
        }
    }
}

// ============================================================================
// Controller creation (for CompositeController)
// ============================================================================

/// Create a type-erased HA light controller for a specific hub key.
///
/// Extracts credentials and registry from state, builds a reqwest transport,
/// and returns the controller as `Arc<dyn HubLightController>`.
pub fn create_ha_controller(
    state: &SharedState,
    key: &HubKey,
) -> Result<std::sync::Arc<dyn rhythm_core::HubLightController>> {
    use crate::controller::HaLightController;
    use crate::hub_state::HaHubData;

    let (config, registry) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

        let ha_creds = s
            .hub_credentials
            .get(key)
            .or_else(|| {
                s.hub_credentials.values().find(|c| {
                    c.hub_type
                        .as_ref()
                        .is_some_and(|t| t.as_str() == "homeassistant")
                })
            })
            .ok_or_else(|| anyhow::anyhow!("No HA credentials for {}", key))?;
        let config = crate::provider::config_from_credentials(&ha_creds.address, ha_creds)?;

        let ha_data = s
            .hubs
            .get(key)
            .and_then(|h| h.data::<HaHubData>())
            .or_else(|| s.hubs.values().find_map(|h| h.data::<HaHubData>()));
        let reg = ha_data
            .map(|ha| ha.registry.clone())
            .ok_or_else(|| anyhow::anyhow!("HA hub not active for {}", key))?;

        (config, reg)
    };

    let transport = ReqwestHaTransport::new(config)?;
    let controller = HaLightController::new(transport, registry)
        .with_capability_source(state.clone(), key.clone());
    Ok(std::sync::Arc::new(controller))
}

// ============================================================================
// Internal helpers
// ============================================================================

/// Create a temporary `ReqwestHaTransport` from the HA credentials in state.
fn transport_from_state(state: &SharedState) -> Option<ReqwestHaTransport> {
    let config = {
        let s = state.lock().ok()?;
        let ha_creds = s.hub_credentials.values().find(|c| {
            c.hub_type
                .as_ref()
                .is_some_and(|t| t.as_str() == crate::ha_lifecycle::HA_HUB_TYPE)
        })?;
        crate::provider::config_from_credentials(&ha_creds.address, ha_creds).ok()?
    };
    ReqwestHaTransport::new(config).ok()
}

/// Connect HA WebSocket with event translator, including discovery.
fn connect_ha_ws(
    state: &SharedState,
    key: &HubKey,
    config: HaConnectionConfig,
    snapshot: Option<HaRegistrySnapshot>,
) -> Result<(ActiveHub, Receiver<HubEvent>)> {
    let discovery_config = config.clone();
    let (mut hub, event_rx) = crate::ha_lifecycle::connect_ha(
        state,
        key.clone(),
        config,
        snapshot,
        |config, registry, shutdown, cache| {
            start_event_stream(config.clone(), registry, shutdown, cache)
        },
    )?;

    // Attach HA discovery to the hub
    let discovery = crate::area_sync::HaDiscovery::new(discovery_config);
    hub.discovery = Some(Arc::new(discovery));

    Ok((hub, event_rx))
}

/// Start the WS event stream using tokio-tungstenite + a shared translator thread.
fn start_event_stream(
    config: HaConnectionConfig,
    registry: Arc<Mutex<HaDeviceRegistry>>,
    shutdown: Arc<AtomicBool>,
    device_area_cache: Arc<Mutex<HashMap<String, String>>>,
) -> Receiver<HubEvent> {
    let ws_rx = start_ha_ws(config, shutdown.clone());
    let button_registry = registry.clone();
    let button_cache = device_area_cache.clone();
    let on_unknown_button: Arc<dyn Fn(&RawButtonEvent) + Send + Sync> =
        Arc::new(move |evt: &RawButtonEvent| {
            crate::events::register_unknown_button_from_cache(evt, &button_registry, &button_cache);
        });

    let motion_registry = registry.clone();
    let motion_cache = device_area_cache;
    let on_unknown_motion: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |sensor_id: &str| {
        crate::events::register_unknown_motion_from_cache(
            sensor_id,
            &motion_registry,
            &motion_cache,
        );
    });

    crate::ha_lifecycle::start_event_translator(
        ws_rx,
        registry,
        shutdown,
        None,
        Some(on_unknown_button),
        Some(on_unknown_motion),
        None,
    )
}
