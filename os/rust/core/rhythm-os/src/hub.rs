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
use rhythm_core::runtime::hub_registry::DeviceType;
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
    /// Motion detected or cleared by a known sensor. `room_id` is empty when
    /// the integration has no native room mapping and topology must route it.
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
    /// The integration observed an upstream resource add/delete that requires
    /// fresh room/device discovery before live routing can remain authoritative.
    TopologyChanged {
        hub_key: Option<HubKey>,
        resource_id: String,
        resource_type: String,
    },
    /// Integration-authoritative liveness evidence for one physical endpoint.
    ///
    /// This is intentionally separate from power state: an off light remains
    /// healthy when it reports or responds, and command intent is never proof.
    /// Failed command outcomes use the bounded `Command` class, and only when
    /// the sidecar classified the failure as connectivity; only structured
    /// subscription termination metadata may identify address resolution.
    DeviceReachability {
        hub_key: Option<HubKey>,
        device_id: String,
        fabric_id: String,
        controller_stream_id: Option<String>,
        evidence: DeviceReachabilityEvidence,
    },
    /// Terminal outcome for controller-owned asynchronous physical work.
    CommandOutcome {
        hub_key: Option<HubKey>,
        controller_stream_id: String,
        command_id: u64,
        device_id: String,
        status: HubCommandOutcomeStatus,
        detail: Option<String>,
    },
    /// The controller event stream restarted or lost retained events. Any
    /// accepted command without a terminal outcome is now indeterminate.
    CommandStreamReset {
        hub_key: Option<HubKey>,
        stream_id: String,
        /// A retention gap invalidates commands accepted by the current
        /// process too. A process restart invalidates only older stream ids.
        history_gap: bool,
        reason: String,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubCommandOutcomeStatus {
    Succeeded,
    Failed,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceReachabilityEvidence {
    Proof,
    Failure(DeviceReachabilityFailureClass),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceReachabilityFailureClass {
    AddressResolution,
    Read,
    Command,
    Subscription,
}

impl HubCommandOutcomeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Superseded => "superseded",
        }
    }
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
            HubEvent::TopologyChanged { hub_key, .. } => hub_key.as_ref(),
            HubEvent::DeviceReachability { hub_key, .. } => hub_key.as_ref(),
            HubEvent::CommandOutcome { hub_key, .. } => hub_key.as_ref(),
            HubEvent::CommandStreamReset { hub_key, .. } => hub_key.as_ref(),
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
            HubEvent::TopologyChanged { hub_key, .. } => *hub_key = Some(key),
            HubEvent::DeviceReachability { hub_key, .. } => *hub_key = Some(key),
            HubEvent::CommandOutcome { hub_key, .. } => *hub_key = Some(key),
            HubEvent::CommandStreamReset { hub_key, .. } => *hub_key = Some(key),
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
    pub const HUE_BLE: &'static str = "hue_ble";
    /// Vendor-neutral local BLE profile host. Rich compatibility drivers may
    /// retain a legacy hub type while sharing the same adapter runtime.
    pub const LOCAL_BLE: &'static str = "local_ble";
    pub const HA: &'static str = "ha";
    pub const MATTER: &'static str = "matter";
    /// Monster/Ayla static lighting commissioned over the shared appliance
    /// Bluetooth adapter and controlled over the authenticated LAN protocol.
    pub const MONSTER: &'static str = "monster";

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

/// Maximum serialized length shared by capability, pairing-history, registry,
/// and local-store profile IDs.
pub const DEVICE_PROFILE_ID_MAX_LEN: usize = 80;

/// Parsed identity for one versioned device-profile protocol family.
///
/// IDs use `<family-segments>.v<positive version>`. The family is everything
/// before the version segment; vendors and product names may be useful family
/// segments, but callers must not infer behavior from them. Compatibility is
/// declared explicitly by the active decoder instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParsedDeviceProfileId<'a> {
    family: &'a str,
    version: u32,
}

impl<'a> ParsedDeviceProfileId<'a> {
    pub fn parse(value: &'a str) -> Option<Self> {
        if value.is_empty()
            || value.len() > DEVICE_PROFILE_ID_MAX_LEN
            || !value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
            })
        {
            return None;
        }

        let (family, version_segment) = value.rsplit_once('.')?;
        let family_segments = family.split('.').collect::<Vec<_>>();
        if family_segments.len() < 2 || family_segments.iter().any(|segment| segment.is_empty()) {
            return None;
        }

        let version_digits = version_segment.strip_prefix('v')?;
        if version_digits.is_empty()
            || version_digits.starts_with('0')
            || !version_digits.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        let version = version_digits.parse::<u32>().ok()?;
        Some(Self { family, version })
    }

    pub fn family(self) -> &'a str {
        self.family
    }

    pub fn version(self) -> u32 {
        self.version
    }
}

pub fn is_valid_device_profile_id(value: &str) -> bool {
    ParsedDeviceProfileId::parse(value).is_some()
}

/// One device profile supported by a generic integration onboarding method.
///
/// Profile identifiers are stable protocol contracts rather than marketing
/// names. Clients use this bounded metadata for routing, presentation and
/// known phone provisioning protocols. Final device admission and LAN identity
/// verification remain appliance-side.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubDeviceProfileCapability {
    pub id: String,
    /// Older IDs whose setup and persisted state the current profile decoder
    /// explicitly understands. Clients may classify input with one of these
    /// IDs, but must submit the advertised current `id` to the appliance.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compatible_profile_ids: Vec<String>,
    pub device_type: String,
    pub display_name: String,
    pub input_only: bool,
    /// Intake strategies implemented for this profile. A client must support
    /// both the profile parser/handler and one advertised method before it
    /// offers onboarding.
    #[serde(default)]
    pub onboarding_methods: Vec<String>,
    /// Bluetooth service UUIDs a phone may scan for to notice this profile's
    /// devices waiting in setup mode. Empty when the profile is not found by
    /// nearby scanning.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nearby_service_uuids: Vec<String>,
    /// Name of the Rhythm cloud function the app brokers commissioning
    /// through for this profile, when its vendor requires a cloud step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud_broker: Option<String>,
    /// Versioned phone GATT protocol. Advertising it guarantees the owner-only
    /// Wi-Fi credential route and durable, queryable final `adopt` receipts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone_provisioning_protocol: Option<String>,
}

impl HubDeviceProfileCapability {
    /// Resolve an exact current ID or one explicitly compatible older ID to
    /// this capability's current ID. Malformed/cross-family/newer claims fail
    /// closed even if an integration accidentally advertises them.
    pub fn canonical_id_for<'a>(&'a self, candidate: &str) -> Option<&'a str> {
        let current = ParsedDeviceProfileId::parse(&self.id)?;
        if candidate == self.id.as_str() {
            return Some(self.id.as_str());
        }
        if !self
            .compatible_profile_ids
            .iter()
            .any(|compatible_id| compatible_id == candidate)
        {
            return None;
        }
        let compatible = ParsedDeviceProfileId::parse(candidate)?;
        (compatible.family() == current.family() && compatible.version() < current.version())
            .then_some(self.id.as_str())
    }
}

/// Shared API-facing capability metadata for a registered hub integration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubIntegrationCapability {
    pub hub_type: String,
    pub configurable: bool,
    pub device_onboarding_methods: Vec<String>,
    pub device_profiles: Vec<HubDeviceProfileCapability>,
    pub supports_unpairing: bool,
    /// Stable device-type labels whose physical/integration-owned resources
    /// this hub can remove. An empty list means callers must use the legacy
    /// aggregate flag conservatively.
    pub unpairable_device_types: Vec<String>,
    pub supports_roomless_devices: bool,
    /// Whether a configured-but-disconnected instance should hold the app's
    /// Rooms startup gate because its control plane must rebuild authoritative
    /// topology. This is deliberately integration-level, not inferred from a
    /// device being an input or a light: an appliance-local store can restore
    /// both kinds without blocking unrelated Rooms.
    pub blocks_room_readiness: bool,
}

pub const DEVICE_ONBOARDING_METHOD_MATTER_ON_NETWORK_SETUP_CODE: &str =
    "matter_on_network_setup_code";
pub const DEVICE_ONBOARDING_METHOD_MATTER_BLE_WIFI_COMMISSIONING: &str =
    "matter_ble_wifi_commissioning";
pub const DEVICE_ONBOARDING_METHOD_MATTER_PHONE_COMMISSIONING_HANDOFF: &str =
    "matter_phone_commissioning_handoff";
/// Scan for and bond every newly advertising factory-reset Hue BLE bulb.
pub const DEVICE_ONBOARDING_METHOD_HUE_BLE_NEARBY_SCAN: &str = "hue_ble_nearby_scan";
/// Resolve a locally parsed, server-advertised BLE device profile.
pub const DEVICE_ONBOARDING_METHOD_LOCAL_BLE_QR: &str = "local_ble_qr";
/// Find a nearby Wi-Fi light in Bluetooth setup mode, commission it onto the
/// appliance's Wi-Fi, then adopt its LAN credentials. Staged and vendor
/// neutral: the app brokers any cloud steps between `discover`, `provision`
/// and `adopt`, guided by the hub's advertised device profiles.
pub const DEVICE_ONBOARDING_METHOD_BLE_WIFI_NEARBY_SCAN: &str = "ble_wifi_nearby_scan";
/// Ask an already connected Hue Bridge to find one Zigbee light by its
/// six-character printed serial.
pub const DEVICE_ONBOARDING_METHOD_HUE_BRIDGE_SERIAL_SEARCH: &str = "hue_bridge_serial_search";
/// Ask an already connected Hue Bridge to discover a physical button, remote,
/// or wall switch through its native Zigbee accessory search.
pub const DEVICE_ONBOARDING_METHOD_HUE_BRIDGE_BUTTON_SEARCH: &str = "hue_bridge_button_search";

impl HubIntegrationCapability {
    pub fn new(hub_type: impl Into<String>) -> Self {
        Self {
            hub_type: hub_type.into(),
            configurable: true,
            device_onboarding_methods: Vec::new(),
            device_profiles: Vec::new(),
            supports_unpairing: false,
            unpairable_device_types: Vec::new(),
            supports_roomless_devices: false,
            blocks_room_readiness: true,
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
    /// A full backup restore durably stages Hue recovery credentials before
    /// publishing the matching ownership manifest. While this marker is set,
    /// the credentials may restore/finalize that controller epoch but must not
    /// auto-connect and acquire authority from a partially imported graph.
    #[serde(default)]
    pub backup_restore_pending: bool,
}

impl HubCredentials {
    /// Check if any hub is configured.
    pub fn is_configured(&self) -> bool {
        self.hub_type.is_some()
    }

    /// Check whether these credentials are usable for a live hub connection.
    pub fn can_connect(&self) -> bool {
        self.can_restore_external_controller() && !self.backup_restore_pending
    }

    /// Check whether these credentials retain enough secret material to
    /// restore an external controller during a fail-closed lifecycle retry.
    pub fn can_restore_external_controller(&self) -> bool {
        self.is_configured() && !self.secrets_redacted
    }

    /// Create credentials for any hub type.
    pub fn new(hub_type: impl Into<String>, address: &str, data: serde_json::Value) -> Self {
        Self {
            hub_type: Some(HubType::new(hub_type)),
            address: address.to_string(),
            data,
            secrets_redacted: false,
            backup_restore_pending: false,
        }
    }

    /// Create a configured-but-disconnected placeholder restored from a redacted backup.
    pub fn redacted_placeholder(hub_type: impl Into<String>, address: &str) -> Self {
        Self {
            hub_type: Some(HubType::new(hub_type)),
            address: address.to_string(),
            data: serde_json::Value::Null,
            secrets_redacted: true,
            backup_restore_pending: false,
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

    /// Validate secret-bearing backup credentials against their physical
    /// controller before the current installation crosses a destructive
    /// restore boundary. The default fails closed; commands invoke this only
    /// when a backup carries external-controller recovery material.
    fn validate_backup_credentials(
        &self,
        _address: &str,
        _credentials: &serde_json::Value,
    ) -> Result<()> {
        anyhow::bail!("Hub provider does not support backup credential validation")
    }

    /// Store credentials and initialize the hub runtime.
    fn configure(&self, address: &str, credentials_json: &str, state: &SharedState) -> Result<()>;
}

// ============================================================================
// ExternalLightHubIntegration — integration lifecycle bundled for platform crates
// ============================================================================

/// Integration-neutral context for a requested canonical device move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubDeviceRoomAssignment {
    pub hub_key: HubKey,
    pub native_device_id: String,
    pub device_type: DeviceType,
    /// Whether this endpoint is the canonical route Rhythm will use for
    /// control. A duplicate non-preferred endpoint must not force a second
    /// grouped dispatch path for the same physical light.
    pub preferred_for_control: bool,
    pub target_rhythm_room_id: Option<String>,
    pub target_hub_room_ids: Vec<String>,
}

/// Roll back an integration-native room assignment after a later step fails.
pub type HubDeviceRoomAssignmentRollback = Box<dyn FnOnce() -> Result<()> + Send>;

/// Why Rhythm is relinquishing authority over an external controller.
///
/// Integrations may use the reason for durable receipts, but every variant is
/// a release boundary: credentials and recovery material can be removed after
/// the callback succeeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalControllerReleaseReason {
    /// A reviewed room policy no longer grants the integration authority.
    /// Credentials remain connected after the integration restores its work.
    RoomAuthorityChanged,
    UserDisconnect,
    FactoryReset,
    BackupRestore,
    BinaryRollback,
}

/// Whether an integration changed its authoritative native room membership.
pub enum HubDeviceRoomAssignmentOutcome {
    Unchanged,
    Reassigned {
        target_hub_room_id: Option<String>,
        rollback: HubDeviceRoomAssignmentRollback,
    },
    /// The integration confirmed the native move and returned the complete
    /// grouped-room binding that Rhythm must persist for the target room.
    ///
    /// `target_binding` is `None` only for a standalone/unassigned light. For
    /// an attached light it contains the hub-native room ID, grouped control
    /// ID, and the exact native light membership after the move. This variant
    /// also allows an integration to create the native room during prepare.
    ReassignedWithBinding {
        target_binding: Option<crate::topology::HubRoomBinding>,
        /// Explicit ownership receipt. Authoritative bindings are never
        /// inferred from a native room name or identifier.
        managed_by_rhythm: bool,
        rollback: HubDeviceRoomAssignmentRollback,
    },
}

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

    /// Delete one source-owned native room after Rhythm has durably staged
    /// the corresponding local topology deletion.
    fn delete_source_room(
        &self,
        _state: &SharedState,
        _binding: &crate::topology::HubRoomBinding,
    ) -> Result<()> {
        anyhow::bail!("Source room deletion is not supported by this integration")
    }

    /// Rename one integration-native device to match its canonical Rhythm name.
    ///
    /// Integrations whose source name is not user-visible may keep the default
    /// no-op. Hue Bridge overrides this because its device name is visible in
    /// the Hue app and must remain aligned with Rhythm.
    fn rename_device(
        &self,
        _state: &SharedState,
        _hub_key: &HubKey,
        _native_device_id: &str,
        _name: &str,
    ) -> Result<()> {
        Ok(())
    }

    /// Whether every light attached to a Rhythm room must route through a
    /// hub-native grouped room binding.
    ///
    /// Integrations that return `true` must use
    /// [`HubDeviceRoomAssignmentOutcome::ReassignedWithBinding`] for light
    /// moves. Standalone lights remain directly addressable.
    fn requires_grouped_room_control(&self) -> bool {
        false
    }

    /// Whether this integration must acquire bridge-scoped authority before
    /// ordinary control is safe.
    ///
    /// This is separate from grouped-room routing: an integration may need to
    /// suppress a controller's native automations while leaving its topology
    /// untouched and routing lights individually. The default preserves the
    /// legacy contract for integrations whose authority exists solely to own
    /// grouped rooms.
    fn requires_external_controller_authority(&self) -> bool {
        self.requires_grouped_room_control()
    }

    /// Acquire or resume authoritative control after the integration's first
    /// discovery sync established Rhythm's desired state.
    ///
    /// A successful return is required before the hub can be treated as ready.
    /// Implementations must durably capture recovery state before their first
    /// destructive external mutation.
    fn reconcile_external_controller_authority(
        &self,
        _state: &SharedState,
        _key: &HubKey,
    ) -> Result<()> {
        Ok(())
    }

    /// Restore externally owned state before Rhythm discards credentials or
    /// crosses another destructive local lifecycle boundary.
    fn release_external_controller_authority(
        &self,
        _state: &SharedState,
        _key: &HubKey,
        _reason: ExternalControllerReleaseReason,
    ) -> Result<()> {
        Ok(())
    }

    /// Finalize local recovery state after credential removal is durable.
    /// This must not perform controller I/O or require credentials from disk;
    /// commands call it while the old in-memory credential is still present.
    fn finalize_external_controller_release(
        &self,
        _state: &SharedState,
        _key: &HubKey,
    ) -> Result<()> {
        Ok(())
    }

    /// Prepare an integration-native device room assignment before Rhythm
    /// commits the corresponding topology change.
    ///
    /// Integrations whose native room membership is authoritative override
    /// this. Returning an error aborts the Rhythm-side move so the two
    /// topologies cannot silently diverge.
    fn prepare_device_room_assignment(
        &self,
        _state: &SharedState,
        _assignment: &HubDeviceRoomAssignment,
    ) -> Result<HubDeviceRoomAssignmentOutcome> {
        Ok(HubDeviceRoomAssignmentOutcome::Unchanged)
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
    /// Default returns an error. Direct integrations and hubs with an explicit
    /// upstream add/search operation override this.
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

    /// Start pairing with the request's original monotonic acceptance time.
    /// Integrations with an absolute end-to-end SLA override this; existing
    /// integrations inherit the ordinary pairing behavior.
    fn start_pairing_with_context(
        &self,
        state: &SharedState,
        params: &serde_json::Value,
        _context: crate::pairing::PairingRequestContext,
    ) -> Result<crate::pairing::PairingSession> {
        self.start_pairing(state, params)
    }

    /// Reconcile integration-owned durable completion receipts with the
    /// shared pairing ledger. Implementations must not require their transport
    /// to be available: this hook is used by status polling to repair the
    /// response after an already-committed device activation.
    fn reconcile_pairing_results(&self, _state: &SharedState) -> Result<()> {
        Ok(())
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

    /// Load owner-visible recovery material for a native paired device.
    ///
    /// Integrations that do not retain pairing secrets return `None`. Callers
    /// must keep the returned value out of logs and diagnostics.
    fn load_pairing_recovery(
        &self,
        _state: &SharedState,
        _native_device_id: &str,
    ) -> Result<Option<crate::pairing::PairingRecoverySecret>> {
        Ok(None)
    }

    /// Permanently delete integration-owned pairing recovery material.
    fn purge_pairing_recovery(&self, _state: &SharedState, _native_device_id: &str) -> Result<()> {
        Ok(())
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
    due_authority: Vec<StoredHubBootstrapCandidate<'a>>,
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
/// keeps retrying failed startup connects and authority acquisition until each
/// supported configured hub is safe for normal routing.
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

    if let Err(e) = spawn_result {
        // Thread spawn fails under fd/thread exhaustion; panicking here would
        // abort startup (or the configure request) instead of retrying later.
        warn!(target: "sys", "Failed to spawn hub bootstrap thread: {}", e);
        if let Ok(mut s) = state.lock() {
            s.finish_hub_bootstrap_worker();
        }
    }
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

    if scan.due_supported.is_empty() && scan.due_authority.is_empty() {
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

    finish_bootstrapped_hubs(state, &newly_connected, clock);

    for candidate in scan.due_authority {
        retry_active_hub_authority(state, &candidate, clock);
    }

    match scan_stored_hub_bootstrap_candidates(state, integrations, clock.now_instant()) {
        Ok(next_scan) => bootstrap_loop_decision_after_scan(&next_scan),
        Err(_) => BootstrapLoopDecision::Done,
    }
}

fn bootstrap_loop_decision_after_scan(scan: &StoredHubBootstrapScan<'_>) -> BootstrapLoopDecision {
    if scan.scheduled_supported_count > 0 {
        return BootstrapLoopDecision::Sleep(
            scan.next_retry_after
                .unwrap_or(STORED_HUB_BOOTSTRAP_WAKE_INTERVAL)
                .min(STORED_HUB_BOOTSTRAP_WAKE_INTERVAL),
        );
    }

    // Successful hubs may take longer to reconcile than another hub's retry
    // delay. In that case the second scan reports the failed hub as due now.
    // Continue immediately instead of ending the only bootstrap worker with a
    // stale `scheduled` retry that can never run.
    if !scan.due_supported.is_empty() || !scan.due_authority.is_empty() {
        return BootstrapLoopDecision::Sleep(Duration::ZERO);
    }

    BootstrapLoopDecision::Done
}

fn controller_authority_is_enabled(
    state: &crate::state::AppState,
    key: &HubKey,
    integration: &dyn ExternalLightHubIntegration,
) -> bool {
    integration.requires_external_controller_authority()
        && state.external_controller_authority_is_enabled_for(key)
}

fn grouped_room_control_is_enabled(
    state: &crate::state::AppState,
    key: &HubKey,
    integration: &dyn ExternalLightHubIntegration,
) -> bool {
    integration.requires_grouped_room_control()
        && state.external_controller_authority_is_enabled_for(key)
}

fn finish_bootstrapped_hubs<'a>(
    state: &SharedState,
    newly_connected: &[StoredHubBootstrapCandidate<'a>],
    clock: &mut impl BootstrapClock,
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

    let mut authority_ready = Vec::new();
    for candidate in newly_connected {
        let requires_authority = state
            .lock()
            .map(|state| {
                controller_authority_is_enabled(&state, &candidate.key, candidate.integration)
            })
            .unwrap_or(false);
        if requires_authority {
            if let Ok(mut state) = state.lock() {
                state.mark_external_controller_initial_sync_pending(&candidate.key);
            }
        }
        let sync_result = if requires_authority {
            crate::room_sync::sync_from_hub_for_key_before_authority(
                state,
                &candidate.key,
                discover_devices,
            )
        } else {
            crate::room_sync::sync_from_hub_for_key(state, &candidate.key, discover_devices)
        };
        if let Err(error) = sync_result {
            discard_active_hub_for_full_bootstrap_retry(state, &candidate.key);
            let error_text = error.to_string();
            let retry_status = note_stored_hub_bootstrap_failure(
                state,
                &candidate.key,
                &error_text,
                clock.now_instant(),
                clock.now_epoch_ms(),
            );
            log_sync_bootstrap_failure(&candidate.key, &error_text, &retry_status);
            continue;
        }
        if requires_authority {
            if let Ok(mut state) = state.lock() {
                state.mark_external_controller_initial_sync_complete(&candidate.key);
            }
        }
        if requires_authority {
            if let Err(error) = reconcile_bootstrap_external_authority(state, candidate) {
                let error_text = error.to_string();
                let retry_status = note_stored_hub_bootstrap_failure(
                    state,
                    &candidate.key,
                    &error_text,
                    clock.now_instant(),
                    clock.now_epoch_ms(),
                );
                log_authority_bootstrap_failure(&candidate.key, &error_text, &retry_status);
                continue;
            }
        }
        if let Ok(mut state) = state.lock() {
            state.clear_hub_startup_retry(&candidate.key);
        }
        authority_ready.push(candidate);
    }

    crate::room_sync::poll_initial_light_state(state);

    for candidate in authority_ready {
        candidate.integration.post_connect(state, &candidate.key);
    }
}

pub(crate) fn discard_active_hub_for_full_bootstrap_retry(state: &SharedState, hub_key: &HubKey) {
    let (old_hub, composite) = match state.lock() {
        Ok(mut state) => {
            let requires_controller_authority =
                state.external_controller_authority_is_required(hub_key);
            let old_hub = state.hubs.remove(hub_key);
            if let Some(hub) = old_hub.as_ref() {
                hub.shutdown.store(true, Ordering::SeqCst);
            }
            state.clear_hub_connected(hub_key);
            // Clearing the live instance must not create a dispatch window
            // before the bootstrap worker reacquires controller authority.
            // Initial-sync pending is intentionally cleared so that worker
            // performs a full discovery, while the authority fence remains.
            if requires_controller_authority {
                state.mark_external_controller_authority_pending(hub_key);
            }
            (old_hub, state.composite_controller.clone())
        }
        Err(_) => return,
    };

    if let Some(composite) = composite {
        composite.remove_controller(&hub_key.to_string());
        crate::commands::rebuild_composite_routing(state);
    }

    if let Some(old_hub) = old_hub {
        std::thread::Builder::new()
            .name("hub-drop".to_string())
            .spawn(move || drop(old_hub))
            .ok();
    }
}

fn reconcile_bootstrap_external_authority(
    state: &SharedState,
    candidate: &StoredHubBootstrapCandidate<'_>,
) -> Result<()> {
    let transaction_lock = state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .external_topology_transaction_lock
        .clone();
    let _transaction = transaction_lock
        .lock()
        .map_err(|_| anyhow::anyhow!("External topology transaction lock poisoned"))?;
    {
        let mut state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        if !state.hubs.contains_key(&candidate.key) {
            anyhow::bail!("Hub {} is no longer active", candidate.key);
        }
        if !state
            .hub_credentials
            .get(&candidate.key)
            .is_some_and(HubCredentials::can_connect)
        {
            anyhow::bail!(
                "Hub {} no longer has connectable credentials",
                candidate.key
            );
        }
        if state.authority_state_recovery_required {
            anyhow::bail!("Authoritative topology state requires recovery");
        }
        if !controller_authority_is_enabled(&state, &candidate.key, candidate.integration) {
            return Ok(());
        }
        if state.external_controller_initial_sync_is_pending(&candidate.key) {
            anyhow::bail!("Initial authoritative topology sync is incomplete");
        }
        state.mark_external_controller_authority_pending(&candidate.key);
    }

    candidate
        .integration
        .reconcile_external_controller_authority(state, &candidate.key)?;
    state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .mark_external_controller_authority_ready(&candidate.key);
    Ok(())
}

fn retry_active_hub_authority(
    state: &SharedState,
    candidate: &StoredHubBootstrapCandidate<'_>,
    clock: &mut impl BootstrapClock,
) {
    info!(
        target: "sys",
        "Retrying external-controller authority reconciliation for {}",
        candidate.key
    );
    match reconcile_bootstrap_external_authority(state, candidate) {
        Ok(()) => {
            if let Ok(mut state) = state.lock() {
                state.clear_hub_startup_retry(&candidate.key);
            }
            candidate.integration.post_connect(state, &candidate.key);
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
            log_authority_bootstrap_failure(&candidate.key, &error_text, &retry_status);
        }
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
    let authority_pending = s.external_controller_authority_pending.clone();
    s.hub_startup_retry.retain(|key, _| {
        connectable_keys.contains(key)
            && (!active_keys.contains(key) || authority_pending.contains(key))
    });

    let mut due_supported = Vec::new();
    let mut due_authority = Vec::new();
    let mut scheduled_supported_count = 0usize;
    let mut manual_retry_required_count = 0usize;
    let mut next_retry_after = None;
    let mut unsupported_missing = Vec::new();

    for (key, creds) in &s.hub_credentials {
        if !creds.can_connect() {
            continue;
        }

        connectable_credential_count += 1;

        let Some(hub_type) = creds.hub_type.as_ref() else {
            continue;
        };

        if let Some(integration) = find_integration(integrations, hub_type.as_str()) {
            let active = s.hubs.contains_key(key);
            let retrying_authority = active
                && s.external_controller_authority_pending.contains(key)
                && !s.external_controller_initial_sync_is_pending(key)
                && controller_authority_is_enabled(&s, key, integration);
            if active && !retrying_authority {
                continue;
            }

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

            let candidate = StoredHubBootstrapCandidate {
                key: key.clone(),
                integration,
            };
            if retrying_authority {
                due_authority.push(candidate);
            } else {
                due_supported.push(candidate);
            }
        } else {
            unsupported_missing.push((key.clone(), hub_type.as_str().to_string()));
        }
    }

    Ok(StoredHubBootstrapScan {
        connectable_credential_count,
        due_supported,
        due_authority,
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

fn log_authority_bootstrap_failure(
    hub_key: &HubKey,
    error: &str,
    outcome: &BootstrapFailureOutcome,
) {
    match outcome {
        BootstrapFailureOutcome::RetryScheduled { next_delay } => warn!(
            target: "sys",
            "External-controller authority reconciliation failed for {}: {} (retrying in {}s)",
            hub_key,
            error,
            next_delay.as_secs()
        ),
        BootstrapFailureOutcome::ManualRetryRequired => warn!(
            target: "sys",
            "External-controller authority reconciliation failed for {}: {} (automatic retry window exhausted; waiting for app retry)",
            hub_key,
            error
        ),
    }
}

fn log_sync_bootstrap_failure(hub_key: &HubKey, error: &str, outcome: &BootstrapFailureOutcome) {
    match outcome {
        BootstrapFailureOutcome::RetryScheduled { next_delay } => warn!(
            target: "sys",
            "Startup discovery sync failed for {}: {} (retrying full bootstrap in {}s)",
            hub_key,
            error,
            next_delay.as_secs()
        ),
        BootstrapFailureOutcome::ManualRetryRequired => warn!(
            target: "sys",
            "Startup discovery sync failed for {}: {} (automatic retry window exhausted; waiting for app retry)",
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

/// Fail closed after an integration-native grouped topology mutation becomes
/// uncertain and request the ordinary authority reconciliation loop.
///
/// The real bootstrap callback spawns a worker which may immediately lock
/// [`SharedState`], so it must be invoked only after the application-state
/// guard is released. Callers may still hold the external-topology transaction;
/// the worker will wait for that transaction before reconciling.
pub fn fence_required_group_authority_uncertainty(state: &SharedState, key: &HubKey) -> bool {
    let request_bootstrap = {
        let Ok(mut state) = state.lock() else {
            warn!(
                target: "sys",
                "Unable to fence uncertain grouped-controller authority: state lock poisoned"
            );
            return false;
        };
        if !state.topology.grouped_room_control_is_required(key) {
            return false;
        }
        state.clear_hub_startup_retry(key);
        state.mark_external_controller_authority_pending(key);
        state.request_hub_bootstrap_fn.clone()
    };

    crate::state::emit_server_event(
        state,
        crate::server_event::ServerEvent::HubStatus {
            hub_type: Some(key.hub_type.as_str().to_string()),
            address: Some(key.address.clone()),
            connected: false,
        },
    );
    if let Some(request_bootstrap) = request_bootstrap {
        request_bootstrap(state);
    }
    true
}

/// Fail closed after any external-controller authority operation becomes
/// uncertain, including integrations that route lights directly.
pub fn fence_external_controller_authority_uncertainty(state: &SharedState, key: &HubKey) -> bool {
    let request_bootstrap = {
        let Ok(mut state) = state.lock() else {
            warn!(
                target: "sys",
                "Unable to fence uncertain external-controller authority: state lock poisoned"
            );
            return false;
        };
        if !state.external_controller_authority_is_required(key) {
            return false;
        }
        state.clear_hub_startup_retry(key);
        state.mark_external_controller_authority_pending(key);
        state.request_hub_bootstrap_fn.clone()
    };

    crate::state::emit_server_event(
        state,
        crate::server_event::ServerEvent::HubStatus {
            hub_type: Some(key.hub_type.as_str().to_string()),
            address: Some(key.address.clone()),
            connected: false,
        },
    );
    if let Some(request_bootstrap) = request_bootstrap {
        request_bootstrap(state);
    }
    true
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
    /// Synchronously acknowledge topology mutations for integrations that
    /// require grouped room control before Rhythm commits them.
    pub sync_required_topology_groups_fn: Arc<dyn Fn(&SharedState) -> Result<()> + Send + Sync>,
    /// Acquire/resume external-controller authority for one connected hub.
    pub reconcile_external_controller_authority_fn:
        Arc<dyn Fn(&SharedState, &HubKey) -> Result<()> + Send + Sync>,
    /// Release external-controller authority before local teardown.
    pub release_external_controller_authority_fn: Arc<
        dyn Fn(&SharedState, &HubKey, ExternalControllerReleaseReason) -> Result<()> + Send + Sync,
    >,
    /// Finalize verified release state after durable local credential removal.
    pub finalize_external_controller_release_fn:
        Arc<dyn Fn(&SharedState, &HubKey) -> Result<()> + Send + Sync>,
    /// Give each integration a pre-commit device room assignment hook.
    pub prepare_hub_device_room_assignment_fn: Arc<
        dyn Fn(&SharedState, &HubDeviceRoomAssignment) -> Result<HubDeviceRoomAssignmentOutcome>
            + Send
            + Sync,
    >,
    /// Delete a source-owned native room through its owning integration.
    pub delete_source_room_fn:
        Arc<dyn Fn(&SharedState, &crate::topology::HubRoomBinding) -> Result<()> + Send + Sync>,
    /// Rename one integration-native device through its owning integration.
    pub rename_hub_device_fn:
        Arc<dyn Fn(&SharedState, &HubKey, &str, &str) -> Result<()> + Send + Sync>,
    /// Start a device pairing session (delegates to integration's `start_pairing`).
    pub start_pairing_fn: Arc<
        dyn Fn(
                &SharedState,
                &str,
                &serde_json::Value,
                crate::pairing::PairingRequestContext,
            ) -> Result<crate::pairing::PairingSession>
            + Send
            + Sync,
    >,
    /// Repair durable integration completion receipts before pairing status
    /// admission or lookup. `Some(hub_type)` targets one integration; `None`
    /// reconciles all registered integrations.
    pub reconcile_pairing_results_fn:
        Arc<dyn Fn(&SharedState, Option<&str>) -> Result<()> + Send + Sync>,
    /// Start a device unpairing session (delegates to integration's `start_unpairing`).
    pub start_unpairing_fn: Arc<
        dyn Fn(&SharedState, &str, &serde_json::Value) -> Result<crate::pairing::UnpairingResult>
            + Send
            + Sync,
    >,
    /// Load secret pairing recovery material through the owning integration.
    pub load_pairing_recovery_fn: Arc<
        dyn Fn(&SharedState, &str, &str) -> Result<Option<crate::pairing::PairingRecoverySecret>>
            + Send
            + Sync,
    >,
    /// Purge secret pairing recovery material through the owning integration.
    pub purge_pairing_recovery_fn:
        Arc<dyn Fn(&SharedState, &str, &str) -> Result<()> + Send + Sync>,
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
fn mark_external_controller_policies_for_active_hubs(
    state: &SharedState,
    integrations: &'static [&'static dyn ExternalLightHubIntegration],
) -> Result<()> {
    let policies = {
        let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        state
            .hubs
            .keys()
            .filter_map(|key| {
                find_integration(integrations, key.hub_type.as_str()).map(|integration| {
                    let grouped = grouped_room_control_is_enabled(&state, key, integration);
                    let authority = controller_authority_is_enabled(&state, key, integration);
                    (key.clone(), grouped, authority)
                })
            })
            .collect::<Vec<_>>()
    };

    let mut state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    for (key, grouped_required, authority_required) in policies {
        state
            .topology
            .set_grouped_room_control_required(&key, grouped_required);
        state.set_external_controller_authority_required(&key, authority_required);
    }
    Ok(())
}

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
        mark_external_controller_policies_for_active_hubs(state, integrations)?;
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
        let hub_type_str = key.hub_type.as_str();
        let integration = find_integration(integrations, hub_type_str)
            .ok_or_else(|| anyhow::anyhow!("No integration for hub type '{}'", hub_type_str))?;
        {
            let mut state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            let requires_grouped_room_control =
                grouped_room_control_is_enabled(&state, key, integration);
            let requires_controller_authority =
                controller_authority_is_enabled(&state, key, integration);
            state
                .topology
                .set_grouped_room_control_required(key, requires_grouped_room_control);
            state.set_external_controller_authority_required(key, requires_controller_authority);
            // Fence before the controller is inserted into composite routing.
            // Initial discovery and authority acquisition happen immediately
            // after registration, but no periodic/user command may slip into
            // the interval between those steps.
            if requires_controller_authority {
                state.mark_external_controller_initial_sync_pending(key);
            }
        }
        let composite = {
            let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            s.composite_controller.clone()
        };
        let Some(composite) = composite else {
            // No composite yet — runtime hasn't been created. Controller will be
            // picked up when ensure_composite_runtime runs on first room arrival.
            return Ok(());
        };
        let controller = integration.create_controller(state, key)?;
        let key_str = key.to_string();
        log::info!(target: "sys", "Dynamically registered {} controller with composite", key_str);
        composite.register_controller(&key_str, controller);
        Ok(())
    });

    let sync_topology_groups_fn = Arc::new(move |state: &SharedState| -> Result<()> {
        let transaction_lock = state
            .lock()
            .map_err(|_| anyhow::anyhow!("lock"))?
            .external_topology_transaction_lock
            .clone();
        let _transaction = transaction_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("External topology transaction lock poisoned"))?;
        if state
            .lock()
            .map_err(|_| anyhow::anyhow!("lock"))?
            .authority_state_recovery_required
        {
            anyhow::bail!("Authoritative topology state requires recovery");
        }
        let active_hubs = {
            let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            s.hubs
                .keys()
                .filter_map(|key| {
                    find_integration(integrations, key.hub_type.as_str()).map(|integration| {
                        (
                            key.clone(),
                            integration,
                            grouped_room_control_is_enabled(&s, key, integration),
                        )
                    })
                })
                .collect::<Vec<_>>()
        };

        for (key, integration, requires_grouped_room_control) in active_hubs {
            state
                .lock()
                .map_err(|_| anyhow::anyhow!("lock"))?
                .topology
                .set_grouped_room_control_required(&key, requires_grouped_room_control);
            if integration.requires_grouped_room_control() && !requires_grouped_room_control {
                continue;
            }
            if let Err(error) = integration.sync_topology_groups(state, &key) {
                if requires_grouped_room_control {
                    fence_required_group_authority_uncertainty(state, &key);
                }
                return Err(error);
            }
        }

        crate::commands::reconcile_room_binding_triage_best_effort(state);
        Ok(())
    });

    let sync_required_topology_groups_fn = Arc::new(move |state: &SharedState| -> Result<()> {
        if state
            .lock()
            .map_err(|_| anyhow::anyhow!("lock"))?
            .authority_state_recovery_required
        {
            anyhow::bail!("Authoritative topology state requires recovery");
        }
        let active_hubs = {
            let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            state
                .hubs
                .keys()
                .filter_map(|key| {
                    find_integration(integrations, key.hub_type.as_str())
                        .filter(|integration| {
                            grouped_room_control_is_enabled(&state, key, *integration)
                        })
                        .map(|integration| (key.clone(), integration))
                })
                .collect::<Vec<_>>()
        };

        for (key, integration) in active_hubs {
            state
                .lock()
                .map_err(|_| anyhow::anyhow!("lock"))?
                .topology
                .set_grouped_room_control_required(&key, true);
            if let Err(error) = integration.sync_topology_groups(state, &key) {
                fence_required_group_authority_uncertainty(state, &key);
                return Err(error);
            }
        }
        Ok(())
    });

    let reconcile_external_controller_authority_fn =
        Arc::new(move |state: &SharedState, key: &HubKey| -> Result<()> {
            let transaction_lock = state
                .lock()
                .map_err(|_| anyhow::anyhow!("lock"))?
                .external_topology_transaction_lock
                .clone();
            let _transaction = transaction_lock
                .lock()
                .map_err(|_| anyhow::anyhow!("External topology transaction lock poisoned"))?;
            if state
                .lock()
                .map_err(|_| anyhow::anyhow!("lock"))?
                .authority_state_recovery_required
            {
                anyhow::bail!("Authoritative topology state requires recovery");
            }
            let integration =
                find_integration(integrations, key.hub_type.as_str()).ok_or_else(|| {
                    anyhow::anyhow!("No integration for hub type '{}'", key.hub_type.as_str())
                })?;
            let requires_controller_authority = {
                let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
                controller_authority_is_enabled(&state, key, integration)
            };
            if integration.requires_external_controller_authority()
                && !requires_controller_authority
            {
                return Ok(());
            }
            if requires_controller_authority {
                state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("lock"))?
                    .mark_external_controller_authority_pending(key);
            }
            integration.reconcile_external_controller_authority(state, key)?;
            if requires_controller_authority {
                state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("lock"))?
                    .mark_external_controller_authority_ready(key);
            }
            Ok(())
        });

    let release_external_controller_authority_fn = Arc::new(
        move |state: &SharedState,
              key: &HubKey,
              reason: ExternalControllerReleaseReason|
              -> Result<()> {
            // The command lifecycle owns the external-topology transaction
            // across restore, credential commit, finalization, and local
            // teardown. Reacquiring the non-reentrant lock here would
            // deadlock every real release callback.
            let integration =
                find_integration(integrations, key.hub_type.as_str()).ok_or_else(|| {
                    anyhow::anyhow!("No integration for hub type '{}'", key.hub_type.as_str())
                })?;
            if integration.requires_external_controller_authority() {
                state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("lock"))?
                    .mark_external_controller_authority_pending(key);
            }
            integration.release_external_controller_authority(state, key, reason)
        },
    );

    let finalize_external_controller_release_fn =
        Arc::new(move |state: &SharedState, key: &HubKey| -> Result<()> {
            let integration =
                find_integration(integrations, key.hub_type.as_str()).ok_or_else(|| {
                    anyhow::anyhow!("No integration for hub type '{}'", key.hub_type.as_str())
                })?;
            integration.finalize_external_controller_release(state, key)
        });

    let prepare_hub_device_room_assignment_fn = Arc::new(
        move |state: &SharedState,
              assignment: &HubDeviceRoomAssignment|
              -> Result<HubDeviceRoomAssignmentOutcome> {
            if state
                .lock()
                .map_err(|_| anyhow::anyhow!("lock"))?
                .authority_state_recovery_required
            {
                anyhow::bail!("Authoritative topology state requires recovery");
            }
            let integration = find_integration(integrations, assignment.hub_key.hub_type.as_str())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "No integration for hub type '{}'",
                        assignment.hub_key.hub_type.as_str()
                    )
                })?;
            let requires_grouped_room_control = {
                let mut state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
                let required =
                    grouped_room_control_is_enabled(&state, &assignment.hub_key, integration);
                state
                    .topology
                    .set_grouped_room_control_required(&assignment.hub_key, required);
                required
            };
            if integration.requires_grouped_room_control() && !requires_grouped_room_control {
                return Ok(HubDeviceRoomAssignmentOutcome::Unchanged);
            }
            integration.prepare_device_room_assignment(state, assignment)
        },
    );

    let delete_source_room_fn = Arc::new(
        move |state: &SharedState, binding: &crate::topology::HubRoomBinding| -> Result<()> {
            let integration = find_integration(integrations, binding.hub_key.hub_type.as_str())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "No integration for hub type '{}'",
                        binding.hub_key.hub_type.as_str()
                    )
                })?;
            integration.delete_source_room(state, binding)
        },
    );

    let rename_hub_device_fn = Arc::new(
        move |state: &SharedState,
              hub_key: &HubKey,
              native_device_id: &str,
              name: &str|
              -> Result<()> {
            let integration = find_integration(integrations, hub_key.hub_type.as_str())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "No integration for hub type '{}'",
                        hub_key.hub_type.as_str()
                    )
                })?;
            integration.rename_device(state, hub_key, native_device_id, name)
        },
    );

    let start_pairing_fn = Arc::new(
        move |state: &SharedState,
              hub_type: &str,
              params: &serde_json::Value,
              context: crate::pairing::PairingRequestContext|
              -> Result<crate::pairing::PairingSession> {
            let integration = find_integration(integrations, hub_type)
                .ok_or_else(|| anyhow::anyhow!("No integration for hub type '{}'", hub_type))?;
            integration.start_pairing_with_context(state, params, context)
        },
    );

    let reconcile_pairing_results_fn = Arc::new(
        move |state: &SharedState, hub_type: Option<&str>| -> Result<()> {
            if let Some(hub_type) = hub_type {
                let integration = find_integration(integrations, hub_type)
                    .ok_or_else(|| anyhow::anyhow!("No integration for hub type '{}'", hub_type))?;
                return integration.reconcile_pairing_results(state);
            }
            for integration in integrations {
                integration.reconcile_pairing_results(state)?;
            }
            Ok(())
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

    let load_pairing_recovery_fn = Arc::new(
        move |state: &SharedState,
              hub_type: &str,
              native_device_id: &str|
              -> Result<Option<crate::pairing::PairingRecoverySecret>> {
            let integration = find_integration(integrations, hub_type)
                .ok_or_else(|| anyhow::anyhow!("No integration for hub type '{}'", hub_type))?;
            integration.load_pairing_recovery(state, native_device_id)
        },
    );

    let purge_pairing_recovery_fn = Arc::new(
        move |state: &SharedState, hub_type: &str, native_device_id: &str| -> Result<()> {
            let integration = find_integration(integrations, hub_type)
                .ok_or_else(|| anyhow::anyhow!("No integration for hub type '{}'", hub_type))?;
            integration.purge_pairing_recovery(state, native_device_id)
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
        sync_required_topology_groups_fn,
        reconcile_external_controller_authority_fn,
        release_external_controller_authority_fn,
        finalize_external_controller_release_fn,
        prepare_hub_device_room_assignment_fn,
        delete_source_room_fn,
        rename_hub_device_fn,
        start_pairing_fn,
        reconcile_pairing_results_fn,
        start_unpairing_fn,
        load_pairing_recovery_fn,
        purge_pairing_recovery_fn,
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

    #[test]
    fn phone_provisioning_metadata_defaults_off_for_previous_profiles() {
        let legacy = serde_json::json!({
            "id": "vendor.strip.light.v1", "device_type": "light",
            "display_name": "Strip", "input_only": false,
            "onboarding_methods": ["ble_wifi_nearby_scan"]
        });
        let mut profile: HubDeviceProfileCapability = serde_json::from_value(legacy).unwrap();
        assert!(profile.phone_provisioning_protocol.is_none());
        assert!(serde_json::to_value(&profile)
            .unwrap()
            .get("phone_provisioning_protocol")
            .is_none());
        profile.phone_provisioning_protocol = Some("ayla_v1".into());
        let current = serde_json::to_value(&profile).unwrap();
        assert_eq!(current["phone_provisioning_protocol"], "ayla_v1");
        assert_eq!(
            serde_json::from_value::<HubDeviceProfileCapability>(current).unwrap(),
            profile
        );
    }

    #[test]
    fn device_profile_ids_have_one_canonical_versioned_grammar() {
        let parsed = ParsedDeviceProfileId::parse("orein.oc02001.button.v12").unwrap();
        assert_eq!(parsed.family(), "orein.oc02001.button");
        assert_eq!(parsed.version(), 12);

        for invalid in [
            "",
            "button.v1",
            "orein..button.v1",
            "Orein.oc02001.button.v1",
            "orein.oc02001.button.V1",
            "orein.oc02001.button.v0",
            "orein.oc02001.button.v01",
            "orein.oc02001.button.latest",
            "orein.oc02001.button.v4294967296",
        ] {
            assert!(
                ParsedDeviceProfileId::parse(invalid).is_none(),
                "accepted {invalid}"
            );
        }
        assert!(!is_valid_device_profile_id(
            &"a".repeat(DEVICE_PROFILE_ID_MAX_LEN + 1)
        ));
    }

    #[test]
    fn profile_capability_resolves_only_valid_explicit_older_lineage() {
        let profile = HubDeviceProfileCapability {
            id: "future.vendor.bulb.v2".to_string(),
            compatible_profile_ids: vec![
                "future.vendor.bulb.v1".to_string(),
                "future.vendor.bulb.v3".to_string(),
                "other.vendor.bulb.v1".to_string(),
            ],
            device_type: "light".to_string(),
            display_name: "BLE bulb".to_string(),
            input_only: false,
            onboarding_methods: vec!["local_ble_qr".to_string()],
            nearby_service_uuids: Vec::new(),
            cloud_broker: None,
            phone_provisioning_protocol: None,
        };
        assert_eq!(
            profile.canonical_id_for("future.vendor.bulb.v2"),
            Some("future.vendor.bulb.v2")
        );
        assert_eq!(
            profile.canonical_id_for("future.vendor.bulb.v1"),
            Some("future.vendor.bulb.v2")
        );
        assert_eq!(profile.canonical_id_for("future.vendor.bulb.v3"), None);
        assert_eq!(profile.canonical_id_for("other.vendor.bulb.v1"), None);
    }

    // ── Mock integration for testing ──────────────────────────────────

    /// Tracks how many times ensure_runtime was called.
    struct MockIntegration {
        hub_type: &'static str,
        ensure_count: AtomicU32,
        post_connect_count: AtomicU32,
        sync_count: AtomicU32,
        authority_count: AtomicU32,
        release_count: AtomicU32,
        prepare_count: AtomicU32,
        requires_grouped_room_control: bool,
        requires_external_controller_authority: bool,
        topology_sync_fails: bool,
    }

    impl MockIntegration {
        const fn new(hub_type: &'static str) -> Self {
            Self {
                hub_type,
                ensure_count: AtomicU32::new(0),
                post_connect_count: AtomicU32::new(0),
                sync_count: AtomicU32::new(0),
                authority_count: AtomicU32::new(0),
                release_count: AtomicU32::new(0),
                prepare_count: AtomicU32::new(0),
                requires_grouped_room_control: false,
                requires_external_controller_authority: false,
                topology_sync_fails: false,
            }
        }

        const fn new_group_required(hub_type: &'static str) -> Self {
            Self {
                hub_type,
                ensure_count: AtomicU32::new(0),
                post_connect_count: AtomicU32::new(0),
                sync_count: AtomicU32::new(0),
                authority_count: AtomicU32::new(0),
                release_count: AtomicU32::new(0),
                prepare_count: AtomicU32::new(0),
                requires_grouped_room_control: true,
                requires_external_controller_authority: true,
                topology_sync_fails: false,
            }
        }

        const fn new_authority_only(hub_type: &'static str) -> Self {
            Self {
                hub_type,
                ensure_count: AtomicU32::new(0),
                post_connect_count: AtomicU32::new(0),
                sync_count: AtomicU32::new(0),
                authority_count: AtomicU32::new(0),
                release_count: AtomicU32::new(0),
                prepare_count: AtomicU32::new(0),
                requires_grouped_room_control: false,
                requires_external_controller_authority: true,
                topology_sync_fails: false,
            }
        }

        const fn new_group_required_with_sync_failure(hub_type: &'static str) -> Self {
            Self {
                hub_type,
                ensure_count: AtomicU32::new(0),
                post_connect_count: AtomicU32::new(0),
                sync_count: AtomicU32::new(0),
                authority_count: AtomicU32::new(0),
                release_count: AtomicU32::new(0),
                prepare_count: AtomicU32::new(0),
                requires_grouped_room_control: true,
                requires_external_controller_authority: true,
                topology_sync_fails: true,
            }
        }

        #[allow(dead_code)]
        fn ensure_calls(&self) -> u32 {
            self.ensure_count.load(Ordering::Relaxed)
        }

        fn post_connect_calls(&self) -> u32 {
            self.post_connect_count.load(Ordering::Relaxed)
        }

        fn sync_calls(&self) -> u32 {
            self.sync_count.load(Ordering::Relaxed)
        }

        fn authority_calls(&self) -> u32 {
            self.authority_count.load(Ordering::Relaxed)
        }

        fn release_calls(&self) -> u32 {
            self.release_count.load(Ordering::Relaxed)
        }

        fn prepare_calls(&self) -> u32 {
            self.prepare_count.load(Ordering::Relaxed)
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

        fn sync_topology_groups(&self, _state: &SharedState, _key: &HubKey) -> Result<()> {
            self.sync_count.fetch_add(1, Ordering::Relaxed);
            if self.topology_sync_fails {
                anyhow::bail!("simulated grouped topology uncertainty");
            }
            Ok(())
        }

        fn requires_grouped_room_control(&self) -> bool {
            self.requires_grouped_room_control
        }

        fn requires_external_controller_authority(&self) -> bool {
            self.requires_external_controller_authority
        }

        fn reconcile_external_controller_authority(
            &self,
            _state: &SharedState,
            _key: &HubKey,
        ) -> Result<()> {
            self.authority_count.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn release_external_controller_authority(
            &self,
            _state: &SharedState,
            _key: &HubKey,
            _reason: ExternalControllerReleaseReason,
        ) -> Result<()> {
            self.release_count.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn prepare_device_room_assignment(
            &self,
            _state: &SharedState,
            _assignment: &HubDeviceRoomAssignment,
        ) -> Result<HubDeviceRoomAssignmentOutcome> {
            self.prepare_count.fetch_add(1, Ordering::Relaxed);
            Ok(HubDeviceRoomAssignmentOutcome::Unchanged)
        }
    }

    static MOCK_HUE: MockIntegration = MockIntegration::new("hue");
    static MOCK_HA: MockIntegration = MockIntegration::new("homeassistant");
    static MOCK_GROUP_REQUIRED: MockIntegration =
        MockIntegration::new_group_required("group_required");
    static MOCK_AUTHORITY_ONLY: MockIntegration =
        MockIntegration::new_authority_only("authority_only");
    static MOCK_ROLLOUT_GATE: MockIntegration = MockIntegration::new_group_required("rollout_gate");
    static MOCK_GROUP_REQUIRED_SYNC_FAILURE: MockIntegration =
        MockIntegration::new_group_required_with_sync_failure("group_required_sync_failure");
    static FAILING_GROUP_INTEGRATIONS: &[&dyn ExternalLightHubIntegration] =
        &[&MOCK_GROUP_REQUIRED_SYNC_FAILURE];

    static TEST_INTEGRATIONS: &[&dyn ExternalLightHubIntegration] = &[&MOCK_HUE, &MOCK_HA];
    static GROUP_POLICY_TEST_INTEGRATIONS: &[&dyn ExternalLightHubIntegration] =
        &[&MOCK_GROUP_REQUIRED, &MOCK_HA];
    static AUTHORITY_ONLY_TEST_INTEGRATIONS: &[&dyn ExternalLightHubIntegration] =
        &[&MOCK_AUTHORITY_ONLY];
    static ROLLOUT_GATE_TEST_INTEGRATIONS: &[&dyn ExternalLightHubIntegration] =
        &[&MOCK_ROLLOUT_GATE];

    fn string_error<T>(result: Result<T>) -> String {
        match result {
            Ok(_) => panic!("expected error"),
            Err(error) => error.to_string(),
        }
    }

    fn enable_authority_rollout(state: &SharedState, hub_type: &str) {
        state
            .lock()
            .unwrap()
            .set_external_controller_authority_enabled(HubType::new(hub_type), true);
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

    struct CountingBootstrapDiscovery {
        room_discovery_calls: Arc<AtomicU32>,
        failures_before_success: Arc<AtomicU32>,
        device_failures_before_success: Arc<AtomicU32>,
        identity_failures_before_success: Arc<AtomicU32>,
    }

    impl HubDiscovery for CountingBootstrapDiscovery {
        fn discover_rooms(&self) -> Result<Vec<DiscoveredRoom>> {
            self.room_discovery_calls.fetch_add(1, Ordering::Relaxed);
            if self.failures_before_success.load(Ordering::Relaxed) > 0 {
                self.failures_before_success.fetch_sub(1, Ordering::Relaxed);
                anyhow::bail!("temporary startup sync failure");
            }
            Ok(Vec::new())
        }

        fn discover_devices(&self) -> Result<Vec<DiscoveredDevice>> {
            if self.device_failures_before_success.load(Ordering::Relaxed) > 0 {
                self.device_failures_before_success
                    .fetch_sub(1, Ordering::Relaxed);
                anyhow::bail!("temporary device discovery failure");
            }
            Ok(Vec::new())
        }

        fn discover_identities(
            &self,
        ) -> Result<Vec<crate::canonical::identity::DiscoveredIdentity>> {
            if self
                .identity_failures_before_success
                .load(Ordering::Relaxed)
                > 0
            {
                self.identity_failures_before_success
                    .fetch_sub(1, Ordering::Relaxed);
                anyhow::bail!("temporary identity discovery failure");
            }
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
        authority_failures_before_success: AtomicU32,
        connect_attempts: AtomicU32,
        authority_attempts: AtomicU32,
        post_connect_count: AtomicU32,
        room_discovery_calls: Arc<AtomicU32>,
        room_discovery_failures_before_success: Arc<AtomicU32>,
        device_discovery_failures_before_success: Arc<AtomicU32>,
        identity_discovery_failures_before_success: Arc<AtomicU32>,
        requires_grouped_room_control: bool,
    }

    impl BootstrapTestIntegration {
        fn new(failures_before_success: u32) -> Self {
            Self {
                failures_before_success: AtomicU32::new(failures_before_success),
                authority_failures_before_success: AtomicU32::new(0),
                connect_attempts: AtomicU32::new(0),
                authority_attempts: AtomicU32::new(0),
                post_connect_count: AtomicU32::new(0),
                room_discovery_calls: Arc::new(AtomicU32::new(0)),
                room_discovery_failures_before_success: Arc::new(AtomicU32::new(0)),
                device_discovery_failures_before_success: Arc::new(AtomicU32::new(0)),
                identity_discovery_failures_before_success: Arc::new(AtomicU32::new(0)),
                requires_grouped_room_control: false,
            }
        }

        fn with_authority_failures(authority_failures_before_success: u32) -> Self {
            Self {
                failures_before_success: AtomicU32::new(0),
                authority_failures_before_success: AtomicU32::new(
                    authority_failures_before_success,
                ),
                connect_attempts: AtomicU32::new(0),
                authority_attempts: AtomicU32::new(0),
                post_connect_count: AtomicU32::new(0),
                room_discovery_calls: Arc::new(AtomicU32::new(0)),
                room_discovery_failures_before_success: Arc::new(AtomicU32::new(0)),
                device_discovery_failures_before_success: Arc::new(AtomicU32::new(0)),
                identity_discovery_failures_before_success: Arc::new(AtomicU32::new(0)),
                requires_grouped_room_control: true,
            }
        }

        fn with_sync_failures(room_discovery_failures_before_success: u32) -> Self {
            Self {
                failures_before_success: AtomicU32::new(0),
                authority_failures_before_success: AtomicU32::new(0),
                connect_attempts: AtomicU32::new(0),
                authority_attempts: AtomicU32::new(0),
                post_connect_count: AtomicU32::new(0),
                room_discovery_calls: Arc::new(AtomicU32::new(0)),
                room_discovery_failures_before_success: Arc::new(AtomicU32::new(
                    room_discovery_failures_before_success,
                )),
                device_discovery_failures_before_success: Arc::new(AtomicU32::new(0)),
                identity_discovery_failures_before_success: Arc::new(AtomicU32::new(0)),
                requires_grouped_room_control: true,
            }
        }

        fn with_device_sync_failures(device_discovery_failures_before_success: u32) -> Self {
            let mut integration = Self::with_sync_failures(0);
            integration.device_discovery_failures_before_success =
                Arc::new(AtomicU32::new(device_discovery_failures_before_success));
            integration
        }

        fn with_identity_sync_failures(identity_discovery_failures_before_success: u32) -> Self {
            let mut integration = Self::with_sync_failures(0);
            integration.identity_discovery_failures_before_success =
                Arc::new(AtomicU32::new(identity_discovery_failures_before_success));
            integration
        }

        fn connect_attempts(&self) -> u32 {
            self.connect_attempts.load(Ordering::Relaxed)
        }

        fn authority_attempts(&self) -> u32 {
            self.authority_attempts.load(Ordering::Relaxed)
        }

        fn room_discovery_calls(&self) -> u32 {
            self.room_discovery_calls.load(Ordering::Relaxed)
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
                    discovery: Some(Arc::new(CountingBootstrapDiscovery {
                        room_discovery_calls: self.room_discovery_calls.clone(),
                        failures_before_success: self
                            .room_discovery_failures_before_success
                            .clone(),
                        device_failures_before_success: self
                            .device_discovery_failures_before_success
                            .clone(),
                        identity_failures_before_success: self
                            .identity_discovery_failures_before_success
                            .clone(),
                    })),
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

        fn requires_grouped_room_control(&self) -> bool {
            self.requires_grouped_room_control
        }

        fn reconcile_external_controller_authority(
            &self,
            _state: &SharedState,
            _key: &HubKey,
        ) -> Result<()> {
            self.authority_attempts.fetch_add(1, Ordering::Relaxed);
            if self
                .authority_failures_before_success
                .load(Ordering::Relaxed)
                > 0
            {
                self.authority_failures_before_success
                    .fetch_sub(1, Ordering::Relaxed);
                anyhow::bail!("temporary authority failure");
            }
            Ok(())
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
    fn external_controller_authority_rollout_defaults_off_and_is_per_hub_type() {
        let mut state = crate::state::AppState::default();
        let hue = HubType::new("hue");
        let other = HubType::new("group_required");

        assert!(!state.external_controller_authority_is_enabled(&hue));
        assert!(!state.external_controller_authority_is_enabled(&other));

        state.set_external_controller_authority_enabled(hue.clone(), true);
        assert!(state.external_controller_authority_is_enabled(&hue));
        assert!(!state.external_controller_authority_is_enabled(&other));

        state.set_external_controller_authority_enabled(hue.clone(), false);
        assert!(!state.external_controller_authority_is_enabled(&hue));
    }

    #[test]
    fn disabled_authority_skips_acquisition_but_keeps_release_and_recovery_fence() {
        let callbacks = integration_callbacks(ROLLOUT_GATE_TEST_INTEGRATIONS);
        let state: SharedState = Arc::new(Mutex::new(crate::state::AppState::default()));
        let key = HubKey::new(HubType::new("rollout_gate"), "bridge");
        let authority_before = MOCK_ROLLOUT_GATE.authority_calls();
        let release_before = MOCK_ROLLOUT_GATE.release_calls();
        let prepare_before = MOCK_ROLLOUT_GATE.prepare_calls();
        let sync_before = MOCK_ROLLOUT_GATE.sync_calls();

        {
            let mut app = state.lock().unwrap();
            app.hubs.insert(
                key.clone(),
                ActiveHub {
                    hub_type: key.hub_type.clone(),
                    hub_key: key.clone(),
                    runtime: None,
                    hub_data: Box::new(()),
                    registry: None,
                    discovery: None,
                    shutdown: Arc::new(AtomicBool::new(false)),
                },
            );
            // Simulate recovery evidence left by an earlier enabled build.
            app.mark_external_controller_authority_pending(&key);
        }

        (callbacks.reconcile_external_controller_authority_fn)(&state, &key).unwrap();
        (callbacks.sync_required_topology_groups_fn)(&state).unwrap();
        let assignment = HubDeviceRoomAssignment {
            hub_key: key.clone(),
            native_device_id: "light".to_string(),
            device_type: DeviceType::Light,
            preferred_for_control: true,
            target_rhythm_room_id: Some("room".to_string()),
            target_hub_room_ids: Vec::new(),
        };
        assert!(matches!(
            (callbacks.prepare_hub_device_room_assignment_fn)(&state, &assignment).unwrap(),
            HubDeviceRoomAssignmentOutcome::Unchanged
        ));

        assert_eq!(MOCK_ROLLOUT_GATE.authority_calls(), authority_before);
        assert_eq!(MOCK_ROLLOUT_GATE.prepare_calls(), prepare_before);
        assert_eq!(MOCK_ROLLOUT_GATE.sync_calls(), sync_before);
        assert!(state
            .lock()
            .unwrap()
            .external_controller_authority_pending
            .contains(&key));

        (callbacks.release_external_controller_authority_fn)(
            &state,
            &key,
            ExternalControllerReleaseReason::UserDisconnect,
        )
        .unwrap();
        assert_eq!(MOCK_ROLLOUT_GATE.release_calls(), release_before + 1);
        assert!(state
            .lock()
            .unwrap()
            .external_controller_authority_pending
            .contains(&key));
    }

    #[test]
    #[should_panic(expected = "No integration registered for hub type: unknown")]
    fn provider_callback_panics_for_unknown_hub_type() {
        let callbacks = integration_callbacks(TEST_INTEGRATIONS);

        let _ = (callbacks.get_hub_provider_fn)(HubType::new("unknown"));
    }

    #[test]
    fn corrupt_authority_snapshot_fences_external_topology_callbacks() {
        let callbacks = integration_callbacks(GROUP_POLICY_TEST_INTEGRATIONS);
        let state: SharedState = Arc::new(Mutex::new(crate::state::AppState::default()));
        state.lock().unwrap().authority_state_recovery_required = true;
        let key = HubKey::new(HubType::new("group_required"), "bridge");

        assert!(
            string_error((callbacks.reconcile_external_controller_authority_fn)(
                &state, &key
            ))
            .contains("requires recovery")
        );
        assert!(
            string_error((callbacks.sync_required_topology_groups_fn)(&state))
                .contains("requires recovery")
        );
        assert!(
            string_error((callbacks.prepare_hub_device_room_assignment_fn)(
                &state,
                &HubDeviceRoomAssignment {
                    hub_key: key,
                    native_device_id: "native-light".to_string(),
                    device_type: DeviceType::Light,
                    preferred_for_control: true,
                    target_rhythm_room_id: Some("room".to_string()),
                    target_hub_room_ids: Vec::new(),
                },
            ))
            .contains("requires recovery")
        );
    }

    #[test]
    fn required_group_sync_targets_only_declaring_integrations_and_installs_policy() {
        let callbacks = integration_callbacks(GROUP_POLICY_TEST_INTEGRATIONS);
        let state: SharedState = Arc::new(Mutex::new(crate::state::AppState::default()));
        let required_key = HubKey::new(HubType::new("group_required"), "bridge");
        let ordinary_key = HubKey::new(HubType::new("homeassistant"), "server");
        enable_authority_rollout(&state, "group_required");
        {
            let mut state = state.lock().unwrap();
            for key in [&required_key, &ordinary_key] {
                state.hubs.insert(
                    key.clone(),
                    ActiveHub {
                        hub_type: key.hub_type.clone(),
                        hub_key: key.clone(),
                        runtime: None,
                        hub_data: Box::new(()),
                        registry: None,
                        discovery: None,
                        shutdown: Arc::new(AtomicBool::new(false)),
                    },
                );
            }
        }
        let required_before = MOCK_GROUP_REQUIRED.sync_calls();
        let ordinary_before = MOCK_HA.sync_calls();

        (callbacks.sync_required_topology_groups_fn)(&state).unwrap();

        assert_eq!(MOCK_GROUP_REQUIRED.sync_calls(), required_before + 1);
        assert_eq!(MOCK_HA.sync_calls(), ordinary_before);
        let state = state.lock().unwrap();
        assert!(state
            .topology
            .grouped_room_control_is_required(&required_key));
        assert!(!state
            .topology
            .grouped_room_control_is_required(&ordinary_key));
    }

    #[test]
    fn required_group_sync_failure_fences_exact_hub_and_requests_recovery_unlocked() {
        let callbacks = integration_callbacks(FAILING_GROUP_INTEGRATIONS);
        let state: SharedState = Arc::new(Mutex::new(crate::state::AppState::default()));
        let key = HubKey::new(HubType::new("group_required_sync_failure"), "bridge");
        enable_authority_rollout(&state, "group_required_sync_failure");
        let recovery_requests = Arc::new(AtomicU32::new(0));
        {
            let recovery_requests = recovery_requests.clone();
            let mut app = state.lock().unwrap();
            app.hubs.insert(
                key.clone(),
                ActiveHub {
                    hub_type: key.hub_type.clone(),
                    hub_key: key.clone(),
                    runtime: None,
                    hub_data: Box::new(()),
                    registry: None,
                    discovery: None,
                    shutdown: Arc::new(AtomicBool::new(false)),
                },
            );
            app.set_hub_connected(&key, true);
            app.request_hub_bootstrap_fn = Some(Arc::new(move |state| {
                assert!(
                    state.try_lock().is_ok(),
                    "recovery callback ran while AppState remained locked"
                );
                recovery_requests.fetch_add(1, Ordering::SeqCst);
            }));
        }

        let error = (callbacks.sync_topology_groups_fn)(&state).unwrap_err();

        assert!(error
            .to_string()
            .contains("simulated grouped topology uncertainty"));
        assert_eq!(recovery_requests.load(Ordering::SeqCst), 1);
        let app = state.lock().unwrap();
        assert!(app.external_controller_authority_pending.contains(&key));
        assert!(!app.external_controller_authority_is_ready(&key));
        assert!(!app.hub_is_connected(&key));
    }

    #[test]
    fn required_group_registration_installs_policy_before_composite_exists() {
        let callbacks = integration_callbacks(GROUP_POLICY_TEST_INTEGRATIONS);
        let state: SharedState = Arc::new(Mutex::new(crate::state::AppState::default()));
        let key = HubKey::new(HubType::new("group_required"), "fresh-bridge");
        enable_authority_rollout(&state, "group_required");

        assert!((callbacks.register_controller_fn)(&state, &key).is_ok());
        let state = state.lock().unwrap();
        assert!(state.composite_controller.is_none());
        assert!(state.topology.grouped_room_control_is_required(&key));
        assert!(state.external_controller_initial_sync_is_pending(&key));
        assert!(!state.external_controller_authority_is_ready(&key));
    }

    #[test]
    fn authority_only_registration_fences_without_requiring_group_routing() {
        let callbacks = integration_callbacks(AUTHORITY_ONLY_TEST_INTEGRATIONS);
        let state: SharedState = Arc::new(Mutex::new(crate::state::AppState::default()));
        let key = HubKey::new(HubType::new("authority_only"), "fresh-bridge");
        enable_authority_rollout(&state, "authority_only");

        assert!((callbacks.register_controller_fn)(&state, &key).is_ok());
        let state = state.lock().unwrap();
        assert!(state.external_controller_authority_is_required(&key));
        assert!(!state.topology.grouped_room_control_is_required(&key));
        assert!(state.external_controller_initial_sync_is_pending(&key));
        assert!(!state.external_controller_authority_is_ready(&key));
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
            &serde_json::json!({}),
            crate::pairing::PairingRequestContext::accepted_now(),
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
    fn stored_hub_bootstrap_retries_authority_on_existing_active_hub() {
        let integration = BootstrapTestIntegration::with_authority_failures(1);
        let integrations: &[&dyn ExternalLightHubIntegration] = &[&integration];
        let state: SharedState = Arc::new(Mutex::new(crate::state::AppState::default()));
        let key = HubKey::new(HubType::new("hue"), "192.168.1.5");
        enable_authority_rollout(&state, "hue");

        state.lock().unwrap().hub_credentials.insert(
            key.clone(),
            HubCredentials::new("hue", "192.168.1.5", serde_json::json!({"username": "abc"})),
        );

        let mut clock = MockBootstrapClock::new();
        bootstrap_stored_hubs_until_settled(&state, integrations, &mut clock);

        let state = state.lock().unwrap();
        assert!(state.hubs.contains_key(&key));
        assert!(state.external_controller_authority_is_ready(&key));
        assert!(state.hub_is_connected(&key));
        assert!(state.hub_startup_retry(&key).is_none());
        assert_eq!(integration.connect_attempts(), 1);
        assert_eq!(integration.room_discovery_calls(), 1);
        assert_eq!(integration.authority_attempts(), 2);
        assert_eq!(integration.post_connect_calls(), 1);
        assert_eq!(clock.sleeps, vec![Duration::from_secs(1); 5]);
    }

    #[test]
    fn failed_active_hub_authority_retry_retains_backoff_without_reconnect() {
        let integration = BootstrapTestIntegration::with_authority_failures(2);
        let integrations: &[&dyn ExternalLightHubIntegration] = &[&integration];
        let state: SharedState = Arc::new(Mutex::new(crate::state::AppState::default()));
        let key = HubKey::new(HubType::new("hue"), "192.168.1.5");
        enable_authority_rollout(&state, "hue");
        state.lock().unwrap().hub_credentials.insert(
            key.clone(),
            HubCredentials::new("hue", "192.168.1.5", serde_json::json!({"username": "abc"})),
        );

        let mut clock = MockBootstrapClock::new();
        assert!(matches!(
            bootstrap_stored_hubs_once(&state, integrations, &mut clock),
            BootstrapLoopDecision::Sleep(duration) if duration == Duration::from_secs(1)
        ));
        clock.sleep(Duration::from_secs(5));
        assert!(matches!(
            bootstrap_stored_hubs_once(&state, integrations, &mut clock),
            BootstrapLoopDecision::Sleep(duration) if duration == Duration::from_secs(1)
        ));

        let state = state.lock().unwrap();
        let retry = state.hub_startup_retry(&key).unwrap();
        assert_eq!(retry.attempt_count, 2);
        assert_eq!(
            retry.next_retry_at,
            Some(clock.now_instant() + Duration::from_secs(10))
        );
        assert!(state.external_controller_authority_pending.contains(&key));
        assert!(!state.hub_is_connected(&key));
        assert_eq!(integration.connect_attempts(), 1);
        assert_eq!(integration.room_discovery_calls(), 1);
        assert_eq!(integration.authority_attempts(), 2);
        assert_eq!(integration.post_connect_calls(), 0);
    }

    #[test]
    fn startup_sync_failure_retries_full_bootstrap_before_authority() {
        let integration = BootstrapTestIntegration::with_sync_failures(2);
        let integrations: &[&dyn ExternalLightHubIntegration] = &[&integration];
        let state: SharedState = Arc::new(Mutex::new(crate::state::AppState::default()));
        let key = HubKey::new(HubType::new("hue"), "192.168.1.5");
        enable_authority_rollout(&state, "hue");
        state.lock().unwrap().hub_credentials.insert(
            key.clone(),
            HubCredentials::new("hue", "192.168.1.5", serde_json::json!({"username": "abc"})),
        );

        let mut clock = MockBootstrapClock::new();
        bootstrap_stored_hubs_until_settled(&state, integrations, &mut clock);

        let state = state.lock().unwrap();
        assert!(state.hubs.contains_key(&key));
        assert!(state.external_controller_authority_is_ready(&key));
        assert_eq!(integration.connect_attempts(), 3);
        assert_eq!(integration.room_discovery_calls(), 3);
        assert_eq!(integration.authority_attempts(), 1);
        assert_eq!(integration.post_connect_calls(), 1);
        assert_eq!(clock.sleeps, vec![Duration::from_secs(1); 15]);
    }

    #[test]
    fn initial_device_discovery_failure_performs_no_authority_writes() {
        let integration = BootstrapTestIntegration::with_device_sync_failures(1);
        let integrations: &[&dyn ExternalLightHubIntegration] = &[&integration];
        let state: SharedState = Arc::new(Mutex::new(crate::state::AppState::default()));
        let key = HubKey::new(HubType::new("hue"), "192.168.1.5");
        enable_authority_rollout(&state, "hue");
        state.lock().unwrap().hub_credentials.insert(
            key.clone(),
            HubCredentials::new("hue", "192.168.1.5", serde_json::json!({"username": "abc"})),
        );

        let mut clock = MockBootstrapClock::new();
        let _ = bootstrap_stored_hubs_once(&state, integrations, &mut clock);

        let state = state.lock().unwrap();
        assert_eq!(integration.connect_attempts(), 1);
        assert_eq!(integration.authority_attempts(), 0);
        assert!(!state.hubs.contains_key(&key));
        assert!(!state.hub_is_connected(&key));
    }

    #[test]
    fn initial_identity_discovery_failure_performs_no_authority_writes() {
        let integration = BootstrapTestIntegration::with_identity_sync_failures(1);
        let integrations: &[&dyn ExternalLightHubIntegration] = &[&integration];
        let state: SharedState = Arc::new(Mutex::new(crate::state::AppState::default()));
        let key = HubKey::new(HubType::new("hue"), "192.168.1.5");
        enable_authority_rollout(&state, "hue");
        state.lock().unwrap().hub_credentials.insert(
            key.clone(),
            HubCredentials::new("hue", "192.168.1.5", serde_json::json!({"username": "abc"})),
        );

        let mut clock = MockBootstrapClock::new();
        let _ = bootstrap_stored_hubs_once(&state, integrations, &mut clock);

        let state = state.lock().unwrap();
        assert_eq!(integration.connect_attempts(), 1);
        assert_eq!(integration.authority_attempts(), 0);
        assert!(!state.hubs.contains_key(&key));
        assert!(!state.hub_is_connected(&key));
    }

    #[test]
    fn stored_hub_bootstrap_continues_when_retry_is_already_due_after_sync() {
        let integration = BootstrapTestIntegration::new(0);
        let candidate = StoredHubBootstrapCandidate {
            key: HubKey::new(HubType::new("hue"), "192.168.1.5"),
            integration: &integration,
        };
        let scan = StoredHubBootstrapScan {
            connectable_credential_count: 1,
            due_supported: vec![candidate],
            due_authority: Vec::new(),
            scheduled_supported_count: 0,
            manual_retry_required_count: 0,
            next_retry_after: None,
            unsupported_missing: Vec::new(),
        };

        assert!(matches!(
            bootstrap_loop_decision_after_scan(&scan),
            BootstrapLoopDecision::Sleep(duration) if duration.is_zero()
        ));
    }

    #[test]
    fn stored_hub_bootstrap_continues_when_authority_retry_becomes_due_during_work() {
        let integration = BootstrapTestIntegration::with_authority_failures(0);
        let candidate = StoredHubBootstrapCandidate {
            key: HubKey::new(HubType::new("hue"), "192.168.1.5"),
            integration: &integration,
        };
        let scan = StoredHubBootstrapScan {
            connectable_credential_count: 1,
            due_supported: Vec::new(),
            due_authority: vec![candidate],
            scheduled_supported_count: 0,
            manual_retry_required_count: 0,
            next_retry_after: None,
            unsupported_missing: Vec::new(),
        };

        assert!(matches!(
            bootstrap_loop_decision_after_scan(&scan),
            BootstrapLoopDecision::Sleep(duration) if duration.is_zero()
        ));
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
