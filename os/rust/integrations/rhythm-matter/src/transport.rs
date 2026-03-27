//! Matter transport abstraction.
//!
//! Defines the `MatterTransport` trait that platform-specific crates implement
//! to provide Matter communication. On desktop this could use `matter-rs` or
//! shell out to `chip-tool`; on ESP32 it could use the ESP-IDF Matter SDK.

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Platform-agnostic interface to a Matter controller/commissioner.
///
/// Implementors provide the actual Matter stack. On desktop this uses
/// `matter-rs` or `chip-tool`; on ESP32 it could use ESP-IDF's Matter SDK.
pub trait MatterTransport: Send + Sync {
    /// Commission a new device using its setup code (from QR or manual entry).
    ///
    /// Performs the full PASE → CASE commissioning flow and adds the device
    /// to the local Matter fabric.
    fn commission(&self, setup_code: &str) -> Result<CommissionedDevice>;

    /// Send a cluster command to a specific device endpoint.
    ///
    /// # Arguments
    /// * `node_id` - Matter node ID of the target device
    /// * `endpoint` - Endpoint on the device (typically 1 for lights)
    /// * `cluster` - Cluster ID (e.g., 0x0006 for On/Off)
    /// * `cmd_id` - Command ID within the cluster
    /// * `payload` - TLV-encoded command payload
    fn send_cluster_cmd(
        &self,
        node_id: u64,
        endpoint: u16,
        cluster: u16,
        cmd_id: u8,
        payload: &[u8],
    ) -> Result<()>;

    /// Read an attribute from a device.
    ///
    /// # Arguments
    /// * `node_id` - Matter node ID
    /// * `endpoint` - Endpoint on the device
    /// * `cluster` - Cluster ID
    /// * `attr_id` - Attribute ID within the cluster
    fn read_attribute(
        &self,
        node_id: u64,
        endpoint: u16,
        cluster: u16,
        attr_id: u16,
    ) -> Result<Vec<u8>>;

    /// Subscribe to attribute change reports from a device.
    fn subscribe(&self, node_id: u64, specs: &[SubscribeSpec]) -> Result<()>;

    /// List all devices commissioned into the local fabric.
    fn commissioned_devices(&self) -> Result<Vec<MatterDeviceInfo>>;

    /// Test connectivity to a specific device.
    fn ping(&self, node_id: u64) -> Result<bool> {
        // Default: try reading the basic cluster's vendor name attribute
        self.read_attribute(node_id, 0, 0x0028, 1).map(|_| true)
    }
}

/// A device that has been commissioned into the local Matter fabric.
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
    /// Supported color modes discovered during commissioning.
    pub color_modes: Vec<MatterColorMode>,
    /// Color temperature range in Kelvin (if CT supported).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_kelvin: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_kelvin: Option<u16>,
}

/// Matter color modes (from Color Control cluster features).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatterColorMode {
    /// Hue/Saturation control.
    HueSaturation,
    /// CIE xy color.
    Xy,
    /// Color temperature in mireds.
    ColorTemperature,
}

/// Subscription specification for attribute reports.
#[derive(Debug, Clone)]
pub struct SubscribeSpec {
    /// Endpoint to subscribe on.
    pub endpoint: u16,
    /// Cluster containing the attribute.
    pub cluster: u16,
    /// Attribute to subscribe to.
    pub attr_id: u16,
    /// Minimum reporting interval in seconds.
    pub min_interval_secs: u16,
    /// Maximum reporting interval in seconds.
    pub max_interval_secs: u16,
}

/// Basic device info returned by `commissioned_devices()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatterDeviceInfo {
    /// Matter node ID.
    pub node_id: u64,
    /// Vendor name.
    pub vendor_name: String,
    /// Product name.
    pub product_name: String,
    /// Whether the device is currently reachable.
    pub reachable: bool,
}
