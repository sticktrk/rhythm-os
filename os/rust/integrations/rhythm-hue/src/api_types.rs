//! Hue V2 API response types.

use serde::{Deserialize, Serialize};

/// V2 API envelope: `{ "data": [...], "errors": [] }`
#[derive(Debug, Deserialize)]
pub struct HueV2Response<T> {
    pub data: Vec<T>,
    #[serde(default)]
    pub errors: Vec<serde_json::Value>,
}

/// A Hue V2 grouped_light resource (room's aggregate light state).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HueV2GroupedLight {
    pub id: String,
    #[serde(default)]
    pub on: Option<HueV2OnState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HueV2OnState {
    pub on: bool,
}
