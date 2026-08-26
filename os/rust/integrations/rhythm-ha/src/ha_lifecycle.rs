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

use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use log::info;

use crate::hub_state::{HaEventRoutingCache, HaHubData};
use crate::registry::HaDeviceRegistry;
use crate::transport::HaConnectionConfig;

use rhythm_os::button_resolve::RawButtonEvent;
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::{ActiveHub, HubEvent, HubType};
use rhythm_os::registry::RegistrySnapshot;
use rhythm_os::state::SharedState;

/// HA hub type constant.
pub const HA_HUB_TYPE: &str = "homeassistant";

/// Connect to HA and set up the hub (generic over transport).
///
/// Creates an event routing cache (initially empty) and shares it with
/// both `HaHubData` (for later population) and the event translator
/// (for on-demand button discovery).
///
/// The `start_event_stream` closure is provided by the platform crate to
/// handle the actual WS transport. It receives the routing cache so the event
/// translator can resolve buttons and camera motion sub-events.
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
        Arc<Mutex<HaEventRoutingCache>>,
    ) -> Receiver<HubEvent>,
{
    info!(target: "sys", "Connecting to HA at {}:{}...", config.host, config.port);

    let config_clone = config.clone();
    let event_routing_cache = Arc::new(Mutex::new(HaEventRoutingCache::default()));
    let cache_for_hub = event_routing_cache.clone();

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
                event_routing_cache: cache_for_hub,
            })
        },
        // start_event_stream: wrap the platform closure with HA config + cache
        move |registry, shutdown| {
            start_event_stream(&config, registry, shutdown, event_routing_cache)
        },
    )
}

/// Create the RhythmRuntime for an HA hub, generic over transport.
///
/// Reads credentials, registry, and config from AppState.
/// Creates `HaLightController<H>`, `SystemTimeProvider`, `ThreadScheduler`,
/// and `RhythmRuntime`. Restores persisted room state from storage.
pub fn ensure_ha_runtime<H: crate::transport::HaTransport + 'static>(
    state: &SharedState,
    transport: H,
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
        .with_capability_source(state.clone(), hub_key.clone());

    rhythm_os::lifecycle::ensure_hub_runtime(
        state, &hub_key, controller, registry, None, // No TLS warmup for HA
    )
}

/// Spawn a translator thread that converts raw HA WS events to hub-agnostic `HubEvent`s.
///
/// Like Hue, discovery behavior is provided via callbacks. Desktop/server
/// wrappers can build cache-backed callbacks, while tests and future platforms
/// can inject custom registration logic directly.
#[allow(clippy::type_complexity)]
pub fn start_event_translator(
    ws_rx: Receiver<HaWsEvent>,
    registry: Arc<Mutex<HaDeviceRegistry>>,
    shutdown: Arc<AtomicBool>,
    event_routing_cache: Arc<Mutex<HaEventRoutingCache>>,
    on_activity: Option<Arc<dyn Fn() + Send + Sync>>,
    on_unknown_button: Option<Arc<dyn Fn(&RawButtonEvent) + Send + Sync>>,
    on_unknown_motion: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    on_unknown_contact: Option<Arc<dyn Fn(&str) + Send + Sync>>,
) -> Receiver<HubEvent> {
    rhythm_os::lifecycle::start_event_translator(
        ws_rx,
        move |event| {
            let activity_ref: Option<&dyn Fn()> =
                on_activity.as_ref().map(|f| f.as_ref() as &dyn Fn());
            let unknown_button_ref: Option<&dyn Fn(&RawButtonEvent)> = on_unknown_button
                .as_ref()
                .map(|f| f.as_ref() as &dyn Fn(&RawButtonEvent));
            let unknown_motion_ref: Option<&dyn Fn(&str)> = on_unknown_motion
                .as_ref()
                .map(|f| f.as_ref() as &dyn Fn(&str));
            let unknown_contact_ref: Option<&dyn Fn(&str)> = on_unknown_contact
                .as_ref()
                .map(|f| f.as_ref() as &dyn Fn(&str));

            match event {
                HaWsEvent::Connected => vec![HubEvent::Connected { hub_key: None }],
                HaWsEvent::ServiceEvent { event_type, data } => {
                    crate::events::translate_ws_event_with_hooks(
                        event_type,
                        data,
                        &registry,
                        &event_routing_cache,
                        activity_ref,
                        unknown_button_ref,
                        unknown_motion_ref,
                        unknown_contact_ref,
                    )
                }
                HaWsEvent::Heartbeat => vec![HubEvent::Heartbeat { hub_key: None }],
                HaWsEvent::Disconnected(reason) => vec![HubEvent::Disconnected {
                    hub_key: None,
                    reason: reason.clone(),
                }],
            }
        },
        shutdown,
        "ha-evt",
        None, // activity is handled inside the translate closure
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::mpsc;

    use serde_json::Value;

    use rhythm_os::state::AppState;

    use crate::transport::{EntityState, HaTransport};

    struct FakeHaTransport;

    impl HaTransport for FakeHaTransport {
        fn call_service(&self, _domain: &str, _service: &str, _data: &Value) -> Result<()> {
            Ok(())
        }

        fn get_states(&self) -> Result<Vec<EntityState>> {
            Ok(Vec::new())
        }

        fn get_state(&self, entity_id: &str) -> Result<EntityState> {
            Ok(EntityState {
                entity_id: entity_id.to_string(),
                state: "off".to_string(),
                attributes: Value::Null,
            })
        }

        fn test_connection(&self) -> Result<bool> {
            Ok(true)
        }
    }

    fn shared_state() -> SharedState {
        Arc::new(Mutex::new(AppState::default()))
    }

    fn ha_key(address: &str) -> HubKey {
        HubKey::new(HubType::new(HA_HUB_TYPE), address)
    }

    fn ha_config() -> HaConnectionConfig {
        HaConnectionConfig {
            host: "ha.local".to_string(),
            port: 8123,
            token: "token-1".to_string(),
            use_ssl: false,
        }
    }

    #[test]
    fn connect_ha_builds_hub_data_cache_and_tags_events() {
        let state = shared_state();
        let key = ha_key("ha.local");
        let (raw_tx, raw_rx) = mpsc::channel();

        let (hub, event_rx) = connect_ha(
            &state,
            key.clone(),
            ha_config(),
            None,
            move |config, registry, shutdown, cache| {
                assert_eq!(config.host, "ha.local");
                assert_eq!(config.port, 8123);
                assert!(!shutdown.load(std::sync::atomic::Ordering::Relaxed));
                registry
                    .lock()
                    .unwrap()
                    .upsert_room("area-1", "Kitchen", "", &[]);
                cache
                    .lock()
                    .unwrap()
                    .insert("device-1".to_string(), "area-1".to_string());
                raw_rx
            },
        )
        .unwrap();

        let data = hub.data::<HaHubData>().unwrap();
        assert_eq!(data.config.host, "ha.local");
        assert_eq!(
            data.registry.lock().unwrap().room_name("area-1"),
            Some("Kitchen")
        );
        assert_eq!(
            data.event_routing_cache
                .lock()
                .unwrap()
                .get("device-1")
                .cloned(),
            Some("area-1".to_string())
        );

        raw_tx.send(HubEvent::Heartbeat { hub_key: None }).unwrap();
        assert_eq!(
            event_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap()
                .hub_key(),
            Some(&key)
        );
    }

    #[test]
    fn ensure_ha_runtime_reports_missing_active_hub() {
        let state = shared_state();

        let error = ensure_ha_runtime(&state, FakeHaTransport).unwrap_err();

        assert_eq!(error.to_string(), "HA hub not active (call connect first)");
    }

    #[test]
    fn start_event_translator_maps_connection_heartbeat_and_disconnect() {
        let (tx, rx) = mpsc::channel();
        let registry = Arc::new(Mutex::new(HaDeviceRegistry::with_options(true)));
        let hub_rx = start_event_translator(
            rx,
            registry,
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(HaEventRoutingCache::default())),
            None,
            None,
            None,
            None,
        );

        tx.send(HaWsEvent::Connected).unwrap();
        tx.send(HaWsEvent::Heartbeat).unwrap();
        tx.send(HaWsEvent::Disconnected("closed".to_string()))
            .unwrap();

        assert!(matches!(
            hub_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            HubEvent::Connected { hub_key: None }
        ));
        assert!(matches!(
            hub_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            HubEvent::Heartbeat { hub_key: None }
        ));
        assert!(matches!(
            hub_rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),
            HubEvent::Disconnected {
                hub_key: None,
                reason
            } if reason == "closed"
        ));
    }
}
