//! Local RPC protocol shared by the Rust Matter transport and the native CHIP daemon.

use anyhow::Result;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::transport::{
    CommissionedDevice, MatterAttributeReport, MatterCommissionRequest, MatterDeviceInfo,
    MatterGroup, MatterGroupMember, MatterLevelCommandVariant, MatterLevelStepMode,
    MatterSubscriptionTarget,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChipRpcRequestEnvelope {
    pub id: u64,
    #[serde(flatten)]
    pub request: ChipRpcRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ChipRpcResponse {
    Ok { result: serde_json::Value },
    Error { message: String },
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
            ChipRpcResponse::Error { message } => Err(anyhow::anyhow!(message)),
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
                message: message.into(),
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
pub struct ChipRpcListDevicesResponse {
    pub devices: Vec<MatterDeviceInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChipRpcCommissionLightResponse {
    pub device: CommissionedDevice,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChipRpcProbeLightResponse {
    pub device: CommissionedDevice,
}
