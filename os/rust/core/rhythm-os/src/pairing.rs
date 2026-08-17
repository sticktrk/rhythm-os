//! Device pairing types for direct-connection protocols.
//!
//! Direct protocols use this for commissioning, while hub integrations may
//! use it to initiate an upstream device search (for example Hue Bridge
//! serial search).

use rhythm_core::runtime::hub_registry::DeviceType;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Monotonic timing context captured when the API first accepts a pairing
/// request. Integrations use this instead of starting a fresh budget after
/// handler-side validation, locking, or durable idempotency writes.
#[derive(Debug, Clone, Copy)]
pub struct PairingRequestContext {
    accepted_at: Instant,
}

impl PairingRequestContext {
    pub fn new(accepted_at: Instant) -> Self {
        Self { accepted_at }
    }

    pub fn accepted_now() -> Self {
        Self::new(Instant::now())
    }

    pub fn deadline_after(self, budget: Duration) -> Instant {
        self.accepted_at + budget
    }
}

/// Pairing correlation IDs are persisted and also appear in a URL path.
/// Keep the grammar deliberately small so they cannot become an unbounded
/// storage key, a path-like value, or an accidental secret container.
pub const PAIRING_SESSION_ID_MAX_LEN: usize = 96;
pub const PAIRING_REQUEST_FINGERPRINT_LEN: usize = 64;

/// Pairing results are only needed long enough for clients to reconcile an
/// interrupted response. The history itself has a separate, larger audit cap.
pub const PAIRING_RESULT_LIMIT: usize = 100;
// Tombstones are intentionally cheap and substantially outnumber retained
// result payloads. This prevents an authenticated burst of unknown lookups
// from consuming the slots needed by real pairing operations while staying
// comfortably inside the pairing document's 1 MiB file bound.
const PAIRING_TOMBSTONE_LIMIT: usize = 2_048;
const PAIRING_TOMBSTONE_TTL_MS: u64 = 24 * 60 * 60 * 1_000;
const PAIRING_TERMINAL_RETENTION_MS: u64 = 30 * 24 * 60 * 60 * 1_000;
const PAIRING_TOMBSTONE_HUB_TYPE: &str = "pairing_tombstone";
const PAIRING_TOMBSTONE_FINGERPRINT: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";
const LOCAL_BLE_PENDING_RESULT_TTL_MS: u64 = 5 * 60 * 1_000;
const DEFAULT_PENDING_RESULT_TTL_MS: u64 = 60 * 60 * 1_000;

static PAIRING_DOCUMENT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static ACTIVE_PAIRING_RESULTS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

/// Owner-visible secret recovery material retained by a pairing integration.
///
/// The shared layer transports this value but must never persist it in pairing
/// history, logs, analytics, or ordinary diagnostics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingRecoverySecret {
    pub payload_kind: String,
    pub setup_payload: String,
    pub captured_at: String,
}

/// Validate a client-generated pairing correlation ID.
pub fn validate_pairing_session_id(session_id: &str) -> Result<(), &'static str> {
    if session_id.is_empty() {
        return Err("Pairing session ID cannot be empty");
    }
    if session_id.len() > PAIRING_SESSION_ID_MAX_LEN {
        return Err("Pairing session ID is too long");
    }
    if !session_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err("Pairing session ID contains unsupported characters");
    }
    Ok(())
}

fn valid_persisted_hub_type(hub_type: &str) -> bool {
    !hub_type.is_empty()
        && hub_type.len() <= 64
        && hub_type
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub fn validate_pairing_request_fingerprint(fingerprint: &str) -> Result<(), &'static str> {
    if fingerprint.len() != PAIRING_REQUEST_FINGERPRINT_LEN
        || !fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err("Pairing request fingerprint is invalid");
    }
    Ok(())
}

/// Build a privacy-safe, deterministic binding between one session ID and
/// the exact logical pairing request. Only an installation-secret HMAC is
/// persisted, so low-entropy setup payloads cannot be verified offline from a
/// support bundle or copied pairing-history file.
pub fn pairing_request_fingerprint(
    hmac_key: &str,
    hub_type: &str,
    params: &serde_json::Value,
) -> anyhow::Result<String> {
    if hmac_key.len() != 64
        || !hmac_key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        anyhow::bail!("pairing fingerprint key is invalid");
    }
    if !valid_persisted_hub_type(hub_type) {
        anyhow::bail!("pairing hub type is invalid");
    }
    fn append_len(canonical: &mut Vec<u8>, len: usize) {
        canonical.extend_from_slice(&(len as u64).to_be_bytes());
    }
    fn append_value(canonical: &mut Vec<u8>, value: &serde_json::Value) {
        match value {
            serde_json::Value::Null => canonical.extend_from_slice(b"n"),
            serde_json::Value::Bool(value) => {
                canonical.extend_from_slice(if *value { b"t" } else { b"f" });
            }
            serde_json::Value::Number(value) => {
                let value = value.to_string();
                canonical.extend_from_slice(b"#");
                append_len(canonical, value.len());
                canonical.extend_from_slice(value.as_bytes());
            }
            serde_json::Value::String(value) => {
                canonical.extend_from_slice(b"s");
                append_len(canonical, value.len());
                canonical.extend_from_slice(value.as_bytes());
            }
            serde_json::Value::Array(values) => {
                canonical.extend_from_slice(b"[");
                append_len(canonical, values.len());
                for value in values {
                    append_value(canonical, value);
                }
            }
            serde_json::Value::Object(values) => {
                canonical.extend_from_slice(b"{");
                append_len(canonical, values.len());
                let mut keys = values.keys().collect::<Vec<_>>();
                keys.sort_unstable();
                for key in keys {
                    append_len(canonical, key.len());
                    canonical.extend_from_slice(key.as_bytes());
                    append_value(canonical, &values[key]);
                }
            }
        }
    }

    let mut canonical = Vec::new();
    canonical.extend_from_slice(b"rhythm-pairing-request-v2\0");
    canonical.extend_from_slice(hub_type.as_bytes());
    canonical.extend_from_slice(b"\0");
    // These fields are transport-owned correlation data, not logical
    // integration parameters. Older clients duplicated `session_id` inside
    // `params`, and the server injects both fields before dispatch. Ignoring
    // them here makes all equivalent clients bind the same request and keeps a
    // caller-supplied fingerprint from recursively influencing the digest.
    let logical_params = match params.as_object() {
        Some(values)
            if values.contains_key("session_id") || values.contains_key("request_fingerprint") =>
        {
            let mut values = values.clone();
            values.remove("session_id");
            values.remove("request_fingerprint");
            serde_json::Value::Object(values)
        }
        _ => params.clone(),
    };
    append_value(&mut canonical, &logical_params);

    const BLOCK_SIZE: usize = 64;
    let mut key_block = [0u8; BLOCK_SIZE];
    key_block[..hmac_key.len()].copy_from_slice(hmac_key.as_bytes());
    let mut outer_key_pad = [0x5c; BLOCK_SIZE];
    let mut inner_key_pad = [0x36; BLOCK_SIZE];
    for index in 0..BLOCK_SIZE {
        outer_key_pad[index] ^= key_block[index];
        inner_key_pad[index] ^= key_block[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_key_pad);
    inner.update(&canonical);
    let inner_hash = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_key_pad);
    outer.update(inner_hash);
    Ok(outer
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

pub fn pairing_request_fingerprint_for_state(
    state: &crate::state::SharedState,
    hub_type: &str,
    params: &serde_json::Value,
) -> anyhow::Result<String> {
    let state = state
        .lock()
        .map_err(|_| anyhow::anyhow!("pairing state lock poisoned"))?;
    if !state.pairing_hmac_key_durable {
        anyhow::bail!("pairing request identity is not durably available");
    }
    let key = state.pairing_hmac_key.clone();
    drop(state);
    pairing_request_fingerprint(&key, hub_type, params)
}

/// Request to start a device pairing session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingRequest {
    /// Which integration handles this pairing (e.g., "matter", "zigbee").
    pub hub_type: String,
    /// Optional client-generated ID for correlating SSE progress events with
    /// this request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Protocol-specific pairing parameters.
    ///
    /// Matter: `{ "setup_payload": "3497-011-2332", "network": "wifi", "rendezvous": "on_network" }`
    /// Hue BLE: `{}` (nearby scan)
    /// Hue Bridge: `{ "serial": "E277DA", "hub_address": "192.0.2.10" }`
    /// Local BLE profile: `{ "profile_id": "...", "setup": { ...bounded parsed fields... } }`
    #[serde(default)]
    pub params: serde_json::Value,
}

/// Status of an ongoing pairing session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingStatus {
    /// Scanning for devices (Matter mDNS, Zigbee permit join).
    Searching,
    /// Device found, negotiating connection.
    Found,
    /// Commissioning / interview in progress.
    Commissioning,
    /// Pairing completed successfully.
    Complete,
    /// Pairing failed.
    Failed,
}

/// User-visible stage of a pairing flow.
///
/// These stages are intentionally protocol-neutral. Integrations can emit the
/// closest stage they can know without leaking transport-specific internals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingStage {
    /// Request accepted and queued for processing.
    Requested,
    /// Hub/runtime transport is being prepared.
    HubConnecting,
    /// Device discovery or rendezvous is in progress.
    Searching,
    /// Device was found and a transport connection is being negotiated.
    Connecting,
    /// Protocol commissioning/interview is in progress.
    Commissioning,
    /// Device was commissioned and local registries are being updated.
    Finalizing,
    /// Pairing completed successfully.
    Complete,
    /// Pairing failed.
    Failed,
}

/// Privacy-safe terminal stage for an actionable pairing failure.
///
/// This is deliberately evidence-shaped rather than cause-shaped: a transport
/// can report the deepest stage it reached without claiming why the peripheral
/// or radio failed. The value is safe for durable pairing history and support
/// bundles; it never contains an address, setup payload, or device identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingFailureStage {
    TargetNotObserved,
    CandidateOpen,
    CandidateConnect,
    CandidateServiceDiscovery,
    CandidateServiceMismatch,
    CandidateCleanup,
    Transport,
    /// A stage added by a newer server. Older readers retain a safe marker
    /// instead of rejecting the complete durable pairing document.
    #[serde(other)]
    Unknown,
}

impl PairingFailureStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TargetNotObserved => "target_not_observed",
            Self::CandidateOpen => "candidate_open",
            Self::CandidateConnect => "candidate_connect",
            Self::CandidateServiceDiscovery => "candidate_service_discovery",
            Self::CandidateServiceMismatch => "candidate_service_mismatch",
            Self::CandidateCleanup => "candidate_cleanup",
            Self::Transport => "transport",
            Self::Unknown => "unknown",
        }
    }
}

/// Typed integration error carrying a bounded public message and safe stage.
///
/// Integration-specific causes stay inside their crate. The shared handler can
/// downcast this marker to persist the stage while preserving its existing
/// HTTP status and terminal-result behavior.
#[derive(Debug)]
pub struct PairingFailure {
    stage: PairingFailureStage,
    user_message: &'static str,
}

impl PairingFailure {
    pub const fn new(stage: PairingFailureStage, user_message: &'static str) -> Self {
        Self {
            stage,
            user_message,
        }
    }

    pub const fn stage(&self) -> PairingFailureStage {
        self.stage
    }
}

impl std::fmt::Display for PairingFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.user_message)
    }
}

impl std::error::Error for PairingFailure {}

/// Information about a successfully paired device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairedDeviceInfo {
    /// Hub-native device identifier.
    pub device_id: String,
    /// Human-readable device name.
    pub name: String,
    /// Device type (Light, Button, Motion).
    pub device_type: DeviceType,
    /// Manufacturer name (if known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    /// Model identifier (if known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// State of a pairing session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairingSession {
    /// Which integration is handling this pairing.
    pub hub_type: String,
    /// Current status.
    pub status: PairingStatus,
    /// Device info (populated on completion).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<PairedDeviceInfo>,
    /// Every device completed by a batch pairing request.
    ///
    /// `device` remains the first entry for compatibility with clients that
    /// predate batch discovery.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<PairedDeviceInfo>,
    /// Error message (populated on failure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Deepest privacy-safe stage reached by a failed attempt.
    ///
    /// Optional and additive so clients from before this field continue to
    /// decode pairing results while newer clients can present targeted help.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_stage: Option<PairingFailureStage>,
    /// Non-fatal candidate failures from a partially successful batch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// Integration-specific structured result details used by auxiliary
    /// pairing UI steps such as safe recovery-target selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// Polling state for a client-generated pairing session ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingResultState {
    Pending,
    Terminal,
    NotFound,
}

/// Stable response returned by `GET /api/devices/pair/:session_id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairingResultStatus {
    pub session_id: String,
    pub state: PairingResultState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<PairingSession>,
}

/// A bounded durable reconciliation record. It intentionally contains no
/// request parameters or private integration details; setup payloads and
/// hardware identities must never enter this document. The Matter pairing
/// path may retain one explicitly allowlisted low-cardinality recovery action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairingResultRecord {
    pub session_id: String,
    pub hub_type: String,
    pub request_fingerprint: String,
    pub updated_at_epoch_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<PairingSession>,
}

#[derive(Debug)]
pub enum BeginPairingResult {
    Started(PairingResultLease),
    Pending(PairingResultStatus),
    Terminal(PairingSession),
    HubTypeConflict,
    RequestConflict,
    /// A status lookup or definitive pre-start cancellation already proved
    /// that this correlation ID cannot own a later integration invocation.
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcknowledgePairingResult {
    Acknowledged,
    Pending,
    NotFound,
}

/// In-process ownership fence for a durable pending operation. While held,
/// status polling cannot age the record into an interrupted failure.
#[derive(Debug)]
pub struct PairingResultLease {
    session_id: String,
}

impl PairingResultLease {
    fn acquire(session_id: &str) -> anyhow::Result<Self> {
        let mut active = ACTIVE_PAIRING_RESULTS
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
            .map_err(|_| anyhow::anyhow!("pairing result ownership fence is poisoned"))?;
        if !active.insert(session_id.to_string()) {
            anyhow::bail!("pairing session is already owned by this process");
        }
        Ok(Self {
            session_id: session_id.to_string(),
        })
    }
}

impl Drop for PairingResultLease {
    fn drop(&mut self) {
        if let Ok(mut active) = ACTIVE_PAIRING_RESULTS
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
        {
            active.remove(&self.session_id);
        }
    }
}

/// Emit a pairing progress event to SSE clients.
#[allow(clippy::too_many_arguments)]
pub fn emit_pairing_progress(
    state: &crate::state::SharedState,
    hub_type: &str,
    session_id: Option<&str>,
    status: PairingStatus,
    stage: PairingStage,
    message: impl Into<String>,
    device: Option<PairedDeviceInfo>,
    error: Option<String>,
) {
    emit_pairing_progress_with_devices(
        state,
        hub_type,
        session_id,
        status,
        stage,
        message,
        device,
        Vec::new(),
        Vec::new(),
        error,
    );
}

/// Emit a pairing progress event whose terminal result contains a complete
/// batch projection.
#[allow(clippy::too_many_arguments)]
pub fn emit_pairing_progress_with_devices(
    state: &crate::state::SharedState,
    hub_type: &str,
    session_id: Option<&str>,
    status: PairingStatus,
    stage: PairingStage,
    message: impl Into<String>,
    device: Option<PairedDeviceInfo>,
    devices: Vec<PairedDeviceInfo>,
    warnings: Vec<String>,
    error: Option<String>,
) {
    emit_pairing_progress_with_devices_and_failure(
        state, hub_type, session_id, status, stage, message, device, devices, warnings, error, None,
    );
}

/// Emit pairing progress with an additive privacy-safe terminal failure stage.
#[allow(clippy::too_many_arguments)]
pub fn emit_pairing_progress_with_devices_and_failure(
    state: &crate::state::SharedState,
    hub_type: &str,
    session_id: Option<&str>,
    status: PairingStatus,
    stage: PairingStage,
    message: impl Into<String>,
    device: Option<PairedDeviceInfo>,
    devices: Vec<PairedDeviceInfo>,
    warnings: Vec<String>,
    error: Option<String>,
    failure_stage: Option<PairingFailureStage>,
) {
    let device = device.or_else(|| devices.first().cloned());
    crate::state::emit_server_event(
        state,
        crate::server_event::ServerEvent::PairingProgress {
            hub_type: hub_type.to_string(),
            session_id: session_id.map(str::to_string),
            status,
            stage,
            message: message.into(),
            device,
            devices,
            warnings,
            error,
            failure_stage,
        },
    );
}

// ---------------------------------------------------------------------------
// Unpairing (decommission)
// ---------------------------------------------------------------------------

/// Request to unpair/decommission a device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnpairingRequest {
    /// Which integration handles this unpairing (e.g., "matter").
    pub hub_type: String,
    /// Protocol-specific parameters.
    ///
    /// Matter: `{ "device_id": "matter-100", "force": false }`
    /// Hue Bridge: `{ "device_id": "v2-device-id", "hub_address": "192.0.2.10", "force": false }`
    /// Hue Bluetooth: `{ "device_id": "hue-ble-001788010f76565a", "force": false }`
    #[serde(default)]
    pub params: serde_json::Value,
}

/// Result of an unpairing operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnpairingCompletionScope {
    /// Peripheral trust was released before local credentials were removed.
    DeviceReleased,
    /// Rhythm removed its local bond only after an authenticated vendor
    /// handoff. The peripheral's replacement-pairing window may be
    /// time-limited, so this is intentionally weaker than DeviceReleased.
    LocalBondRemoved,
    /// Rhythm forgot the endpoint but intentionally retained its local bond.
    LocalBondRetained,
    /// Only Rhythm-side records were removed; peripheral trust is unknown.
    LocalStateOnly,
    /// No endpoint or bond metadata remained when removal was requested.
    AlreadyAbsent,
}

/// Result of an unpairing operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnpairingResult {
    /// Which integration handled this unpairing.
    pub hub_type: String,
    /// Exact hub instance whose endpoint was removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_address: Option<String>,
    /// Outcome status (Complete or Failed).
    pub status: PairingStatus,
    /// Device ID that was unpaired (populated on completion).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    /// Error message (populated on failure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What a successful completion actually accomplished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_scope: Option<UnpairingCompletionScope>,
    /// Nonfatal follow-up guidance for a partial/local-only completion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

// ---------------------------------------------------------------------------
// Pairing history (persisted audit trail)
// ---------------------------------------------------------------------------

/// Cap on persisted pairing-history entries. Pairing is user-driven and
/// rare, so 200 entries covers months while keeping the file tiny.
pub const PAIRING_HISTORY_LIMIT: usize = 200;
// Durable operation results were introduced in v2. A v1 binary therefore
// recognizes this document as future/read-only instead of silently dropping
// pairing receipts during an unrelated history rewrite.
pub const PAIRING_HISTORY_SCHEMA_VERSION: u32 = 2;

/// One pair/unpair attempt, persisted so a debug bundle can answer "what
/// removal/pairing attempts happened" even after the logs rotate away
/// (the issue #123 bundle lost a 5-hour window to rotation).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingHistoryEntry {
    /// RFC3339 UTC timestamp.
    pub at: String,
    pub epoch_ms: u64,
    /// "pair" or "unpair".
    pub kind: String,
    pub hub_type: String,
    /// App-generated privacy-safe journey correlation. This deliberately does
    /// not fall back to the transport session ID because retries have their
    /// own session IDs while one user journey owns the lifecycle outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    /// Canonical device class involved in the lifecycle attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_type: Option<String>,
    /// Bounded protocol profile identifier (for example
    /// `orein.oc02001.button.v1`). Raw setup or hardware identity values are
    /// never persisted in pairing history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendezvous: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    /// Final status (e.g. "complete", "failed").
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Deepest privacy-safe stage reached by a failed attempt, when the
    /// integration can distinguish it without inferring an exact cause.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_stage: Option<PairingFailureStage>,
    /// Paired device summary, e.g. "Leedarson Smart RGBTW Bulb (matter-106)".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Every device completed by a batch pairing attempt.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<String>,
    /// Non-fatal candidate failures from a partially successful batch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Persisted document shape (`pairing_history.json`), oldest entry first.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingHistory {
    pub schema_version: u32,
    #[serde(default)]
    pub entries: Vec<PairingHistoryEntry>,
    /// Recent idempotency/reconciliation records, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pairing_results: Vec<PairingResultRecord>,
}

impl PairingHistory {
    pub fn normalized(mut self) -> Self {
        self.entries.sort_by_key(|entry| entry.epoch_ms);
        if self.entries.len() > PAIRING_HISTORY_LIMIT {
            let excess = self.entries.len() - PAIRING_HISTORY_LIMIT;
            self.entries.drain(..excess);
        }
        self.pairing_results
            .sort_by_key(|record| record.updated_at_epoch_ms);
        self
    }
}

fn empty_pairing_history() -> PairingHistory {
    PairingHistory {
        schema_version: PAIRING_HISTORY_SCHEMA_VERSION,
        entries: Vec::new(),
        pairing_results: Vec::new(),
    }
}

fn pairing_storage(
    state: &crate::state::SharedState,
) -> anyhow::Result<std::sync::Arc<dyn crate::storage::Storage>> {
    state
        .lock()
        .map_err(|_| anyhow::anyhow!("pairing state lock poisoned"))?
        .storage
        .clone()
        .ok_or_else(|| anyhow::anyhow!("pairing reconciliation storage is unavailable"))
}

/// Serialize factory-reset deletion against every pairing-ledger
/// load/modify/save operation. The platform's BLE reset fence closes outbox
/// admission before this runs, so no terminal receipt can recreate the file
/// after deletion.
pub fn clear_persisted_state_for_factory_reset(
    state: &crate::state::SharedState,
) -> anyhow::Result<()> {
    let storage = state
        .lock()
        .map_err(|_| anyhow::anyhow!("pairing state lock poisoned"))?
        .storage
        .clone();
    let Some(storage) = storage else {
        return Ok(());
    };
    let _guard = PAIRING_DOCUMENT_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("pairing document lock poisoned"))?;
    storage.clear_factory_reset_state()
}

fn load_pairing_document(storage: &dyn crate::storage::Storage) -> anyhow::Result<PairingHistory> {
    let document = storage
        .load_pairing_history()?
        .unwrap_or_else(empty_pairing_history);
    validate_pairing_document(&document)?;
    Ok(document.normalized())
}

fn prepare_writable_pairing_document(document: &mut PairingHistory) -> anyhow::Result<()> {
    validate_pairing_document(document)?;
    document.schema_version = PAIRING_HISTORY_SCHEMA_VERSION;
    Ok(())
}

fn validate_pairing_document(document: &PairingHistory) -> anyhow::Result<()> {
    if document.schema_version == 0 || document.schema_version > PAIRING_HISTORY_SCHEMA_VERSION {
        anyhow::bail!(
            "pairing history schema {} is unsupported",
            document.schema_version
        );
    }
    if document.schema_version < 2 && !document.pairing_results.is_empty() {
        anyhow::bail!("pairing result records require pairing history schema v2");
    }
    let durable_result_count = document
        .pairing_results
        .iter()
        .filter(|record| record.hub_type != PAIRING_TOMBSTONE_HUB_TYPE)
        .count();
    let tombstone_count = document.pairing_results.len() - durable_result_count;
    if durable_result_count > PAIRING_RESULT_LIMIT {
        anyhow::bail!("pairing history exceeds its durable result limit");
    }
    if tombstone_count > PAIRING_TOMBSTONE_LIMIT {
        anyhow::bail!("pairing history exceeds its causal tombstone limit");
    }

    let mut session_ids = HashSet::with_capacity(document.pairing_results.len());
    for record in &document.pairing_results {
        validate_pairing_session_id(&record.session_id).map_err(anyhow::Error::msg)?;
        if !session_ids.insert(record.session_id.as_str()) {
            anyhow::bail!("pairing history contains a duplicate session ID");
        }
        if record.hub_type == PAIRING_TOMBSTONE_HUB_TYPE {
            if record.request_fingerprint != PAIRING_TOMBSTONE_FINGERPRINT
                || record.result.is_some()
            {
                anyhow::bail!("pairing history contains an invalid causal tombstone");
            }
            continue;
        }
        if !valid_persisted_hub_type(&record.hub_type) {
            anyhow::bail!("pairing history contains an invalid hub type");
        }
        validate_pairing_request_fingerprint(&record.request_fingerprint)
            .map_err(anyhow::Error::msg)?;
        if let Some(result) = &record.result {
            validate_terminal_session(result, &record.hub_type)?;
        }
    }
    Ok(())
}

fn validate_terminal_session(session: &PairingSession, hub_type: &str) -> anyhow::Result<()> {
    validate_terminal_session_input(session, hub_type)?;
    if session.details != sanitized_terminal_details(session) {
        anyhow::bail!("durable pairing results cannot contain private details");
    }
    Ok(())
}

fn validate_terminal_session_input(session: &PairingSession, hub_type: &str) -> anyhow::Result<()> {
    if session.hub_type != hub_type {
        anyhow::bail!("terminal pairing result belongs to another hub type");
    }
    if !matches!(
        session.status,
        PairingStatus::Complete | PairingStatus::Failed
    ) {
        anyhow::bail!("pairing history contains a non-terminal result");
    }
    if session.devices.len() > 64 || session.warnings.len() > 32 {
        anyhow::bail!("durable pairing result exceeds its collection bounds");
    }
    match session.status {
        PairingStatus::Complete => {
            if session.device.is_none() && session.devices.is_empty() {
                anyhow::bail!("successful durable pairing result has no device");
            }
            if session.error.is_some() {
                anyhow::bail!("successful durable pairing result contains an error");
            }
            if session.failure_stage.is_some() {
                anyhow::bail!("successful durable pairing result contains a failure stage");
            }
        }
        PairingStatus::Failed => {
            if session.device.is_some()
                || !session.devices.is_empty()
                || !session.warnings.is_empty()
            {
                anyhow::bail!("failed durable pairing result contains success-only fields");
            }
        }
        _ => unreachable!("terminal status was checked above"),
    }
    if let (Some(device), Some(first)) = (session.device.as_ref(), session.devices.first()) {
        if device != first {
            anyhow::bail!("durable pairing result has inconsistent first-device projections");
        }
    }
    if session
        .error
        .as_ref()
        .is_some_and(|value| value.len() > 512)
        || session.warnings.iter().any(|value| value.len() > 512)
    {
        anyhow::bail!("durable pairing result exceeds its text bounds");
    }
    let devices = session.device.iter().chain(session.devices.iter());
    for device in devices {
        if device.device_id.trim().is_empty()
            || device.device_id.len() > 256
            || device.name.trim().is_empty()
            || device.name.len() > 256
            || device
                .manufacturer
                .as_ref()
                .is_some_and(|value| value.len() > 128)
            || device.model.as_ref().is_some_and(|value| value.len() > 128)
        {
            anyhow::bail!("durable paired-device result exceeds its text bounds");
        }
    }
    Ok(())
}

fn pairing_result_is_active(session_id: &str) -> bool {
    ACTIVE_PAIRING_RESULTS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|active| active.contains(session_id))
        // A poisoned ownership fence must fail safe: do not manufacture a
        // terminal timeout while the process may still own the operation.
        .unwrap_or(true)
}

fn expire_stale_pending_results(
    document: &mut PairingHistory,
    completion_session_id: Option<&str>,
) -> bool {
    let now = crate::state::current_epoch_ms();
    let mut changed = false;
    for record in &mut document.pairing_results {
        if record.hub_type == PAIRING_TOMBSTONE_HUB_TYPE
            || record.result.is_some()
            || completion_session_id == Some(record.session_id.as_str())
            || pairing_result_is_active(&record.session_id)
        {
            continue;
        }
        let ttl = if record.hub_type == crate::hub::HubType::LOCAL_BLE {
            LOCAL_BLE_PENDING_RESULT_TTL_MS
        } else {
            DEFAULT_PENDING_RESULT_TTL_MS
        };
        if now.saturating_sub(record.updated_at_epoch_ms) <= ttl {
            continue;
        }
        record.updated_at_epoch_ms = now;
        record.result = Some(PairingSession {
            hub_type: record.hub_type.clone(),
            status: PairingStatus::Failed,
            device: None,
            devices: Vec::new(),
            error: Some(
                "Pairing was interrupted before the appliance recorded a final result".to_string(),
            ),
            failure_stage: None,
            warnings: Vec::new(),
            details: None,
        });
        changed = true;
    }
    changed
}

fn prune_expired_pairing_records(document: &mut PairingHistory) -> bool {
    let now = crate::state::current_epoch_ms();
    let previous_len = document.pairing_results.len();
    document.pairing_results.retain(|record| {
        let age = now.saturating_sub(record.updated_at_epoch_ms);
        if record.hub_type == PAIRING_TOMBSTONE_HUB_TYPE {
            return age <= PAIRING_TOMBSTONE_TTL_MS;
        }
        // Pending records are never evicted. They first become an explicit
        // interrupted terminal through `expire_stale_pending_results`, after
        // which clients retain a full month to reconcile or acknowledge them.
        record.result.is_none() || age <= PAIRING_TERMINAL_RETENTION_MS
    });
    document.pairing_results.len() != previous_len
}

fn durable_pairing_result_count(document: &PairingHistory) -> usize {
    document
        .pairing_results
        .iter()
        .filter(|record| record.hub_type != PAIRING_TOMBSTONE_HUB_TYPE)
        .count()
}

fn insert_pairing_tombstone(
    document: &mut PairingHistory,
    session_id: &str,
) -> anyhow::Result<bool> {
    if document
        .pairing_results
        .iter()
        .any(|record| record.session_id == session_id)
    {
        return Ok(false);
    }
    let tombstone_count = document
        .pairing_results
        .iter()
        .filter(|record| record.hub_type == PAIRING_TOMBSTONE_HUB_TYPE)
        .count();
    if tombstone_count >= PAIRING_TOMBSTONE_LIMIT {
        anyhow::bail!("pairing causal tombstone ledger is full");
    }
    document.pairing_results.push(PairingResultRecord {
        session_id: session_id.to_string(),
        hub_type: PAIRING_TOMBSTONE_HUB_TYPE.to_string(),
        request_fingerprint: PAIRING_TOMBSTONE_FINGERPRINT.to_string(),
        updated_at_epoch_ms: crate::state::current_epoch_ms(),
        result: None,
    });
    Ok(true)
}

fn pairing_status_for(record: &PairingResultRecord) -> PairingResultStatus {
    PairingResultStatus {
        session_id: record.session_id.clone(),
        state: if record.result.is_some() {
            PairingResultState::Terminal
        } else {
            PairingResultState::Pending
        },
        hub_type: Some(record.hub_type.clone()),
        result: record.result.clone(),
    }
}

fn bounded_pairing_text(value: String, max_len: usize) -> String {
    let mut value = value;
    if value.len() > max_len {
        let mut boundary = max_len;
        while !value.is_char_boundary(boundary) {
            boundary -= 1;
        }
        value.truncate(boundary);
    }
    // Pairing warnings from integrations can contain a BlueZ address or a
    // setup code. Redact recognizable credential/identity tokens before the
    // text enters durable reconciliation state.
    value
        .split_whitespace()
        .map(|token| {
            let core = token.trim_matches(|character: char| {
                !character.is_ascii_alphanumeric() && !matches!(character, ':' | '%' | '$')
            });
            let compact_hex = core
                .chars()
                .filter(|character| *character != ':')
                .collect::<String>();
            let is_mac = core.matches(':').count() == 5
                && compact_hex.len() == 12
                && compact_hex.bytes().all(|byte| byte.is_ascii_hexdigit());
            let is_identity =
                compact_hex.len() >= 12 && compact_hex.bytes().all(|byte| byte.is_ascii_hexdigit());
            if core.starts_with("MT:") || core.starts_with("B:") || is_mac || is_identity {
                "<redacted>"
            } else {
                token
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn sanitized_paired_device(device: &PairedDeviceInfo) -> PairedDeviceInfo {
    PairedDeviceInfo {
        // These fields are the public canonical result consumed by room
        // assignment. They were strictly bounded before this copy and must not
        // be token-redacted into an unusable or non-existent device ID.
        device_id: device.device_id.clone(),
        name: device.name.clone(),
        device_type: device.device_type.clone(),
        manufacturer: device.manufacturer.clone(),
        model: device.model.clone(),
    }
}

fn sanitized_terminal_session(session: &PairingSession) -> PairingSession {
    PairingSession {
        hub_type: session.hub_type.clone(),
        status: session.status.clone(),
        device: session.device.as_ref().map(sanitized_paired_device),
        devices: session
            .devices
            .iter()
            .take(64)
            .map(sanitized_paired_device)
            .collect(),
        error: session
            .error
            .clone()
            .map(|error| bounded_pairing_text(error, 512)),
        failure_stage: session.failure_stage,
        warnings: session
            .warnings
            .iter()
            .take(32)
            .cloned()
            .map(|warning| bounded_pairing_text(warning, 512))
            .collect(),
        // Details may contain candidate addresses or protocol setup fields.
        // Preserve only the explicit Matter recovery enum used to explain a
        // repeat-pair outcome to the initiating app.
        details: sanitized_terminal_details(session),
    }
}

fn sanitized_terminal_details(session: &PairingSession) -> Option<serde_json::Value> {
    if session.hub_type != "matter" {
        return None;
    }
    let recovery_action = session.details.as_ref()?.get("recovery_action")?.as_str()?;
    if !matches!(
        recovery_action,
        "existing_connection_recovered"
            | "existing_node_recommissioned"
            | "existing_node_recommission_failed"
    ) {
        return None;
    }
    Some(serde_json::json!({
        "recovery_action": recovery_action,
    }))
}

/// Validate an integration terminal before producing the bounded, privacy-safe
/// representation suitable for durable storage, HTTP, SSE, and app recovery.
/// Public device identity fields remain exact; private `details` are removed,
/// the bounded Matter recovery outcome is retained, and human diagnostic text
/// has recognizable setup/address tokens redacted.
pub fn sanitized_terminal_session_for_delivery(
    session: &PairingSession,
    hub_type: &str,
) -> anyhow::Result<PairingSession> {
    validate_terminal_session_input(session, hub_type)?;
    let result = sanitized_terminal_session(session);
    validate_terminal_session(&result, hub_type)?;
    Ok(result)
}

/// Reserve a durable idempotency key before invoking an integration.
pub fn begin_pairing_result(
    state: &crate::state::SharedState,
    session_id: &str,
    hub_type: &str,
    request_fingerprint: &str,
) -> anyhow::Result<BeginPairingResult> {
    validate_pairing_session_id(session_id).map_err(anyhow::Error::msg)?;
    if !valid_persisted_hub_type(hub_type) {
        anyhow::bail!("pairing hub type is invalid");
    }
    validate_pairing_request_fingerprint(request_fingerprint).map_err(anyhow::Error::msg)?;
    let storage = pairing_storage(state)?;
    let _guard = PAIRING_DOCUMENT_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("pairing document lock poisoned"))?;
    let mut document = load_pairing_document(storage.as_ref())?;
    prepare_writable_pairing_document(&mut document)?;
    let mut changed = prune_expired_pairing_records(&mut document);
    changed |= expire_stale_pending_results(&mut document, None);
    if changed {
        storage.save_pairing_history(&document)?;
    }
    if let Some(existing) = document
        .pairing_results
        .iter()
        .find(|record| record.session_id == session_id)
    {
        if existing.hub_type == PAIRING_TOMBSTONE_HUB_TYPE {
            return Ok(BeginPairingResult::Cancelled);
        }
        if existing.hub_type != hub_type {
            return Ok(BeginPairingResult::HubTypeConflict);
        }
        if existing.request_fingerprint != request_fingerprint {
            return Ok(BeginPairingResult::RequestConflict);
        }
        return Ok(match existing.result.clone() {
            Some(result) => BeginPairingResult::Terminal(result),
            None => BeginPairingResult::Pending(pairing_status_for(existing)),
        });
    }
    if durable_pairing_result_count(&document) >= PAIRING_RESULT_LIMIT {
        anyhow::bail!("pairing reconciliation ledger is full; reset or service is required");
    }
    document.pairing_results.push(PairingResultRecord {
        session_id: session_id.to_string(),
        hub_type: hub_type.to_string(),
        request_fingerprint: request_fingerprint.to_string(),
        updated_at_epoch_ms: crate::state::current_epoch_ms(),
        result: None,
    });
    let lease = PairingResultLease::acquire(session_id)?;
    storage.save_pairing_history(&document.normalized())?;
    Ok(BeginPairingResult::Started(lease))
}

/// Persist a terminal result before returning it over HTTP or SSE.
pub fn complete_pairing_result(
    state: &crate::state::SharedState,
    session_id: &str,
    hub_type: &str,
    session: &PairingSession,
) -> anyhow::Result<()> {
    complete_pairing_result_inner(state, session_id, hub_type, None, session, false)
}

/// Persist a terminal result while binding it to an activation receipt's
/// request digest. This form can repair a missing operation record after a
/// crash because the authoritative device store supplies the same digest.
pub fn complete_pairing_result_with_fingerprint(
    state: &crate::state::SharedState,
    session_id: &str,
    hub_type: &str,
    request_fingerprint: &str,
    session: &PairingSession,
) -> anyhow::Result<()> {
    validate_pairing_request_fingerprint(request_fingerprint).map_err(anyhow::Error::msg)?;
    complete_pairing_result_inner(
        state,
        session_id,
        hub_type,
        Some(request_fingerprint),
        session,
        false,
    )
}

/// Reconcile an authoritative activation outbox with the terminal ledger.
///
/// The first terminal payload remains immutable. If the handler already
/// persisted success with an additional durability warning before a crash,
/// the activation outbox may safely acknowledge that same device-bound
/// success without trying to rewrite the observed payload.
pub fn reconcile_committed_pairing_success_with_fingerprint(
    state: &crate::state::SharedState,
    session_id: &str,
    hub_type: &str,
    request_fingerprint: &str,
    session: &PairingSession,
) -> anyhow::Result<()> {
    if session.status != PairingStatus::Complete {
        anyhow::bail!("activation reconciliation requires a successful result");
    }
    validate_pairing_request_fingerprint(request_fingerprint).map_err(anyhow::Error::msg)?;
    complete_pairing_result_inner(
        state,
        session_id,
        hub_type,
        Some(request_fingerprint),
        session,
        true,
    )
}

fn complete_pairing_result_inner(
    state: &crate::state::SharedState,
    session_id: &str,
    hub_type: &str,
    request_fingerprint: Option<&str>,
    session: &PairingSession,
    accept_existing_success: bool,
) -> anyhow::Result<()> {
    if !matches!(
        session.status,
        PairingStatus::Complete | PairingStatus::Failed
    ) {
        anyhow::bail!("cannot persist a non-terminal pairing result");
    }
    validate_pairing_session_id(session_id).map_err(anyhow::Error::msg)?;
    if !valid_persisted_hub_type(hub_type) {
        anyhow::bail!("pairing hub type is invalid");
    }
    if session.hub_type != hub_type {
        anyhow::bail!("terminal pairing result belongs to another hub type");
    }
    // Reject oversized/malformed integration output before the privacy copy.
    // Sanitization may redact error text and remove details, but it must never
    // make an invalid collection or public device identity appear valid.
    validate_terminal_session_input(session, hub_type)?;
    let storage = pairing_storage(state)?;
    let _guard = PAIRING_DOCUMENT_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("pairing document lock poisoned"))?;
    let mut document = load_pairing_document(storage.as_ref())?;
    prepare_writable_pairing_document(&mut document)?;
    // Receipt reconciliation must be able to recover a committed activation
    // even when the ledger was previously full of now-expired terminals.
    // Prune before both the session lookup and the capacity check; otherwise a
    // first GET could tombstone the receipt's session after this repair fails.
    let mut changed = prune_expired_pairing_records(&mut document);
    changed |= expire_stale_pending_results(&mut document, Some(session_id));
    let result = sanitized_terminal_session_for_delivery(session, hub_type)?;
    match document
        .pairing_results
        .iter_mut()
        .find(|record| record.session_id == session_id)
    {
        Some(record) if record.hub_type != hub_type => {
            anyhow::bail!("pairing session ID belongs to another hub type")
        }
        Some(record) => {
            if request_fingerprint.is_some_and(|value| value != record.request_fingerprint) {
                anyhow::bail!("pairing session ID belongs to another request");
            }
            match &record.result {
                Some(existing) if existing == &result => {}
                Some(existing)
                    if accept_existing_success
                        && successful_pairing_identity(existing)
                            == successful_pairing_identity(&result)
                        && successful_pairing_identity(existing).is_some() => {}
                Some(_) => anyhow::bail!("terminal pairing result is immutable"),
                None => {
                    record.updated_at_epoch_ms = crate::state::current_epoch_ms();
                    record.result = Some(result);
                    changed = true;
                }
            }
        }
        None => {
            let Some(request_fingerprint) = request_fingerprint else {
                anyhow::bail!("pairing result reservation is missing");
            };
            if durable_pairing_result_count(&document) >= PAIRING_RESULT_LIMIT {
                anyhow::bail!(
                    "pairing reconciliation ledger is full; activation receipt was retained"
                );
            }
            document.pairing_results.push(PairingResultRecord {
                session_id: session_id.to_string(),
                hub_type: hub_type.to_string(),
                request_fingerprint: request_fingerprint.to_string(),
                updated_at_epoch_ms: crate::state::current_epoch_ms(),
                result: Some(result),
            });
            changed = true;
        }
    }
    if changed {
        storage.save_pairing_history(&document.normalized())?;
    }
    Ok(())
}

fn successful_pairing_identity(session: &PairingSession) -> Option<(&str, &str)> {
    if session.status != PairingStatus::Complete {
        return None;
    }
    let device = session
        .device
        .as_ref()
        .or_else(|| session.devices.first())?;
    Some((session.hub_type.as_str(), device.device_id.as_str()))
}

/// Close a reservation when platform resource admission fails before the
/// integration starts. A duplicate request may already have observed
/// `Pending`, so removing the record would create an invalid Pending→404
/// transition. Persist an explicit terminal failure instead.
pub fn fail_pairing_result_before_start(
    state: &crate::state::SharedState,
    session_id: &str,
    hub_type: &str,
    request_fingerprint: &str,
    error: &str,
) -> anyhow::Result<()> {
    complete_pairing_result_with_fingerprint(
        state,
        session_id,
        hub_type,
        request_fingerprint,
        &PairingSession {
            hub_type: hub_type.to_string(),
            status: PairingStatus::Failed,
            device: None,
            devices: Vec::new(),
            error: Some(error.to_string()),
            failure_stage: None,
            warnings: Vec::new(),
            details: None,
        },
    )
}

/// Load a pairing reconciliation record without mutating it.
pub fn lookup_pairing_result(
    state: &crate::state::SharedState,
    session_id: &str,
) -> anyhow::Result<Option<PairingResultStatus>> {
    validate_pairing_session_id(session_id).map_err(anyhow::Error::msg)?;
    let storage = pairing_storage(state)?;
    let _guard = PAIRING_DOCUMENT_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("pairing document lock poisoned"))?;
    let mut document = load_pairing_document(storage.as_ref())?;
    if expire_stale_pending_results(&mut document, None) {
        storage.save_pairing_history(&document)?;
    }
    Ok(document
        .pairing_results
        .iter()
        .find(|record| record.session_id == session_id)
        .filter(|record| record.hub_type != PAIRING_TOMBSTONE_HUB_TYPE)
        .map(pairing_status_for))
}

/// Load a pairing result or durably record that this lookup won the race with
/// any later POST using the same session ID. Returning a 404 from this path is
/// therefore a causal guarantee: an overtaken POST cannot subsequently start
/// the integration and become a ghost operation.
pub fn lookup_or_tombstone_pairing_result(
    state: &crate::state::SharedState,
    session_id: &str,
) -> anyhow::Result<Option<PairingResultStatus>> {
    validate_pairing_session_id(session_id).map_err(anyhow::Error::msg)?;
    let storage = pairing_storage(state)?;
    let _guard = PAIRING_DOCUMENT_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("pairing document lock poisoned"))?;
    let mut document = load_pairing_document(storage.as_ref())?;
    let mut changed = prune_expired_pairing_records(&mut document);
    changed |= expire_stale_pending_results(&mut document, None);
    let status = document
        .pairing_results
        .iter()
        .find(|record| record.session_id == session_id)
        .filter(|record| record.hub_type != PAIRING_TOMBSTONE_HUB_TYPE)
        .map(pairing_status_for);
    if status.is_none()
        && !document
            .pairing_results
            .iter()
            .any(|record| record.session_id == session_id)
    {
        changed |= insert_pairing_tombstone(&mut document, session_id)?;
    }
    if changed {
        storage.save_pairing_history(&document.normalized())?;
    }
    Ok(status)
}

/// Acknowledge that the client has durably consumed a terminal result. The
/// heavier payload can then be removed without permitting session-ID reuse;
/// a small, expiring tombstone retains that causal fence.
pub fn acknowledge_pairing_result(
    state: &crate::state::SharedState,
    session_id: &str,
) -> anyhow::Result<AcknowledgePairingResult> {
    validate_pairing_session_id(session_id).map_err(anyhow::Error::msg)?;
    let storage = pairing_storage(state)?;
    let _guard = PAIRING_DOCUMENT_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("pairing document lock poisoned"))?;
    let mut document = load_pairing_document(storage.as_ref())?;
    let mut changed = prune_expired_pairing_records(&mut document);
    changed |= expire_stale_pending_results(&mut document, None);
    let outcome = match document
        .pairing_results
        .iter()
        .position(|record| record.session_id == session_id)
    {
        Some(index) if document.pairing_results[index].hub_type == PAIRING_TOMBSTONE_HUB_TYPE => {
            AcknowledgePairingResult::Acknowledged
        }
        Some(index) if document.pairing_results[index].result.is_none() => {
            AcknowledgePairingResult::Pending
        }
        Some(index) => {
            document.pairing_results.remove(index);
            changed |= insert_pairing_tombstone(&mut document, session_id)?;
            AcknowledgePairingResult::Acknowledged
        }
        None => {
            // DELETE is idempotent beyond tombstone retention. A client may
            // keep its locally cached terminal snapshot indefinitely after a
            // crash between server acknowledgement and pointer cleanup.
            changed |= insert_pairing_tombstone(&mut document, session_id)?;
            AcknowledgePairingResult::Acknowledged
        }
    };
    if changed {
        storage.save_pairing_history(&document.normalized())?;
    }
    Ok(outcome)
}

fn status_label(status: &PairingStatus) -> String {
    match serde_json::to_value(status) {
        Ok(serde_json::Value::String(label)) => label,
        _ => format!("{status:?}"),
    }
}

fn param_str(params: &serde_json::Value, key: &str) -> Option<String> {
    params.get(key)?.as_str().map(str::to_string)
}

fn bounded_profile_id(params: &serde_json::Value) -> Option<String> {
    let profile_id = params.get("profile_id")?.as_str()?;
    crate::hub::is_valid_device_profile_id(profile_id).then(|| profile_id.to_string())
}

fn bounded_correlation_id(params: &serde_json::Value) -> Option<String> {
    let correlation_id = params.get("correlation_id")?.as_str()?.trim();
    validate_pairing_session_id(correlation_id)
        .is_ok()
        .then(|| correlation_id.to_string())
}

fn device_type_label(device_type: &DeviceType) -> &'static str {
    match device_type {
        DeviceType::Light => "light",
        DeviceType::Button => "button",
        DeviceType::Motion => "motion",
        DeviceType::Contact => "contact",
    }
}

fn bounded_device_type(params: &serde_json::Value) -> Option<String> {
    match params.get("device_type")?.as_str()?.trim() {
        value @ ("light" | "button" | "motion" | "contact") => Some(value.to_string()),
        _ => None,
    }
}

fn canonical_advertised_profile_id(
    capabilities: &[crate::hub::HubIntegrationCapability],
    hub_type: &str,
    profile_id: &str,
) -> Option<String> {
    capabilities
        .iter()
        .filter(|capability| capability.hub_type == hub_type)
        .flat_map(|capability| &capability.device_profiles)
        .find_map(|profile| profile.canonical_id_for(profile_id).map(str::to_string))
}

/// Build a history entry for a pair attempt from its request params + session.
pub fn pairing_history_entry_for_pair(
    hub_type: &str,
    params: &serde_json::Value,
    session: &PairingSession,
) -> PairingHistoryEntry {
    let completed_devices = if session.devices.is_empty() {
        session.device.iter().collect::<Vec<_>>()
    } else {
        session.devices.iter().collect::<Vec<_>>()
    };
    PairingHistoryEntry {
        at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        epoch_ms: crate::state::current_epoch_ms(),
        kind: "pair".to_string(),
        hub_type: hub_type.to_string(),
        correlation_id: bounded_correlation_id(params),
        device_type: completed_devices
            .first()
            .map(|device| device_type_label(&device.device_type).to_string())
            .or_else(|| bounded_device_type(params))
            .or_else(|| {
                (params
                    .get("device_kind")
                    .and_then(serde_json::Value::as_str)
                    == Some("button"))
                .then(|| "button".to_string())
            }),
        profile_id: bounded_profile_id(params),
        device_id: session
            .device
            .as_ref()
            .map(|device| device.device_id.clone()),
        force: None,
        rendezvous: param_str(params, "rendezvous"),
        network: param_str(params, "network"),
        status: status_label(&session.status),
        error: session.error.clone(),
        failure_stage: session.failure_stage,
        device: session
            .device
            .as_ref()
            .map(|device| format!("{} ({})", device.name, device.device_id)),
        devices: completed_devices
            .into_iter()
            .map(|device| format!("{} ({})", device.name, device.device_id))
            .collect(),
        warnings: session.warnings.clone(),
    }
}

/// Build a history entry for an unpair attempt from its params + result.
pub fn pairing_history_entry_for_unpair(
    hub_type: &str,
    params: &serde_json::Value,
    status: &PairingStatus,
    device_id: Option<&str>,
    error: Option<&str>,
) -> PairingHistoryEntry {
    PairingHistoryEntry {
        at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        epoch_ms: crate::state::current_epoch_ms(),
        kind: "unpair".to_string(),
        hub_type: hub_type.to_string(),
        correlation_id: bounded_correlation_id(params),
        device_type: bounded_device_type(params),
        profile_id: bounded_profile_id(params),
        device_id: device_id
            .map(str::to_string)
            .or_else(|| param_str(params, "device_id")),
        force: params.get("force").and_then(serde_json::Value::as_bool),
        rendezvous: None,
        network: None,
        status: status_label(status),
        error: error.map(str::to_string),
        failure_stage: None,
        device: None,
        devices: Vec::new(),
        warnings: Vec::new(),
    }
}

/// Append an entry to the persisted pairing history (load-modify-save).
///
/// Pairing events are rare and user-driven, so the read-modify-write cost is
/// irrelevant; keeping the history out of `AppState` avoids another
/// hydration path. IO runs outside the state lock.
pub fn record_pairing_history(state: &crate::state::SharedState, mut entry: PairingHistoryEntry) {
    let storage = {
        let Ok(s) = state.lock() else { return };
        // Request parameters are caller-controlled. Persist only an
        // advertised profile contract, canonicalizing a declared compatible
        // alias to the appliance's current decoder ID. A merely well-formed
        // value could still be a deliberately lowercased hardware identity.
        entry.profile_id = entry.profile_id.as_deref().and_then(|profile_id| {
            canonical_advertised_profile_id(&s.hub_capabilities, &entry.hub_type, profile_id)
        });
        s.storage.clone()
    };
    let Some(storage) = storage else { return };

    let Ok(_guard) = PAIRING_DOCUMENT_LOCK.get_or_init(|| Mutex::new(())).lock() else {
        log::warn!(target: "pair", "Pairing document lock is poisoned");
        return;
    };

    let mut history = match load_pairing_document(storage.as_ref()) {
        Ok(history) => history,
        Err(e) => {
            log::warn!(target: "pair", "Failed to load pairing history: {e}");
            return;
        }
    };
    if let Err(error) = prepare_writable_pairing_document(&mut history) {
        log::warn!(target: "pair", "Pairing history cannot be modified safely: {error:#}");
        return;
    }
    history.entries.push(entry);
    let history = history.normalized();
    if let Err(e) = storage.save_pairing_history(&history) {
        log::warn!(target: "pair", "Failed to save pairing history: {e}");
        return;
    }
    drop(_guard);
    crate::activity_cloud::enqueue_recent_activity_upload(state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AppState, SharedState};
    use std::sync::{Arc, Mutex};

    const TEST_PAIRING_HMAC_KEY: &str =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn request_context_deadline_is_anchored_before_handler_preamble() {
        let accepted_at = Instant::now() - Duration::from_millis(20);
        let context = PairingRequestContext::new(accepted_at);

        assert!(context.deadline_after(Duration::from_millis(10)) <= Instant::now());
    }

    #[test]
    fn pairing_failure_stage_is_additive_and_future_tolerant() {
        let without_stage: PairingSession = serde_json::from_value(serde_json::json!({
            "hub_type": "local_ble",
            "status": "failed",
            "error": "Pairing failed"
        }))
        .unwrap();
        assert_eq!(without_stage.failure_stage, None);

        let known: PairingSession = serde_json::from_value(serde_json::json!({
            "hub_type": "local_ble",
            "status": "failed",
            "error": "Pairing failed",
            "failure_stage": "candidate_connect"
        }))
        .unwrap();
        assert_eq!(
            known.failure_stage,
            Some(PairingFailureStage::CandidateConnect)
        );

        let future: PairingSession = serde_json::from_value(serde_json::json!({
            "hub_type": "local_ble",
            "status": "failed",
            "error": "Pairing failed",
            "failure_stage": "future_bounded_stage"
        }))
        .unwrap();
        assert_eq!(future.failure_stage, Some(PairingFailureStage::Unknown));
    }

    fn state_with_storage(path: &std::path::Path) -> SharedState {
        let mut app = AppState::default();
        app.storage = Some(Arc::new(
            crate::storage::FileStorage::new(path.to_str().unwrap()).unwrap(),
        ));
        Arc::new(Mutex::new(app))
    }

    fn test_directory(label: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "rhythm-pairing-{label}-{}-{}",
            std::process::id(),
            crate::state::current_epoch_ms()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn test_fingerprint(hub_type: &str) -> String {
        pairing_request_fingerprint(TEST_PAIRING_HMAC_KEY, hub_type, &serde_json::json!({}))
            .unwrap()
    }

    #[test]
    fn pair_entry_extracts_safe_params_and_redacts_nothing_sensitive() {
        let params = serde_json::json!({
            "setup_payload": "MT:SECRET",
            "network": "wifi",
            "rendezvous": "auto",
            "session_id": "abc",
        });
        let session = PairingSession {
            hub_type: "matter".to_string(),
            status: PairingStatus::Failed,
            device: None,
            devices: Vec::new(),
            error: Some("BLE timeout".to_string()),
            failure_stage: None,
            warnings: Vec::new(),
            details: None,
        };
        let entry = pairing_history_entry_for_pair("matter", &params, &session);
        assert_eq!(entry.kind, "pair");
        assert_eq!(entry.network.as_deref(), Some("wifi"));
        assert_eq!(entry.rendezvous.as_deref(), Some("auto"));
        assert_eq!(entry.status, "failed");
        assert_eq!(entry.error.as_deref(), Some("BLE timeout"));
        let json = serde_json::to_string(&entry).unwrap();
        assert!(
            !json.contains("SECRET"),
            "setup payload must never reach the history: {json}"
        );
    }

    #[test]
    fn pair_entry_keeps_only_a_bounded_profile_identifier() {
        let session = PairingSession {
            hub_type: "local_ble".to_string(),
            status: PairingStatus::Failed,
            device: None,
            devices: Vec::new(),
            error: Some("identity mismatch".to_string()),
            failure_stage: Some(PairingFailureStage::CandidateServiceMismatch),
            warnings: Vec::new(),
            details: None,
        };
        let params = serde_json::json!({
            "profile_id": "orein.oc02001.button.v1",
            "setup": {"ble_identity": "0A0B0C0D0E0F", "serial_metadata": "secret"}
        });
        let entry = pairing_history_entry_for_pair("local_ble", &params, &session);
        assert_eq!(entry.profile_id.as_deref(), Some("orein.oc02001.button.v1"));
        assert_eq!(
            entry.failure_stage,
            Some(PairingFailureStage::CandidateServiceMismatch)
        );
        let json = serde_json::to_string(&entry).unwrap();
        assert!(!json.contains("0A0B0C0D0E0F"));
        assert!(!json.contains("secret"));

        let invalid = pairing_history_entry_for_pair(
            "local_ble",
            &serde_json::json!({"profile_id": "RAW IDENTITY/NOT SAFE"}),
            &session,
        );
        assert_eq!(invalid.profile_id, None);
        let lowercased_identity = pairing_history_entry_for_pair(
            "local_ble",
            &serde_json::json!({"profile_id": "0a0b0c0d0e0f"}),
            &session,
        );
        assert_eq!(lowercased_identity.profile_id, None);

        let hue_button = pairing_history_entry_for_pair(
            "hue",
            &serde_json::json!({
                "device_kind": "button",
                "correlation_id": "hue-button-journey"
            }),
            &session,
        );
        assert_eq!(hue_button.device_type.as_deref(), Some("button"));
        assert_eq!(
            hue_button.correlation_id.as_deref(),
            Some("hue-button-journey")
        );
        let unsafe_correlation = pairing_history_entry_for_pair(
            "hue",
            &serde_json::json!({
                "device_kind": "button",
                "correlation_id": "contains/unsafe/path"
            }),
            &session,
        );
        assert_eq!(unsafe_correlation.correlation_id, None);
    }

    #[test]
    fn only_an_advertised_profile_identifier_is_eligible_for_persistence() {
        let capabilities = vec![crate::hub::HubIntegrationCapability {
            hub_type: "local_ble".to_string(),
            configurable: false,
            device_onboarding_methods: vec!["local_ble_qr".to_string()],
            device_profiles: vec![crate::hub::HubDeviceProfileCapability {
                id: "orein.oc02001.button.v2".to_string(),
                compatible_profile_ids: vec!["orein.oc02001.button.v1".to_string()],
                device_type: "button".to_string(),
                display_name: "Button".to_string(),
                input_only: true,
                onboarding_methods: vec!["local_ble_qr".to_string()],
            }],
            supports_unpairing: true,
            unpairable_device_types: vec!["button".to_string()],
            supports_roomless_devices: true,
            blocks_room_readiness: false,
        }];

        assert_eq!(
            canonical_advertised_profile_id(&capabilities, "local_ble", "orein.oc02001.button.v2",)
                .as_deref(),
            Some("orein.oc02001.button.v2")
        );
        assert_eq!(
            canonical_advertised_profile_id(&capabilities, "local_ble", "orein.oc02001.button.v1",)
                .as_deref(),
            Some("orein.oc02001.button.v2")
        );
        assert!(canonical_advertised_profile_id(
            &capabilities,
            "local_ble",
            "identity.abcdef123456.device.v1",
        )
        .is_none());
        assert!(canonical_advertised_profile_id(
            &capabilities,
            "matter",
            "orein.oc02001.button.v1",
        )
        .is_none());
    }

    #[test]
    fn batch_pairing_serializes_all_devices_and_legacy_first_device() {
        let first = PairedDeviceInfo {
            device_id: "hue-ble-1".to_string(),
            name: "Hue 1".to_string(),
            device_type: DeviceType::Light,
            manufacturer: Some("Signify".to_string()),
            model: Some("LCA013".to_string()),
        };
        let second = PairedDeviceInfo {
            device_id: "hue-ble-2".to_string(),
            name: "Hue 2".to_string(),
            device_type: DeviceType::Light,
            manufacturer: Some("Signify".to_string()),
            model: Some("LWA003".to_string()),
        };
        let session = PairingSession {
            hub_type: "hue_ble".to_string(),
            status: PairingStatus::Complete,
            device: Some(first.clone()),
            devices: vec![first.clone(), second],
            error: None,
            failure_stage: None,
            warnings: vec!["One candidate was out of range".to_string()],
            details: None,
        };

        let entry = pairing_history_entry_for_pair("hue_ble", &serde_json::json!({}), &session);
        assert_eq!(entry.device.as_deref(), Some("Hue 1 (hue-ble-1)"));
        assert_eq!(
            entry.devices,
            vec![
                "Hue 1 (hue-ble-1)".to_string(),
                "Hue 2 (hue-ble-2)".to_string()
            ]
        );
        assert_eq!(entry.warnings, vec!["One candidate was out of range"]);

        let json = serde_json::to_value(session).unwrap();
        assert_eq!(json["device"]["device_id"], first.device_id);
        assert_eq!(json["devices"].as_array().unwrap().len(), 2);
        assert_eq!(json["warnings"][0], "One candidate was out of range");
    }

    #[test]
    fn legacy_pairing_session_defaults_batch_devices_and_warnings() {
        let session: PairingSession = serde_json::from_value(serde_json::json!({
            "hub_type": "matter",
            "status": "complete",
            "device": {
                "device_id": "matter-100",
                "name": "Legacy bulb",
                "device_type": "light"
            }
        }))
        .unwrap();

        assert_eq!(
            session
                .device
                .as_ref()
                .map(|device| device.device_id.as_str()),
            Some("matter-100")
        );
        assert!(session.devices.is_empty());
        assert!(session.warnings.is_empty());
    }

    #[test]
    fn unpair_entry_captures_force_flag_and_device_id() {
        let params = serde_json::json!({
            "device_id": "matter-102",
            "device_type": "button",
            "correlation_id": "hue-remove-journey",
            "force": true
        });
        let entry = pairing_history_entry_for_unpair(
            "matter",
            &params,
            &PairingStatus::Complete,
            None,
            None,
        );
        assert_eq!(entry.kind, "unpair");
        assert_eq!(entry.device_id.as_deref(), Some("matter-102"));
        assert_eq!(entry.force, Some(true));
        assert_eq!(entry.status, "complete");
        assert_eq!(entry.device_type.as_deref(), Some("button"));
        assert_eq!(entry.correlation_id.as_deref(), Some("hue-remove-journey"));
    }

    #[test]
    fn unpair_result_round_trips_exact_hub_address_and_defaults_legacy_payloads() {
        let result = UnpairingResult {
            hub_type: "hue".to_string(),
            hub_address: Some("192.0.2.10".to_string()),
            status: PairingStatus::Complete,
            device_id: Some("hue-device-a".to_string()),
            error: None,
            completion_scope: Some(UnpairingCompletionScope::DeviceReleased),
            warning: None,
        };
        let json = serde_json::to_value(result).unwrap();
        assert_eq!(json["hub_address"], "192.0.2.10");
        assert_eq!(json["completion_scope"], "device_released");

        let hue_ble_result = UnpairingResult {
            hub_type: "hue_ble".to_string(),
            hub_address: Some("local".to_string()),
            status: PairingStatus::Complete,
            device_id: Some("hue-ble-001788010f76565a".to_string()),
            error: None,
            completion_scope: Some(UnpairingCompletionScope::LocalBondRemoved),
            warning: Some("The bulb's replacement-pairing window may be brief.".to_string()),
        };
        let hue_ble_json = serde_json::to_value(hue_ble_result).unwrap();
        assert_eq!(hue_ble_json["completion_scope"], "local_bond_removed");

        let legacy: UnpairingResult = serde_json::from_value(serde_json::json!({
            "hub_type": "matter",
            "status": "complete",
            "device_id": "matter-100"
        }))
        .unwrap();
        assert_eq!(legacy.hub_address, None);
        assert_eq!(legacy.completion_scope, None);
        assert_eq!(legacy.warning, None);
    }

    #[test]
    fn history_normalization_sorts_by_time_and_caps_length() {
        let entry = |epoch_ms: u64| PairingHistoryEntry {
            at: String::new(),
            epoch_ms,
            kind: "pair".to_string(),
            hub_type: "matter".to_string(),
            correlation_id: None,
            device_type: None,
            profile_id: None,
            device_id: None,
            force: None,
            rendezvous: None,
            network: None,
            status: "complete".to_string(),
            error: None,
            failure_stage: None,
            device: None,
            devices: Vec::new(),
            warnings: Vec::new(),
        };
        let mut entries: Vec<_> = (0..(PAIRING_HISTORY_LIMIT as u64 + 10))
            .map(entry)
            .collect();
        entries.reverse();
        let history = PairingHistory {
            schema_version: PAIRING_HISTORY_SCHEMA_VERSION,
            entries,
            pairing_results: Vec::new(),
        }
        .normalized();
        assert_eq!(history.entries.len(), PAIRING_HISTORY_LIMIT);
        assert_eq!(history.entries.first().unwrap().epoch_ms, 10);
        assert_eq!(
            history.entries.last().unwrap().epoch_ms,
            PAIRING_HISTORY_LIMIT as u64 + 9,
            "newest entries must survive the cap"
        );
    }

    #[test]
    fn existing_v1_history_defaults_profile_id_without_being_relabelled() {
        let history: PairingHistory = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "entries": [{
                "at": "2026-01-01T00:00:00.000Z",
                "epoch_ms": 1,
                "kind": "pair",
                "hub_type": "matter",
                "status": "complete"
            }]
        }))
        .unwrap();
        let history = history.normalized();
        assert_eq!(history.schema_version, 1);
        assert_eq!(history.entries[0].profile_id, None);
        assert_eq!(history.entries[0].correlation_id, None);
        assert_eq!(history.entries[0].device_type, None);
        assert!(history.pairing_results.is_empty());
    }

    #[test]
    fn normalization_never_relabels_an_unknown_future_schema() {
        let history = PairingHistory {
            schema_version: 99,
            entries: Vec::new(),
            pairing_results: Vec::new(),
        }
        .normalized();

        assert_eq!(history.schema_version, 99);
    }

    #[test]
    fn oversized_durable_result_documents_fail_closed_before_normalization() {
        let pairing_results = (0..=PAIRING_RESULT_LIMIT)
            .map(|index| PairingResultRecord {
                session_id: format!("bounded-session-{index}"),
                hub_type: "local_ble".to_string(),
                request_fingerprint: test_fingerprint("local_ble"),
                updated_at_epoch_ms: index as u64,
                result: None,
            })
            .collect();
        let history = PairingHistory {
            schema_version: PAIRING_HISTORY_SCHEMA_VERSION,
            entries: Vec::new(),
            pairing_results,
        };

        assert!(validate_pairing_document(&history).is_err());
    }

    #[test]
    fn expired_terminals_release_capacity_without_evicting_pending_work() {
        let directory = test_directory("terminal-retention");
        let state = state_with_storage(&directory);
        let storage = state.lock().unwrap().storage.clone().unwrap();
        let mut pairing_results = (0..(PAIRING_RESULT_LIMIT - 1))
            .map(|index| PairingResultRecord {
                session_id: format!("expired-terminal-{index}"),
                hub_type: "local_ble".to_string(),
                request_fingerprint: test_fingerprint("local_ble"),
                updated_at_epoch_ms: 1,
                result: Some(PairingSession {
                    hub_type: "local_ble".to_string(),
                    status: PairingStatus::Failed,
                    device: None,
                    devices: Vec::new(),
                    error: Some("old failure".to_string()),
                    failure_stage: None,
                    warnings: Vec::new(),
                    details: None,
                }),
            })
            .collect::<Vec<_>>();
        pairing_results.push(PairingResultRecord {
            session_id: "retained-pending".to_string(),
            hub_type: "matter".to_string(),
            request_fingerprint: test_fingerprint("matter"),
            updated_at_epoch_ms: crate::state::current_epoch_ms(),
            result: None,
        });
        storage
            .save_pairing_history(&PairingHistory {
                schema_version: PAIRING_HISTORY_SCHEMA_VERSION,
                entries: Vec::new(),
                pairing_results,
            })
            .unwrap();

        let new_lease = match begin_pairing_result(
            &state,
            "new-after-retention",
            "local_ble",
            &test_fingerprint("local_ble"),
        )
        .unwrap()
        {
            BeginPairingResult::Started(lease) => lease,
            result => panic!("unexpected reservation result: {result:?}"),
        };
        assert!(lookup_pairing_result(&state, "retained-pending")
            .unwrap()
            .is_some());
        assert!(lookup_pairing_result(&state, "expired-terminal-0")
            .unwrap()
            .is_none());
        drop(new_lease);
        std::fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn activation_receipt_repair_prunes_expired_terminals_before_capacity_check() {
        let directory = test_directory("receipt-terminal-retention");
        let state = state_with_storage(&directory);
        let storage = state.lock().unwrap().storage.clone().unwrap();
        let pairing_results = (0..PAIRING_RESULT_LIMIT)
            .map(|index| PairingResultRecord {
                session_id: format!("expired-before-receipt-{index}"),
                hub_type: "local_ble".to_string(),
                request_fingerprint: test_fingerprint("local_ble"),
                updated_at_epoch_ms: 1,
                result: Some(PairingSession {
                    hub_type: "local_ble".to_string(),
                    status: PairingStatus::Failed,
                    device: None,
                    devices: Vec::new(),
                    error: Some("old failure".to_string()),
                    failure_stage: None,
                    warnings: Vec::new(),
                    details: None,
                }),
            })
            .collect();
        storage
            .save_pairing_history(&PairingHistory {
                schema_version: PAIRING_HISTORY_SCHEMA_VERSION,
                entries: Vec::new(),
                pairing_results,
            })
            .unwrap();
        let device = PairedDeviceInfo {
            device_id: "local-ble-repaired".to_string(),
            name: "Button".to_string(),
            device_type: DeviceType::Button,
            manufacturer: Some("Orein".to_string()),
            model: Some("OC02001".to_string()),
        };

        complete_pairing_result_with_fingerprint(
            &state,
            "receipt-after-retention",
            "local_ble",
            &test_fingerprint("local_ble"),
            &PairingSession {
                hub_type: "local_ble".to_string(),
                status: PairingStatus::Complete,
                device: Some(device.clone()),
                devices: vec![device],
                error: None,
                failure_stage: None,
                warnings: Vec::new(),
                details: None,
            },
        )
        .unwrap();

        let repaired = lookup_pairing_result(&state, "receipt-after-retention")
            .unwrap()
            .unwrap();
        assert_eq!(repaired.state, PairingResultState::Terminal);
        assert_eq!(repaired.result.unwrap().status, PairingStatus::Complete);
        assert!(lookup_pairing_result(&state, "expired-before-receipt-0")
            .unwrap()
            .is_none());
        std::fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn a_large_unknown_lookup_history_does_not_consume_result_capacity() {
        let directory = test_directory("tombstone-headroom");
        let state = state_with_storage(&directory);
        let storage = state.lock().unwrap().storage.clone().unwrap();
        let pairing_results = (0..512)
            .map(|index| PairingResultRecord {
                session_id: format!("unknown-lookup-{index}"),
                hub_type: PAIRING_TOMBSTONE_HUB_TYPE.to_string(),
                request_fingerprint: PAIRING_TOMBSTONE_FINGERPRINT.to_string(),
                updated_at_epoch_ms: crate::state::current_epoch_ms(),
                result: None,
            })
            .collect();
        storage
            .save_pairing_history(&PairingHistory {
                schema_version: PAIRING_HISTORY_SCHEMA_VERSION,
                entries: Vec::new(),
                pairing_results,
            })
            .unwrap();

        let lease = match begin_pairing_result(
            &state,
            "real-after-lookup-burst",
            "local_ble",
            &test_fingerprint("local_ble"),
        )
        .unwrap()
        {
            BeginPairingResult::Started(lease) => lease,
            result => panic!("unexpected reservation result: {result:?}"),
        };
        drop(lease);
        std::fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn session_ids_are_bounded_path_safe_correlation_tokens() {
        assert!(validate_pairing_session_id("device-pair_123.v1:retry").is_ok());
        assert!(validate_pairing_session_id("").is_err());
        assert!(validate_pairing_session_id("contains/slash").is_err());
        assert!(validate_pairing_session_id("contains setup payload").is_err());
        assert!(validate_pairing_session_id(&"x".repeat(PAIRING_SESSION_ID_MAX_LEN + 1)).is_err());
    }

    #[test]
    fn request_fingerprints_are_canonical_and_request_bound() {
        let first = pairing_request_fingerprint(
            TEST_PAIRING_HMAC_KEY,
            "local_ble",
            &serde_json::json!({"profile_id": "orein", "setup": {"b": 2, "a": 1}}),
        )
        .unwrap();
        let reordered = pairing_request_fingerprint(
            TEST_PAIRING_HMAC_KEY,
            "local_ble",
            &serde_json::json!({"setup": {"a": 1, "b": 2}, "profile_id": "orein"}),
        )
        .unwrap();
        let changed = pairing_request_fingerprint(
            TEST_PAIRING_HMAC_KEY,
            "local_ble",
            &serde_json::json!({"profile_id": "orein", "setup": {"a": 1, "b": 3}}),
        )
        .unwrap();

        assert_eq!(first, reordered);
        assert_ne!(first, changed);
        assert_ne!(
            first,
            pairing_request_fingerprint(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "local_ble",
                &serde_json::json!({
                    "profile_id": "orein",
                    "setup": {"a": 1, "b": 2}
                }),
            )
            .unwrap()
        );
        assert!(validate_pairing_request_fingerprint(&first).is_ok());

        let with_transport_fields = pairing_request_fingerprint(
            TEST_PAIRING_HMAC_KEY,
            "local_ble",
            &serde_json::json!({
                "profile_id": "orein",
                "setup": {"a": 1, "b": 2},
                "session_id": "legacy-duplicate",
                "request_fingerprint": "caller-controlled"
            }),
        )
        .unwrap();
        assert_eq!(first, with_transport_fields);
    }

    #[test]
    fn an_uncommitted_installation_key_cannot_admit_durable_pairing() {
        let mut app = AppState::default();
        app.pairing_hmac_key_durable = false;
        let state = Arc::new(Mutex::new(app));
        let error =
            pairing_request_fingerprint_for_state(&state, "local_ble", &serde_json::json!({}))
                .unwrap_err();
        assert!(error.to_string().contains("not durably available"));
    }

    #[test]
    fn unknown_lookup_durably_closes_the_session_before_a_later_post() {
        let directory = test_directory("causal-tombstone");
        let state = state_with_storage(&directory);
        assert!(
            lookup_or_tombstone_pairing_result(&state, "overtaken-session")
                .unwrap()
                .is_none()
        );

        let restarted = state_with_storage(&directory);
        assert!(matches!(
            begin_pairing_result(
                &restarted,
                "overtaken-session",
                "local_ble",
                &test_fingerprint("local_ble")
            )
            .unwrap(),
            BeginPairingResult::Cancelled
        ));
        std::fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn terminal_acknowledgement_releases_payload_but_forbids_session_reuse() {
        let directory = test_directory("acknowledge");
        let state = state_with_storage(&directory);
        let fingerprint = test_fingerprint("local_ble");
        let lease =
            match begin_pairing_result(&state, "acknowledged-session", "local_ble", &fingerprint)
                .unwrap()
            {
                BeginPairingResult::Started(lease) => lease,
                result => panic!("unexpected reservation result: {result:?}"),
            };
        assert_eq!(
            acknowledge_pairing_result(&state, "acknowledged-session").unwrap(),
            AcknowledgePairingResult::Pending
        );
        let device = PairedDeviceInfo {
            device_id: "local-ble-ack".to_string(),
            name: "Button".to_string(),
            device_type: DeviceType::Button,
            manufacturer: Some("Orein".to_string()),
            model: Some("OC02001".to_string()),
        };
        complete_pairing_result(
            &state,
            "acknowledged-session",
            "local_ble",
            &PairingSession {
                hub_type: "local_ble".to_string(),
                status: PairingStatus::Complete,
                device: Some(device.clone()),
                devices: vec![device],
                error: None,
                failure_stage: None,
                warnings: Vec::new(),
                details: None,
            },
        )
        .unwrap();
        drop(lease);
        assert_eq!(
            acknowledge_pairing_result(&state, "acknowledged-session").unwrap(),
            AcknowledgePairingResult::Acknowledged
        );
        assert!(lookup_pairing_result(&state, "acknowledged-session")
            .unwrap()
            .is_none());
        assert!(matches!(
            begin_pairing_result(&state, "acknowledged-session", "local_ble", &fingerprint)
                .unwrap(),
            BeginPairingResult::Cancelled
        ));
        std::fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn terminal_result_survives_restart_and_redacts_private_fields() {
        let directory = test_directory("restart");
        let state = state_with_storage(&directory);
        let fingerprint = test_fingerprint("local_ble");
        let lease = match begin_pairing_result(&state, "pair-restart-1", "local_ble", &fingerprint)
            .unwrap()
        {
            BeginPairingResult::Started(lease) => lease,
            result => panic!("unexpected reservation result: {result:?}"),
        };
        let session = PairingSession {
            hub_type: "local_ble".to_string(),
            status: PairingStatus::Complete,
            device: Some(PairedDeviceInfo {
                device_id: "local-ble-opaque".to_string(),
                name: "Button".to_string(),
                device_type: DeviceType::Button,
                manufacturer: Some("Orein".to_string()),
                model: Some("OC02001".to_string()),
            }),
            devices: Vec::new(),
            error: None,
            failure_stage: None,
            warnings: vec!["EA:84:C2:50:A8:65 needed a retry".to_string()],
            details: Some(serde_json::json!({
                "setup_payload": "MT:SECRET",
                "ble_identity": "0A0B0C0D0E0F"
            })),
        };
        complete_pairing_result(&state, "pair-restart-1", "local_ble", &session).unwrap();
        drop(lease);

        let restarted = state_with_storage(&directory);
        let status = lookup_pairing_result(&restarted, "pair-restart-1")
            .unwrap()
            .unwrap();
        assert_eq!(status.state, PairingResultState::Terminal);
        let result = status.result.unwrap();
        assert_eq!(result.status, PairingStatus::Complete);
        assert_eq!(result.details, None);
        assert_eq!(result.warnings, vec!["<redacted> needed a retry"]);

        let persisted = std::fs::read_to_string(directory.join("pairing_history.json")).unwrap();
        assert!(!persisted.contains("MT:SECRET"));
        assert!(!persisted.contains("0A0B0C0D0E0F"));
        assert!(!persisted.contains("EA:84:C2:50:A8:65"));
        std::fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn matter_terminal_result_keeps_only_allowlisted_recovery_action() {
        let session = PairingSession {
            hub_type: "matter".to_string(),
            status: PairingStatus::Complete,
            device: Some(PairedDeviceInfo {
                device_id: "matter-42".to_string(),
                name: "Light".to_string(),
                device_type: DeviceType::Light,
                manufacturer: None,
                model: None,
            }),
            devices: Vec::new(),
            error: None,
            failure_stage: None,
            warnings: Vec::new(),
            details: Some(serde_json::json!({
                "recovery_action": "existing_connection_recovered",
                "setup_payload": "MT:PAIRING-SECRET",
                "node_id": 42,
            })),
        };

        let result = sanitized_terminal_session_for_delivery(&session, "matter").unwrap();
        assert_eq!(
            result.details,
            Some(serde_json::json!({
                "recovery_action": "existing_connection_recovered",
            }))
        );
        let serialized = serde_json::to_string(&result).unwrap();
        assert!(!serialized.contains("PAIRING-SECRET"));
        assert!(!serialized.contains("node_id"));

        let mut unknown = session;
        unknown.details = Some(serde_json::json!({
            "recovery_action": "future_recovery_action",
        }));
        assert_eq!(
            sanitized_terminal_session_for_delivery(&unknown, "matter")
                .unwrap()
                .details,
            None
        );
    }

    #[test]
    fn public_hex_device_id_is_preserved_in_the_durable_result() {
        let directory = test_directory("hex-device-id");
        let state = state_with_storage(&directory);
        let fingerprint = test_fingerprint("local_ble");
        let lease =
            match begin_pairing_result(&state, "hex-device-session", "local_ble", &fingerprint)
                .unwrap()
            {
                BeginPairingResult::Started(lease) => lease,
                result => panic!("unexpected reservation result: {result:?}"),
            };
        let device = PairedDeviceInfo {
            device_id: "abcdef1234567890".to_string(),
            name: "Button".to_string(),
            device_type: DeviceType::Button,
            manufacturer: Some("Orein".to_string()),
            model: Some("OC02001".to_string()),
        };
        complete_pairing_result(
            &state,
            "hex-device-session",
            "local_ble",
            &PairingSession {
                hub_type: "local_ble".to_string(),
                status: PairingStatus::Complete,
                device: Some(device.clone()),
                devices: vec![device],
                error: None,
                failure_stage: None,
                warnings: Vec::new(),
                details: None,
            },
        )
        .unwrap();
        drop(lease);
        assert_eq!(
            lookup_pairing_result(&state, "hex-device-session")
                .unwrap()
                .unwrap()
                .result
                .unwrap()
                .device
                .unwrap()
                .device_id,
            "abcdef1234567890"
        );
        std::fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn oversized_terminal_output_is_rejected_before_sanitization() {
        let directory = test_directory("oversized-terminal");
        let state = state_with_storage(&directory);
        let fingerprint = test_fingerprint("local_ble");
        let lease = match begin_pairing_result(
            &state,
            "oversized-terminal-session",
            "local_ble",
            &fingerprint,
        )
        .unwrap()
        {
            BeginPairingResult::Started(lease) => lease,
            result => panic!("unexpected reservation result: {result:?}"),
        };
        let device = PairedDeviceInfo {
            device_id: "local-ble-bounded".to_string(),
            name: "Button".to_string(),
            device_type: DeviceType::Button,
            manufacturer: None,
            model: None,
        };
        let oversized_devices = PairingSession {
            hub_type: "local_ble".to_string(),
            status: PairingStatus::Complete,
            device: Some(device.clone()),
            devices: vec![device.clone(); 65],
            error: None,
            failure_stage: None,
            warnings: Vec::new(),
            details: None,
        };
        assert!(complete_pairing_result(
            &state,
            "oversized-terminal-session",
            "local_ble",
            &oversized_devices
        )
        .is_err());
        let oversized_warnings = PairingSession {
            hub_type: "local_ble".to_string(),
            status: PairingStatus::Failed,
            device: None,
            devices: Vec::new(),
            error: Some("failed".to_string()),
            failure_stage: None,
            warnings: vec!["warning".to_string(); 33],
            details: None,
        };
        assert!(complete_pairing_result(
            &state,
            "oversized-terminal-session",
            "local_ble",
            &oversized_warnings
        )
        .is_err());
        let complete_with_error = PairingSession {
            hub_type: "local_ble".to_string(),
            status: PairingStatus::Complete,
            device: Some(device.clone()),
            devices: vec![device.clone()],
            error: Some("inconsistent success".to_string()),
            failure_stage: None,
            warnings: Vec::new(),
            details: None,
        };
        assert!(complete_pairing_result(
            &state,
            "oversized-terminal-session",
            "local_ble",
            &complete_with_error
        )
        .is_err());
        let failed_with_success_fields = PairingSession {
            hub_type: "local_ble".to_string(),
            status: PairingStatus::Failed,
            device: Some(device.clone()),
            devices: vec![device],
            error: Some("failed".to_string()),
            failure_stage: None,
            warnings: vec!["partial result".to_string()],
            details: None,
        };
        assert!(complete_pairing_result(
            &state,
            "oversized-terminal-session",
            "local_ble",
            &failed_with_success_fields
        )
        .is_err());
        assert_eq!(
            lookup_pairing_result(&state, "oversized-terminal-session")
                .unwrap()
                .unwrap()
                .state,
            PairingResultState::Pending
        );
        drop(lease);
        std::fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn stale_pending_result_becomes_a_durable_interrupted_terminal() {
        let directory = test_directory("stale");
        let state = state_with_storage(&directory);
        let storage = state.lock().unwrap().storage.clone().unwrap();
        storage
            .save_pairing_history(&PairingHistory {
                schema_version: PAIRING_HISTORY_SCHEMA_VERSION,
                entries: Vec::new(),
                pairing_results: vec![PairingResultRecord {
                    session_id: "stale-local-pair".to_string(),
                    hub_type: "local_ble".to_string(),
                    request_fingerprint: test_fingerprint("local_ble"),
                    updated_at_epoch_ms: 1,
                    result: None,
                }],
            })
            .unwrap();

        let status = lookup_pairing_result(&state, "stale-local-pair")
            .unwrap()
            .unwrap();
        assert_eq!(status.state, PairingResultState::Terminal);
        assert_eq!(status.result.unwrap().status, PairingStatus::Failed);

        let restarted = state_with_storage(&directory);
        assert_eq!(
            lookup_pairing_result(&restarted, "stale-local-pair")
                .unwrap()
                .unwrap()
                .state,
            PairingResultState::Terminal
        );
        std::fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn an_active_operation_cannot_expire_and_first_terminal_result_is_immutable() {
        let directory = test_directory("active");
        let state = state_with_storage(&directory);
        let storage = state.lock().unwrap().storage.clone().unwrap();
        let fingerprint = test_fingerprint("local_ble");
        storage
            .save_pairing_history(&PairingHistory {
                schema_version: PAIRING_HISTORY_SCHEMA_VERSION,
                entries: Vec::new(),
                pairing_results: vec![PairingResultRecord {
                    session_id: "active-local-pair".to_string(),
                    hub_type: "local_ble".to_string(),
                    request_fingerprint: fingerprint.clone(),
                    updated_at_epoch_ms: 1,
                    result: None,
                }],
            })
            .unwrap();
        let lease = PairingResultLease::acquire("active-local-pair").unwrap();

        assert_eq!(
            lookup_pairing_result(&state, "active-local-pair")
                .unwrap()
                .unwrap()
                .state,
            PairingResultState::Pending
        );

        let complete = PairingSession {
            hub_type: "local_ble".to_string(),
            status: PairingStatus::Complete,
            device: Some(PairedDeviceInfo {
                device_id: "local-ble-opaque".to_string(),
                name: "Button".to_string(),
                device_type: DeviceType::Button,
                manufacturer: Some("Orein".to_string()),
                model: Some("OC02001".to_string()),
            }),
            devices: Vec::new(),
            error: None,
            failure_stage: None,
            warnings: Vec::new(),
            details: None,
        };
        complete_pairing_result_with_fingerprint(
            &state,
            "active-local-pair",
            "local_ble",
            &fingerprint,
            &complete,
        )
        .unwrap();
        let mut recovered = complete.clone();
        recovered
            .warnings
            .push("outbox replay had different transport context".to_string());
        reconcile_committed_pairing_success_with_fingerprint(
            &state,
            "active-local-pair",
            "local_ble",
            &fingerprint,
            &recovered,
        )
        .unwrap();
        let failed = PairingSession {
            hub_type: "local_ble".to_string(),
            status: PairingStatus::Failed,
            device: None,
            devices: Vec::new(),
            error: Some("late failure".to_string()),
            failure_stage: None,
            warnings: Vec::new(),
            details: None,
        };
        assert!(complete_pairing_result_with_fingerprint(
            &state,
            "active-local-pair",
            "local_ble",
            &fingerprint,
            &failed,
        )
        .is_err());
        let persisted = lookup_pairing_result(&state, "active-local-pair")
            .unwrap()
            .unwrap()
            .result
            .unwrap();
        assert_eq!(persisted.status, PairingStatus::Complete);
        assert!(persisted.warnings.is_empty());
        drop(lease);
        std::fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn semantically_corrupt_pairing_results_fail_closed() {
        let directory = test_directory("semantic-corruption");
        let state = state_with_storage(&directory);
        let storage = state.lock().unwrap().storage.clone().unwrap();
        storage
            .save_pairing_history(&PairingHistory {
                schema_version: PAIRING_HISTORY_SCHEMA_VERSION,
                entries: Vec::new(),
                pairing_results: vec![PairingResultRecord {
                    session_id: "corrupt-local-pair".to_string(),
                    hub_type: "local_ble".to_string(),
                    request_fingerprint: test_fingerprint("local_ble"),
                    updated_at_epoch_ms: 1,
                    result: Some(PairingSession {
                        hub_type: "local_ble".to_string(),
                        status: PairingStatus::Searching,
                        device: None,
                        devices: Vec::new(),
                        error: None,
                        failure_stage: None,
                        warnings: Vec::new(),
                        details: None,
                    }),
                }],
            })
            .unwrap();

        assert!(lookup_pairing_result(&state, "corrupt-local-pair").is_err());
        storage
            .save_pairing_history(&PairingHistory {
                schema_version: PAIRING_HISTORY_SCHEMA_VERSION,
                entries: Vec::new(),
                pairing_results: vec![PairingResultRecord {
                    session_id: "corrupt-empty-success".to_string(),
                    hub_type: "local_ble".to_string(),
                    request_fingerprint: test_fingerprint("local_ble"),
                    updated_at_epoch_ms: 1,
                    result: Some(PairingSession {
                        hub_type: "local_ble".to_string(),
                        status: PairingStatus::Complete,
                        device: None,
                        devices: Vec::new(),
                        error: None,
                        failure_stage: None,
                        warnings: Vec::new(),
                        details: None,
                    }),
                }],
            })
            .unwrap();
        assert!(lookup_pairing_result(&state, "corrupt-empty-success").is_err());
        std::fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn newer_pairing_document_is_read_only() {
        let directory = test_directory("future");
        let state = state_with_storage(&directory);
        let storage = state.lock().unwrap().storage.clone().unwrap();
        storage
            .save_pairing_history(&PairingHistory {
                schema_version: 99,
                entries: Vec::new(),
                pairing_results: Vec::new(),
            })
            .unwrap();
        assert!(begin_pairing_result(
            &state,
            "future-schema-pair",
            "local_ble",
            &test_fingerprint("local_ble")
        )
        .is_err());
        assert!(lookup_pairing_result(&state, "future-schema-pair").is_err());
        let persisted = storage.load_pairing_history().unwrap().unwrap();
        assert_eq!(persisted.schema_version, 99);
        assert!(persisted.pairing_results.is_empty());
        std::fs::remove_dir_all(directory).ok();
    }
}
