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
use rhythm_os::hub::{
    ActiveHub, ExternalLightHubIntegration, HubDeviceRoomAssignment,
    HubDeviceRoomAssignmentOutcome, HubEvent, HubProvider, HubType,
};
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
            .ok_or_else(|| anyhow::anyhow!("No HA credentials configured"))?;
        crate::provider::config_from_credentials(&ha_creds.address, ha_creds)?
    };

    let transport = ReqwestHaTransport::new(config)?;
    crate::ha_lifecycle::ensure_ha_runtime(state, transport)
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

                let ha_creds = s.hub_credentials.get(&configure_key).ok_or_else(|| {
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

    fn prepare_device_room_assignment(
        &self,
        state: &SharedState,
        assignment: &HubDeviceRoomAssignment,
    ) -> Result<HubDeviceRoomAssignmentOutcome> {
        if assignment.device_type != rhythm_core::runtime::hub_registry::DeviceType::Light {
            return Ok(HubDeviceRoomAssignmentOutcome::Unchanged);
        }

        let target_area_id = match assignment.target_rhythm_room_id.as_deref() {
            Some(target_room_id) => match assignment.target_hub_room_ids.as_slice() {
                [target_area_id] => Some(target_area_id.as_str()),
                [] => {
                    return Err(anyhow::anyhow!(
                        "Cannot move Home Assistant light to Rhythm room '{}': that room is not backed by this Home Assistant instance",
                        target_room_id
                    ));
                }
                _ => {
                    return Err(anyhow::anyhow!(
                        "Cannot move Home Assistant light to Rhythm room '{}': it maps to multiple areas in this Home Assistant instance",
                        target_room_id
                    ));
                }
            },
            None => None,
        };

        let config = {
            let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            state
                .hubs
                .get(&assignment.hub_key)
                .and_then(|hub| hub.data::<crate::hub_state::HaHubData>())
                .map(|ha| ha.config.clone())
                .ok_or_else(|| {
                    anyhow::anyhow!("Home Assistant hub is not active: {}", assignment.hub_key)
                })?
        };
        let rollback = crate::area_membership::reassign_entity_area(
            &config,
            &assignment.native_device_id,
            target_area_id,
        )?;
        Ok(HubDeviceRoomAssignmentOutcome::Reassigned {
            target_hub_room_id: target_area_id.map(str::to_string),
            rollback: Box::new(move || {
                crate::area_membership::rollback_entity_area(&config, rollback)
            }),
        })
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
    let motion_cache = device_area_cache.clone();
    let on_unknown_motion: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |sensor_id: &str| {
        crate::events::register_unknown_motion_from_cache(
            sensor_id,
            &motion_registry,
            &motion_cache,
        );
    });

    let contact_registry = registry.clone();
    let contact_cache = device_area_cache;
    let on_unknown_contact: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |sensor_id: &str| {
        crate::events::register_unknown_contact_from_cache(
            sensor_id,
            &contact_registry,
            &contact_cache,
        );
    });

    crate::ha_lifecycle::start_event_translator(
        ws_rx,
        registry,
        shutdown,
        None,
        Some(on_unknown_button),
        Some(on_unknown_motion),
        Some(on_unknown_contact),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    use rhythm_core::{
        InputEvent, LightProfileConfig, LightingCommand, ModeConfig, RestoredRoomState,
        RoomSnapshot, RuntimeHandle, SolarTime,
    };
    use rhythm_os::hub::{ActiveHub, ExternalLightHubIntegration, HubCredentials};

    use crate::hub_state::HaHubData;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct NoopRuntime;

    impl RuntimeHandle for NoopRuntime {
        fn handle_event(&self, _: &InputEvent) -> anyhow::Result<bool> {
            Ok(false)
        }

        fn sync_rooms(&self) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_solar(&self, _: SolarTime) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_light_profile_config(&self, _: LightProfileConfig) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_mode_configs(&self, _: Vec<ModeConfig>) -> anyhow::Result<()> {
            Ok(())
        }

        fn periodic_tick_room(&self, _: &str, _: f32) -> anyhow::Result<()> {
            Ok(())
        }

        fn engine_room_snapshot(&self, _: &str) -> Option<RoomSnapshot> {
            None
        }

        fn engine_all_room_snapshots(&self) -> Vec<RoomSnapshot> {
            Vec::new()
        }

        fn restore_room_state(&self, _: &str, _: RestoredRoomState) {}

        fn add_room(&self, _: &str, _: &str) {}

        fn remove_room(&self, _: &str) {}

        fn dim_room(&self, _: &str, _: f32) -> anyhow::Result<()> {
            Ok(())
        }

        fn turn_on_room(&self, _: &str) -> anyhow::Result<()> {
            Ok(())
        }

        fn apply_room_command(&self, _: &str, _: LightingCommand) -> anyhow::Result<()> {
            Ok(())
        }

        fn lights_off_room(&self, _: &str, _: Option<u32>) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_power_save(&self, _: bool) -> Vec<String> {
            Vec::new()
        }

        fn is_power_save(&self) -> bool {
            false
        }

        fn set_room_brightness(&self, _: &str, _: u8) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_room_time_offset(&self, _: &str, _: f32) -> anyhow::Result<()> {
            Ok(())
        }

        fn idle_brightness(&self) -> u8 {
            1
        }

        fn soft_off_tick_room(&self, _: &str) -> anyhow::Result<()> {
            Ok(())
        }

        fn any_lights_on(&self, _: &str) -> anyhow::Result<bool> {
            Ok(false)
        }

        fn current_hour(&self) -> f32 {
            12.0
        }

        fn set_light_profile(&self, _: &str) -> bool {
            true
        }

        fn active_light_profile_id(&self) -> String {
            rhythm_core::RHYTHM_PROFILE_ID.to_string()
        }

        fn available_light_profiles(&self) -> Vec<(String, String)> {
            vec![(
                rhythm_core::RHYTHM_PROFILE_ID.to_string(),
                "Rhythm".to_string(),
            )]
        }
    }

    fn state() -> SharedState {
        Arc::new(Mutex::new(rhythm_os::state::AppState::default()))
    }

    fn ha_key(address: &str) -> HubKey {
        HubKey::new(HubType::new(crate::ha_lifecycle::HA_HUB_TYPE), address)
    }

    fn ha_config() -> HaConnectionConfig {
        HaConnectionConfig {
            host: "ha.local".to_string(),
            port: 8123,
            token: "token".to_string(),
            use_ssl: false,
        }
    }

    fn install_credentials(state: &SharedState, address: &str) {
        let creds = crate::provider::ha_credentials(address, "token");
        state
            .lock()
            .unwrap()
            .hub_credentials
            .insert(ha_key(address), creds);
    }

    fn install_hub(
        state: &SharedState,
        address: &str,
        runtime: Option<Arc<dyn RuntimeHandle>>,
        hub_data: Box<dyn std::any::Any + Send + Sync>,
    ) {
        let key = ha_key(address);
        let hub = ActiveHub {
            hub_type: key.hub_type.clone(),
            hub_key: key.clone(),
            runtime,
            hub_data,
            registry: None,
            discovery: None,
            shutdown: Arc::new(AtomicBool::new(false)),
        };
        state.lock().unwrap().hubs.insert(key, hub);
    }

    fn ha_hub_data() -> HaHubData {
        HaHubData {
            config: ha_config(),
            registry: Arc::new(Mutex::new(HaDeviceRegistry::new())),
            device_area_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn string_error<T>(result: Result<T>) -> String {
        match result {
            Ok(_) => panic!("expected error"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn integration_metadata_and_credentials_interceptor_branches() {
        let state = state();
        let integration = HaIntegration;

        assert_eq!(integration.hub_type(), crate::ha_lifecycle::HA_HUB_TYPE);
        assert_eq!(
            integration.provider().hub_type().as_str(),
            crate::ha_lifecycle::HA_HUB_TYPE
        );
        let capabilities = integration.api_capabilities();
        assert_eq!(capabilities.hub_type, crate::ha_lifecycle::HA_HUB_TYPE);
        assert!(capabilities.configurable);
        assert!(!capabilities.supports_unpairing);

        assert!(integration
            .credentials_interceptor(&state, &serde_json::json!({ "hub_type": "hue" }))
            .is_none());
        assert!(integration
            .credentials_interceptor(
                &state,
                &serde_json::json!({
                    "hub_type": crate::ha_lifecycle::HA_HUB_TYPE,
                    "credentials": { "token": "explicit" }
                })
            )
            .is_none());

        let _guard = ENV_LOCK.lock().unwrap();
        let previous = std::env::var("SUPERVISOR_TOKEN").ok();
        std::env::set_var("SUPERVISOR_TOKEN", "supervisor-token");
        let intercepted = integration
            .credentials_interceptor(
                &state,
                &serde_json::json!({ "hub_type": crate::ha_lifecycle::HA_HUB_TYPE }),
            )
            .expect("empty HA credentials should be intercepted when supervisor token exists");
        assert_eq!(intercepted.unwrap_err(), "No hub provider registered");
        if let Some(previous) = previous {
            std::env::set_var("SUPERVISOR_TOKEN", previous);
        } else {
            std::env::remove_var("SUPERVISOR_TOKEN");
        }
    }

    #[test]
    fn ha_light_move_requires_exactly_one_native_target_area() {
        let state = state();
        let assignment = |target_hub_room_ids: Vec<String>| HubDeviceRoomAssignment {
            hub_key: ha_key("ha.local:8123"),
            native_device_id: "light.desk".to_string(),
            device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
            target_rhythm_room_id: Some("office".to_string()),
            target_hub_room_ids,
        };

        let missing = string_error(
            INTEGRATION.prepare_device_room_assignment(&state, &assignment(Vec::new())),
        );
        assert!(missing.contains("not backed by this Home Assistant instance"));

        let ambiguous = string_error(INTEGRATION.prepare_device_room_assignment(
            &state,
            &assignment(vec!["office-a".to_string(), "office-b".to_string()]),
        ));
        assert!(ambiguous.contains("maps to multiple areas"));
    }

    #[test]
    fn connect_and_start_requires_stored_credentials() {
        let state = state();
        assert_eq!(
            string_error(connect_and_start(state, &ha_key("ha.local"))),
            "No HA credentials configured for homeassistant@ha.local"
        );
    }

    #[test]
    fn ensure_runtime_requires_credentials_unless_runtime_already_exists() {
        let state = state();
        assert_eq!(
            string_error(ensure_runtime(&state)),
            "No HA credentials configured"
        );

        install_hub(
            &state,
            "ha.local",
            Some(Arc::new(NoopRuntime) as Arc<dyn RuntimeHandle>),
            Box::new(()),
        );
        ensure_runtime(&state).unwrap();
    }

    #[test]
    fn create_ha_controller_reports_missing_credentials_or_hub_data_then_constructs() {
        let state = state();
        assert_eq!(
            string_error(create_ha_controller(&state, &ha_key("ha.local"))),
            "No HA credentials for homeassistant@ha.local"
        );

        install_credentials(&state, "ha.local");
        assert_eq!(
            string_error(create_ha_controller(&state, &ha_key("ha.local"))),
            "HA hub not active for homeassistant@ha.local"
        );

        install_hub(&state, "ha.local", None, Box::new(ha_hub_data()));
        let controller = create_ha_controller(&state, &ha_key("ha.local")).unwrap();
        assert_eq!(controller.name(), "HomeAssistant");
    }

    #[test]
    fn create_ha_controller_can_fallback_to_matching_credentials_and_any_ha_hub_data() {
        let state = state();
        install_credentials(&state, "ha.local");
        install_hub(&state, "different.local", None, Box::new(ha_hub_data()));

        let controller = create_ha_controller(&state, &ha_key("ha.local")).unwrap();
        assert_eq!(controller.name(), "HomeAssistant");
    }

    #[test]
    fn transport_from_state_uses_stored_ha_credentials_only() {
        let state = state();
        assert!(transport_from_state(&state).is_none());

        state.lock().unwrap().hub_credentials.insert(
            HubKey::new(HubType::new("matter"), "local"),
            HubCredentials::new(
                "matter",
                "local",
                serde_json::json!({ "fabric_id": "default" }),
            ),
        );
        assert!(transport_from_state(&state).is_none());

        install_credentials(&state, "ha.local");
        assert!(transport_from_state(&state).is_some());
    }
}
