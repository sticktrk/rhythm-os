//! Application state shared across threads.
//!
//! Platform-agnostic `AppState` with `dyn Storage` instead of NVS.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rhythm_core::{
    normalize_mode_transition_configs, ButtonAction, LightProfileConfig, ModeChangeCause,
    ModeConfig, ModeTransitionConfig, RhythmMode, RuntimeConfig, RuntimeHandle,
};
use rhythm_profile::profile_config::DEFAULT_FADE_MS;

use crate::canonical::identity::HubKey;
use crate::canonical::registry::CanonicalRegistry;
use crate::factory_default_config::{
    factory_default_active_mode, factory_default_light_profile_config_map,
    factory_default_mode_config_map, factory_default_mode_transition_configs,
    factory_default_power_save,
};
use crate::hub::{ActiveHub, HubCredentials, HubEvent};
use crate::storage::Storage;
use crate::topology::{NodeControlKind, RoomTopologyStore};

/// Work items for background processing.
///
/// Heavy operations (TLS to hub, float math, JSON formatting) are
/// offloaded to a worker thread to avoid blocking the main/HTTP thread.
pub enum WorkItem {
    /// Execute a button action (involves TLS to hub).
    ButtonAction {
        node_id: String,
        action: ButtonAction,
        device_id: Option<String>,
    },
    /// Apply a rendered lighting command to a single addressable node.
    ///
    /// Used by batch node-output updates so HTTP-triggered changes can be
    /// dispatched node-by-node on the background worker instead of blocking
    /// the caller while talking to hubs.
    ApplyNodeCommand {
        node_id: String,
        command: rhythm_core::LightingCommand,
    },
    /// Periodic update for a single schedulable light node.
    PeriodicNodeTick {
        node_id: String,
        settings_node_id: String,
        current_hour: f32,
        emit_parent_node_id: Option<String>,
    },
    /// Deferred persist after inline button processing.
    DeferredPersist { node_id: String },
    /// Full state persist (registry + rooms) after batch HTTP operations.
    ///
    /// Unlike `DeferredPersist` (rooms-only), batch room updates modify the
    /// registry too, so this runs the full `persist_state` on the worker
    /// thread (16KB stack) instead of the HTTP handler stack (12KB).
    DeferredPersistState,
}

/// Snapshot of motion timer state for a single controlled target node, exposed via API.
#[derive(Clone)]
pub struct MotionSnapshot {
    /// Any sensor currently controlling this target node.
    pub motion_active: bool,
    /// Motion still owns this target node (vs. manual control took over).
    pub motion_owned: bool,
    /// None if any sensor still active, Some(secs) if all cleared and counting down.
    pub remaining_secs: Option<u64>,
    /// Configured timeout for this target node.
    pub timeout_secs: u64,
    /// Target node is currently in warning dim state (dimmed before timeout).
    pub warning_active: bool,
}

/// Runtime state for a room whose rendered output is currently transitioning
/// between two global modes.
#[derive(Clone)]
pub struct RoomModeTransition {
    /// When the transition fade itself is expected to complete.
    pub ends_at: Instant,
    /// When the periodic loop may resume touching this room.
    ///
    /// This can intentionally extend past `ends_at` so the first scheduler
    /// pass after a mode transition does not immediately refresh the room and
    /// re-hit the hub with a redundant command/status probe.
    pub periodic_resume_at: Instant,
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
    /// Stored light profile configs keyed by profile ID.
    pub light_profile_configs: BTreeMap<String, LightProfileConfig>,
    /// Shareable mode/state profile mappings keyed by high-level mode.
    pub mode_configs: BTreeMap<RhythmMode, ModeConfig>,
    /// Configured mode-to-mode rendered-output transitions.
    pub mode_transition_configs: Vec<ModeTransitionConfig>,
    /// The currently active global mode.
    pub active_mode: RhythmMode,
    /// Cause of the most recent active mode change.
    pub last_active_mode_cause: ModeChangeCause,
    /// Saved transition that most recently changed the active mode, if any.
    pub last_active_mode_transition_id: Option<String>,
    /// UTC timestamp of the most recent active mode change.
    pub last_active_mode_change_utc_ms: Option<i64>,
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
    /// Live connection status keyed by HubKey.
    ///
    /// A hub can remain configured and active in-process while its transport
    /// is temporarily disconnected and reconnecting.
    pub hub_connection_status: HashMap<HubKey, bool>,
    /// Per-hub room syncs currently running in background threads.
    ///
    /// Used to avoid racing the initial bootstrap sync against reconnect-
    /// triggered syncs after the event stream comes back.
    pub hub_sync_in_progress: HashSet<HubKey>,
    /// Last time a reconnect-triggered full hub sync was scheduled.
    ///
    /// Rapid SSE reconnect churn should refresh live light state, but it
    /// should not keep re-running full room/device discovery every time.
    pub hub_reconnect_sync_at: HashMap<HubKey, Instant>,
    /// Hub credentials keyed by HubKey. Supports multiple simultaneous hubs.
    pub hub_credentials: HashMap<HubKey, HubCredentials>,
    /// API-facing capability metadata for integrations available on this platform.
    pub hub_capabilities: Vec<crate::hub::HubIntegrationCapability>,

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

    /// Per-target motion timer snapshots, updated by the main loop.
    /// Keyed by **topology node IDs** (not hub-native IDs).
    pub motion_snapshots: HashMap<String, MotionSnapshot>,
    /// Rooms currently transitioning between global modes.
    pub room_mode_transitions: HashMap<String, RoomModeTransition>,
    /// Last periodic update hour (for solar midnight detection).
    pub last_check_hour: Option<f32>,
    /// Epoch milliseconds of the most recent periodic tick (for client bootstrap).
    pub last_tick_epoch_ms: u64,

    // ---- Global settings ----
    /// Current globally resolved motion timeout in seconds for the active profile.
    pub default_motion_timeout_secs: u64,
    /// Current curve-computed fade/transition time in milliseconds.
    pub default_fade_ms: u32,
    /// Power save mode. When false, lights dim to soft-off brightness
    /// instead of turning fully off.
    pub power_save: bool,
    // ---- Storage ----
    /// Platform-specific storage backend.
    pub storage: Option<Box<dyn Storage>>,

    // ---- Worker ----
    /// Sender for offloading work to a background thread.
    pub work_tx: Option<std::sync::mpsc::SyncSender<WorkItem>>,
    /// Dedicated sender for background periodic room ticks.
    ///
    /// When present, periodic updates no longer compete with button actions
    /// and deferred persists on the main worker queue.
    pub periodic_work_tx: Option<std::sync::mpsc::SyncSender<WorkItem>>,
    /// Latest pending periodic tick hour per schedulable light node.
    ///
    /// Used for latest-only coalescing so repeated scheduler passes update the
    /// most recent hour for a node without queueing duplicate work items.
    pub pending_periodic_ticks: HashMap<String, f32>,
    /// Pending hub event receivers from hub reconfiguration (picked up by main loop).
    /// Multiple hubs produce multiple receivers — the event loop drains this Vec.
    pub pending_hub_event_rxs: Vec<std::sync::mpsc::Receiver<HubEvent>>,
    /// Target node IDs whose motion timers should be cleared (picked up by event loop).
    pub pending_motion_clear: Vec<String>,
    /// Active motion sources from startup prefetch (picked up by event loop).
    /// Vec of `(source_node_id, target_node_id)` for sources with state="on"
    /// at boot.
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
            dyn Fn(
                    &SharedState,
                    &str,
                    &serde_json::Value,
                ) -> anyhow::Result<crate::pairing::PairingSession>
                + Send
                + Sync,
        >,
    >,

    /// Start a device unpairing/decommission session (Matter, Zigbee, etc.).
    /// Built from the integration registry by `integration_callbacks`.
    #[allow(clippy::type_complexity)]
    pub start_unpairing_fn: Option<
        Arc<
            dyn Fn(
                    &SharedState,
                    &str,
                    &serde_json::Value,
                ) -> anyhow::Result<crate::pairing::UnpairingResult>
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
        let default_motion_timeout = rhythm_core::config::DEFAULT_MOTION_TIMEOUT_SECS as u64;
        let default_fade = DEFAULT_FADE_MS as u32;
        let now_utc_ms = chrono::Utc::now().timestamp_millis();
        let mut state = Self {
            light_profile_configs: default_light_profile_configs(),
            mode_configs: default_mode_config_map(),
            mode_transition_configs: factory_default_mode_transition_configs(),
            active_mode: factory_default_active_mode(),
            last_active_mode_cause: ModeChangeCause::Manual,
            last_active_mode_transition_id: None,
            last_active_mode_change_utc_ms: Some(now_utc_ms),
            runtime_config: RuntimeConfig::default().with_solar_noon(12.5),
            utc_offset_hours: 0.0,
            latitude: None,
            longitude: None,
            timezone_name: None,
            hubs: HashMap::new(),
            hub_connection_status: HashMap::new(),
            hub_sync_in_progress: HashSet::new(),
            hub_reconnect_sync_at: HashMap::new(),
            hub_credentials: HashMap::new(),
            hub_capabilities: Vec::new(),
            canonical_registry: CanonicalRegistry::new(),
            topology: RoomTopologyStore::new(),
            room_lights_on: HashMap::new(),
            motion_snapshots: HashMap::new(),
            room_mode_transitions: HashMap::new(),
            last_check_hour: None,
            last_tick_epoch_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            default_motion_timeout_secs: default_motion_timeout,
            default_fade_ms: default_fade,
            power_save: factory_default_power_save(),
            storage: None,
            work_tx: None,
            periodic_work_tx: None,
            pending_periodic_ticks: HashMap::new(),
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
            start_unpairing_fn: None,
            hub_credentials_interceptor: None,
            firmware_version: "0.0.0",
            platform_type: "desktop",
            platform_context: "server",
            data_dir: String::new(),
            listen_port: None,
            platform: PlatformConfig::default(),
            #[cfg(feature = "desktop")]
            event_tx: None,
        };
        state.sync_active_mode_runtime_overrides();
        state
    }
}

fn default_light_profile_configs() -> BTreeMap<String, LightProfileConfig> {
    factory_default_light_profile_config_map()
}

fn default_mode_config_map() -> BTreeMap<RhythmMode, ModeConfig> {
    factory_default_mode_config_map()
}

impl AppState {
    /// Get a stored profile config by ID.
    pub fn light_profile_config(&self, id: &str) -> Option<&LightProfileConfig> {
        self.light_profile_configs.get(id)
    }

    /// Get the configured mode transitions.
    pub fn mode_transition_configs(&self) -> Vec<ModeTransitionConfig> {
        self.mode_transition_configs.clone()
    }

    /// Replace the configured mode transitions.
    pub fn set_mode_transition_configs<I>(&mut self, configs: I)
    where
        I: IntoIterator<Item = ModeTransitionConfig>,
    {
        self.mode_transition_configs = normalize_mode_transition_configs(configs);
    }

    /// Resolve the selected base profile ID for a mode, falling back to the
    /// built-in profile for that mode when the configured ID is missing.
    pub fn resolved_active_profile_id_for_mode(&self, mode: RhythmMode) -> String {
        let requested = self
            .mode_configs
            .get(&mode)
            .and_then(|config| config.active_profile_id.clone())
            .unwrap_or_else(|| mode.default_active_profile_id().to_string());

        if !rhythm_core::is_builtin_state_profile_id(&requested)
            && self.light_profile_configs.contains_key(&requested)
        {
            requested
        } else {
            mode.default_active_profile_id().to_string()
        }
    }

    /// Get the resolved active profile ID for the current global mode.
    pub fn active_mode_profile_id(&self) -> String {
        self.resolved_active_profile_id_for_mode(self.active_mode)
    }

    /// Get the currently active mode's selected profile config.
    pub fn active_mode_profile_config(&self) -> Option<&LightProfileConfig> {
        let profile_id = self.active_mode_profile_id();
        self.light_profile_config(&profile_id)
    }

    /// Get the persisted mode/state profile mappings in stable mode order.
    pub fn mode_configs(&self) -> Vec<ModeConfig> {
        RhythmMode::ALL
            .into_iter()
            .map(|mode| {
                self.mode_configs
                    .get(&mode)
                    .cloned()
                    .unwrap_or_else(|| ModeConfig::default_for_mode(mode))
            })
            .collect()
    }

    /// Replace the persisted mode/state profile mappings.
    ///
    /// Missing modes fall back to built-in defaults.
    pub fn set_mode_configs<I>(&mut self, configs: I)
    where
        I: IntoIterator<Item = ModeConfig>,
    {
        self.mode_configs = default_mode_config_map();
        for mut config in configs {
            config.normalize_profile_ids();
            self.mode_configs.insert(config.mode, config);
        }
    }

    /// Insert or replace a stored profile config.
    pub fn set_light_profile_config(&mut self, config: LightProfileConfig) {
        let mut config = config;
        rhythm_core::normalize_builtin_state_profile_config(&mut config);
        let is_active = config.id == self.active_mode_profile_id();
        self.light_profile_configs.insert(config.id.clone(), config);
        if is_active {
            self.sync_active_mode_runtime_overrides();
        }
    }

    /// Replace the stored profile set, preserving built-ins as fallbacks.
    pub fn replace_light_profile_configs<I>(&mut self, configs: I)
    where
        I: IntoIterator<Item = LightProfileConfig>,
    {
        self.light_profile_configs = default_light_profile_configs();
        for config in configs {
            self.set_light_profile_config(config);
        }
        self.sync_active_mode_runtime_overrides();
    }

    /// Restore the built-in profile set.
    pub fn reset_light_profile_configs(&mut self) {
        self.light_profile_configs = default_light_profile_configs();
        self.sync_active_mode_runtime_overrides();
    }

    /// Sync runtime-level defaults from the current active mode's selected
    /// profile config.
    ///
    /// Resolves `TimerSetting` values at a reference hour (12.0) for initial
    /// state. The periodic tick updates these live via `LightingValues`.
    pub fn sync_active_mode_runtime_overrides(&mut self) {
        use rhythm_core::config::DEFAULT_MOTION_TIMEOUT_SECS;
        use rhythm_core::runtime::config::DEFAULT_UPDATE_INTERVAL_SECS;
        use rhythm_profile::profile_config::DEFAULT_FADE_MS;

        const REF_HOUR: f32 = 12.0;

        if let Some(active) = self.active_mode_profile_config().cloned() {
            self.runtime_config.update_interval_secs = active
                .rhythm_interval_secs
                .resolve(REF_HOUR)
                .map(|v| v as u64)
                .unwrap_or(DEFAULT_UPDATE_INTERVAL_SECS);
            self.default_motion_timeout_secs = active
                .motion_timeout_secs
                .resolve(REF_HOUR)
                .map(|v| v as u64)
                .unwrap_or(DEFAULT_MOTION_TIMEOUT_SECS as u64);
            self.default_fade_ms = active
                .fade_ms
                .resolve(REF_HOUR)
                .unwrap_or(DEFAULT_FADE_MS as u32);
        }
    }

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
        for (hub_key, hub) in &self.hubs {
            if let Some(ref reg_arc) = hub.registry {
                if let Ok(reg) = reg_arc.lock() {
                    for room_id in reg.rooms_with_motion_sensors() {
                        let translated = self
                            .topology
                            .resolve_room_alias(&self.canonical_registry, Some(hub_key), &room_id)
                            .unwrap_or(room_id);
                        ids.insert(translated);
                    }
                }
            }
        }
        ids
    }

    /// Get controlled target node IDs for motion sources across the topology.
    ///
    /// Falls back to room-based registry mappings when a motion source has not
    /// been promoted into the topology graph yet.
    pub fn motion_control_target_ids(&self) -> std::collections::HashSet<String> {
        let mut ids = self
            .topology
            .effective_control_targets_for_kind(&NodeControlKind::Motion, &self.canonical_registry);
        ids.extend(self.motion_sensor_room_ids());
        ids
    }

    /// Check if any hub is active.
    pub fn has_any_hub(&self) -> bool {
        !self.hubs.is_empty()
    }

    /// Check whether a specific hub transport is currently live.
    pub fn hub_is_connected(&self, key: &HubKey) -> bool {
        self.hub_connection_status
            .get(key)
            .copied()
            .unwrap_or(false)
    }

    /// Check if any active hub transport is currently live.
    pub fn has_any_connected_hub(&self) -> bool {
        self.hubs.keys().any(|key| self.hub_is_connected(key))
    }

    /// Update the live connection state for a hub.
    pub fn set_hub_connected(&mut self, key: &HubKey, connected: bool) {
        self.hub_connection_status.insert(key.clone(), connected);
    }

    /// Forget the live connection state for a hub.
    pub fn clear_hub_connected(&mut self, key: &HubKey) {
        self.hub_connection_status.remove(key);
        self.hub_reconnect_sync_at.remove(key);
    }

    /// Whether a reconnect-triggered full sync ran recently for this hub.
    pub fn reconnect_sync_recently_ran(&self, key: &HubKey, cooldown: Duration) -> bool {
        self.hub_reconnect_sync_at
            .get(key)
            .is_some_and(|last| last.elapsed() < cooldown)
    }

    /// Record that a reconnect-triggered full sync was scheduled for this hub.
    pub fn note_hub_reconnect_sync(&mut self, key: &HubKey) {
        self.hub_reconnect_sync_at
            .insert(key.clone(), Instant::now());
    }

    /// Clear reconnect-sync timing so the next reconnect can try again.
    pub fn clear_hub_reconnect_sync(&mut self, key: &HubKey) {
        self.hub_reconnect_sync_at.remove(key);
    }

    /// Mark a per-hub room sync as running.
    ///
    /// Returns `true` when this call acquired the sync slot and the caller
    /// should proceed. Returns `false` when another sync is already running.
    pub fn begin_hub_sync(&mut self, key: &HubKey) -> bool {
        self.hub_sync_in_progress.insert(key.clone())
    }

    /// Mark a per-hub room sync as finished.
    pub fn finish_hub_sync(&mut self, key: &HubKey) {
        self.hub_sync_in_progress.remove(key);
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
    for snap in runtime.engine_all_node_snapshots() {
        let room =
            rooms.get_or_create_node(&snap.id, &snap.name, snap.kind, snap.parent_id.clone());
        room.rhythm_enabled = snap.rhythm_enabled;
        room.disabled = snap.disabled;
        room.time_offset_minutes = snap.time_offset_minutes;
        room.brightness_offset = snap.brightness_offset;
        room.soft_off = snap.soft_off;
        room.hard_off = snap.hard_off;
        room.profile_settings = snap.profile_settings;
    }
    rooms
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::factory_default_config::{
        factory_default_active_mode, factory_default_light_profile_config,
        factory_default_mode_transition_configs, factory_default_power_save,
    };
    use rhythm_core::config::DEFAULT_MOTION_TIMEOUT_SECS;
    use rhythm_core::runtime::RoomSnapshot;

    #[test]
    fn app_state_default_has_sensible_values() {
        let state = AppState::default();
        assert_eq!(
            state.default_motion_timeout_secs,
            DEFAULT_MOTION_TIMEOUT_SECS as u64
        );
        assert!(!state.power_save);
        assert!(state.hubs.is_empty());
        assert!(state.latitude.is_none());
        assert!(state.longitude.is_none());
        assert_eq!(state.utc_offset_hours, 0.0);
        assert!(state.room_lights_on.is_empty());
        assert!(state.last_active_mode_change_utc_ms.is_some());
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
            fn set_light_profile_config(
                &self,
                _: rhythm_core::LightProfileConfig,
            ) -> anyhow::Result<()> {
                Ok(())
            }
            fn set_mode_configs(&self, _: Vec<rhythm_core::ModeConfig>) -> anyhow::Result<()> {
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
            fn restore_room_state(&self, _: &str, _: rhythm_core::RestoredRoomState) {}
            fn add_room(&self, _: &str, _: &str) {}
            fn remove_room(&self, _: &str) {}
            fn dim_room(&self, _: &str, _: f32) -> anyhow::Result<()> {
                Ok(())
            }
            fn turn_on_room(&self, _: &str) -> anyhow::Result<()> {
                Ok(())
            }
            fn apply_room_command(
                &self,
                _: &str,
                _: rhythm_core::LightingCommand,
            ) -> anyhow::Result<()> {
                Ok(())
            }
            fn lights_off_room(&self, _: &str, _: Option<u32>) -> anyhow::Result<()> {
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
                "rhythm".into()
            }
            fn available_light_profiles(&self) -> Vec<(String, String)> {
                vec![
                    ("rhythm".into(), "Rhythm Curve".into()),
                    ("sleep".into(), "Sleep Curve".into()),
                ]
            }
        }

        let runtime = MockRuntime {
            snapshots: vec![
                RoomSnapshot {
                    id: "kitchen".into(),
                    name: "Kitchen".into(),
                    kind: rhythm_core::LightNodeKind::Room,
                    parent_id: None,
                    rhythm_enabled: true,
                    disabled: false,
                    time_offset_minutes: 15.0,
                    brightness_offset: -5.0,
                    soft_off: true,
                    hard_off: false,
                    profile_settings: rhythm_core::RoomProfileSettings::default(),
                },
                RoomSnapshot {
                    id: "bedroom".into(),
                    name: "Bedroom".into(),
                    kind: rhythm_core::LightNodeKind::Room,
                    parent_id: None,
                    rhythm_enabled: false,
                    disabled: true,
                    time_offset_minutes: 0.0,
                    brightness_offset: 0.0,
                    soft_off: false,
                    hard_off: false,
                    profile_settings: rhythm_core::RoomProfileSettings::default(),
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

    #[test]
    fn set_mode_configs_rejects_state_profile_as_active() {
        let mut state = AppState::default();
        state.set_mode_configs(vec![rhythm_core::ModeConfig {
            mode: rhythm_core::RhythmMode::Day,
            active_profile_id: Some(rhythm_core::DAY_IDLE_PROFILE_ID.into()),
            idle_profile_id: Some(rhythm_core::DAY_IDLE_PROFILE_ID.into()),
            wake_profile_id: None,
            warning_profile_id: None,
            room_defaults: vec![],
        }]);

        assert_eq!(
            state.mode_configs()[0].active_profile_id.as_deref(),
            Some(rhythm_core::RHYTHM_PROFILE_ID)
        );
    }

    #[test]
    fn app_state_default_uses_bundled_defaults() {
        let state = AppState::default();

        assert_eq!(state.active_mode, factory_default_active_mode());
        assert_eq!(state.power_save, factory_default_power_save());
        assert_eq!(
            state.mode_transition_configs(),
            factory_default_mode_transition_configs()
        );
        assert_eq!(
            state
                .light_profile_config(rhythm_core::RHYTHM_PROFILE_ID)
                .cloned(),
            factory_default_light_profile_config(rhythm_core::RHYTHM_PROFILE_ID)
        );
        assert_eq!(
            state
                .light_profile_config(rhythm_core::SLEEP_PROFILE_ID)
                .cloned(),
            factory_default_light_profile_config(rhythm_core::SLEEP_PROFILE_ID)
        );
    }

    #[test]
    fn reset_light_profile_configs_restores_bundled_profile_values() {
        let mut state = AppState::default();
        let mut rhythm = state
            .light_profile_config(rhythm_core::RHYTHM_PROFILE_ID)
            .cloned()
            .unwrap();
        rhythm.name = "Custom Day".into();
        rhythm.min_brightness = 17;
        state.set_light_profile_config(rhythm);

        state.reset_light_profile_configs();

        assert_eq!(
            state
                .light_profile_config(rhythm_core::RHYTHM_PROFILE_ID)
                .cloned(),
            factory_default_light_profile_config(rhythm_core::RHYTHM_PROFILE_ID)
        );
    }
}
