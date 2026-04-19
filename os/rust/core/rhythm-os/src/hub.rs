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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use anyhow::Result;
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
}

impl HubCredentials {
    /// Check if any hub is configured.
    pub fn is_configured(&self) -> bool {
        self.hub_type.is_some()
    }

    /// Create credentials for any hub type.
    pub fn new(hub_type: impl Into<String>, address: &str, data: serde_json::Value) -> Self {
        Self {
            hub_type: Some(HubType::new(hub_type)),
            address: address.to_string(),
            data,
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
/// Desktop integration crates (rhythm-hue, rhythm-ha) provide a static
/// `INTEGRATION` behind their `desktop` feature flag. Embedded platforms
/// create their own struct wrapping the embedded lifecycle with
/// platform-specific transport.
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
    /// Default returns an error — embedded integrations that don't support
    /// composite mode don't need to implement this.
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

/// Callback set returned by [`integration_callbacks`].
#[allow(clippy::type_complexity)]
pub struct IntegrationCallbacks {
    /// Create the shared runtime (called when first room arrives).
    pub ensure_runtime_fn: Arc<dyn Fn(&SharedState) -> Result<()> + Send + Sync>,
    /// Look up a hub provider by hub type.
    pub get_hub_provider_fn: Arc<dyn Fn(HubType) -> &'static (dyn HubProvider) + Send + Sync>,
    /// Create and register a per-hub controller with the composite (desktop only).
    #[cfg(feature = "desktop")]
    pub register_controller_fn: Arc<dyn Fn(&SharedState, &HubKey) -> Result<()> + Send + Sync>,
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
        // Use composite runtime on desktop (creates CompositeController + single shared runtime).
        // Falls back to per-integration ensure_runtime for backward compat (ESP32).
        #[cfg(feature = "blocking")]
        {
            #[allow(clippy::needless_return)]
            return crate::lifecycle::ensure_composite_runtime(state, integrations);
        }
        #[cfg(not(feature = "blocking"))]
        {
            let hub_type_str = {
                let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
                s.first_hub_credentials()
                    .and_then(|c| c.hub_type.as_ref().map(|t| t.as_str().to_string()))
            };
            if let Some(ht) = &hub_type_str {
                if let Some(integration) = find_integration(integrations, ht) {
                    return integration.ensure_runtime(state);
                }
            }
            if let Some(integration) = integrations.first() {
                return integration.ensure_runtime(state);
            }
            Err(anyhow::anyhow!("No integrations registered"))
        }
    });

    let get_hub_provider_fn = Arc::new(move |hub_type: HubType| -> &'static dyn HubProvider {
        if let Some(integration) = find_integration(integrations, hub_type.as_str()) {
            return integration.provider();
        }
        // Fallback: first integration's provider
        integrations
            .first()
            .expect("No integrations registered")
            .provider()
    });

    #[cfg(feature = "desktop")]
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

    IntegrationCallbacks {
        ensure_runtime_fn,
        get_hub_provider_fn,
        #[cfg(feature = "desktop")]
        register_controller_fn,
        start_pairing_fn,
        start_unpairing_fn,
        hub_capabilities,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    use std::sync::mpsc;

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
    fn provider_callback_falls_back_to_first() {
        let callbacks = integration_callbacks(TEST_INTEGRATIONS);

        // Unknown hub type should fall back to first integration (hue)
        let fallback = (callbacks.get_hub_provider_fn)(HubType::new("unknown"));
        assert_eq!(fallback.hub_type().as_str(), "hue");
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
}
