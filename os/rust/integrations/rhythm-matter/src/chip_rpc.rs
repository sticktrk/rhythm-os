//! Local RPC protocol shared by the Rust Matter transport and the native CHIP daemon.

use anyhow::Result;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::transport::{CommissionedDevice, MatterCommissionRequest, MatterDeviceInfo};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChipInitControllerRequest {
    pub fabric_id: String,
    pub storage_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ble_controller: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChipInitControllerResponse {
    pub fabric_id: String,
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
    SetBrightness {
        node_id: u64,
        endpoint: u16,
        level: u8,
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
