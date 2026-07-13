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
use crate::sse_liveness::HueSseLiveness;

use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::{
    ActiveHub, ExternalLightHubIntegration, HubDeviceRoomAssignment,
    HubDeviceRoomAssignmentOutcome, HubEvent, HubProvider, HubType,
};
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

    let (bridge_ip, username, registry, sse_liveness) = {
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
        let (reg, sse_liveness) = hue_data
            .map(|hue| (hue.registry.clone(), hue.sse_liveness.clone()))
            .ok_or_else(|| anyhow::anyhow!("Hue hub not active for {}", key))?;

        (bridge_ip, username, reg, sse_liveness)
    };

    let transport = ReqwestHueTransport::new(&bridge_ip)?;
    let controller = HueLightController::new(transport, username, registry)
        .with_capability_source(state.clone(), key.clone())
        .with_sse_liveness(sse_liveness);
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

    fn prepare_device_room_assignment(
        &self,
        state: &SharedState,
        assignment: &HubDeviceRoomAssignment,
    ) -> Result<HubDeviceRoomAssignmentOutcome> {
        if assignment.device_type != rhythm_core::runtime::hub_registry::DeviceType::Light {
            return Ok(HubDeviceRoomAssignmentOutcome::Unchanged);
        }

        let target_hub_room_id = match assignment.target_rhythm_room_id.as_deref() {
            Some(target_room_id) => match assignment.target_hub_room_ids.as_slice() {
                [target_hub_room_id] => Some(target_hub_room_id.as_str()),
                [] => {
                    return Err(anyhow::anyhow!(
                        "Cannot move Hue light to Rhythm room '{}': that room is not backed by this Hue bridge",
                        target_room_id
                    ));
                }
                _ => {
                    return Err(anyhow::anyhow!(
                        "Cannot move Hue light to Rhythm room '{}': it maps to multiple rooms on this Hue bridge",
                        target_room_id
                    ));
                }
            },
            None => None,
        };

        let (bridge_ip, username) = {
            let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            let hue = state
                .hubs
                .get(&assignment.hub_key)
                .and_then(|hub| hub.data::<HueHubData>())
                .ok_or_else(|| anyhow::anyhow!("Hue hub is not active: {}", assignment.hub_key))?;
            (hue.bridge_ip.clone(), hue.username.clone())
        };
        let transport = ReqwestHueTransport::new(&bridge_ip)?;
        let rollback = crate::room_membership::reassign_device_room(
            &transport,
            &username,
            &assignment.native_device_id,
            target_hub_room_id,
        )?;
        Ok(HubDeviceRoomAssignmentOutcome::Reassigned {
            target_hub_room_id: target_hub_room_id.map(str::to_string),
            rollback: Box::new(move || rollback.rollback(&transport, &username)),
        })
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
        |config, registry, shutdown, sse_liveness| {
            start_event_stream(
                config.bridge_ip,
                config.username,
                registry,
                shutdown,
                sse_liveness,
            )
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
    sse_liveness: Arc<HueSseLiveness>,
) -> Receiver<HubEvent> {
    let sse_config = HueSseConfig {
        bridge_ip,
        username,
    };

    let sse_rx = start_reqwest_sse(sse_config, shutdown.clone(), sse_liveness);

    crate::events::start_event_translator(sse_rx, registry, shutdown, None, None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    use rhythm_core::{
        InputEvent, LightProfileConfig, LightingCommand, ModeConfig, RestoredRoomState,
        RoomSnapshot, RuntimeHandle, SolarTime,
    };
    use rhythm_os::hub::HubCredentials;
    use rhythm_os::state::AppState;

    use crate::provider::hue_credentials;

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

    fn shared_state() -> SharedState {
        Arc::new(Mutex::new(AppState::default()))
    }

    fn hue_key(address: &str) -> HubKey {
        HubKey::new(HubType::new(HubType::HUE), address)
    }

    fn string_error<T>(result: Result<T>) -> String {
        match result {
            Ok(_) => panic!("expected error"),
            Err(error) => error.to_string(),
        }
    }

    fn active_hue_hub(key: HubKey) -> ActiveHub {
        let registry = Arc::new(Mutex::new(HueDeviceRegistry::with_options(false)));
        ActiveHub {
            hub_type: HubType::new(HubType::HUE),
            hub_key: key,
            runtime: None,
            hub_data: Box::new(HueHubData {
                bridge_ip: "192.0.2.10".to_string(),
                username: "user-123".to_string(),
                registry,
                sse_liveness: Arc::new(HueSseLiveness::default()),
            }),
            registry: None,
            discovery: None,
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    #[test]
    fn provider_and_integration_report_hue_metadata() {
        let provider = get_hub_provider();
        assert_eq!(provider.hub_type().as_str(), HubType::HUE);
        assert_eq!(INTEGRATION.hub_type(), HubType::HUE);
        assert_eq!(INTEGRATION.provider().hub_type().as_str(), HubType::HUE);
        let caps = INTEGRATION.api_capabilities();
        assert_eq!(caps.hub_type, HubType::HUE);
        assert!(caps.configurable);
    }

    #[test]
    fn hue_light_move_requires_exactly_one_native_target_room() {
        let state = shared_state();
        let assignment = |target_hub_room_ids: Vec<String>| HubDeviceRoomAssignment {
            hub_key: hue_key("192.0.2.10"),
            native_device_id: "light-device".to_string(),
            device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
            target_rhythm_room_id: Some("office".to_string()),
            target_hub_room_ids,
        };

        let missing = string_error(
            INTEGRATION.prepare_device_room_assignment(&state, &assignment(Vec::new())),
        );
        assert!(missing.contains("not backed by this Hue bridge"));

        let ambiguous = string_error(INTEGRATION.prepare_device_room_assignment(
            &state,
            &assignment(vec!["hue-office-a".to_string(), "hue-office-b".to_string()]),
        ));
        assert!(ambiguous.contains("maps to multiple rooms"));
    }

    #[test]
    fn connect_and_start_and_private_connect_report_missing_credentials() {
        let state = shared_state();
        let key = hue_key("192.0.2.10");

        assert!(string_error(connect_and_start(state.clone(), &key))
            .contains("No hub credentials configured"));
        assert!(string_error(connect_hue_sse(&state, &key, None))
            .contains("No hub credentials configured"));
    }

    #[test]
    fn ensure_runtime_returns_when_runtime_already_exists() {
        let state = shared_state();
        let key = hue_key("192.0.2.10");
        state
            .lock()
            .unwrap()
            .hubs
            .insert(key, active_hue_hub(hue_key("192.0.2.10")));

        state
            .lock()
            .unwrap()
            .hubs
            .values_mut()
            .next()
            .unwrap()
            .runtime = Some(Arc::new(NoopRuntime));

        ensure_runtime(&state).unwrap();
    }

    #[test]
    fn create_hue_controller_validates_credentials_and_active_hub_before_transport_use() {
        let state = shared_state();
        let key = hue_key("192.0.2.10");

        assert_eq!(
            string_error(create_hue_controller(&state, &key)),
            format!("No Hue credentials for {}", key)
        );

        state.lock().unwrap().hub_credentials.insert(
            key.clone(),
            HubCredentials::new(HubType::HUE, "192.0.2.10", serde_json::json!({})),
        );
        assert_eq!(
            string_error(create_hue_controller(&state, &key)),
            "Hue credentials missing username"
        );

        state
            .lock()
            .unwrap()
            .hub_credentials
            .insert(key.clone(), hue_credentials("192.0.2.10", "user-123"));
        assert_eq!(
            string_error(create_hue_controller(&state, &key)),
            format!("Hue hub not active for {}", key)
        );
    }

    #[test]
    fn create_hue_controller_builds_controller_for_active_hub() {
        let state = shared_state();
        let key = hue_key("192.0.2.10");
        {
            let mut guard = state.lock().unwrap();
            guard
                .hub_credentials
                .insert(key.clone(), hue_credentials("192.0.2.10", "user-123"));
            guard.hubs.insert(key.clone(), active_hue_hub(key.clone()));
        }

        let controller = create_hue_controller(&state, &key).unwrap();

        assert_eq!(Arc::strong_count(&controller), 1);
    }

    #[test]
    fn reqwest_provider_rejects_invalid_credentials_without_connecting() {
        let state = shared_state();
        let provider = ReqwestHueHubProvider;

        let error = provider
            .configure("192.0.2.10", "{}", &state)
            .unwrap_err()
            .to_string();

        assert!(error.contains("Invalid Hue credentials"));
        assert!(state.lock().unwrap().hubs.is_empty());
    }
}
