//! Typed Matter controller transport abstraction.
//!
//! The transport boundary is intentionally shaped around the light operations
//! Rhythm actually needs, not around raw cluster/TLV plumbing. Desktop builds
//! map this to the official Python CHIP controller APIs; embedded targets can
//! provide their own controller implementation later.

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Matter network type for commissioning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatterCommissioningNetwork {
    /// Matter-over-WiFi.
    Wifi,
}

/// Rendezvous method used during commissioning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatterCommissioningRendezvous {
    /// Let the backend choose the best supported flow for the device.
    Auto,
    /// Force BLE rendezvous.
    Ble,
    /// Force on-network rendezvous.
    OnNetwork,
}

/// Appliance Wi-Fi credentials used for Matter accessory commissioning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatterCommissioningWifiCredentials {
    pub ssid: String,
    pub password: String,
}

/// Shared typed commissioning request built by the orchestrator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatterCommissionRequest {
    /// Raw Matter setup payload. May be a QR payload (`MT:`) or a manual code.
    pub setup_payload: String,
    /// Matter node ID to assign on our fabric.
    pub node_id: u64,
    /// Matter network type being commissioned.
    pub network: MatterCommissioningNetwork,
    /// Rendezvous method used to reach the device.
    pub rendezvous: MatterCommissioningRendezvous,
    /// Stored appliance Wi-Fi credentials used during commissioning.
    pub wifi_credentials: MatterCommissioningWifiCredentials,
}

/// Platform-agnostic typed interface to a Matter light controller.
pub trait MatterTransport: Send + Sync {
    /// Commission a light and return the fully probed device description.
    fn commission_light(&self, request: &MatterCommissionRequest) -> Result<CommissionedDevice>;

    /// Remove a device from the local fabric.
    fn decommission_device(&self, node_id: u64, force: bool) -> Result<()>;

    /// List devices currently known to the controller.
    fn list_devices(&self) -> Result<Vec<MatterDeviceInfo>>;

    /// Probe a single node and return its typed light capabilities.
    fn probe_light(&self, node_id: u64) -> Result<CommissionedDevice>;

    /// Set the On/Off state of a light endpoint.
    fn set_on_off(&self, node_id: u64, endpoint: u16, on: bool) -> Result<()>;

    /// Set a light level using Matter's 0-254 level encoding.
    fn set_brightness(
        &self,
        node_id: u64,
        endpoint: u16,
        level: u8,
        transition_ms: Option<u32>,
    ) -> Result<()>;

    /// Set a color temperature in Kelvin.
    fn set_color_temperature(
        &self,
        node_id: u64,
        endpoint: u16,
        kelvin: u16,
        transition_ms: Option<u32>,
    ) -> Result<()>;

    /// Set a CIE xy color.
    fn set_xy(
        &self,
        node_id: u64,
        endpoint: u16,
        x: f32,
        y: f32,
        transition_ms: Option<u32>,
    ) -> Result<()>;

    /// Read the On/Off state from a light endpoint.
    fn read_on_off(&self, node_id: u64, endpoint: u16) -> Result<bool>;
}

/// A light that has been commissioned into the local Matter fabric.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommissionedDevice {
    /// Matter node ID assigned during commissioning.
    pub node_id: u64,
    /// Device vendor name (from Basic Information cluster).
    pub vendor_name: String,
    /// Device product name (from Basic Information cluster).
    pub product_name: String,
    /// Vendor ID.
    pub vendor_id: u16,
    /// Product ID.
    pub product_id: u16,
    /// Serial number (if reported by device).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial_number: Option<String>,
    /// Light endpoint (typically 1).
    pub light_endpoint: u16,
    /// Supported color modes discovered during probing.
    pub color_modes: Vec<MatterColorMode>,
    /// Color temperature range in Kelvin (if CT supported).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_kelvin: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_kelvin: Option<u16>,
}

/// Matter color modes (from the Color Control cluster).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatterColorMode {
    HueSaturation,
    Xy,
    ColorTemperature,
}

/// Basic device info returned by `MatterTransport::list_devices()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatterDeviceInfo {
    pub node_id: u64,
    pub vendor_name: String,
    pub product_name: String,
    pub reachable: bool,
}
