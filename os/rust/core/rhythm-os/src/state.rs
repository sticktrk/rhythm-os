//! Application state shared across threads.
//!
//! Platform-agnostic `AppState` with `dyn Storage` instead of NVS.

use std::collections::HashMap;
use std::sync::atomic::AtomicU16;
use std::sync::{Arc, Mutex};

use rhythm_core::CurveConfig;
use rhythm_core::{ButtonAction, RuntimeConfig, RuntimeHandle};

use crate::canonical::identity::HubKey;
use crate::canonical::registry::CanonicalRegistry;
use crate::hub::{ActiveHub, HubCredentials, HubEvent};
use crate::storage::Storage;
use crate::topology::RoomTopologyStore;

/// Work items for background processing.
///
/// Heavy operations (TLS to hub, float math, JSON formatting) are
/// offloaded to a worker thread to avoid blocking the main/HTTP thread.
pub enum WorkItem {
    /// Execute a button action (involves TLS to hub).
    ButtonAction {
        room_id: String,
        action: ButtonAction,
        device_id: Option<String>,
    },
    /// Periodic update for a single room (holds engine lock only briefly).
    PeriodicRoomTick { room_id: String, current_hour: f32 },
    /// Deferred persist after inline button processing.
    DeferredPersist { room_id: String },
    /// Full state persist (registry + rooms) after batch HTTP operations.
    ///
    /// Unlike `DeferredPersist` (rooms-only), batch room updates modify the
    /// registry too, so this runs the full `persist_state` on the worker
    /// thread (16KB stack) instead of the HTTP handler stack (12KB).
    DeferredPersistState,
}

/// Snapshot of motion timer state for a single room, exposed via API.
#[derive(Clone)]
pub struct MotionSnapshot {
    /// Any sensor in this room still detecting motion.
    pub motion_active: bool,
    /// Motion controls the lights (vs. manual button press took over).
    pub motion_owned: bool,
    /// None if any sensor still active, Some(secs) if all cleared and counting down.
    pub remaining_secs: Option<u64>,
    /// Configured timeout for this room.
    pub timeout_secs: u64,
    /// Room is currently in warning dim state (dimmed before timeout).
    pub warning_active: bool,
}

/// Platform-specific tuning for stack sizes and resource limits.
///
/// Embedded targets (ESP32) have tight memory and need small stacks.
/// Desktop/server targets can use the OS defaults (typically 8MB).
#[derive(Clone, Debug)]
pub struct PlatformConfig {
    /// Stack size for the runtime-init thread (creates TLS clients, serde, etc.).
    pub runtime_init_stack: usize,
    /// Stack size for scheduler periodic threads (Hue HTTPS calls).
    /// `None` = OS default.
    pub scheduler_stack: Option<usize>,
    /// Stack size for SSE event translator threads.
    /// `None` = OS default.
    pub event_thread_stack: Option<usize>,
    /// Whether to eagerly establish the runtime's TLS connection at startup.
    ///
    /// When `true` (desktop default), `warmup_tls()` is called during runtime
    /// creation so the first light command doesn't pay the TLS handshake cost.
    /// When `false` (embedded default), TLS is deferred until the first
    /// periodic tick (~60s), avoiding a third simultaneous TLS session during
    /// hub sync which would exceed the ESP32's heap budget.
    pub eager_tls_warmup: bool,
    /// Whether `sync_from_hub` should discover devices and sensors.
    ///
    /// When `true` (desktop default), room sync also fetches the device
    /// endpoint to map buttons and motion sensors in one shot.
    /// When `false` (embedded default), only rooms are synced — device/sensor
    /// mappings arrive organically via the SSE event stream, avoiding the
    /// large device endpoint response that can OOM the ESP32.
    pub full_device_discovery: bool,
}

impl PlatformConfig {
    /// Preset for ESP32 and other embedded targets.
    pub fn embedded() -> Self {
        Self {
            runtime_init_stack: 16 * 1024,
            scheduler_stack: Some(16 * 1024),
            event_thread_stack: Some(16 * 1024),
            eager_tls_warmup: false,
            full_device_discovery: false,
        }
    }

    /// Preset for desktop/server targets (macOS, Linux, Windows).
    pub fn desktop() -> Self {
        Self {
            runtime_init_stack: 2 * 1024 * 1024,
            scheduler_stack: None,
            event_thread_stack: None,
            eager_tls_warmup: true,
            full_device_discovery: true,
        }
    }
}

impl Default for PlatformConfig {
    fn default() -> Self {
        Self::desktop()
    }
}

/// Application state shared across threads.
pub struct AppState {
    /// Adaptive lighting configuration.
    pub config: CurveConfig,
    /// Runtime configuration.
    pub runtime_config: RuntimeConfig,
    /// UTC offset in hours (e.g., -5.0 for EST, -8.0 for PST).
    pub utc_offset_hours: f32,
    /// Latitude for solar calculations.
    pub latitude: Option<f32>,
    /// Longitude for solar calculations.
    pub longitude: Option<f32>,
    /// IANA timezone name (e.g., "America/New_York").
    /// Set when the client or HA provides a timezone; used for DST-aware solar noon.
    pub timezone_name: Option<String>,

    // ---- Hub abstraction ----
    /// Active hubs keyed by HubKey. Supports multiple simultaneous hubs.
    pub hubs: HashMap<HubKey, ActiveHub>,
    /// Hub credentials keyed by HubKey. Supports multiple simultaneous hubs.
    pub hub_credentials: HashMap<HubKey, HubCredentials>,

    // ---- Canonical device registry + topology ----
    /// Canonical device registry (cross-hub device identity and dedup).
    pub canonical_registry: CanonicalRegistry,
    /// Room topology store (Rhythm's own room hierarchy).
    pub topology: RoomTopologyStore,

    // ---- Room state (all keyed by topology room IDs) ----
    /// Per-room lights-on state, updated by actions and hub events.
    /// Keyed by **topology room IDs** (not hub-native IDs).
    ///
    /// Used to include `lights_on` in SSE events so clients don't need
    /// to poll the hub directly.
    pub room_lights_on: HashMap<String, bool>,

    /// Per-room motion timeout in seconds.
    /// Keyed by **topology room IDs** (not hub-native IDs).
    pub motion_timeouts: HashMap<String, u64>,
    /// Per-room motion timer snapshots, updated by the main loop.
    /// Keyed by **topology room IDs** (not hub-native IDs).
    pub motion_snapshots: HashMap<String, MotionSnapshot>,
    /// Last periodic update hour (for solar midnight detection).
    pub last_check_hour: Option<f32>,

    // ---- Global settings ----
    /// Hue dynamics fade duration in milliseconds (default 500).
    pub bulb_fade_ms: u16,
    /// Atomic copy of `bulb_fade_ms` shared with HueLightController.
    pub bulb_fade_atomic: Arc<AtomicU16>,
    /// Default motion timeout in seconds when no per-room value is configured.
    pub default_motion_timeout_secs: u64,
    /// Power save mode. When false, lights dim to soft-off brightness
    /// instead of turning fully off.
    pub power_save: bool,
    /// Soft-off brightness percentage (1-100).
    pub soft_off_brightness: u8,

    // ---- Storage ----
    /// Platform-specific storage backend.
    pub storage: Option<Box<dyn Storage>>,

    // ---- Worker ----
    /// Sender for offloading work to a background thread.
    pub work_tx: Option<std::sync::mpsc::SyncSender<WorkItem>>,
    /// Pending hub event receivers from hub reconfiguration (picked up by main loop).
    /// Multiple hubs produce multiple receivers — the event loop drains this Vec.
    pub pending_hub_event_rxs: Vec<std::sync::mpsc::Receiver<HubEvent>>,
    /// Room IDs whose motion timers should be cleared (picked up by event loop).
    pub pending_motion_clear: Vec<String>,
    /// Active motion sensors from startup prefetch (picked up by event loop).
    /// Vec of (sensor_entity_id, room_id) for sensors with state="on" at boot.
    pub pending_motion_seed: Vec<(String, String)>,

    // ---- Composite controller ----
    /// The composite controller shared between AppState (for dynamic registration)
    /// and the RhythmEngine (for light control). Both hold Arc refs to the same instance.
    /// None on ESP32 or before runtime creation.
    #[cfg(feature = "desktop")]
    pub composite_controller: Option<Arc<rhythm_core::CompositeController>>,

    // ---- Callbacks ----
    /// Called on hub heartbeat (e.g., ESP32 updates diag vitals).
    pub on_hub_heartbeat: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Called on hub disconnect (e.g., ESP32 updates diag state).
    pub on_hub_disconnect: Option<Arc<dyn Fn() + Send + Sync>>,

    /// Platform-specific runtime initialization callback.
    /// Called when the first room arrives and the runtime needs to be created.
    /// Uses `Arc` so it can be cloned out of the AppState lock before calling.
    #[allow(clippy::type_complexity)]
    pub ensure_runtime_fn: Option<Arc<dyn Fn(&SharedState) -> anyhow::Result<()> + Send + Sync>>,

    /// Create and register a per-hub controller with the composite controller.
    /// Called when a new hub is configured via HTTP while the runtime is already running.
    /// The callback finds the integration by hub type, calls `create_controller`,
    /// and registers the result with `composite_controller`.
    #[cfg(feature = "desktop")]
    #[allow(clippy::type_complexity)]
    pub register_controller_fn: Option<
        Arc<
            dyn Fn(&SharedState, &crate::canonical::identity::HubKey) -> anyhow::Result<()>
                + Send
                + Sync,
        >,
    >,

    /// Platform-specific hub provider lookup.
    /// Returns a hub provider for a given hub type.
    pub get_hub_provider_fn: Option<
        Arc<dyn Fn(crate::hub::HubType) -> &'static (dyn crate::hub::HubProvider) + Send + Sync>,
    >,

    /// Start a device pairing session (Matter, Zigbee, etc.).
    /// Built from the integration registry by `integration_callbacks`.
    #[allow(clippy::type_complexity)]
    pub start_pairing_fn: Option<
        Arc<
            dyn Fn(&SharedState, &str, &serde_json::Value) -> anyhow::Result<crate::pairing::PairingSession>
                + Send
                + Sync,
        >,
    >,

    /// Optional pre-handler for hub credential requests.
    ///
    /// Returns `Some(Ok(json))` to respond with 200, `Some(Err(msg))` for 500,
    /// or `None` to fall through to the default handler.
    /// Used by the addon to auto-fill SUPERVISOR_TOKEN for HA requests.
    #[allow(clippy::type_complexity)]
    pub hub_credentials_interceptor: Option<
        Arc<
            dyn Fn(&SharedState, &serde_json::Value) -> Option<Result<String, String>>
                + Send
                + Sync,
        >,
    >,

    /// Firmware version string (set by the binary crate).
    pub firmware_version: &'static str,

    /// Platform type: "desktop" or "embedded".
    pub platform_type: &'static str,

    /// Deployment context: "ha_addon", "server", "embedded", etc.
    pub platform_context: &'static str,

    /// Base data directory for persistence (set by binary crate).
    /// Used by integrations that need filesystem paths (e.g., Matter fabric data).
    pub data_dir: String,

    /// The port the HTTP server is listening on.
    /// Exposed in the state snapshot so web clients can connect directly
    /// (bypassing reverse proxies like HA ingress) for SSE.
    pub listen_port: Option<u16>,

    /// Platform-specific tuning (stack sizes, resource limits).
    pub platform: PlatformConfig,

    /// Broadcast sender for SSE server events.
    #[cfg(feature = "desktop")]
    pub event_tx: Option<tokio::sync::broadcast::Sender<crate::server_event::ServerEvent>>,
}

impl Default for AppState {
    fn default() -> Self {
        use rhythm_core::primitives::{
            DEFAULT_BULB_FADE_MS, DEFAULT_MOTION_TIMEOUT_SECS, DEFAULT_SOFT_OFF_BRIGHTNESS,
        };

        let bulb_fade_atomic = Arc::new(AtomicU16::new(DEFAULT_BULB_FADE_MS));
        Self {
            config: CurveConfig::default(),
            runtime_config: RuntimeConfig::default().with_solar_noon(12.5),
            utc_offset_hours: 0.0,
            latitude: None,
            longitude: None,
            timezone_name: None,
            hubs: HashMap::new(),
            hub_credentials: HashMap::new(),
            canonical_registry: CanonicalRegistry::new(),
            topology: RoomTopologyStore::new(),
            room_lights_on: HashMap::new(),
            motion_timeouts: HashMap::new(),
            motion_snapshots: HashMap::new(),
            last_check_hour: None,
            bulb_fade_ms: DEFAULT_BULB_FADE_MS,
            bulb_fade_atomic,
            default_motion_timeout_secs: DEFAULT_MOTION_TIMEOUT_SECS,
            power_save: false,
            soft_off_brightness: DEFAULT_SOFT_OFF_BRIGHTNESS,
            storage: None,
            work_tx: None,
            pending_hub_event_rxs: Vec::new(),
            pending_motion_clear: Vec::new(),
            pending_motion_seed: Vec::new(),
            #[cfg(feature = "desktop")]
            composite_controller: None,
            on_hub_heartbeat: None,
            on_hub_disconnect: None,
            ensure_runtime_fn: None,
            #[cfg(feature = "desktop")]
            register_controller_fn: None,
            get_hub_provider_fn: None,
            start_pairing_fn: None,
            hub_credentials_interceptor: None,
            firmware_version: "0.0.0",
            platform_type: "desktop",
            platform_context: "server",
            data_dir: String::new(),
            listen_port: None,
            platform: PlatformConfig::default(),
            #[cfg(feature = "desktop")]
            event_tx: None,
        }
    }
}

impl AppState {
    /// Get the type-erased runtime handle for the first available hub.
    pub fn hub_runtime(&self) -> Option<Arc<dyn RuntimeHandle>> {
        self.hubs.values().find_map(|hub| hub.runtime.clone())
    }

    /// Get the runtime handle for a specific hub.
    pub fn hub_runtime_for(&self, key: &HubKey) -> Option<Arc<dyn RuntimeHandle>> {
        self.hubs.get(key)?.runtime.clone()
    }

    /// Get all hub runtimes.
    pub fn all_hub_runtimes(&self) -> Vec<(HubKey, Arc<dyn RuntimeHandle>)> {
        self.hubs
            .iter()
            .filter_map(|(key, hub)| hub.runtime.as_ref().map(|rt| (key.clone(), rt.clone())))
            .collect()
    }

    /// Get the hub registry for a specific hub.
    pub fn hub_registry_for(
        &self,
        key: &HubKey,
    ) -> Option<Arc<Mutex<dyn rhythm_core::HubRegistry>>> {
        self.hubs.get(key)?.registry.clone()
    }

    /// Get registries from ALL active hubs.
    pub fn all_hub_registries(&self) -> Vec<Arc<Mutex<dyn rhythm_core::HubRegistry>>> {
        self.hubs
            .values()
            .filter_map(|hub| hub.registry.clone())
            .collect()
    }

    /// Get topology room IDs that have motion sensors, across all hubs.
    /// Translates hub-native room IDs to topology IDs.
    pub fn motion_sensor_room_ids(&self) -> std::collections::HashSet<String> {
        let mut ids = std::collections::HashSet::new();
        for hub in self.hubs.values() {
            if let Some(ref reg_arc) = hub.registry {
                if let Ok(reg) = reg_arc.lock() {
                    for room_id in reg.rooms_with_motion_sensors() {
                        let translated = self
                            .topology
                            .translate_room_id_any_hub(&room_id)
                            .map(|s| s.to_string())
                            .unwrap_or(room_id);
                        ids.insert(translated);
                    }
                }
            }
        }
        ids
    }

    /// Check if any hub is active.
    pub fn has_any_hub(&self) -> bool {
        !self.hubs.is_empty()
    }

    /// Get the first configured hub credentials (for legacy single-hub callers).
    pub fn first_hub_credentials(&self) -> Option<&HubCredentials> {
        self.hub_credentials.values().next()
    }

    /// Get solar noon hour from runtime config.
    pub fn solar_noon_hour(&self) -> f32 {
        self.runtime_config.solar_noon_hour
    }

    /// Get solar midnight hour.
    pub fn solar_midnight_hour(&self) -> f32 {
        self.runtime_config.solar_midnight_hour()
    }
}

#[cfg(feature = "desktop")]
impl AppState {
    /// Emit a server event to all connected SSE clients.
    pub fn emit_event(&self, event: crate::server_event::ServerEvent) {
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(event);
        }
    }
}

/// Shared state type for thread-safe access.
pub type SharedState = Arc<Mutex<AppState>>;

/// Emit a server event, briefly locking state to access the broadcast sender.
#[cfg(feature = "desktop")]
pub fn emit_server_event(state: &SharedState, event: crate::server_event::ServerEvent) {
    if let Ok(s) = state.lock() {
        s.emit_event(event);
    }
}

/// Build a RoomManager from the engine's current room snapshots.
///
/// Used for persistence — converts the engine's authoritative state
/// into a serializable RoomManager without needing AppState.rooms.
pub fn rooms_from_engine(runtime: &dyn RuntimeHandle) -> rhythm_core::room::RoomManager {
    let mut rooms = rhythm_core::room::RoomManager::new();
    for snap in runtime.engine_all_room_snapshots() {
        let room = rooms.get_or_create(&snap.id, &snap.name);
        room.rhythm_enabled = snap.rhythm_enabled;
        room.disabled = snap.disabled;
        room.time_offset_minutes = snap.time_offset_minutes;
        room.brightness_offset = snap.brightness_offset;
        room.soft_off = snap.soft_off;
    }
    rooms
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhythm_core::primitives::{
        DEFAULT_BULB_FADE_MS, DEFAULT_MOTION_TIMEOUT_SECS, DEFAULT_SOFT_OFF_BRIGHTNESS,
    };
    use rhythm_core::runtime::RoomSnapshot;

    #[test]
    fn app_state_default_has_sensible_values() {
        let state = AppState::default();
        assert_eq!(state.bulb_fade_ms, DEFAULT_BULB_FADE_MS);
        assert_eq!(
            state.default_motion_timeout_secs,
            DEFAULT_MOTION_TIMEOUT_SECS
        );
        assert_eq!(state.soft_off_brightness, DEFAULT_SOFT_OFF_BRIGHTNESS);
        assert!(!state.power_save);
        assert!(state.hubs.is_empty());
        assert!(state.latitude.is_none());
        assert!(state.longitude.is_none());
        assert_eq!(state.utc_offset_hours, 0.0);
        assert!(state.room_lights_on.is_empty());
        assert!(state.motion_timeouts.is_empty());
        assert_eq!(state.firmware_version, "0.0.0");
        assert_eq!(state.platform_type, "desktop");
        assert_eq!(state.platform_context, "server");
    }

    #[test]
    fn hub_runtime_returns_none_when_no_hub() {
        let state = AppState::default();
        assert!(state.hub_runtime().is_none());
    }

    #[test]
    fn solar_noon_hour_reads_from_runtime_config() {
        let mut state = AppState::default();
        state.runtime_config.solar_noon_hour = 13.2;
        assert!((state.solar_noon_hour() - 13.2).abs() < f32::EPSILON);
    }

    #[test]
    fn solar_midnight_hour_is_12h_offset() {
        let mut state = AppState::default();
        state.runtime_config.solar_noon_hour = 12.5;
        // midnight = (12.5 + 12.0) % 24.0 = 0.5
        assert!((state.solar_midnight_hour() - 0.5).abs() < f32::EPSILON);

        state.runtime_config.solar_noon_hour = 13.0;
        // midnight = (13.0 + 12.0) % 24.0 = 1.0
        assert!((state.solar_midnight_hour() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn platform_config_embedded_vs_desktop() {
        let embedded = PlatformConfig::embedded();
        let desktop = PlatformConfig::desktop();

        assert!(embedded.runtime_init_stack < desktop.runtime_init_stack);
        assert!(embedded.scheduler_stack.is_some());
        assert!(desktop.scheduler_stack.is_none());
        assert!(!embedded.eager_tls_warmup);
        assert!(desktop.eager_tls_warmup);
        assert!(!embedded.full_device_discovery);
        assert!(desktop.full_device_discovery);
    }

    #[test]
    fn platform_config_default_is_desktop() {
        let default = PlatformConfig::default();
        let desktop = PlatformConfig::desktop();
        assert_eq!(default.runtime_init_stack, desktop.runtime_init_stack);
        assert!(default.eager_tls_warmup);
    }

    #[test]
    fn rooms_from_engine_converts_snapshots() {
        // Use a MockRuntime that returns configured snapshots
        struct MockRuntime {
            snapshots: Vec<RoomSnapshot>,
        }

        impl RuntimeHandle for MockRuntime {
            fn handle_event(&self, _: &rhythm_core::InputEvent) -> anyhow::Result<bool> {
                Ok(false)
            }
            fn sync_rooms(&self) -> anyhow::Result<()> {
                Ok(())
            }
            fn set_solar(&self, _: rhythm_core::SolarTime) -> anyhow::Result<()> {
                Ok(())
            }
            fn set_curve_config(&self, _: CurveConfig) -> anyhow::Result<()> {
                Ok(())
            }
            fn periodic_tick_room(&self, _: &str, _: f32) -> anyhow::Result<()> {
                Ok(())
            }
            fn engine_room_snapshot(&self, id: &str) -> Option<RoomSnapshot> {
                self.snapshots.iter().find(|s| s.id == id).cloned()
            }
            fn engine_all_room_snapshots(&self) -> Vec<RoomSnapshot> {
                self.snapshots.clone()
            }
            fn restore_room_state(&self, _: &str, _: bool, _: bool, _: f32, _: f32, _: bool) {}
            fn add_room(&self, _: &str, _: &str) {}
            fn remove_room(&self, _: &str) {}
            fn dim_room(&self, _: &str, _: f32) -> anyhow::Result<()> {
                Ok(())
            }
            fn turn_on_room(&self, _: &str) -> anyhow::Result<()> {
                Ok(())
            }
            fn set_power_save(&self, _: bool) -> Vec<String> {
                vec![]
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
            fn set_soft_off_brightness(&self, _: u8) {}
            fn soft_off_tick_room(&self, _: &str) -> anyhow::Result<()> {
                Ok(())
            }
            fn any_lights_on(&self, _: &str) -> anyhow::Result<bool> {
                Ok(false)
            }
            fn current_hour(&self) -> f32 {
                12.0
            }
        }

        let runtime = MockRuntime {
            snapshots: vec![
                RoomSnapshot {
                    id: "kitchen".into(),
                    name: "Kitchen".into(),
                    rhythm_enabled: true,
                    disabled: false,
                    time_offset_minutes: 15.0,
                    brightness_offset: -5.0,
                    soft_off: true,
                },
                RoomSnapshot {
                    id: "bedroom".into(),
                    name: "Bedroom".into(),
                    rhythm_enabled: false,
                    disabled: true,
                    time_offset_minutes: 0.0,
                    brightness_offset: 0.0,
                    soft_off: false,
                },
            ],
        };

        let rooms = rooms_from_engine(&runtime);
        assert_eq!(rooms.iter().count(), 2);

        let kitchen = rooms.get("kitchen").unwrap();
        assert_eq!(kitchen.name, "Kitchen");
        assert!(kitchen.rhythm_enabled);
        assert!(!kitchen.disabled);
        assert!((kitchen.time_offset_minutes - 15.0).abs() < f32::EPSILON);
        assert!((kitchen.brightness_offset - -5.0).abs() < f32::EPSILON);
        assert!(kitchen.soft_off);

        let bedroom = rooms.get("bedroom").unwrap();
        assert!(!bedroom.rhythm_enabled);
        assert!(bedroom.disabled);
    }
}
