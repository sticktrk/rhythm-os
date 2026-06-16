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
