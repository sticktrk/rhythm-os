//! Device pairing types for direct-connection protocols.
//!
//! Hub-based integrations (Hue bridge, Home Assistant) discover pre-paired
//! devices. Direct protocols (Matter, Zigbee) need an explicit pairing flow:
//! commissioning for Matter, permit-join for Zigbee.

use rhythm_core::runtime::hub_registry::DeviceType;
use serde::{Deserialize, Serialize};

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
    /// Zigbee: `{ "duration_secs": 60 }`
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

/// Information about a successfully paired device.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingSession {
    /// Which integration is handling this pairing.
    pub hub_type: String,
    /// Current status.
    pub status: PairingStatus,
    /// Device info (populated on completion).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<PairedDeviceInfo>,
    /// Error message (populated on failure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
    crate::state::emit_server_event(
        state,
        crate::server_event::ServerEvent::PairingProgress {
            hub_type: hub_type.to_string(),
            session_id: session_id.map(str::to_string),
            status,
            stage,
            message: message.into(),
            device,
            error,
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
    #[serde(default)]
    pub params: serde_json::Value,
}

/// Result of an unpairing operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnpairingResult {
    /// Which integration handled this unpairing.
    pub hub_type: String,
    /// Outcome status (Complete or Failed).
    pub status: PairingStatus,
    /// Device ID that was unpaired (populated on completion).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    /// Error message (populated on failure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Pairing history (persisted audit trail)
// ---------------------------------------------------------------------------

/// Cap on persisted pairing-history entries. Pairing is user-driven and
/// rare, so 200 entries covers months while keeping the file tiny.
pub const PAIRING_HISTORY_LIMIT: usize = 200;
pub const PAIRING_HISTORY_SCHEMA_VERSION: u32 = 1;

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
    /// Paired device summary, e.g. "Leedarson Smart RGBTW Bulb (matter-106)".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
}

/// Persisted document shape (`pairing_history.json`), oldest entry first.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingHistory {
    pub schema_version: u32,
    #[serde(default)]
    pub entries: Vec<PairingHistoryEntry>,
}

impl PairingHistory {
    pub fn normalized(mut self) -> Self {
        self.entries.sort_by_key(|entry| entry.epoch_ms);
        if self.entries.len() > PAIRING_HISTORY_LIMIT {
            let excess = self.entries.len() - PAIRING_HISTORY_LIMIT;
            self.entries.drain(..excess);
        }
        self
    }
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

/// Build a history entry for a pair attempt from its request params + session.
pub fn pairing_history_entry_for_pair(
    hub_type: &str,
    params: &serde_json::Value,
    session: &PairingSession,
) -> PairingHistoryEntry {
    PairingHistoryEntry {
        at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        epoch_ms: crate::state::current_epoch_ms(),
        kind: "pair".to_string(),
        hub_type: hub_type.to_string(),
        device_id: session
            .device
            .as_ref()
            .map(|device| device.device_id.clone()),
        force: None,
        rendezvous: param_str(params, "rendezvous"),
        network: param_str(params, "network"),
        status: status_label(&session.status),
        error: session.error.clone(),
        device: session
            .device
            .as_ref()
            .map(|device| format!("{} ({})", device.name, device.device_id)),
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
        device_id: device_id
            .map(str::to_string)
            .or_else(|| param_str(params, "device_id")),
        force: params.get("force").and_then(serde_json::Value::as_bool),
        rendezvous: None,
        network: None,
        status: status_label(status),
        error: error.map(str::to_string),
        device: None,
    }
}

/// Append an entry to the persisted pairing history (load-modify-save).
///
/// Pairing events are rare and user-driven, so the read-modify-write cost is
/// irrelevant; keeping the history out of `AppState` avoids another
/// hydration path. IO runs outside the state lock.
pub fn record_pairing_history(state: &crate::state::SharedState, entry: PairingHistoryEntry) {
    let storage = {
        let Ok(s) = state.lock() else { return };
        s.storage.clone()
    };
    let Some(storage) = storage else { return };

    let mut history = match storage.load_pairing_history() {
        Ok(Some(history)) => history,
        Ok(None) => PairingHistory {
            schema_version: PAIRING_HISTORY_SCHEMA_VERSION,
            entries: Vec::new(),
        },
        Err(e) => {
            log::warn!(target: "pair", "Failed to load pairing history: {e}");
            return;
        }
    };
    history.entries.push(entry);
    let history = history.normalized();
    if let Err(e) = storage.save_pairing_history(&history) {
        log::warn!(target: "pair", "Failed to save pairing history: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            error: Some("BLE timeout".to_string()),
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
    fn unpair_entry_captures_force_flag_and_device_id() {
        let params = serde_json::json!({ "device_id": "matter-102", "force": true });
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
    }

    #[test]
    fn history_normalization_sorts_by_time_and_caps_length() {
        let entry = |epoch_ms: u64| PairingHistoryEntry {
            at: String::new(),
            epoch_ms,
            kind: "pair".to_string(),
            hub_type: "matter".to_string(),
            device_id: None,
            force: None,
            rendezvous: None,
            network: None,
            status: "complete".to_string(),
            error: None,
            device: None,
        };
        let mut entries: Vec<_> = (0..(PAIRING_HISTORY_LIMIT as u64 + 10)).map(entry).collect();
        entries.reverse();
        let history = PairingHistory {
            schema_version: PAIRING_HISTORY_SCHEMA_VERSION,
            entries,
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
}
