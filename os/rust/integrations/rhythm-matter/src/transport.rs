//! Typed Matter controller transport abstraction.
//!
//! The transport boundary is intentionally shaped around the light operations
//! Rhythm actually needs, not around raw cluster/TLV plumbing. Current
//! server-class builds map this to the official CHIP controller stack through
//! a native daemon; future board-specific targets can provide their own
//! controller implementation later.

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Default minimum interval for Matter attribute subscriptions.
pub const DEFAULT_SUBSCRIPTION_MIN_INTERVAL_SECS: u16 = 1;
/// Default maximum interval for Matter attribute subscriptions.
pub const DEFAULT_SUBSCRIPTION_MAX_INTERVAL_SECS: u16 = 60;

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

    /// Create/update a Matter group and add the requested member endpoints.
    fn configure_group(&self, group: &MatterGroup) -> Result<()> {
        let _ = group;
        anyhow::bail!("Matter groups are not supported by this transport")
    }

    /// Remove a Matter group from the local controller and member endpoints.
    fn remove_group(&self, group_id: u16, members: &[MatterGroupMember]) -> Result<()> {
        let _ = (group_id, members);
        anyhow::bail!("Matter groups are not supported by this transport")
    }

    /// Set the On/Off state of a Matter group.
    fn set_group_on_off(&self, group_id: u16, on: bool) -> Result<()> {
        let _ = (group_id, on);
        anyhow::bail!("Matter group On/Off is not supported by this transport")
    }

    /// Ask a Matter group to identify itself for the given number of seconds.
    fn identify_group(&self, group_id: u16, duration_secs: u16) -> Result<()> {
        let _ = (group_id, duration_secs);
        anyhow::bail!("Matter group Identify is not supported by this transport")
    }

    /// Set a Matter group level using Matter's 0-254 level encoding.
    fn set_group_brightness(
        &self,
        group_id: u16,
        level: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let _ = (group_id, level, transition_ms);
        anyhow::bail!("Matter group Level Control is not supported by this transport")
    }

    /// Set a Matter group color temperature in Kelvin.
    fn set_group_color_temperature(
        &self,
        group_id: u16,
        kelvin: u16,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let _ = (group_id, kelvin, transition_ms);
        anyhow::bail!("Matter group Color Control is not supported by this transport")
    }

    /// Set a Matter group CIE xy color.
    fn set_group_xy(
        &self,
        group_id: u16,
        x: f32,
        y: f32,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let _ = (group_id, x, y, transition_ms);
        anyhow::bail!("Matter group Color Control is not supported by this transport")
    }

    /// Set a Matter group hue/saturation color using Matter's 0-254 encoding.
    fn set_group_hue_saturation(
        &self,
        group_id: u16,
        hue: u8,
        saturation: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let _ = (group_id, hue, saturation, transition_ms);
        anyhow::bail!("Matter group Color Control is not supported by this transport")
    }

    /// Ask a light endpoint to identify itself for the given number of seconds.
    fn identify_light(&self, node_id: u64, endpoint: u16, duration_secs: u16) -> Result<()>;

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

    /// Set a hue/saturation color using Matter's 0-254 encoding.
    fn set_hue_saturation(
        &self,
        node_id: u64,
        endpoint: u16,
        hue: u8,
        saturation: u8,
        transition_ms: Option<u32>,
    ) -> Result<()>;

    /// Read the On/Off state from a light endpoint.
    fn read_on_off(&self, node_id: u64, endpoint: u16) -> Result<bool>;

    /// Subscribe to On/Off attribute reports for the given light endpoints.
    fn subscribe_on_off(
        &self,
        targets: &[MatterSubscriptionTarget],
        min_interval_secs: u16,
        max_interval_secs: u16,
    ) -> Result<()> {
        let _ = (targets, min_interval_secs, max_interval_secs);
        anyhow::bail!("Matter On/Off attribute subscriptions are not supported by this transport")
    }

    /// Drain queued attribute reports from a subscription-capable transport.
    fn drain_attribute_reports(&self) -> Result<Vec<MatterAttributeReport>> {
        Ok(Vec::new())
    }
}

/// A Matter endpoint whose attributes should be subscribed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatterSubscriptionTarget {
    pub node_id: u64,
    pub endpoint: u16,
}

/// A single Matter endpoint that belongs to a group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatterGroupMember {
    pub node_id: u64,
    pub endpoint: u16,
}

/// Matter group configuration known by the local controller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatterGroup {
    pub group_id: u16,
    pub name: String,
    pub members: Vec<MatterGroupMember>,
}

/// A typed Matter attribute value carried over the local transport.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum MatterAttributeValue {
    Bool(bool),
}

/// A raw attribute report from a Matter subscription.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatterAttributeReport {
    /// Source device node ID.
    pub node_id: u64,
    /// Endpoint the report came from.
    pub endpoint: u16,
    /// Cluster ID.
    pub cluster: u32,
    /// Attribute ID.
    pub attr_id: u32,
    /// Decoded attribute value.
    pub value: MatterAttributeValue,
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
