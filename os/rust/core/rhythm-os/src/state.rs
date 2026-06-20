//! Application state shared across threads.
//!
//! Platform-agnostic `AppState` with `dyn Storage` instead of NVS.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rhythm_core::{
    normalize_mode_transition_configs, ButtonAction, LightProfileConfig, ModeChangeCause,
    ModeConfig, ModeTransitionConfig, RhythmMode, RoomModeState, RuntimeConfig, RuntimeHandle,
};
use rhythm_profile::profile_config::DEFAULT_FADE_MS;

use crate::app_runtime::{LightingRuntimeKind, SharedLightingRuntime};
use crate::auth::StoredApiAuth;
use crate::canonical::identity::HubKey;
use crate::canonical::registry::CanonicalRegistry;
use crate::factory_default_config::{
    factory_default_active_mode, factory_default_auto_update,
    factory_default_light_profile_config_map, factory_default_mode_config_map,
    factory_default_mode_transition_configs, factory_default_power_save, factory_default_scene_map,
};
use crate::hub::{ActiveHub, HubCredentials, HubEvent};
use crate::remote_access::RemoteAccessController;
use crate::storage::{
    generate_server_instance_id, Storage, StoredAppRuntimeState, StoredMotionTimerEntry,
};
use crate::topology::{NodeControlKind, RoomTopologyStore};

/// Ephemeral startup-bootstrap retry state for one configured hub.
#[derive(Clone, Debug)]
pub struct HubStartupRetryState {
    /// Number of consecutive failed startup/manual bootstrap attempts.
    pub attempt_count: u32,
    /// Monotonic time of the first failed attempt in the current retry window.
    pub first_failure_at: Instant,
    /// Wall-clock timestamp of the first failed attempt in epoch milliseconds.
    pub first_failure_epoch_ms: i64,
    /// Wall-clock timestamp of the most recent failed attempt in epoch milliseconds.
    pub last_failure_epoch_ms: i64,
    /// Next scheduled retry time, if automatic retrying is still active.
    pub next_retry_at: Option<Instant>,
    /// Wall-clock timestamp for the next scheduled retry in epoch milliseconds.
    pub next_retry_epoch_ms: Option<i64>,
    /// Most recent bootstrap error string for app diagnostics.
    pub last_error: String,
    /// Whether automatic retries have stopped and the app must request another attempt.
    pub manual_retry_required: bool,
}

/// Work items for background processing.
///
/// Heavy operations (TLS to hub, float math, JSON formatting) are
/// offloaded to a worker thread to avoid blocking the main/HTTP thread.
pub enum WorkItem {
    /// Execute a queued node action from HTTP/system batch work.
    ///
    /// Hub button and motion ingress must not use this queue; those paths run
    /// through the event-loop fast lane in `process_button_inline`,
    /// `turn_on_node_inline`, and `dim_node_inline`.
    QueuedNodeAction {
        command_id: String,
        node_id: String,
        action: ButtonAction,
        device_id: Option<String>,
        dispatch_spacing: Duration,
        persist_after: bool,
    },
    /// Set a node brightness through the runtime on the dispatch worker.
    SetNodeBrightness {
        command_id: String,
        node_id: String,
        brightness: u8,
        dispatch_spacing: Duration,
        persist_after: bool,
    },
    /// Apply a curve-aware node modifier through the runtime on the dispatch worker.
    SetNodeCurveModifier {
        command_id: String,
        node_id: String,
        modifier: crate::commands::NodeCurveModifier,
        dispatch_spacing: Duration,
        persist_after: bool,
    },
    /// Apply node preference changes through the dispatch worker.
    SetNodePreferences {
        command_id: String,
        node_id: String,
        rhythm_enabled: Option<bool>,
        disabled: Option<bool>,
        standby_enabled: Option<bool>,
        target_state: Option<RoomModeState>,
        room_profile: Option<crate::commands::RoomProfileSettingsPatch>,
        dispatch_spacing: Duration,
        persist_after: bool,
    },
    /// Apply a rendered lighting command to a single addressable node.
    ///
    /// Used by batch node-output updates so HTTP-triggered changes can be
    /// dispatched node-by-node on the background worker instead of blocking
    /// the caller while talking to hubs.
    ApplyNodeCommand {
        command_id: String,
        node_id: String,
        command: rhythm_core::LightingCommand,
        dispatch_spacing: Duration,
        dispatch_generation: u64,
    },
    /// Turn a single node fully off, optionally with a fade.
    ///
    /// Used by batch mode/default operations that need hard-off semantics
    /// without bypassing the queued node dispatch pacing.
    LightsOffRoom {
        command_id: String,
        node_id: String,
        transition_ms: Option<u32>,
        dispatch_spacing: Duration,
        dispatch_generation: u64,
    },
    /// Periodic update for a single schedulable light node.
    PeriodicNodeTick {
        command_id: String,
        node_id: String,
        settings_node_id: String,
        current_hour: f32,
        emit_parent_node_id: Option<String>,
        dispatch_spacing: Duration,
        dispatch_generation: u64,
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

/// Latest-only periodic tick state for one schedulable light node.
///
/// This stays present while a queued tick is running, not just while it is
/// waiting in the channel. That lets producer cycles coalesce into the single
/// in-flight tick instead of building a stale backlog behind slow controllers.
#[derive(Clone, Copy, Debug)]
pub struct PendingPeriodicTick {
    pub current_hour: f32,
    pub dispatch_generation: u64,
}

impl PendingPeriodicTick {
    pub fn new(current_hour: f32, dispatch_generation: u64) -> Self {
        Self {
            current_hour,
            dispatch_generation,
        }
    }
}

/// One motion sensor handed off from startup prefetch to the event loop.
///
/// Carries the sensor's current state so the event loop can seed both
/// active sensors (countdown not started) and inactive sensors (countdown
/// already ticking, so a room left with lights on after a restart still
/// auto-offs).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MotionSeedEntry {
    pub source_node_id: String,
    pub target_node_id: String,
    pub is_active: bool,
    /// Wall-clock time when the sensor cleared before restart.
    pub stopped_at_epoch_ms: Option<u64>,
    /// Persisted ownership override. `None` falls back to startup inference.
    pub motion_owned: Option<bool>,
    pub warning_active: bool,
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

/// Where the current observed power state came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservedPowerSource {
    /// Server-owned light action or command path.
    Command,
    /// Periodic reconciliation sampled the hub/runtime.
    Periodic,
    /// Startup or reconnect sync sampled the hub/runtime.
    SyncPoll,
    /// Live hub subscription reported a changed light attribute.
    LiveSubscription,
    /// An explicit authoritative API refresh sampled the hub/runtime.
    AuthoritativeRefresh,
    /// Room semantics make power state explicit (`hard_off` / `soft_off`).
    SemanticOverride,
}

impl ObservedPowerSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::Periodic => "periodic",
            Self::SyncPoll => "sync_poll",
            Self::LiveSubscription => "live_subscription",
            Self::AuthoritativeRefresh => "authoritative_refresh",
            Self::SemanticOverride => "semantic_override",
        }
    }
}

/// Last observed power state for one cache key.
#[derive(Clone, Debug)]
pub struct ObservedPowerState {
    pub lights_on: bool,
    pub observed_at_epoch_ms: u64,
    pub source: ObservedPowerSource,
}

impl ObservedPowerState {
    pub fn new(lights_on: bool, source: ObservedPowerSource) -> Self {
        Self {
            lights_on,
            observed_at_epoch_ms: current_epoch_ms(),
            source,
        }
    }
}

pub(crate) fn current_epoch_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
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

/// Platform-specific tuning for startup and sync behavior.
#[derive(Clone, Debug)]
pub struct PlatformConfig {
    /// Whether to eagerly establish the runtime's TLS connection at startup.
    ///
    /// When `true`, `warmup_tls()` is called during runtime creation so the
    /// first light command doesn't pay the TLS handshake cost.
    pub eager_tls_warmup: bool,
    /// Whether `sync_from_hub` should discover devices and sensors.
    ///
    /// When `true`, room sync also fetches the device endpoint to map buttons
    /// and motion sensors in one shot.
    pub full_device_discovery: bool,
}

impl PlatformConfig {
    /// Preset for desktop/server targets (macOS, Linux, Windows).
    pub fn desktop() -> Self {
        Self {
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
    /// Stored scene definitions keyed by scene ID.
    pub scenes: BTreeMap<String, crate::scenes::SceneDefinition>,
    /// Ephemeral light scene previews keyed by preview ID.
    pub light_scene_previews: HashMap<String, crate::scenes::LightScenePreviewSession>,
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
    /// API-visible connection status keyed by HubKey.
    ///
    /// Brief transport gaps stay visible as connected until the pending
    /// disconnect grace period expires.
    pub hub_connection_status: HashMap<HubKey, bool>,
    /// Hubs that have emitted at least one `Connected` event since configuration.
    ///
    /// Initial startup connect is covered by explicit bootstrap sync, so only
    /// later disconnect→connect transitions should trigger reconnect handling.
    pub hub_seen_connected_once: HashSet<HubKey>,
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
    /// Pending API-visible disconnect deadlines keyed by HubKey.
    ///
    /// Event streams can recycle briefly while the hub is still reachable.
    /// During that grace period the hub remains app-visible as connected; if
    /// no reconnect arrives before the deadline, the delayed task marks it
    /// disconnected and emits the hub-status event.
    pub hub_pending_disconnect_at: HashMap<HubKey, Instant>,
    /// Hub credentials keyed by HubKey. Supports multiple simultaneous hubs.
    pub hub_credentials: HashMap<HubKey, HubCredentials>,
    /// Ephemeral startup bootstrap retry state keyed by HubKey.
    pub hub_startup_retry: HashMap<HubKey, HubStartupRetryState>,
    /// API-facing capability metadata for integrations available on this platform.
    pub hub_capabilities: Vec<crate::hub::HubIntegrationCapability>,

    // ---- Canonical device registry + topology ----
    /// Canonical device registry (cross-hub device identity and dedup).
    pub canonical_registry: CanonicalRegistry,
    /// Room topology store (Rhythm's own room hierarchy).
    pub topology: RoomTopologyStore,
    /// Background topology-group sync worker is currently running.
    pub topology_group_sync_in_progress: bool,
    /// Topology-group sync request waiting for the worker to process it.
    pub topology_group_sync_pending: bool,

    // ---- Room state (all keyed by topology room IDs) ----
    /// Typed observed-power cache for API, poll, and SSE projections.
    ///
    /// Keyed by the effective light-state cache key (topology room IDs for
    /// normal room dispatch, parent room IDs for composite/group dispatch).
    pub room_observed_power: HashMap<String, ObservedPowerState>,

    /// Per-target motion timer snapshots, updated by the main loop.
    /// Keyed by **topology node IDs** (not hub-native IDs).
    pub motion_snapshots: HashMap<String, MotionSnapshot>,
    /// Temporary restored motion timers waiting for startup prefetch to
    /// reconcile source/target IDs. Matched entries are removed by room sync;
    /// unmatched stale entries disappear on the next motion timer persist.
    pub motion_timer_restores: HashMap<String, StoredMotionTimerEntry>,
    /// Rooms currently transitioning between global modes.
    pub room_mode_transitions: HashMap<String, RoomModeTransition>,
    /// Set when a mode change wanted to apply room defaults but no runtime
    /// was available (e.g. periodic replayed a missed scheduled transition
    /// before hub bootstrap). Drained by `reconcile_runtime_from_state` once
    /// the runtime is ready. In-memory only — re-detected on next restart.
    pub pending_mode_output_apply: bool,
    /// Last observed local hour for periodic time-based checks.
    pub last_check_hour: Option<f32>,
    /// Monotonic timestamp of the last periodic time observation.
    ///
    /// Used to distinguish real time progression from wall-clock jumps.
    pub last_check_instant: Option<Instant>,
    /// UTC offset captured alongside the last periodic time observation.
    ///
    /// Used to tolerate DST offset changes when validating wall-clock continuity.
    pub last_check_utc_offset_hours: Option<f32>,
    /// Epoch milliseconds of the most recent periodic tick (for client bootstrap).
    pub last_tick_epoch_ms: u64,

    // ---- Global settings ----
    /// Current globally resolved motion timeout in seconds for the active profile.
    pub default_motion_timeout_secs: u64,
    /// Current curve-computed fade/transition time in milliseconds.
    pub default_fade_ms: u32,
    /// Power save mode. When true, idle rooms turn fully off; when false,
    /// idle rooms dim to standby brightness.
    pub power_save: bool,
    /// Global switch for Rhythm's autonomous light breaker.
    ///
    /// When false, Rhythm keeps hub/status/API plumbing alive but drops
    /// control events from hub streams and does not run periodic light ticks.
    pub light_breaker_enabled: bool,
    /// When true, the appliance polls the curated "stable" OTA feed and
    /// auto-applies updates overnight. When false, it polls "beta" and only
    /// updates on an explicit `POST /api/ota/update`.
    pub auto_update: bool,

    // ---- Local API auth ----
    /// Persisted local API bearer tokens. Raw token material is never stored.
    pub api_auth: StoredApiAuth,
    /// Whether HTTP API requests require a local API bearer token.
    pub require_api_auth: bool,

    // ---- Storage ----
    /// Platform-specific storage backend.
    pub storage: Option<Box<dyn Storage>>,
    /// Namespaced durable state for plan-based light app runtimes.
    ///
    /// Shape: runtime id -> node id -> app-defined key -> JSON value.
    pub app_runtime_state: StoredAppRuntimeState,
    /// Selected plan-based lighting behavior.
    pub lighting_runtime_kind: LightingRuntimeKind,
    /// Stateful selected app runtime instance, if the selected runtime needs one.
    pub app_runtime: Option<SharedLightingRuntime>,
    /// Fingerprint of the host-derived app runtime config loaded into
    /// [app_runtime]. Used to rebuild stateful runtimes after topology changes.
    pub app_runtime_config_fingerprint: Option<String>,

    // ---- Worker ----
    /// Sender for offloading work to a background thread.
    pub work_tx: Option<std::sync::mpsc::SyncSender<WorkItem>>,
    /// Dedicated sender for background periodic room ticks.
    ///
    /// When present, periodic updates no longer compete with button actions
    /// and deferred persists on the main worker queue.
    pub periodic_work_tx: Option<std::sync::mpsc::SyncSender<WorkItem>>,
    /// Latest pending/running periodic tick per schedulable light node.
    ///
    /// Used for latest-only coalescing so repeated scheduler passes update the
    /// most recent hour for a node without queueing duplicate work items.
    pub pending_periodic_ticks: HashMap<String, PendingPeriodicTick>,
    /// Generation token for queued light-output dispatches.
    ///
    /// Incrementing this invalidates already queued periodic and mode-apply
    /// light work so an active-mode change cannot leak stale output commands.
    pub light_dispatch_generation: u64,
    /// Next reserved dispatch start slot for generated periodic light-node work.
    pub next_node_dispatch_at: Option<Instant>,
    /// Next reserved dispatch start slot for app-originated queued node work.
    pub next_interactive_node_dispatch_at: Option<Instant>,
    /// Pending hub event receivers from hub reconfiguration (picked up by main loop).
    /// Multiple hubs produce multiple receivers — the event loop drains this Vec.
    pub pending_hub_event_rxs: Vec<std::sync::mpsc::Receiver<HubEvent>>,
    /// Target node IDs whose motion timers should be cleared (picked up by event loop).
    pub pending_motion_clear: Vec<String>,
    /// Target node IDs whose live motion timers should be re-evaluated after a timeout change.
    pub pending_motion_timeout_refresh: Vec<String>,
    /// Restart-time motion timer persist acknowledgements requested by lifecycle code.
    ///
    /// The event loop owns the live motion timer map, so planned restarts ask
    /// it to persist and then acknowledge here instead of trying to duplicate
    /// timer state from AppState snapshots.
    pub pending_motion_timer_persist_acks: Vec<std::sync::mpsc::SyncSender<()>>,
    /// Motion sources from startup prefetch (picked up by event loop).
    ///
    /// Includes both active and inactive sensors so the event loop can
    /// resume timing rooms whose lights are on but whose sensor has
    /// already cleared — without this, restarts would leak motion-driven
    /// rooms with no auto-off timer.
    pub pending_motion_seed: Vec<MotionSeedEntry>,

    // ---- Composite controller ----
    /// The composite controller shared between AppState (for dynamic registration)
    /// and the RhythmEngine (for light breaker). Both hold Arc refs to the same instance.
    /// None on constrained blocking targets or before runtime creation.
    pub composite_controller: Option<Arc<rhythm_core::CompositeController>>,

    // ---- Callbacks ----
    /// Called on hub heartbeat.
    pub on_hub_heartbeat: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Called on hub disconnect.
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
    #[allow(clippy::type_complexity)]
    pub register_controller_fn: Option<
        Arc<
            dyn Fn(&SharedState, &crate::canonical::identity::HubKey) -> anyhow::Result<()>
                + Send
                + Sync,
        >,
    >,

    /// Synchronize integration-managed topology groups after topology changes.
    /// Built from the integration registry by `integration_callbacks`.
    #[allow(clippy::type_complexity)]
    pub sync_topology_groups_fn:
        Option<Arc<dyn Fn(&SharedState) -> anyhow::Result<()> + Send + Sync>>,

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

    /// Run a device diagnostic/test command.
    /// Built from the integration registry by `integration_callbacks`.
    #[allow(clippy::type_complexity)]
    pub run_device_test_fn: Option<
        Arc<
            dyn Fn(&SharedState, &str, &serde_json::Value) -> anyhow::Result<serde_json::Value>
                + Send
                + Sync,
        >,
    >,

    /// Save a device diagnostic/test report and optionally apply discovered
    /// local quirks.
    /// Built from the integration registry by `integration_callbacks`.
    #[allow(clippy::type_complexity)]
    pub save_device_test_report_fn: Option<
        Arc<
            dyn Fn(&SharedState, &str, &serde_json::Value) -> anyhow::Result<serde_json::Value>
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

    /// Request the platform-owned stored-hub bootstrap worker.
    ///
    /// Used when the app manually nudges a configured hub after automatic
    /// startup retries have been exhausted.
    #[allow(clippy::type_complexity)]
    pub request_hub_bootstrap_fn: Option<Arc<dyn Fn(&SharedState) + Send + Sync>>,

    /// Platform-owned fallback for Wi-Fi credentials used during accessory
    /// commissioning.
    ///
    /// Appliance targets may have a live Wi-Fi connection backed by system
    /// network config even when Rhythm's persisted commissioning cache is
    /// missing. Integrations use this callback only as a fallback so they stay
    /// portable and do not read platform files directly.
    #[allow(clippy::type_complexity)]
    pub commissioning_wifi_credentials_provider: Option<
        Arc<dyn Fn() -> anyhow::Result<Option<crate::provisioning::WifiCredentials>> + Send + Sync>,
    >,

    /// Optional platform-owned follow-up for a full factory reset.
    ///
    /// Shared reset logic clears in-memory and persisted Rhythm state, then
    /// delegates lifecycle/platform cleanup (restart, reboot, Wi-Fi reset) to
    /// the active binary crate through this callback.
    #[allow(clippy::type_complexity)]
    pub after_factory_reset_fn: Option<Arc<dyn Fn(&SharedState) + Send + Sync>>,

    /// Platform-owned remote access runtime controller.
    ///
    /// Shared HTTP handlers own persistence and redaction. Concrete targets
    /// install a controller to start/stop cloudflared through their platform's
    /// lifecycle mechanism.
    pub remote_access_controller: Option<Arc<dyn RemoteAccessController>>,

    /// Firmware version string (set by the binary crate).
    pub firmware_version: &'static str,

    /// Stable random identifier for this server installation.
    ///
    /// Used by cloud services to recognize the same Rhythm server when
    /// different app installs have different local hub IDs.
    pub server_instance_id: String,

    /// Platform type: "desktop" or "appliance".
    pub platform_type: &'static str,

    /// Deployment context: "ha_addon", "server", "rpiz", etc.
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
    pub event_tx: Option<tokio::sync::broadcast::Sender<crate::server_event::ServerEvent>>,

    /// Whether the stored-hub bootstrap worker thread is currently running.
    pub hub_bootstrap_worker_running: bool,
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
            scenes: default_scene_map(),
            light_scene_previews: HashMap::new(),
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
            hub_seen_connected_once: HashSet::new(),
            hub_sync_in_progress: HashSet::new(),
            hub_reconnect_sync_at: HashMap::new(),
            hub_pending_disconnect_at: HashMap::new(),
            hub_credentials: HashMap::new(),
            hub_startup_retry: HashMap::new(),
            hub_capabilities: Vec::new(),
            canonical_registry: CanonicalRegistry::new(),
            topology: RoomTopologyStore::new(),
            topology_group_sync_in_progress: false,
            topology_group_sync_pending: false,
            room_observed_power: HashMap::new(),
            motion_snapshots: HashMap::new(),
            motion_timer_restores: HashMap::new(),
            room_mode_transitions: HashMap::new(),
            pending_mode_output_apply: false,
            last_check_hour: None,
            last_check_instant: None,
            last_check_utc_offset_hours: None,
            last_tick_epoch_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            default_motion_timeout_secs: default_motion_timeout,
            default_fade_ms: default_fade,
            power_save: factory_default_power_save(),
            light_breaker_enabled: true,
            auto_update: factory_default_auto_update(),
            api_auth: StoredApiAuth::default(),
            require_api_auth: false,
            storage: None,
            app_runtime_state: StoredAppRuntimeState::new(),
            lighting_runtime_kind: LightingRuntimeKind::default(),
            app_runtime: None,
            app_runtime_config_fingerprint: None,
            work_tx: None,
            periodic_work_tx: None,
            pending_periodic_ticks: HashMap::new(),
            light_dispatch_generation: 0,
            next_node_dispatch_at: None,
            next_interactive_node_dispatch_at: None,
            pending_hub_event_rxs: Vec::new(),
            pending_motion_clear: Vec::new(),
            pending_motion_timeout_refresh: Vec::new(),
            pending_motion_timer_persist_acks: Vec::new(),
            pending_motion_seed: Vec::new(),
            composite_controller: None,
            on_hub_heartbeat: None,
            on_hub_disconnect: None,
            ensure_runtime_fn: None,
            register_controller_fn: None,
            sync_topology_groups_fn: None,
            get_hub_provider_fn: None,
            start_pairing_fn: None,
            start_unpairing_fn: None,
            run_device_test_fn: None,
            save_device_test_report_fn: None,
            hub_credentials_interceptor: None,
            request_hub_bootstrap_fn: None,
            commissioning_wifi_credentials_provider: None,
            after_factory_reset_fn: None,
            remote_access_controller: None,
            firmware_version: "0.0.0",
            server_instance_id: generate_server_instance_id(),
            platform_type: "desktop",
            platform_context: "server",
            data_dir: String::new(),
            listen_port: None,
            platform: PlatformConfig::default(),
            event_tx: None,
            hub_bootstrap_worker_running: false,
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

fn default_scene_map() -> BTreeMap<String, crate::scenes::SceneDefinition> {
    factory_default_scene_map()
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

    pub fn stored_scenes(&self) -> crate::scenes::StoredScenes {
        crate::scenes::StoredScenes {
            schema_version: crate::scenes::LIGHT_SCENE_SCHEMA_VERSION,
            scenes: self.scenes.values().cloned().collect(),
        }
    }

    pub fn scene_preview_blocks_node(&self, node_id: &str, now_epoch_ms: u64) -> bool {
        self.light_scene_previews.values().any(|preview| {
            preview.expires_at_epoch_ms >= now_epoch_ms
                && (preview.target_node_id == node_id
                    || preview
                        .affected_node_ids
                        .iter()
                        .any(|affected| affected == node_id))
        })
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

    /// Invalidate queued generated light-output work and reset dispatch pacing.
    pub fn invalidate_queued_light_dispatches(&mut self) -> u64 {
        self.light_dispatch_generation = self.light_dispatch_generation.wrapping_add(1);
        self.pending_periodic_ticks.clear();
        self.next_node_dispatch_at = None;
        self.next_interactive_node_dispatch_at = None;
        self.light_dispatch_generation
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

    /// Get controlled target node IDs for motion sources across the topology.
    pub fn motion_control_target_ids(&self) -> std::collections::HashSet<String> {
        self.topology
            .effective_control_targets_for_kind(&NodeControlKind::Motion, &self.canonical_registry)
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
        self.hub_seen_connected_once.remove(key);
        self.hub_reconnect_sync_at.remove(key);
        self.hub_pending_disconnect_at.remove(key);
    }

    /// Record that a hub emitted a `Connected` event.
    ///
    /// Returns `true` when this is the first observed connected event for the
    /// hub since it was configured.
    pub fn note_hub_connected_event(&mut self, key: &HubKey) -> bool {
        self.hub_seen_connected_once.insert(key.clone())
    }

    /// Whether a hub has emitted at least one connected event.
    pub fn hub_seen_connected_once(&self, key: &HubKey) -> bool {
        self.hub_seen_connected_once.contains(key)
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

    /// Start the delayed API-visible disconnect deadline.
    ///
    /// The deadline is intentionally not extended by repeated disconnect events;
    /// only a reconnect clears it.
    pub fn note_hub_pending_disconnect(&mut self, key: &HubKey, grace: Duration) -> Instant {
        if let Some(deadline) = self.hub_pending_disconnect_at.get(key) {
            return *deadline;
        }
        let deadline = Instant::now() + grace;
        self.hub_pending_disconnect_at.insert(key.clone(), deadline);
        deadline
    }

    /// Clear any pending API-visible disconnect for this hub.
    ///
    /// Returns whether a pending disconnect was present.
    pub fn clear_hub_pending_disconnect(&mut self, key: &HubKey) -> bool {
        self.hub_pending_disconnect_at.remove(key).is_some()
    }

    /// Whether the delayed API-visible disconnect is still the active one.
    pub fn hub_pending_disconnect_matches(&self, key: &HubKey, deadline: Instant) -> bool {
        self.hub_pending_disconnect_at
            .get(key)
            .is_some_and(|pending| *pending == deadline)
    }

    /// Get startup bootstrap retry state for a configured hub.
    pub fn hub_startup_retry(&self, key: &HubKey) -> Option<&HubStartupRetryState> {
        self.hub_startup_retry.get(key)
    }

    /// Clear startup bootstrap retry state for a hub.
    pub fn clear_hub_startup_retry(&mut self, key: &HubKey) {
        self.hub_startup_retry.remove(key);
    }

    /// Mark the stored-hub bootstrap worker as running.
    ///
    /// Returns `true` when this call claimed the worker slot.
    pub fn begin_hub_bootstrap_worker(&mut self) -> bool {
        if self.hub_bootstrap_worker_running {
            false
        } else {
            self.hub_bootstrap_worker_running = true;
            true
        }
    }

    /// Mark the stored-hub bootstrap worker as no longer running.
    pub fn finish_hub_bootstrap_worker(&mut self) {
        self.hub_bootstrap_worker_running = false;
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

    /// Get solar noon hour from runtime config.
    pub fn solar_noon_hour(&self) -> f32 {
        self.runtime_config.solar_noon_hour
    }

    /// Get solar midnight hour.
    pub fn solar_midnight_hour(&self) -> f32 {
        self.runtime_config.solar_midnight_hour()
    }
}

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

/// Return whether Rhythm's autonomous light breaker is currently enabled.
pub fn light_breaker_enabled(state: &SharedState) -> bool {
    state.lock().ok().is_some_and(|s| s.light_breaker_enabled)
}

/// Emit a server event, briefly locking state to access the broadcast sender.
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
        room.mood_active = snap.mood_active;
        room.standby_enabled = snap.standby_enabled;
        room.hard_off = snap.hard_off;
        room.profile_settings = snap.profile_settings;
    }
    rooms
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::factory_default_config::{
        factory_default_active_mode, factory_default_auto_update,
        factory_default_light_profile_config, factory_default_mode_transition_configs,
        factory_default_power_save,
    };
    use crate::hub::HubType;
    use rhythm_core::config::DEFAULT_MOTION_TIMEOUT_SECS;
    use rhythm_core::runtime::RoomSnapshot;

    #[test]
    fn app_state_default_has_sensible_values() {
        let state = AppState::default();
        assert_eq!(
            state.default_motion_timeout_secs,
            DEFAULT_MOTION_TIMEOUT_SECS as u64
        );
        assert!(state.power_save);
        assert!(state.hubs.is_empty());
        assert!(state.latitude.is_none());
        assert!(state.longitude.is_none());
        assert_eq!(state.utc_offset_hours, 0.0);
        assert!(state.room_observed_power.is_empty());
        assert!(state.last_active_mode_change_utc_ms.is_some());
        assert_eq!(state.firmware_version, "0.0.0");
        assert!(state.server_instance_id.starts_with("srv-"));
        assert_eq!(state.platform_type, "desktop");
        assert_eq!(state.platform_context, "server");
    }

    #[test]
    fn pending_hub_disconnect_grace_does_not_extend_without_reconnect() {
        let mut state = AppState::default();
        let hub_key = HubKey::new(HubType::new("hue"), "192.168.5.221:443");

        let first_deadline = state.note_hub_pending_disconnect(&hub_key, Duration::from_secs(120));
        std::thread::sleep(Duration::from_millis(5));
        let second_deadline = state.note_hub_pending_disconnect(&hub_key, Duration::from_secs(120));

        assert_eq!(
            first_deadline, second_deadline,
            "repeated disconnects before reconnect must not indefinitely extend grace"
        );
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
    fn platform_config_desktop_defaults_match_active_targets() {
        let desktop = PlatformConfig::desktop();

        assert!(desktop.eager_tls_warmup);
        assert!(desktop.full_device_discovery);
    }

    #[test]
    fn platform_config_default_is_desktop() {
        let default = PlatformConfig::default();
        let desktop = PlatformConfig::desktop();
        assert_eq!(default.eager_tls_warmup, desktop.eager_tls_warmup);
        assert_eq!(default.full_device_discovery, desktop.full_device_discovery);
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
                    mood_active: false,
                    standby_enabled: true,
                    hard_off: false,
                    profile_settings: rhythm_core::RoomProfileSettings::default(),
                },
                RoomSnapshot {
                    id: "den".into(),
                    name: "Den".into(),
                    kind: rhythm_core::LightNodeKind::Room,
                    parent_id: None,
                    rhythm_enabled: true,
                    disabled: false,
                    time_offset_minutes: 0.0,
                    brightness_offset: 0.0,
                    soft_off: false,
                    mood_active: true,
                    standby_enabled: false,
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
                    mood_active: false,
                    standby_enabled: false,
                    hard_off: false,
                    profile_settings: rhythm_core::RoomProfileSettings::default(),
                },
            ],
        };

        let rooms = rooms_from_engine(&runtime);
        assert_eq!(rooms.iter().count(), 3);

        let kitchen = rooms.get("kitchen").unwrap();
        assert_eq!(kitchen.name, "Kitchen");
        assert!(kitchen.rhythm_enabled);
        assert!(!kitchen.disabled);
        assert!((kitchen.time_offset_minutes - 15.0).abs() < f32::EPSILON);
        assert!((kitchen.brightness_offset - -5.0).abs() < f32::EPSILON);
        assert!(kitchen.soft_off);
        assert!(!kitchen.mood_active);
        assert!(kitchen.standby_enabled);

        let den = rooms.get("den").unwrap();
        assert!(den.mood_active);
        assert!(!den.soft_off);
        assert!(!den.standby_enabled);

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
        assert_eq!(state.auto_update, factory_default_auto_update());
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
