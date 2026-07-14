//! Local RPC protocol shared by the Rust Matter transport and the native CHIP daemon.

use std::fmt;

use anyhow::Result;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::transport::{
    CommissionedDevice, MatterAttributeReport, MatterCommandSubmission, MatterCommissionRequest,
    MatterControllerEventBatch, MatterControllerEventCursor, MatterDeviceInfo,
    MatterEndpointCommandPlan, MatterGroup, MatterGroupMember, MatterLevelCommandVariant,
    MatterLevelStepMode, MatterSubscriptionTarget,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChipInitControllerRequest {
    /// Rhythm-level fabric label from hub credentials.
    pub fabric_id: String,
    /// Matter operational fabric id used when minting the controller NOC.
    pub operational_fabric_id: u64,
    /// 16-byte Matter Identity Protection Key, hex encoded.
    pub ipk_hex: String,
    pub storage_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ble_controller: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChipInitControllerResponse {
    pub fabric_id: String,
    pub operational_fabric_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compressed_fabric_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum ChipRpcRequest {
    InitController(ChipInitControllerRequest),
    CommissionLight(MatterCommissionRequest),
    ListDevices,
    ProbeLight {
        node_id: u64,
    },
    ScanOperationalNode {
        node_id: u64,
        timeout_ms: u64,
    },
    DecommissionDevice {
        node_id: u64,
        force: bool,
    },
    SetOnOff {
        node_id: u64,
        endpoint: u16,
        on: bool,
    },
    ConfigureGroup {
        group: MatterGroup,
    },
    RemoveGroup {
        group_id: u16,
        members: Vec<MatterGroupMember>,
    },
    SetGroupOnOff {
        group_id: u16,
        on: bool,
    },
    IdentifyGroup {
        group_id: u16,
        duration_secs: u16,
    },
    SetGroupBrightness {
        group_id: u16,
        level: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transition_ms: Option<u32>,
    },
    SetGroupColorTemperature {
        group_id: u16,
        kelvin: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transition_ms: Option<u32>,
    },
    SetGroupXy {
        group_id: u16,
        x: f32,
        y: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transition_ms: Option<u32>,
    },
    SetGroupHueSaturation {
        group_id: u16,
        hue: u8,
        saturation: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transition_ms: Option<u32>,
    },
    IdentifyLight {
        node_id: u64,
        endpoint: u16,
        duration_secs: u16,
    },
    SetBrightness {
        node_id: u64,
        endpoint: u16,
        level: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transition_ms: Option<u32>,
    },
    RunLevelCommand {
        node_id: u64,
        endpoint: u16,
        command: MatterLevelCommandVariant,
        level_or_step: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step_mode: Option<MatterLevelStepMode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transition_ms: Option<u32>,
    },
    SetColorTemperature {
        node_id: u64,
        endpoint: u16,
        kelvin: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transition_ms: Option<u32>,
    },
    SetXy {
        node_id: u64,
        endpoint: u16,
        x: f32,
        y: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transition_ms: Option<u32>,
    },
    SetHueSaturation {
        node_id: u64,
        endpoint: u16,
        hue: u8,
        saturation: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transition_ms: Option<u32>,
    },
    ReadOnOff {
        node_id: u64,
        endpoint: u16,
    },
    ReadLightCapabilitySnapshot {
        node_id: u64,
        endpoint: u16,
    },
    ReadLightState {
        node_id: u64,
        endpoint: u16,
    },
    SubscribeOnOff {
        targets: Vec<MatterSubscriptionTarget>,
        min_interval_secs: u16,
        max_interval_secs: u16,
    },
    DrainAttributeReports,
    /// Accept complete endpoint plans for asynchronous controller-owned
    /// execution. The response confirms admission, not device delivery.
    SubmitEndpointPlans {
        plans: Vec<MatterEndpointCommandPlan>,
    },
    /// Long-poll the restart-aware controller event stream.
    WaitControllerEvents {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<MatterControllerEventCursor>,
        max_wait_ms: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChipRpcRequestEnvelope {
    pub id: u64,
    #[serde(flatten)]
    pub request: ChipRpcRequest,
}

pub const BLE_RECOVERY_RETRY_AFTER_MS: u64 = 3_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChipRpcErrorKind {
    #[default]
    Other,
    ControllerUninitialized,
    BleCommissioningStack,
    OperationalDiscovery,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChipRpcError {
    pub message: String,
    #[serde(default, skip_serializing_if = "chip_rpc_error_kind_is_other")]
    pub kind: ChipRpcErrorKind,
    #[serde(default, skip_serializing_if = "is_false")]
    pub recoverable: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub requires_restart: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

impl ChipRpcError {
    pub fn from_message(message: impl Into<String>) -> Self {
        let message = message.into();
        let lower = message.to_ascii_lowercase();

        if is_controller_uninitialized_message(&lower) {
            return Self {
                message,
                kind: ChipRpcErrorKind::ControllerUninitialized,
                recoverable: true,
                requires_restart: true,
                retry_after_ms: None,
            };
        }

        if is_ble_commissioning_stack_message(&lower) {
            return Self {
                message,
                kind: ChipRpcErrorKind::BleCommissioningStack,
                recoverable: true,
                requires_restart: true,
                retry_after_ms: Some(BLE_RECOVERY_RETRY_AFTER_MS),
            };
        }

        if is_operational_discovery_message(&lower) {
            return Self {
                message,
                kind: ChipRpcErrorKind::OperationalDiscovery,
                recoverable: true,
                requires_restart: true,
                retry_after_ms: Some(BLE_RECOVERY_RETRY_AFTER_MS),
            };
        }

        Self {
            message,
            kind: ChipRpcErrorKind::Other,
            recoverable: false,
            requires_restart: false,
            retry_after_ms: None,
        }
    }

    fn normalized(self) -> Self {
        if self.kind == ChipRpcErrorKind::Other
            && !self.recoverable
            && !self.requires_restart
            && self.retry_after_ms.is_none()
        {
            return Self::from_message(self.message);
        }
        self
    }

    pub fn is_controller_uninitialized(&self) -> bool {
        self.kind == ChipRpcErrorKind::ControllerUninitialized
    }

    pub fn is_recoverable_ble_commissioning_stack_failure(&self) -> bool {
        self.kind == ChipRpcErrorKind::BleCommissioningStack
            && self.recoverable
            && self.requires_restart
    }

    pub fn is_recoverable_operational_discovery_failure(&self) -> bool {
        self.kind == ChipRpcErrorKind::OperationalDiscovery
            && self.recoverable
            && self.requires_restart
    }
}

impl fmt::Display for ChipRpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ChipRpcError {}

fn chip_rpc_error_kind_is_other(kind: &ChipRpcErrorKind) -> bool {
    *kind == ChipRpcErrorKind::Other
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_controller_uninitialized_message(lower_message: &str) -> bool {
    lower_message.contains("incorrect state")
        || lower_message.contains("controller not initialized")
        || lower_message.contains("controller backend not initialized")
}

fn is_ble_commissioning_stack_message(lower_message: &str) -> bool {
    let has_ble_source = [
        "blemanagerimpl.cpp",
        "bluezendpoint.cpp",
        "bluezobjectmanager.cpp",
        "pasesession.cpp",
        "chipoble",
        "ble adapter unavailable",
        "d-bus system bus",
        "operation was cancelled",
    ]
    .iter()
    .any(|needle| lower_message.contains(needle));

    let has_recoverable_failure = [
        "chip error 0x00000032",
        "chip error 0x000000ac",
        "ble error 0x00000401",
        "timeout",
        "internal error",
        "connection refused",
        "unavailable",
        "operation was cancelled",
    ]
    .iter()
    .any(|needle| lower_message.contains(needle));

    has_ble_source && has_recoverable_failure
}

/// Operational discovery failed after commissioning reached the network stage:
/// the device is on the LAN but the controller's mDNS resolve timed out. Seen
/// when chipd's minimal-mDNS sockets go stale after a wlan0 address change
/// (issue #117) — every broadcast fails with `Address not available` until the
/// sidecar restarts, so the failure is unrecoverable without a restart.
fn is_operational_discovery_message(lower_message: &str) -> bool {
    let has_discovery_source = lower_message.contains("addressresolve")
        || lower_message.contains("operational discovery failed");

    let has_timeout =
        lower_message.contains("chip error 0x00000032") || lower_message.contains("timeout");

    has_discovery_source && has_timeout
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ChipRpcResponse {
    Ok {
        result: serde_json::Value,
    },
    Error {
        #[serde(flatten)]
        error: ChipRpcError,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChipRpcResponseEnvelope {
    pub id: u64,
    #[serde(flatten)]
    pub response: ChipRpcResponse,
}

impl ChipRpcResponseEnvelope {
    pub fn into_result<T: DeserializeOwned>(self) -> Result<T> {
        match self.response {
            ChipRpcResponse::Ok { result } => Ok(serde_json::from_value(result)?),
            ChipRpcResponse::Error { error } => Err(anyhow::Error::new(error.normalized())),
        }
    }

    pub fn ok<T: Serialize>(id: u64, result: T) -> Self {
        Self {
            id,
            response: ChipRpcResponse::Ok {
                result: serde_json::to_value(result).unwrap_or(serde_json::Value::Null),
            },
        }
    }

    pub fn error(id: u64, message: impl Into<String>) -> Self {
        Self {
            id,
            response: ChipRpcResponse::Error {
                error: ChipRpcError::from_message(message),
            },
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChipRpcEmpty {}

impl ChipRpcEmpty {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChipRpcReadOnOffResponse {
    pub on: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChipRpcJsonValueResponse {
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChipRpcAttributeReportsResponse {
    pub reports: Vec<MatterAttributeReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChipRpcSubmitEndpointPlansResponse {
    pub submissions: Vec<MatterCommandSubmission>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChipRpcControllerEventsResponse {
    pub batch: MatterControllerEventBatch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChipRpcListDevicesResponse {
    pub devices: Vec<MatterDeviceInfo>,
    /// Complete records from chipd's persisted device store. Optional for
    /// compatibility with older sidecars that returned only basic identity.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commissioned_devices: Vec<CommissionedDevice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChipRpcCommissionLightResponse {
    pub device: CommissionedDevice,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChipRpcProbeLightResponse {
    pub device: CommissionedDevice,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChipRpcOperationalDiscoveryResponse {
    pub node_id: u64,
    pub fabrics: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_devices_response_accepts_legacy_payload_without_full_records() {
        let response: ChipRpcListDevicesResponse = serde_json::from_str(
            r#"{"devices":[{"node_id":10,"vendor_name":"Vendor","product_name":"Lamp","reachable":true}]}"#,
        )
        .unwrap();

        assert_eq!(response.devices.len(), 1);
        assert!(response.commissioned_devices.is_empty());
    }

    #[test]
    fn rpc_error_classifies_ble_stack_failures() {
        for message in [
            "commissioning Matter light: src/platform/Linux/bluez/BluezEndpoint.cpp:623: CHIP Error 0x000000AC: Internal error",
            "commissioning Matter light: src/platform/Linux/BLEManagerImpl.cpp:887: CHIP Error 0x00000032: Timeout",
            "Disabling CHIPoBLE service due to error: src/platform/Linux/BLEManagerImpl.cpp:245: Ble Error 0x00000401: BLE adapter unavailable",
            "FAIL: Get D-Bus system bus: Could not connect: Connection refused",
            "commissioning Matter light: src/protocols/secure_channel/PASESession.cpp:310: CHIP Error 0x00000032: Timeout",
            "commissioning Matter light: src/platform/Linux/bluez/BluezObjectManager.cpp:118: CHIP Error 0x000000AC: Internal error",
            "commissioning Matter light: src/platform/Linux/bluez/BluezEndpoint.cpp:493: Operation was cancelled",
        ] {
            let error = ChipRpcError::from_message(message);

            assert_eq!(
                error.kind,
                ChipRpcErrorKind::BleCommissioningStack,
                "expected BLE stack classification for {message}"
            );
            assert!(error.recoverable);
            assert!(error.requires_restart);
            assert_eq!(error.retry_after_ms, Some(BLE_RECOVERY_RETRY_AFTER_MS));
        }
    }

    /// Regression for issue #117: after a wlan0 address change wedged chipd's
    /// mDNS sockets, commissioning failed at operational discovery with an
    /// AddressResolve timeout. The error was classified `Other`, so the
    /// sidecar was never restarted and every retry failed identically.
    #[test]
    fn rpc_error_classifies_operational_discovery_failures() {
        for message in [
            "commissioning Matter light: src/lib/address_resolve/AddressResolve_DefaultImpl.cpp:124: CHIP Error 0x00000032: Timeout",
            "OperationalSessionSetup[1:0000000000000066]: operational discovery failed: src/lib/address_resolve/AddressResolve_DefaultImpl.cpp:124: CHIP Error 0x00000032: Timeout",
        ] {
            let error = ChipRpcError::from_message(message);

            assert_eq!(
                error.kind,
                ChipRpcErrorKind::OperationalDiscovery,
                "expected operational-discovery classification for {message}"
            );
            assert!(error.recoverable);
            assert!(error.requires_restart);
            assert_eq!(error.retry_after_ms, Some(BLE_RECOVERY_RETRY_AFTER_MS));
        }
    }

    #[test]
    fn rpc_error_does_not_classify_generic_timeouts_as_ble_recovery() {
        let error =
            ChipRpcError::from_message("reading Matter on/off: CHIP Error 0x00000032: Timeout");

        assert_eq!(error.kind, ChipRpcErrorKind::Other);
        assert!(!error.recoverable);
        assert!(!error.requires_restart);
        assert_eq!(error.retry_after_ms, None);
    }

    #[test]
    fn rpc_error_classifies_controller_uninitialized_failures() {
        for message in [
            "Controller not initialized",
            "setting Matter on/off: native/chip_bridge.cc:1014: CHIP Error 0x00000003: Incorrect state",
            "CHIP controller backend not initialized",
        ] {
            let error = ChipRpcError::from_message(message);

            assert_eq!(
                error.kind,
                ChipRpcErrorKind::ControllerUninitialized,
                "expected uninitialized-controller classification for {message}"
            );
            assert!(error.recoverable);
            assert!(error.requires_restart);
            assert_eq!(error.retry_after_ms, None);
        }
    }

    #[test]
    fn legacy_string_error_responses_are_classified_on_decode() {
        for (json, expected_kind) in [
            (
                r#"{
            "id": 7,
            "status": "error",
            "message": "Controller not initialized"
        }"#,
                ChipRpcErrorKind::ControllerUninitialized,
            ),
            (
                r#"{
            "id": 8,
            "status": "error",
            "message": "FAIL: Get D-Bus system bus: Could not connect: Connection refused"
        }"#,
                ChipRpcErrorKind::BleCommissioningStack,
            ),
        ] {
            let envelope: ChipRpcResponseEnvelope = serde_json::from_str(json).unwrap();

            let error = envelope.into_result::<ChipRpcEmpty>().unwrap_err();
            let rpc_error = error.downcast_ref::<ChipRpcError>().unwrap();

            assert_eq!(rpc_error.kind, expected_kind);
            assert!(rpc_error.recoverable);
            assert!(rpc_error.requires_restart);
        }
    }

    #[test]
    fn typed_recoverable_rpc_errors_include_metadata_on_the_wire() {
        let envelope = ChipRpcResponseEnvelope::error(
            9,
            "commissioning Matter light: src/platform/Linux/bluez/BluezEndpoint.cpp:623: CHIP Error 0x000000AC: Internal error",
        );
        let json = serde_json::to_value(envelope).unwrap();

        assert_eq!(json["status"], "error");
        assert_eq!(json["kind"], "ble_commissioning_stack");
        assert_eq!(json["recoverable"], true);
        assert_eq!(json["requires_restart"], true);
        assert_eq!(json["retry_after_ms"], BLE_RECOVERY_RETRY_AFTER_MS);
    }

    #[test]
    fn typed_rpc_error_response_round_trips_recovery_metadata() {
        let json = r#"{
            "id": 11,
            "status": "error",
            "message": "commissioning failed",
            "kind": "ble_commissioning_stack",
            "recoverable": true,
            "requires_restart": true,
            "retry_after_ms": 125
        }"#;
        let envelope: ChipRpcResponseEnvelope = serde_json::from_str(json).unwrap();

        let error = envelope.into_result::<ChipRpcEmpty>().unwrap_err();
        let rpc_error = error.downcast_ref::<ChipRpcError>().unwrap();

        assert_eq!(rpc_error.message, "commissioning failed");
        assert_eq!(rpc_error.kind, ChipRpcErrorKind::BleCommissioningStack);
        assert!(rpc_error.recoverable);
        assert!(rpc_error.requires_restart);
        assert_eq!(rpc_error.retry_after_ms, Some(125));
    }

    #[test]
    fn ordinary_rpc_errors_keep_the_legacy_wire_shape() {
        let envelope = ChipRpcResponseEnvelope::error(3, "boom");
        let json = serde_json::to_value(envelope).unwrap();

        assert_eq!(json["status"], "error");
        assert_eq!(json["message"], "boom");
        assert!(json.get("kind").is_none());
        assert!(json.get("recoverable").is_none());
    }
}
