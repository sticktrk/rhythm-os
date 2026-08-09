//! Device quirks and protocol-specific metadata.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Known device quirks that affect command generation or behavior.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum DeviceQuirk {
    /// Device needs xy color commands instead of color_temperature/mirek.
    NeedsXyNotCt,
    /// Device needs hue/saturation commands instead of color temperature.
    NeedsHueSaturationNotCt,
    /// Device needs a delay between grouped_light commands.
    GroupedLightDelay,
    /// Device reports incorrect on/off state.
    UnreliableOnState,
    /// Device drops commands if sent faster than this interval (ms).
    CommandThrottleMs(u32),
    /// Device needs explicit on:true when changing brightness from off.
    NeedsExplicitOn,
    /// Max transition time the device supports (ms). Commands above this are ignored.
    MaxTransitionMs(u32),
    /// Other quirk (forward-compatible).
    Other(String),
}

/// Zigbee-specific device metadata.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ZigbeeDeviceData {
    /// Zigbee model identifier string (often matches the model field, but not always).
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub model_id: Option<String>,

    /// Manufacturer code (e.g., "0x100B" for Signify).
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub mfr_code: Option<String>,

    /// ZigBee endpoint for light control.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub endpoint: Option<u8>,

    /// Zigbee-specific quirks.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Vec::is_empty")
    )]
    pub quirks: Vec<DeviceQuirk>,
}

/// Hue V2 API specific device metadata.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct HueApiData {
    /// Delay needed between grouped_light commands for this device (ms).
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub grouped_light_delay_ms: Option<u32>,
}

/// Matter-specific device metadata.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct MatterDeviceData {
    /// Matter vendor ID (from Basic Information cluster).
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub vendor_id: Option<u16>,

    /// Matter product ID (from Basic Information cluster).
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub product_id: Option<u16>,

    /// Matter-specific quirks.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Vec::is_empty")
    )]
    pub quirks: Vec<DeviceQuirk>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quirk_equality() {
        assert_eq!(DeviceQuirk::NeedsXyNotCt, DeviceQuirk::NeedsXyNotCt);
        assert_ne!(DeviceQuirk::NeedsXyNotCt, DeviceQuirk::NeedsExplicitOn);
        assert_ne!(
            DeviceQuirk::NeedsXyNotCt,
            DeviceQuirk::NeedsHueSaturationNotCt
        );
        assert_eq!(
            DeviceQuirk::CommandThrottleMs(100),
            DeviceQuirk::CommandThrottleMs(100)
        );
        assert_ne!(
            DeviceQuirk::CommandThrottleMs(100),
            DeviceQuirk::CommandThrottleMs(200)
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_matter_data_serde() {
        let data = super::MatterDeviceData {
            vendor_id: Some(0x1384),
            product_id: Some(1),
            quirks: vec![],
        };
        let json = serde_json::to_string(&data).unwrap();
        let parsed: super::MatterDeviceData = serde_json::from_str(&json).unwrap();
        assert_eq!(data, parsed);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_matter_data_empty_serde() {
        let json = "{}";
        let parsed: super::MatterDeviceData = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.vendor_id, None);
        assert_eq!(parsed.product_id, None);
        assert!(parsed.quirks.is_empty());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_zigbee_data_serde() {
        let data = ZigbeeDeviceData {
            model_id: Some("LCT016".to_string()),
            mfr_code: Some("0x100B".to_string()),
            endpoint: Some(11),
            quirks: vec![DeviceQuirk::NeedsXyNotCt],
        };
        let json = serde_json::to_string(&data).unwrap();
        let parsed: ZigbeeDeviceData = serde_json::from_str(&json).unwrap();
        assert_eq!(data, parsed);
    }
}
