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
use crate::sse_liveness::HueSseLiveness;

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
        Arc<HueSseLiveness>,
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
    let sse_liveness = Arc::new(HueSseLiveness::default());
    let hub_sse_liveness = sse_liveness.clone();
    let stream_sse_liveness = sse_liveness.clone();

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
                sse_liveness: hub_sse_liveness,
            })
        },
        // start_event_stream: wrap the platform closure with SSE config
        move |registry, shutdown| {
            let sse_config = HueSseConnectConfig {
                bridge_ip,
                username,
            };
            start_event_stream(sse_config, registry, shutdown, stream_sse_liveness)
        },
    )
}

/// Create the RhythmRuntime for a Hue hub, generic over transport.
///
/// Reads credentials, registry, and config from AppState.
/// Creates `HueLightController<H>`, `SystemTimeProvider`, `ThreadScheduler`,
/// and `RhythmRuntime`. Restores persisted room state from storage.
pub fn ensure_hue_runtime<H: crate::transport::HueTransport + 'static>(
    state: &SharedState,
    transport: H,
) -> Result<()> {
    use crate::controller::HueLightController;
    use log::warn;

    let (hub_key, username, registry, sse_liveness) = {
        let s = state
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock state"))?;

        let (hub_key, registry, sse_liveness) = s
            .hubs
            .iter()
            .find_map(|(key, hub)| {
                hub.data::<HueHubData>()
                    .map(|hue| (key.clone(), hue.registry.clone(), hue.sse_liveness.clone()))
            })
            .ok_or_else(|| anyhow::anyhow!("Hue hub not active (call connect_sse first)"))?;

        let hue_creds = s
            .hub_credentials
            .get(&hub_key)
            .ok_or_else(|| anyhow::anyhow!("No hub credentials configured"))?;
        let user = crate::provider::hue_username(hue_creds)
            .ok_or_else(|| anyhow::anyhow!("Hue credentials not configured"))?
            .to_string();

        (hub_key, user, registry, sse_liveness)
    };

    let bridge_id = crate::ownership::connected_hue_bridge_id(&transport, &username)?;
    let controller = HueLightController::new(transport, username, registry.clone())
        .with_capability_source(state.clone(), hub_key.clone())
        .with_sse_liveness(sse_liveness)
        .with_controller_operation_lock(crate::ownership::controller_operation_lock(&bridge_id));

    rhythm_os::lifecycle::ensure_hub_runtime(
        state,
        &hub_key,
        controller,
        registry,
        Some(Box::new(|c: &HueLightController<H>| {
            if let Err(e) = c.warmup_tls() {
                warn!(target: "sys", "TLS warmup failed (will retry on first command): {}", e);
            }
        })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::mpsc;

    use serde_json::Value;

    use rhythm_os::hub::HubCredentials;
    use rhythm_os::state::AppState;

    use crate::provider::hue_credentials;
    use crate::transport::HueTransport;

    struct FakeHueTransport;

    impl HueTransport for FakeHueTransport {
        fn test_connection(&self, _username: &str) -> anyhow::Result<bool> {
            Ok(true)
        }

        fn warmup_tls(&self) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_grouped_light(
            &self,
            _username: &str,
            _grouped_light_id: &str,
            _on: bool,
            _brightness: Option<u8>,
            _kelvin: Option<u16>,
            _xy: Option<(f32, f32)>,
            _fade_ms: Option<u16>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_light(
            &self,
            _username: &str,
            _light_id: &str,
            _on: bool,
            _brightness: Option<u8>,
            _kelvin: Option<u16>,
            _xy: Option<(f32, f32)>,
            _fade_ms: Option<u16>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn is_grouped_light_on(
            &self,
            _username: &str,
            _grouped_light_id: &str,
        ) -> anyhow::Result<bool> {
            Ok(false)
        }

        fn is_light_on(&self, _username: &str, _light_id: &str) -> anyhow::Result<bool> {
            Ok(false)
        }

        fn identify_light(&self, _username: &str, _light_id: &str) -> anyhow::Result<()> {
            Ok(())
        }

        fn get_resources(&self, _username: &str, _resource_type: &str) -> anyhow::Result<Value> {
            Ok(serde_json::json!({"data":[]}))
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

    #[test]
    fn connect_hue_sse_requires_credentials_and_username() {
        let state = shared_state();
        let key = hue_key("192.0.2.10");

        let missing = string_error(connect_hue_sse(
            &state,
            key.clone(),
            None,
            |_config, _registry, _shutdown, _sse_liveness| {
                panic!("missing credentials must not start SSE")
            },
        ));
        assert!(missing.contains("No hub credentials configured"));

        state.lock().unwrap().hub_credentials.insert(
            key.clone(),
            HubCredentials::new(HubType::HUE, "192.0.2.10", serde_json::json!({})),
        );
        let invalid = string_error(connect_hue_sse(
            &state,
            key,
            None,
            |_config, _registry, _shutdown, _sse_liveness| {
                panic!("invalid credentials must not start SSE")
            },
        ));
        assert_eq!(invalid, "Hue credentials not configured");
    }

    #[test]
    fn connect_hue_sse_builds_hub_data_and_passes_sse_config() {
        let state = shared_state();
        let key = hue_key("192.0.2.10");
        state
            .lock()
            .unwrap()
            .hub_credentials
            .insert(key.clone(), hue_credentials("192.0.2.10", "user-123"));

        let (raw_tx, raw_rx) = mpsc::channel();
        let (hub, event_rx) = connect_hue_sse(
            &state,
            key.clone(),
            None,
            move |config, registry, shutdown, sse_liveness| {
                assert_eq!(config.bridge_ip, "192.0.2.10");
                assert_eq!(config.username, "user-123");
                assert!(!shutdown.load(std::sync::atomic::Ordering::Relaxed));
                assert_eq!(sse_liveness.pending_count(), 0);
                registry
                    .lock()
                    .unwrap()
                    .upsert_room("room-1", "Kitchen", "grouped-1", &[]);
                raw_rx
            },
        )
        .unwrap();

        let data = hub.data::<HueHubData>().unwrap();
        assert_eq!(data.bridge_ip, "192.0.2.10");
        assert_eq!(data.username, "user-123");
        assert_eq!(
            data.registry.lock().unwrap().room_name("room-1"),
            Some("Kitchen")
        );

        raw_tx.send(HubEvent::Connected { hub_key: None }).unwrap();
        assert_eq!(
            event_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap()
                .hub_key(),
            Some(&key)
        );
    }

    #[test]
    fn ensure_hue_runtime_reports_missing_active_hub_or_credentials() {
        let state = shared_state();
        let no_hub = ensure_hue_runtime(&state, FakeHueTransport).unwrap_err();
        assert_eq!(
            no_hub.to_string(),
            "Hue hub not active (call connect_sse first)"
        );

        let key = hue_key("192.0.2.10");
        let registry = Arc::new(Mutex::new(HubDeviceRegistry::with_options(false)));
        state.lock().unwrap().hubs.insert(
            key,
            ActiveHub {
                hub_type: HubType::new(HubType::HUE),
                hub_key: hue_key("192.0.2.10"),
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
            },
        );

        let no_creds = ensure_hue_runtime(&state, FakeHueTransport).unwrap_err();
        assert_eq!(no_creds.to_string(), "No hub credentials configured");
    }
}
