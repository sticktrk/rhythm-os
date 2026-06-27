//! Hub abstraction layer.
//!
//! Provides hub-agnostic types for events, credentials, state, and the
//! active hub instance. Platform crates implement `HubProvider` to handle
//! hub-specific configuration.
//!
//! Integration crates implement `ExternalLightHubIntegration` to bundle
//! their lifecycle functions into a single trait object that platform
//! crates can store in a static registry.

use std::any::Any;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use log::{info, warn};
use rhythm_core::{ButtonAction, HubRegistry, RuntimeHandle};
use serde::{Deserialize, Serialize};

use crate::canonical::identity::HubKey;
use crate::discovery::HubDiscovery;
use crate::state::SharedState;

// ============================================================================
// HubEvent — normalized events from any hub's event stream
// ============================================================================

/// Normalized events from any hub's event stream.
///
/// The main loop processes these generically via `RuntimeHandle`,
/// regardless of which hub produced them. Each variant carries an
/// optional `hub_key` so the event loop can route to the correct runtime
/// in multi-hub configurations.
#[derive(Debug, Clone)]
pub enum HubEvent {
    /// Connection established or re-established.
    Connected { hub_key: Option<HubKey> },
    /// A button was pressed on a hub device.
    Button {
        hub_key: Option<HubKey>,
        room_id: String,
        action: ButtonAction,
        device_id: Option<String>,
    },
    /// Motion detected or cleared in a room.
    Motion {
        hub_key: Option<HubKey>,
        room_id: String,
        sensor_id: String,
        detected: bool,
    },
    /// A contact sensor reported open or closed in a room.
    Contact {
        hub_key: Option<HubKey>,
        room_id: String,
        sensor_id: String,
        open: bool,
    },
    /// A light endpoint reported a live power-state change.
    LightPower {
        hub_key: Option<HubKey>,
        device_id: String,
        lights_on: bool,
    },
    /// Connection heartbeat.
    Heartbeat { hub_key: Option<HubKey> },
    /// Connection lost.
    Disconnected {
        hub_key: Option<HubKey>,
        reason: String,
    },
    /// A new device was paired/commissioned (Matter, Zigbee direct, etc.).
    DevicePaired {
        hub_key: Option<HubKey>,
        device_id: String,
        name: String,
        device_type: rhythm_core::runtime::hub_registry::DeviceType,
    },
    /// A button was pressed but could not be routed to a room.
    ///
    /// Emitted by `resolve_button_event` when the button's device has no room
    /// mapping. The event loop uses this to create an `UnassignedDevice` triage
    /// entry so the user knows a switch needs attention.
    UnroutableButton {
        hub_key: Option<HubKey>,
        /// Hub-native device ID (if button is in the registry).
        device_id: Option<String>,
        /// Button resource ID that triggered the event.
        button_id: String,
    },
}

impl HubEvent {
    /// Get the hub key for this event, if tagged.
    pub fn hub_key(&self) -> Option<&HubKey> {
        match self {
            HubEvent::Connected { hub_key } => hub_key.as_ref(),
            HubEvent::Button { hub_key, .. } => hub_key.as_ref(),
            HubEvent::Motion { hub_key, .. } => hub_key.as_ref(),
            HubEvent::Contact { hub_key, .. } => hub_key.as_ref(),
            HubEvent::LightPower { hub_key, .. } => hub_key.as_ref(),
            HubEvent::Heartbeat { hub_key } => hub_key.as_ref(),
            HubEvent::Disconnected { hub_key, .. } => hub_key.as_ref(),
            HubEvent::DevicePaired { hub_key, .. } => hub_key.as_ref(),
            HubEvent::UnroutableButton { hub_key, .. } => hub_key.as_ref(),
        }
    }

    /// Tag this event with a hub key (returns self for chaining).
    pub fn with_hub_key(mut self, key: HubKey) -> Self {
        match &mut self {
            HubEvent::Connected { hub_key } => *hub_key = Some(key),
            HubEvent::Button { hub_key, .. } => *hub_key = Some(key),
            HubEvent::Motion { hub_key, .. } => *hub_key = Some(key),
            HubEvent::Contact { hub_key, .. } => *hub_key = Some(key),
            HubEvent::LightPower { hub_key, .. } => *hub_key = Some(key),
            HubEvent::Heartbeat { hub_key } => *hub_key = Some(key),
            HubEvent::Disconnected { hub_key, .. } => *hub_key = Some(key),
            HubEvent::DevicePaired { hub_key, .. } => *hub_key = Some(key),
            HubEvent::UnroutableButton { hub_key, .. } => *hub_key = Some(key),
        }
        self
    }
}

// ============================================================================
// HubType — hub identifier for storage dispatch
// ============================================================================

/// Hub type identifier for storage and dispatch.
///
/// String-based newtype so provider crates can define their own hub types
/// without modifying this crate.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HubType(pub String);

impl HubType {
    pub const HUE: &'static str = "hue";
    pub const HA: &'static str = "ha";
    pub const MATTER: &'static str = "matter";

    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn parse(s: &str) -> Option<Self> {
        if s.is_empty() {
            None
        } else {
            Some(Self(s.to_string()))
        }
    }
}

/// Shared API-facing capability metadata for a registered hub integration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubIntegrationCapability {
    pub hub_type: String,
    pub configurable: bool,
    pub device_onboarding_methods: Vec<String>,
    pub supports_unpairing: bool,
    pub supports_roomless_devices: bool,
}

pub const DEVICE_ONBOARDING_METHOD_MATTER_ON_NETWORK_SETUP_CODE: &str =
    "matter_on_network_setup_code";
pub const DEVICE_ONBOARDING_METHOD_MATTER_BLE_WIFI_COMMISSIONING: &str =
    "matter_ble_wifi_commissioning";

impl HubIntegrationCapability {
    pub fn new(hub_type: impl Into<String>) -> Self {
        Self {
            hub_type: hub_type.into(),
            configurable: true,
            device_onboarding_methods: Vec::new(),
            supports_unpairing: false,
            supports_roomless_devices: false,
        }
    }
}

// ============================================================================
// HubCredentials — per-hub credential storage for persistence
// ============================================================================

/// Per-hub credential storage for persistence.
///
/// Generic struct — provider-specific fields live in `data` as JSON.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HubCredentials {
    /// Which hub type these credentials belong to (None = unconfigured).
    #[serde(default)]
    pub hub_type: Option<HubType>,
    /// Hub address (IP or hostname).
    #[serde(default)]
    pub address: String,
    /// Provider-specific credential data (e.g., username, API key).
    #[serde(default)]
    pub data: serde_json::Value,
    /// Whether this entry came from a redacted restore and cannot be used to connect
    /// until fresh credentials are provided again.
    #[serde(default)]
    pub secrets_redacted: bool,
}

impl HubCredentials {
    /// Check if any hub is configured.
    pub fn is_configured(&self) -> bool {
        self.hub_type.is_some()
    }

    /// Check whether these credentials are usable for a live hub connection.
    pub fn can_connect(&self) -> bool {
        self.is_configured() && !self.secrets_redacted
    }

    /// Create credentials for any hub type.
    pub fn new(hub_type: impl Into<String>, address: &str, data: serde_json::Value) -> Self {
        Self {
            hub_type: Some(HubType::new(hub_type)),
            address: address.to_string(),
            data,
            secrets_redacted: false,
        }
    }

    /// Create a configured-but-disconnected placeholder restored from a redacted backup.
    pub fn redacted_placeholder(hub_type: impl Into<String>, address: &str) -> Self {
        Self {
            hub_type: Some(HubType::new(hub_type)),
            address: address.to_string(),
            data: serde_json::Value::Null,
            secrets_redacted: true,
        }
    }

    /// Extract a string field from the credentials data.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.data.get(key).and_then(|v| v.as_str())
    }

    /// Derive a `HubKey` from these credentials.
    ///
    /// Returns `None` if the hub type is not configured.
    pub fn hub_key(&self) -> Option<HubKey> {
        self.hub_type
            .as_ref()
            .map(|ht| HubKey::new(ht.clone(), &self.address))
    }
}

// ============================================================================
// ActiveHub — the running hub with type-erased runtime
// ============================================================================

/// Active hub with type-erased runtime.
///
/// Created by `connect_sse()` when credentials are available.
/// The runtime may be `None` initially — it's created when the first
/// room is pushed via HTTP (`ensure_runtime()`).
///
/// Hub-specific state is stored in `hub_data` as `Box<dyn Any>`.
/// Use `data::<T>()` to downcast to the concrete type.
pub struct ActiveHub {
    pub hub_type: HubType,
    /// Unique key identifying this hub instance (hub_type + address).
    pub hub_key: HubKey,
    /// Runtime handle. `None` until first room arrives and `ensure_runtime()` is called.
    pub runtime: Option<Arc<dyn RuntimeHandle>>,
    /// Hub-specific state (e.g., `HueHubData` for Hue).
    pub hub_data: Box<dyn Any + Send + Sync>,
    /// Hub-agnostic device registry for command handlers. `None` if the hub doesn't provide one.
    pub registry: Option<Arc<Mutex<dyn HubRegistry>>>,
    /// Hub discovery for server-driven room sync. `None` if the hub doesn't support discovery.
    pub discovery: Option<Arc<dyn HubDiscovery>>,
    /// Shutdown signal for SSE threads. Set to `true` to stop the event stream.
    pub shutdown: Arc<AtomicBool>,
}

impl ActiveHub {
    /// Downcast hub data to a concrete type.
    pub fn data<T: 'static>(&self) -> Option<&T> {
        self.hub_data.downcast_ref::<T>()
    }
}

impl Drop for ActiveHub {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

// ============================================================================
// HubProvider — hub operations called by HTTP/command handlers
// ============================================================================

/// Hub operations called by HTTP/command handlers.
///
/// Each hub type implements this trait so the command layer can
/// configure hubs without hub-specific code.
pub trait HubProvider: Send + Sync {
    fn hub_type(&self) -> HubType;

    /// Store credentials and initialize the hub runtime.
    fn configure(&self, address: &str, credentials_json: &str, state: &SharedState) -> Result<()>;
}

// ============================================================================
// ExternalLightHubIntegration — integration lifecycle bundled for platform crates
// ============================================================================

/// Bundles everything a platform crate needs from an integration.
///
/// Integration crates provide a static `INTEGRATION` value that platform
/// binaries can register directly. Platform crates can still wrap or extend
/// that integration when they need extra board-specific behavior.
///
/// Platform crates store a `&[&dyn ExternalLightHubIntegration]` registry
/// and use [`integration_callbacks`] to derive the `ensure_runtime_fn`
/// and `get_hub_provider_fn` callbacks from it — no match arms needed.
pub trait ExternalLightHubIntegration: Send + Sync {
    /// Hub type string (e.g. `"hue"`, `"homeassistant"`).
    fn hub_type(&self) -> &'static str;

    /// The [`HubProvider`] for HTTP credential pushes.
    fn provider(&self) -> &'static dyn HubProvider;

    /// API-facing capability metadata for this integration.
    ///
    /// Used by `/api/state` so clients can distinguish "supported here" from
    /// "currently configured" and adapt UI to platform constraints.
    fn api_capabilities(&self) -> HubIntegrationCapability {
        HubIntegrationCapability::new(self.hub_type())
    }

    /// Boot-time connection when credentials are already stored.
    ///
    /// The `key` identifies which specific hub instance to connect
    /// (e.g., which Hue bridge IP when multiple bridges are configured).
    ///
    /// Returns the event receiver for the main event loop.
    fn connect_and_start(&self, state: SharedState, key: &HubKey) -> Result<Receiver<HubEvent>>;

    /// Create the `RhythmRuntime` (deferred until first room arrives).
    ///
    /// There is ONE shared runtime across all hubs — the composite
    /// controller fans out commands to per-hub controllers. This method
    /// does NOT take a `HubKey` because the runtime is shared, not per-hub.
    fn ensure_runtime(&self, state: &SharedState) -> Result<()>;

    /// Create a type-erased hub dispatch controller for a specific hub instance.
    ///
    /// Used by the composite controller to collect per-hub controllers.
    /// Returns `Arc<dyn HubLightController>` so the composite can store them
    /// without knowing the concrete type.
    ///
    /// Default returns an error for integrations that do not support
    /// composite mode.
    fn create_controller(
        &self,
        _state: &SharedState,
        _key: &HubKey,
    ) -> Result<std::sync::Arc<dyn rhythm_core::HubLightController>> {
        Err(anyhow::anyhow!(
            "create_controller not implemented for {}",
            self.hub_type()
        ))
    }

    /// Optional post-connect work (location import, device cache, etc.).
    ///
    /// Called after [`connect_and_start`] + `sync_from_hub`. The `key`
    /// identifies which hub instance just connected. Default is a no-op.
    fn post_connect(&self, _state: &SharedState, _key: &HubKey) {}

    /// Optional topology-group synchronization for integrations with generated groups.
    ///
    /// Called after topology/canonical reconciliation and before composite
    /// routing is rebuilt. Default is a no-op.
    fn sync_topology_groups(&self, _state: &SharedState, _key: &HubKey) -> Result<()> {
        Ok(())
    }

    /// Optional interceptor for auto-filling credentials on specific platforms.
    ///
    /// Called by the HTTP credential handler before normal processing.
    /// Returns `None` to skip (let normal flow proceed), `Some(Ok(json))`
    /// on success, or `Some(Err(msg))` on failure.
    ///
    /// Example: HA addon auto-fills SUPERVISOR_TOKEN when a client sends
    /// empty credentials for hub_type "homeassistant".
    fn credentials_interceptor(
        &self,
        _state: &SharedState,
        _body: &serde_json::Value,
    ) -> Option<Result<String, String>> {
        None
    }

    /// Refresh credentials before connecting (e.g., rotating tokens).
    ///
    /// Called at startup before `connect_and_start` for each stored hub.
    /// Default is a no-op — only integrations with rotating credentials
    /// (e.g., HA supervisor token) need to implement this.
    fn refresh_credentials(&self, _state: &SharedState, _key: &HubKey) {}

    /// Start a device pairing session (Matter commissioning, Zigbee permit join).
    ///
    /// Default returns an error — hub-based integrations (Hue, HA) don't support
    /// pairing individual devices. Direct-connection integrations (Matter, Zigbee)
    /// override this.
    fn start_pairing(
        &self,
        _state: &SharedState,
        _params: &serde_json::Value,
    ) -> Result<crate::pairing::PairingSession> {
        Err(anyhow::anyhow!(
            "Pairing not supported for {}",
            self.hub_type()
        ))
    }

    /// Unpair/decommission a device (Matter fabric removal, Zigbee leave).
    ///
    /// Default returns an error — hub-based integrations (Hue, HA) don't support
    /// unpairing individual devices. Direct-connection integrations (Matter, Zigbee)
    /// override this.
    fn start_unpairing(
        &self,
        _state: &SharedState,
        _params: &serde_json::Value,
    ) -> Result<crate::pairing::UnpairingResult> {
        Err(anyhow::anyhow!(
            "Unpairing not supported for {}",
            self.hub_type()
        ))
    }

    /// Run a hub-specific device diagnostic/test command.
    ///
    /// Direct-connection integrations can use this to exercise raw protocol
    /// command variants without going through the normal quirk-adapted runtime
    /// path. The request/response shape is intentionally JSON so shared HTTP
    /// routing stays protocol-neutral.
    fn run_device_test(
        &self,
        _state: &SharedState,
        _params: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        Err(anyhow::anyhow!(
            "Device testing not supported for {}",
            self.hub_type()
        ))
    }

    /// Save a hub-specific device diagnostic report and optionally apply local
    /// quirks discovered by that report.
    fn save_device_test_report(
        &self,
        _state: &SharedState,
        _report: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        Err(anyhow::anyhow!(
            "Device test reports not supported for {}",
            self.hub_type()
        ))
    }
}

/// Look up an integration by hub type string.
pub fn find_integration<'a>(
    integrations: &'a [&'a dyn ExternalLightHubIntegration],
    hub_type: &str,
) -> Option<&'a dyn ExternalLightHubIntegration> {
    integrations
        .iter()
        .find(|i| i.hub_type() == hub_type)
        .copied()
}

/// Build a combined credentials interceptor from all integrations.
///
/// Returns a closure that tries each integration's `credentials_interceptor`
/// in order. The first non-`None` result wins. If all return `None`, the
/// combined interceptor also returns `None` (letting normal flow proceed).
#[allow(clippy::type_complexity)]
pub fn combined_credentials_interceptor(
    integrations: &'static [&'static dyn ExternalLightHubIntegration],
) -> Arc<dyn Fn(&SharedState, &serde_json::Value) -> Option<Result<String, String>> + Send + Sync> {
    Arc::new(move |state, body| {
        for integration in integrations {
            if let Some(result) = integration.credentials_interceptor(state, body) {
                return Some(result);
            }
        }
        None
    })
}

const STORED_HUB_BOOTSTRAP_INITIAL_RETRY_DELAY: Duration = Duration::from_secs(5);
const STORED_HUB_BOOTSTRAP_MAX_RETRY_DELAY: Duration = Duration::from_secs(60 * 60);
const STORED_HUB_BOOTSTRAP_AUTO_RETRY_WINDOW: Duration = Duration::from_secs(60 * 60 * 24);
const STORED_HUB_BOOTSTRAP_WAKE_INTERVAL: Duration = Duration::from_secs(1);

struct StoredHubBootstrapCandidate<'a> {
    key: HubKey,
    integration: &'a dyn ExternalLightHubIntegration,
}

struct StoredHubBootstrapScan<'a> {
    connectable_credential_count: usize,
    due_supported: Vec<StoredHubBootstrapCandidate<'a>>,
    scheduled_supported_count: usize,
    manual_retry_required_count: usize,
    next_retry_after: Option<Duration>,
    unsupported_missing: Vec<(HubKey, String)>,
}

enum BootstrapLoopDecision {
    Done,
    Sleep(Duration),
}

trait BootstrapClock {
    fn now_instant(&self) -> Instant;
    fn now_epoch_ms(&self) -> i64;
    fn sleep(&mut self, duration: Duration);
}

struct SystemBootstrapClock;

impl BootstrapClock for SystemBootstrapClock {
    fn now_instant(&self) -> Instant {
        Instant::now()
    }

    fn now_epoch_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    fn sleep(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

struct HubBootstrapWorkerGuard {
    state: SharedState,
}

impl Drop for HubBootstrapWorkerGuard {
    fn drop(&mut self) {
        if let Ok(mut s) = self.state.lock() {
            s.finish_hub_bootstrap_worker();
        }
    }
}

/// Spawn a background worker that connects hubs from stored credentials and
/// keeps retrying failed startup connects until each supported configured hub
/// leaves an active hub object behind in state.
pub fn spawn_stored_hub_bootstrap(
    state: SharedState,
    integrations: &'static [&'static dyn ExternalLightHubIntegration],
) {
    let should_spawn = match state.lock() {
        Ok(mut s) => s.begin_hub_bootstrap_worker(),
        Err(_) => {
            warn!(
                target: "sys",
                "Skipping hub bootstrap spawn: state lock poisoned"
            );
            false
        }
    };
    if !should_spawn {
        return;
    }

    info!(
        target: "sys",
        "Starting hub bootstrap in background; HTTP startup will not wait for hub sync"
    );

    let thread_state = state.clone();
    let spawn_result = std::thread::Builder::new()
        .name("hub-bootstrap".to_string())
        .spawn(move || {
            let _guard = HubBootstrapWorkerGuard {
                state: thread_state.clone(),
            };
            let mut clock = SystemBootstrapClock;
            bootstrap_stored_hubs_until_settled(&thread_state, integrations, &mut clock);
        });

    if spawn_result.is_err() {
        if let Ok(mut s) = state.lock() {
            s.finish_hub_bootstrap_worker();
        }
    }
    spawn_result.expect("Failed to spawn hub bootstrap thread");
}

fn bootstrap_stored_hubs_until_settled<'a>(
    state: &SharedState,
    integrations: &'a [&'a dyn ExternalLightHubIntegration],
    clock: &mut impl BootstrapClock,
) {
    loop {
        match bootstrap_stored_hubs_once(state, integrations, clock) {
            BootstrapLoopDecision::Done => return,
            BootstrapLoopDecision::Sleep(duration) => clock.sleep(duration),
        }
    }
}

fn bootstrap_stored_hubs_once<'a>(
    state: &SharedState,
    integrations: &'a [&'a dyn ExternalLightHubIntegration],
    clock: &mut impl BootstrapClock,
) -> BootstrapLoopDecision {
    let now = clock.now_instant();
    let scan = match scan_stored_hub_bootstrap_candidates(state, integrations, now) {
        Ok(scan) => scan,
        Err(_) => {
            warn!(target: "sys", "Hub bootstrap aborted: state lock poisoned");
            return BootstrapLoopDecision::Done;
        }
    };

    if scan.connectable_credential_count == 0 {
        info!(
            target: "sys",
            "No connectable hub credentials loaded, waiting for credential push"
        );
        return BootstrapLoopDecision::Done;
    }

    if scan.due_supported.is_empty() {
        if scan.scheduled_supported_count > 0 {
            return BootstrapLoopDecision::Sleep(
                scan.next_retry_after
                    .unwrap_or(STORED_HUB_BOOTSTRAP_WAKE_INTERVAL)
                    .min(STORED_HUB_BOOTSTRAP_WAKE_INTERVAL),
            );
        }

        if !scan.unsupported_missing.is_empty() {
            for (key, hub_type_str) in &scan.unsupported_missing {
                warn!(
                    target: "sys",
                    "No integration for hub type '{}', skipping stored hub {}",
                    hub_type_str,
                    key
                );
            }
        }
        if scan.manual_retry_required_count > 0 {
            info!(
                target: "sys",
                "Hub bootstrap waiting for manual retry on {} stored hub(s)",
                scan.manual_retry_required_count
            );
        }
        return BootstrapLoopDecision::Done;
    }

    let mut newly_connected: Vec<StoredHubBootstrapCandidate<'a>> = Vec::new();

    for candidate in scan.due_supported {
        candidate
            .integration
            .refresh_credentials(state, &candidate.key);
        info!(
            target: "sys",
            "Connecting stored hub {} (type={})...",
            candidate.key,
            candidate.integration.hub_type()
        );

        match candidate
            .integration
            .connect_and_start(state.clone(), &candidate.key)
        {
            Ok(event_rx) => {
                let registered = match state.lock() {
                    Ok(mut s) => {
                        let registered = s.hubs.contains_key(&candidate.key);
                        if registered {
                            s.pending_hub_event_rxs.push(event_rx);
                        }
                        registered
                    }
                    Err(_) => {
                        warn!(
                            target: "sys",
                            "Connected stored hub {} but failed to register its event stream",
                            candidate.key
                        );
                        false
                    }
                };

                if registered {
                    if let Ok(mut s) = state.lock() {
                        s.clear_hub_startup_retry(&candidate.key);
                    }
                    newly_connected.push(candidate);
                } else {
                    let error = format!(
                        "Stored hub {} connected but did not leave an active hub behind",
                        candidate.key
                    );
                    let retry_status = note_stored_hub_bootstrap_failure(
                        state,
                        &candidate.key,
                        &error,
                        clock.now_instant(),
                        clock.now_epoch_ms(),
                    );
                    log_bootstrap_failure(&candidate.key, &error, &retry_status);
                }
            }
            Err(error) => {
                let error_text = error.to_string();
                let retry_status = note_stored_hub_bootstrap_failure(
                    state,
                    &candidate.key,
                    &error_text,
                    clock.now_instant(),
                    clock.now_epoch_ms(),
                );
                log_bootstrap_failure(&candidate.key, &error_text, &retry_status);
            }
        }
    }

    finish_bootstrapped_hubs(state, &newly_connected);

    match scan_stored_hub_bootstrap_candidates(state, integrations, clock.now_instant()) {
        Ok(next_scan) if next_scan.scheduled_supported_count > 0 => BootstrapLoopDecision::Sleep(
            next_scan
                .next_retry_after
                .unwrap_or(STORED_HUB_BOOTSTRAP_WAKE_INTERVAL)
                .min(STORED_HUB_BOOTSTRAP_WAKE_INTERVAL),
        ),
        Ok(next_scan)
            if !next_scan.due_supported.is_empty()
                || next_scan.manual_retry_required_count > 0
                || !next_scan.unsupported_missing.is_empty() =>
        {
            BootstrapLoopDecision::Done
        }
        Ok(_) => BootstrapLoopDecision::Done,
        Err(_) => BootstrapLoopDecision::Done,
    }
}

fn finish_bootstrapped_hubs<'a>(
    state: &SharedState,
    newly_connected: &[StoredHubBootstrapCandidate<'a>],
) {
    if newly_connected.is_empty() {
        return;
    }

    let discover_devices = state
        .lock()
        .map(|s| s.platform.full_device_discovery)
        .unwrap_or(true);

    for candidate in newly_connected {
        crate::commands::register_hub_with_composite(state, &candidate.key);
    }

    if let Err(error) = crate::commands::reconcile_runtime_from_state(state) {
        warn!(
            target: "sys",
            "Startup runtime reconciliation failed after hub connect: {}",
            error
        );
    }

    for candidate in newly_connected {
        if let Err(error) =
            crate::room_sync::sync_from_hub_for_key(state, &candidate.key, discover_devices)
        {
            warn!(
                target: "sys",
                "Startup sync failed for hub {}: {}",
                candidate.key,
                error
            );
        }
    }

    crate::room_sync::poll_initial_light_state(state);

    for candidate in newly_connected {
        candidate.integration.post_connect(state, &candidate.key);
    }
}

fn scan_stored_hub_bootstrap_candidates<'a>(
    state: &SharedState,
    integrations: &'a [&'a dyn ExternalLightHubIntegration],
    now: Instant,
) -> Result<StoredHubBootstrapScan<'a>> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

    let mut connectable_credential_count = 0usize;
    let connectable_keys: HashSet<_> = s
        .hub_credentials
        .iter()
        .filter(|(_, creds)| creds.can_connect())
        .map(|(key, _)| key.clone())
        .collect();
    let active_keys: HashSet<_> = s.hubs.keys().cloned().collect();
    s.hub_startup_retry
        .retain(|key, _| connectable_keys.contains(key) && !active_keys.contains(key));

    let mut due_supported = Vec::new();
    let mut scheduled_supported_count = 0usize;
    let mut manual_retry_required_count = 0usize;
    let mut next_retry_after = None;
    let mut unsupported_missing = Vec::new();

    for (key, creds) in &s.hub_credentials {
        if !creds.can_connect() {
            continue;
        }

        connectable_credential_count += 1;

        if s.hubs.contains_key(key) {
            continue;
        }

        let Some(hub_type) = creds.hub_type.as_ref() else {
            continue;
        };

        if let Some(integration) = find_integration(integrations, hub_type.as_str()) {
            let retry = s.hub_startup_retry(key);
            if retry.is_some_and(|retry| retry.manual_retry_required) {
                manual_retry_required_count += 1;
                continue;
            }

            if let Some(retry) = retry {
                if let Some(next_retry_at) = retry.next_retry_at {
                    if next_retry_at > now {
                        scheduled_supported_count += 1;
                        let wait = next_retry_at.duration_since(now);
                        next_retry_after = Some(
                            next_retry_after.map_or(wait, |current: Duration| current.min(wait)),
                        );
                        continue;
                    }
                }
            }

            due_supported.push(StoredHubBootstrapCandidate {
                key: key.clone(),
                integration,
            });
        } else {
            unsupported_missing.push((key.clone(), hub_type.as_str().to_string()));
        }
    }

    Ok(StoredHubBootstrapScan {
        connectable_credential_count,
        due_supported,
        scheduled_supported_count,
        manual_retry_required_count,
        next_retry_after,
        unsupported_missing,
    })
}

enum BootstrapFailureOutcome {
    RetryScheduled { next_delay: Duration },
    ManualRetryRequired,
}

fn log_bootstrap_failure(hub_key: &HubKey, error: &str, outcome: &BootstrapFailureOutcome) {
    match outcome {
        BootstrapFailureOutcome::RetryScheduled { next_delay } => warn!(
            target: "sys",
            "Failed to connect stored hub {}: {} (retrying in {}s)",
            hub_key,
            error,
            next_delay.as_secs()
        ),
        BootstrapFailureOutcome::ManualRetryRequired => warn!(
            target: "sys",
            "Failed to connect stored hub {}: {} (automatic retry window exhausted; waiting for app retry)",
            hub_key,
            error
        ),
    }
}

fn note_stored_hub_bootstrap_failure(
    state: &SharedState,
    hub_key: &HubKey,
    error: &str,
    now: Instant,
    now_epoch_ms: i64,
) -> BootstrapFailureOutcome {
    let outcome = match state.lock() {
        Ok(mut s) => {
            let retry = s
                .hub_startup_retry
                .entry(hub_key.clone())
                .or_insert_with(|| crate::state::HubStartupRetryState {
                    attempt_count: 0,
                    first_failure_at: now,
                    first_failure_epoch_ms: now_epoch_ms,
                    last_failure_epoch_ms: now_epoch_ms,
                    next_retry_at: None,
                    next_retry_epoch_ms: None,
                    last_error: error.to_string(),
                    manual_retry_required: false,
                });
            retry.attempt_count += 1;
            retry.last_failure_epoch_ms = now_epoch_ms;
            retry.last_error = error.to_string();

            match next_bootstrap_retry_delay(retry.attempt_count, retry.first_failure_at, now) {
                Some(next_delay) => {
                    retry.next_retry_at = Some(now + next_delay);
                    retry.next_retry_epoch_ms =
                        Some(now_epoch_ms.saturating_add(next_delay.as_millis() as i64));
                    retry.manual_retry_required = false;
                    BootstrapFailureOutcome::RetryScheduled { next_delay }
                }
                None => {
                    retry.next_retry_at = None;
                    retry.next_retry_epoch_ms = None;
                    retry.manual_retry_required = true;
                    BootstrapFailureOutcome::ManualRetryRequired
                }
            }
        }
        Err(_) => BootstrapFailureOutcome::ManualRetryRequired,
    };

    crate::state::emit_server_event(
        state,
        crate::server_event::ServerEvent::HubStatus {
            hub_type: Some(hub_key.hub_type.as_str().to_string()),
            address: Some(hub_key.address.clone()),
            connected: false,
        },
    );

    outcome
}

fn next_bootstrap_retry_delay(
    attempt_count: u32,
    first_failure_at: Instant,
    now: Instant,
) -> Option<Duration> {
    let next_delay = bootstrap_retry_delay_for_attempt(attempt_count);
    let window_deadline = first_failure_at + STORED_HUB_BOOTSTRAP_AUTO_RETRY_WINDOW;
    let next_retry_at = now.checked_add(next_delay)?;
    (next_retry_at <= window_deadline).then_some(next_delay)
}

fn bootstrap_retry_delay_for_attempt(attempt_count: u32) -> Duration {
    let exponent = attempt_count.saturating_sub(1).min(16);
    let seconds = STORED_HUB_BOOTSTRAP_INITIAL_RETRY_DELAY
        .as_secs()
        .saturating_mul(1u64 << exponent)
        .min(STORED_HUB_BOOTSTRAP_MAX_RETRY_DELAY.as_secs());
    Duration::from_secs(seconds)
}

/// Callback set returned by [`integration_callbacks`].
#[allow(clippy::type_complexity)]
pub struct IntegrationCallbacks {
    /// Create the shared runtime (called when first room arrives).
    pub ensure_runtime_fn: Arc<dyn Fn(&SharedState) -> Result<()> + Send + Sync>,
    /// Look up a hub provider by hub type.
    pub get_hub_provider_fn: Arc<dyn Fn(HubType) -> &'static (dyn HubProvider) + Send + Sync>,
    /// Create and register a per-hub controller with the composite (desktop only).
    pub register_controller_fn: Arc<dyn Fn(&SharedState, &HubKey) -> Result<()> + Send + Sync>,
    /// Synchronize integration-managed topology groups.
    pub sync_topology_groups_fn: Arc<dyn Fn(&SharedState) -> Result<()> + Send + Sync>,
    /// Start a device pairing session (delegates to integration's `start_pairing`).
    pub start_pairing_fn: Arc<
        dyn Fn(&SharedState, &str, &serde_json::Value) -> Result<crate::pairing::PairingSession>
            + Send
            + Sync,
    >,
    /// Start a device unpairing session (delegates to integration's `start_unpairing`).
    pub start_unpairing_fn: Arc<
        dyn Fn(&SharedState, &str, &serde_json::Value) -> Result<crate::pairing::UnpairingResult>
            + Send
            + Sync,
    >,
    /// Run a device diagnostic/test command.
    pub run_device_test_fn: Arc<
        dyn Fn(&SharedState, &str, &serde_json::Value) -> Result<serde_json::Value> + Send + Sync,
    >,
    /// Save a device diagnostic/test report.
    pub save_device_test_report_fn: Arc<
        dyn Fn(&SharedState, &str, &serde_json::Value) -> Result<serde_json::Value> + Send + Sync,
    >,
    /// API-facing capability metadata for all registered integrations.
    pub hub_capabilities: Vec<HubIntegrationCapability>,
}

/// Build callbacks from a static integration registry.
///
/// Platform crates call this once at startup and store the results in
/// [`AppState`]. Eliminates per-binary match arms for hub dispatch.
pub fn integration_callbacks(
    integrations: &'static [&'static dyn ExternalLightHubIntegration],
) -> IntegrationCallbacks {
    let mut hub_capabilities: Vec<HubIntegrationCapability> = integrations
        .iter()
        .map(|integration| integration.api_capabilities())
        .collect();
    hub_capabilities.sort_by(|left, right| left.hub_type.cmp(&right.hub_type));
    hub_capabilities.dedup_by(|left, right| left.hub_type == right.hub_type);

    let ensure_runtime_fn = Arc::new(move |state: &SharedState| -> Result<()> {
        crate::lifecycle::ensure_composite_runtime(state, integrations)
    });

    let get_hub_provider_fn = Arc::new(move |hub_type: HubType| -> &'static dyn HubProvider {
        find_integration(integrations, hub_type.as_str())
            .unwrap_or_else(|| {
                panic!(
                    "No integration registered for hub type: {}",
                    hub_type.as_str()
                )
            })
            .provider()
    });

    let register_controller_fn = Arc::new(move |state: &SharedState, key: &HubKey| -> Result<()> {
        let composite = {
            let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            s.composite_controller.clone()
        };
        let Some(composite) = composite else {
            // No composite yet — runtime hasn't been created. Controller will be
            // picked up when ensure_composite_runtime runs on first room arrival.
            return Ok(());
        };
        let hub_type_str = key.hub_type.as_str();
        let integration = find_integration(integrations, hub_type_str)
            .ok_or_else(|| anyhow::anyhow!("No integration for hub type '{}'", hub_type_str))?;
        let controller = integration.create_controller(state, key)?;
        let key_str = key.to_string();
        log::info!(target: "sys", "Dynamically registered {} controller with composite", key_str);
        composite.register_controller(&key_str, controller);
        Ok(())
    });

    let sync_topology_groups_fn = Arc::new(move |state: &SharedState| -> Result<()> {
        let active_hubs = {
            let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            s.hubs
                .keys()
                .filter_map(|key| {
                    find_integration(integrations, key.hub_type.as_str())
                        .map(|integration| (key.clone(), integration))
                })
                .collect::<Vec<_>>()
        };

        for (key, integration) in active_hubs {
            integration.sync_topology_groups(state, &key)?;
        }

        Ok(())
    });

    let start_pairing_fn = Arc::new(
        move |state: &SharedState,
              hub_type: &str,
              params: &serde_json::Value|
              -> Result<crate::pairing::PairingSession> {
            let integration = find_integration(integrations, hub_type)
                .ok_or_else(|| anyhow::anyhow!("No integration for hub type '{}'", hub_type))?;
            integration.start_pairing(state, params)
        },
    );

    let start_unpairing_fn = Arc::new(
        move |state: &SharedState,
              hub_type: &str,
              params: &serde_json::Value|
              -> Result<crate::pairing::UnpairingResult> {
            let integration = find_integration(integrations, hub_type)
                .ok_or_else(|| anyhow::anyhow!("No integration for hub type '{}'", hub_type))?;
            integration.start_unpairing(state, params)
        },
    );

    let run_device_test_fn = Arc::new(
        move |state: &SharedState,
              hub_type: &str,
              params: &serde_json::Value|
              -> Result<serde_json::Value> {
            let integration = find_integration(integrations, hub_type)
                .ok_or_else(|| anyhow::anyhow!("No integration for hub type '{}'", hub_type))?;
            integration.run_device_test(state, params)
        },
    );

    let save_device_test_report_fn = Arc::new(
        move |state: &SharedState,
              hub_type: &str,
              report: &serde_json::Value|
              -> Result<serde_json::Value> {
            let integration = find_integration(integrations, hub_type)
                .ok_or_else(|| anyhow::anyhow!("No integration for hub type '{}'", hub_type))?;
            integration.save_device_test_report(state, report)
        },
    );

    IntegrationCallbacks {
        ensure_runtime_fn,
        get_hub_provider_fn,
        register_controller_fn,
        sync_topology_groups_fn,
        start_pairing_fn,
        start_unpairing_fn,
        run_device_test_fn,
        save_device_test_report_fn,
        hub_capabilities,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    use std::sync::mpsc;
    use std::sync::Arc;

    use crate::discovery::{DiscoveredDevice, DiscoveredRoom, HubDiscovery};

    // ── Mock integration for testing ──────────────────────────────────

    /// Tracks how many times ensure_runtime was called.
    struct MockIntegration {
        hub_type: &'static str,
        ensure_count: AtomicU32,
        post_connect_count: AtomicU32,
    }

    impl MockIntegration {
        const fn new(hub_type: &'static str) -> Self {
            Self {
                hub_type,
                ensure_count: AtomicU32::new(0),
                post_connect_count: AtomicU32::new(0),
            }
        }

        #[allow(dead_code)]
        fn ensure_calls(&self) -> u32 {
            self.ensure_count.load(Ordering::Relaxed)
        }

        fn post_connect_calls(&self) -> u32 {
            self.post_connect_count.load(Ordering::Relaxed)
        }
    }

    /// Mock provider that just reports its hub type.
    struct MockProvider {
        hub_type: &'static str,
    }

    impl HubProvider for MockProvider {
        fn hub_type(&self) -> HubType {
            HubType::new(self.hub_type)
        }
        fn configure(&self, _: &str, _: &str, _: &SharedState) -> Result<()> {
            Ok(())
        }
    }

    static MOCK_HUE_PROVIDER: MockProvider = MockProvider { hub_type: "hue" };
    static MOCK_HA_PROVIDER: MockProvider = MockProvider {
        hub_type: "homeassistant",
    };

    impl ExternalLightHubIntegration for MockIntegration {
        fn hub_type(&self) -> &'static str {
            self.hub_type
        }

        fn provider(&self) -> &'static dyn HubProvider {
            match self.hub_type {
                "hue" => &MOCK_HUE_PROVIDER,
                _ => &MOCK_HA_PROVIDER,
            }
        }

        fn connect_and_start(
            &self,
            _state: SharedState,
            _key: &HubKey,
        ) -> Result<Receiver<HubEvent>> {
            let (_tx, rx) = mpsc::channel();
            Ok(rx)
        }

        fn ensure_runtime(&self, _state: &SharedState) -> Result<()> {
            self.ensure_count.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn post_connect(&self, _state: &SharedState, _key: &HubKey) {
            self.post_connect_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    static MOCK_HUE: MockIntegration = MockIntegration::new("hue");
    static MOCK_HA: MockIntegration = MockIntegration::new("homeassistant");

    static TEST_INTEGRATIONS: &[&dyn ExternalLightHubIntegration] = &[&MOCK_HUE, &MOCK_HA];

    fn string_error<T>(result: Result<T>) -> String {
        match result {
            Ok(_) => panic!("expected error"),
            Err(error) => error.to_string(),
        }
    }

    struct EmptyDiscovery;

    impl HubDiscovery for EmptyDiscovery {
        fn discover_rooms(&self) -> Result<Vec<DiscoveredRoom>> {
            Ok(Vec::new())
        }

        fn discover_devices(&self) -> Result<Vec<DiscoveredDevice>> {
            Ok(Vec::new())
        }
    }

    static BOOTSTRAP_DISCOVERY_SAW_RUNTIME: AtomicBool = AtomicBool::new(false);

    struct RuntimeRequiredBeforeDiscovery {
        state: SharedState,
    }

    impl HubDiscovery for RuntimeRequiredBeforeDiscovery {
        fn discover_rooms(&self) -> Result<Vec<DiscoveredRoom>> {
            let has_runtime = self
                .state
                .lock()
                .map(|state| state.hub_runtime().is_some())
                .unwrap_or(false);
            assert!(
                has_runtime,
                "startup sync began before runtime reconciliation"
            );
            BOOTSTRAP_DISCOVERY_SAW_RUNTIME.store(true, Ordering::Relaxed);
            Ok(Vec::new())
        }

        fn discover_devices(&self) -> Result<Vec<DiscoveredDevice>> {
            Ok(Vec::new())
        }
    }

    struct RuntimeBeforeSyncIntegration;

    impl ExternalLightHubIntegration for RuntimeBeforeSyncIntegration {
        fn hub_type(&self) -> &'static str {
            "hue"
        }

        fn provider(&self) -> &'static dyn HubProvider {
            &MOCK_HUE_PROVIDER
        }

        fn connect_and_start(
            &self,
            state: SharedState,
            key: &HubKey,
        ) -> Result<Receiver<HubEvent>> {
            let (_tx, rx) = mpsc::channel();
            let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            s.hubs.insert(
                key.clone(),
                ActiveHub {
                    hub_type: HubType::new("hue"),
                    hub_key: key.clone(),
                    runtime: None,
                    hub_data: Box::new(()),
                    registry: None,
                    discovery: Some(Arc::new(RuntimeRequiredBeforeDiscovery {
                        state: state.clone(),
                    })),
                    shutdown: Arc::new(AtomicBool::new(false)),
                },
            );
            s.set_hub_connected(key, false);
            Ok(rx)
        }

        fn ensure_runtime(&self, state: &SharedState) -> Result<()> {
            crate::lifecycle::ensure_composite_runtime(state, RUNTIME_BEFORE_SYNC_INTEGRATIONS)
        }

        fn create_controller(
            &self,
            _state: &SharedState,
            _key: &HubKey,
        ) -> Result<Arc<dyn rhythm_core::HubLightController>> {
            Ok(Arc::new(rhythm_core::NoOpController::new()))
        }
    }

    static RUNTIME_BEFORE_SYNC_INTEGRATION: RuntimeBeforeSyncIntegration =
        RuntimeBeforeSyncIntegration;
    static RUNTIME_BEFORE_SYNC_INTEGRATIONS: &[&dyn ExternalLightHubIntegration] =
        &[&RUNTIME_BEFORE_SYNC_INTEGRATION];

    struct MockBootstrapClock {
        now_instant: Instant,
        now_epoch_ms: i64,
        sleeps: Vec<Duration>,
    }

    impl MockBootstrapClock {
        fn new() -> Self {
            Self {
                now_instant: Instant::now(),
                now_epoch_ms: 1_700_000_000_000,
                sleeps: Vec::new(),
            }
        }
    }

    impl BootstrapClock for MockBootstrapClock {
        fn now_instant(&self) -> Instant {
            self.now_instant
        }

        fn now_epoch_ms(&self) -> i64 {
            self.now_epoch_ms
        }

        fn sleep(&mut self, duration: Duration) {
            self.sleeps.push(duration);
            self.now_instant += duration;
            self.now_epoch_ms += duration.as_millis() as i64;
        }
    }

    struct BootstrapTestIntegration {
        failures_before_success: AtomicU32,
        connect_attempts: AtomicU32,
        post_connect_count: AtomicU32,
    }

    impl BootstrapTestIntegration {
        fn new(failures_before_success: u32) -> Self {
            Self {
                failures_before_success: AtomicU32::new(failures_before_success),
                connect_attempts: AtomicU32::new(0),
                post_connect_count: AtomicU32::new(0),
            }
        }

        fn connect_attempts(&self) -> u32 {
            self.connect_attempts.load(Ordering::Relaxed)
        }

        fn post_connect_calls(&self) -> u32 {
            self.post_connect_count.load(Ordering::Relaxed)
        }
    }

    impl ExternalLightHubIntegration for BootstrapTestIntegration {
        fn hub_type(&self) -> &'static str {
            "hue"
        }

        fn provider(&self) -> &'static dyn HubProvider {
            &MOCK_HUE_PROVIDER
        }

        fn connect_and_start(
            &self,
            state: SharedState,
            key: &HubKey,
        ) -> Result<Receiver<HubEvent>> {
            self.connect_attempts.fetch_add(1, Ordering::Relaxed);

            if self.failures_before_success.load(Ordering::Relaxed) > 0 {
                self.failures_before_success.fetch_sub(1, Ordering::Relaxed);
                return Err(anyhow::anyhow!("temporary startup failure"));
            }

            let (_tx, rx) = mpsc::channel();
            let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            s.hubs.insert(
                key.clone(),
                ActiveHub {
                    hub_type: HubType::new("hue"),
                    hub_key: key.clone(),
                    runtime: None,
                    hub_data: Box::new(()),
                    registry: None,
                    discovery: Some(Arc::new(EmptyDiscovery)),
                    shutdown: Arc::new(AtomicBool::new(false)),
                },
            );
            s.set_hub_connected(key, false);
            Ok(rx)
        }

        fn ensure_runtime(&self, _state: &SharedState) -> Result<()> {
            Ok(())
        }

        fn post_connect(&self, _state: &SharedState, _key: &HubKey) {
            self.post_connect_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    // ── find_integration tests ────────────────────────────────────────

    #[test]
    fn find_integration_returns_matching_type() {
        let result = find_integration(TEST_INTEGRATIONS, "hue");
        assert!(result.is_some());
        assert_eq!(result.unwrap().hub_type(), "hue");
    }

    #[test]
    fn find_integration_returns_ha() {
        let result = find_integration(TEST_INTEGRATIONS, "homeassistant");
        assert!(result.is_some());
        assert_eq!(result.unwrap().hub_type(), "homeassistant");
    }

    #[test]
    fn find_integration_returns_none_for_unknown() {
        let result = find_integration(TEST_INTEGRATIONS, "ikea");
        assert!(result.is_none());
    }

    #[test]
    fn find_integration_returns_none_for_empty_registry() {
        let empty: &[&dyn ExternalLightHubIntegration] = &[];
        let result = find_integration(empty, "hue");
        assert!(result.is_none());
    }

    // ── integration_callbacks tests ───────────────────────────────────

    #[test]
    fn provider_callback_dispatches_by_hub_type() {
        let callbacks = integration_callbacks(TEST_INTEGRATIONS);

        let hue_provider = (callbacks.get_hub_provider_fn)(HubType::new("hue"));
        assert_eq!(hue_provider.hub_type().as_str(), "hue");

        let ha_provider = (callbacks.get_hub_provider_fn)(HubType::new("homeassistant"));
        assert_eq!(ha_provider.hub_type().as_str(), "homeassistant");
    }

    #[test]
    #[should_panic(expected = "No integration registered for hub type: unknown")]
    fn provider_callback_panics_for_unknown_hub_type() {
        let callbacks = integration_callbacks(TEST_INTEGRATIONS);

        let _ = (callbacks.get_hub_provider_fn)(HubType::new("unknown"));
    }

    #[test]
    fn post_connect_is_callable_per_integration() {
        let hue = MockIntegration::new("hue");
        let ha = MockIntegration::new("homeassistant");

        let state: SharedState = Arc::new(Mutex::new(crate::state::AppState::default()));
        let hue_key = HubKey::new(HubType::new("hue"), "192.168.1.5");
        let ha_key = HubKey::new(HubType::new("homeassistant"), "192.168.1.10");

        assert_eq!(hue.post_connect_calls(), 0);
        assert_eq!(ha.post_connect_calls(), 0);

        hue.post_connect(&state, &hue_key);
        assert_eq!(hue.post_connect_calls(), 1);
        assert_eq!(ha.post_connect_calls(), 0);

        ha.post_connect(&state, &ha_key);
        assert_eq!(ha.post_connect_calls(), 1);
    }

    // ── Trait default method ──────────────────────────────────────────

    #[test]
    fn post_connect_default_is_noop() {
        // A struct that doesn't override post_connect
        struct MinimalIntegration;
        impl ExternalLightHubIntegration for MinimalIntegration {
            fn hub_type(&self) -> &'static str {
                "test"
            }
            fn provider(&self) -> &'static dyn HubProvider {
                &MOCK_HUE_PROVIDER
            }
            fn connect_and_start(&self, _: SharedState, _: &HubKey) -> Result<Receiver<HubEvent>> {
                let (_tx, rx) = mpsc::channel();
                Ok(rx)
            }
            fn ensure_runtime(&self, _: &SharedState) -> Result<()> {
                Ok(())
            }
            // post_connect not overridden — uses default no-op
        }

        let integration = MinimalIntegration;
        let state: SharedState = Arc::new(Mutex::new(crate::state::AppState::default()));
        let key = HubKey::new(HubType::new("test"), "127.0.0.1");
        integration.post_connect(&state, &key); // should not panic
    }

    #[test]
    fn trait_default_methods_report_unsupported_operations() {
        struct MinimalIntegration;
        impl ExternalLightHubIntegration for MinimalIntegration {
            fn hub_type(&self) -> &'static str {
                "test"
            }
            fn provider(&self) -> &'static dyn HubProvider {
                &MOCK_HUE_PROVIDER
            }
            fn connect_and_start(&self, _: SharedState, _: &HubKey) -> Result<Receiver<HubEvent>> {
                let (_tx, rx) = mpsc::channel();
                Ok(rx)
            }
            fn ensure_runtime(&self, _: &SharedState) -> Result<()> {
                Ok(())
            }
        }

        let integration = MinimalIntegration;
        let state: SharedState = Arc::new(Mutex::new(crate::state::AppState::default()));
        let key = HubKey::new(HubType::new("test"), "local");
        let params = serde_json::json!({"device_id": "test-1"});

        let caps = integration.api_capabilities();
        assert_eq!(caps.hub_type, "test");
        assert!(caps.configurable);
        assert!(integration.create_controller(&state, &key).is_err());
        assert!(integration.start_pairing(&state, &params).is_err());
        assert!(integration.start_unpairing(&state, &params).is_err());
        assert!(integration.run_device_test(&state, &params).is_err());
        assert!(integration
            .save_device_test_report(&state, &params)
            .is_err());
        assert!(integration.sync_topology_groups(&state, &key).is_ok());
        integration.refresh_credentials(&state, &key);
        assert!(integration
            .credentials_interceptor(&state, &serde_json::json!({}))
            .is_none());
    }

    #[test]
    fn combined_credentials_interceptor_returns_first_integration_result() {
        struct InterceptorIntegration {
            hub_type: &'static str,
            response: Option<&'static str>,
        }

        impl ExternalLightHubIntegration for InterceptorIntegration {
            fn hub_type(&self) -> &'static str {
                self.hub_type
            }
            fn provider(&self) -> &'static dyn HubProvider {
                &MOCK_HUE_PROVIDER
            }
            fn connect_and_start(&self, _: SharedState, _: &HubKey) -> Result<Receiver<HubEvent>> {
                let (_tx, rx) = mpsc::channel();
                Ok(rx)
            }
            fn ensure_runtime(&self, _: &SharedState) -> Result<()> {
                Ok(())
            }
            fn credentials_interceptor(
                &self,
                _state: &SharedState,
                _body: &serde_json::Value,
            ) -> Option<Result<String, String>> {
                self.response.map(|value| Ok(value.to_string()))
            }
        }

        static FIRST: InterceptorIntegration = InterceptorIntegration {
            hub_type: "first",
            response: None,
        };
        static SECOND: InterceptorIntegration = InterceptorIntegration {
            hub_type: "second",
            response: Some("filled"),
        };
        static INTERCEPTOR_INTEGRATIONS: &[&dyn ExternalLightHubIntegration] = &[&FIRST, &SECOND];
        let state: SharedState = Arc::new(Mutex::new(crate::state::AppState::default()));

        let interceptor = combined_credentials_interceptor(INTERCEPTOR_INTEGRATIONS);
        let result = interceptor(&state, &serde_json::json!({}))
            .unwrap()
            .unwrap();

        assert_eq!(result, "filled");
    }

    #[test]
    fn integration_callbacks_cover_capabilities_and_unsupported_dispatch() {
        let callbacks = integration_callbacks(TEST_INTEGRATIONS);
        let state: SharedState = Arc::new(Mutex::new(crate::state::AppState::default()));

        assert_eq!(
            callbacks
                .hub_capabilities
                .iter()
                .map(|cap| cap.hub_type.as_str())
                .collect::<Vec<_>>(),
            vec!["homeassistant", "hue"]
        );
        assert!((callbacks.register_controller_fn)(
            &state,
            &HubKey::new(HubType::new("hue"), "192.168.1.5")
        )
        .is_ok());

        state.lock().unwrap().composite_controller =
            Some(Arc::new(rhythm_core::CompositeController::new()));
        let missing_key = HubKey::new(HubType::new("unknown"), "local");
        assert!(
            string_error((callbacks.register_controller_fn)(&state, &missing_key))
                .contains("No integration for hub type")
        );

        assert!(string_error((callbacks.start_pairing_fn)(
            &state,
            "unknown",
            &serde_json::json!({})
        ))
        .contains("No integration for hub type"));
        assert!(string_error((callbacks.start_unpairing_fn)(
            &state,
            "unknown",
            &serde_json::json!({})
        ))
        .contains("No integration for hub type"));
        assert!(string_error((callbacks.run_device_test_fn)(
            &state,
            "unknown",
            &serde_json::json!({})
        ))
        .contains("No integration for hub type"));
        assert!(string_error((callbacks.save_device_test_report_fn)(
            &state,
            "unknown",
            &serde_json::json!({})
        ))
        .contains("No integration for hub type"));
    }

    #[test]
    fn stored_hub_bootstrap_retries_until_connect_succeeds() {
        let integration = BootstrapTestIntegration::new(1);
        let integrations: &[&dyn ExternalLightHubIntegration] = &[&integration];
        let state: SharedState = Arc::new(Mutex::new(crate::state::AppState::default()));
        let key = HubKey::new(HubType::new("hue"), "192.168.1.5");

        state.lock().unwrap().hub_credentials.insert(
            key.clone(),
            HubCredentials::new("hue", "192.168.1.5", serde_json::json!({"username": "abc"})),
        );

        let mut clock = MockBootstrapClock::new();
        bootstrap_stored_hubs_until_settled(&state, integrations, &mut clock);

        let s = state.lock().unwrap();
        assert!(s.hubs.contains_key(&key));
        assert_eq!(s.pending_hub_event_rxs.len(), 1);
        assert_eq!(integration.connect_attempts(), 2);
        assert_eq!(integration.post_connect_calls(), 1);
        assert_eq!(clock.sleeps.len(), 5);
        assert!(clock
            .sleeps
            .iter()
            .all(|sleep| *sleep == Duration::from_secs(1)));
    }

    #[test]
    fn stored_hub_bootstrap_skips_hubs_that_are_already_active() {
        let integration = BootstrapTestIntegration::new(0);
        let integrations: &[&dyn ExternalLightHubIntegration] = &[&integration];
        let state: SharedState = Arc::new(Mutex::new(crate::state::AppState::default()));
        let key = HubKey::new(HubType::new("hue"), "192.168.1.5");

        {
            let mut s = state.lock().unwrap();
            s.hub_credentials.insert(
                key.clone(),
                HubCredentials::new("hue", "192.168.1.5", serde_json::json!({"username": "abc"})),
            );
            s.hubs.insert(
                key.clone(),
                ActiveHub {
                    hub_type: HubType::new("hue"),
                    hub_key: key.clone(),
                    runtime: None,
                    hub_data: Box::new(()),
                    registry: None,
                    discovery: Some(Arc::new(EmptyDiscovery)),
                    shutdown: Arc::new(AtomicBool::new(false)),
                },
            );
        }

        let mut clock = MockBootstrapClock::new();
        bootstrap_stored_hubs_until_settled(&state, integrations, &mut clock);

        assert_eq!(integration.connect_attempts(), 0);
        assert_eq!(integration.post_connect_calls(), 0);
        assert_eq!(state.lock().unwrap().pending_hub_event_rxs.len(), 0);
        assert!(clock.sleeps.is_empty());
    }

    #[test]
    fn stored_hub_bootstrap_reconciles_runtime_before_startup_sync() {
        BOOTSTRAP_DISCOVERY_SAW_RUNTIME.store(false, Ordering::Relaxed);
        let state: SharedState = Arc::new(Mutex::new(crate::state::AppState::default()));
        let key = HubKey::new(HubType::new("hue"), "192.168.1.5");
        let callbacks = integration_callbacks(RUNTIME_BEFORE_SYNC_INTEGRATIONS);

        {
            let mut s = state.lock().unwrap();
            s.ensure_runtime_fn = Some(callbacks.ensure_runtime_fn.clone());
            s.register_controller_fn = Some(callbacks.register_controller_fn.clone());
            s.hub_credentials.insert(
                key.clone(),
                HubCredentials::new("hue", "192.168.1.5", serde_json::json!({"username": "abc"})),
            );
            s.topology
                .insert_room(crate::topology::TopologyRoom::new("room-1", "Room 1"));
        }

        let mut clock = MockBootstrapClock::new();
        bootstrap_stored_hubs_until_settled(&state, RUNTIME_BEFORE_SYNC_INTEGRATIONS, &mut clock);

        let s = state.lock().unwrap();
        assert!(s.hub_runtime().is_some());
        assert_eq!(s.pending_hub_event_rxs.len(), 1);
        assert!(BOOTSTRAP_DISCOVERY_SAW_RUNTIME.load(Ordering::Relaxed));
    }

    #[test]
    fn bootstrap_retry_delay_caps_at_one_hour() {
        assert_eq!(
            bootstrap_retry_delay_for_attempt(1),
            STORED_HUB_BOOTSTRAP_INITIAL_RETRY_DELAY
        );
        assert_eq!(
            bootstrap_retry_delay_for_attempt(2),
            Duration::from_secs(10)
        );
        assert_eq!(
            bootstrap_retry_delay_for_attempt(10),
            Duration::from_secs(2560)
        );
        assert_eq!(
            bootstrap_retry_delay_for_attempt(20),
            STORED_HUB_BOOTSTRAP_MAX_RETRY_DELAY
        );
    }

    #[test]
    fn bootstrap_retry_window_eventually_requires_manual_retry() {
        let first_failure = Instant::now();
        assert_eq!(
            next_bootstrap_retry_delay(1, first_failure, first_failure),
            Some(Duration::from_secs(5))
        );
        let almost_done =
            first_failure + STORED_HUB_BOOTSTRAP_AUTO_RETRY_WINDOW - Duration::from_secs(30);
        assert_eq!(
            next_bootstrap_retry_delay(20, first_failure, almost_done),
            None
        );
    }

    // ── HubCredentials / HubKey ───────────────────────────────────────

    #[test]
    fn hub_credentials_key_includes_type_and_address() {
        let creds = HubCredentials::new(
            "homeassistant",
            "192.168.1.5:8123",
            serde_json::json!({"token": "abc"}),
        );
        let key = creds.hub_key().unwrap();
        assert_eq!(key.hub_type.as_str(), "homeassistant");
        assert_eq!(key.address, "192.168.1.5:8123");
    }

    #[test]
    fn hub_credentials_get_str_extracts_token() {
        let creds = HubCredentials::new(
            "homeassistant",
            "ha.local:8123",
            serde_json::json!({"token": "my_token"}),
        );
        assert_eq!(creds.get_str("token"), Some("my_token"));
        assert_eq!(creds.get_str("username"), None);
    }

    #[test]
    fn redacted_hub_credentials_are_configured_but_not_connectable() {
        let creds = HubCredentials::redacted_placeholder("matter", "local");
        let key = creds.hub_key().unwrap();
        assert!(creds.is_configured());
        assert!(!creds.can_connect());
        assert!(creds.secrets_redacted);
        assert_eq!(key.hub_type.as_str(), "matter");
        assert_eq!(key.address, "local");
    }
}
