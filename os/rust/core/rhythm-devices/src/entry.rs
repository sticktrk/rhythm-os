//! Device database entry — one per known device model.

#[cfg(feature = "serde")]
use serde::{Deserialize, Deserializer, Serialize};

use crate::capabilities::{ColorMode, LightCapabilities, LightType};
use crate::gamut::GamutTriangle;
use crate::quirks::{HueApiData, ZigbeeDeviceData};

/// A device entry from the device database.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct DeviceEntry {
    /// Device manufacturer (e.g., "Signify Netherlands B.V.").
    pub manufacturer: String,

    /// Device model identifier (e.g., "LCT016").
    pub model: String,

    /// Human-readable device name (e.g., "Hue White and Color Ambiance A19/E26").
    pub name: String,

    /// Light type classification.
    pub light_type: LightType,

    /// Supported color modes, best to worst.
    pub color_modes: Vec<ColorMode>,

    /// Minimum color temperature in Kelvin.
    #[cfg_attr(feature = "serde", serde(default))]
    pub min_kelvin: Option<u16>,

    /// Maximum color temperature in Kelvin.
    #[cfg_attr(feature = "serde", serde(default))]
    pub max_kelvin: Option<u16>,

    /// Named gamut identifier (e.g., "A", "B", "C") or explicit triangle.
    #[cfg_attr(
        feature = "serde",
        serde(default, deserialize_with = "deserialize_gamut")
    )]
    pub gamut: Option<GamutTriangle>,

    /// Minimum brightness (1-100).
    #[cfg_attr(feature = "serde", serde(default))]
    pub min_brightness: Option<u8>,

    /// Whether transitions/dynamics are supported.
    #[cfg_attr(feature = "serde", serde(default = "default_true"))]
    pub supports_transition: bool,

    /// Alternate model identifiers (Zigbee model strings, EAN codes, etc.).
    #[cfg_attr(feature = "serde", serde(default))]
    pub aliases: Vec<String>,

    /// Zigbee-specific metadata.
    #[cfg_attr(feature = "serde", serde(default))]
    pub zigbee: Option<ZigbeeDeviceData>,

    /// Hue V2 API specific metadata.
    #[cfg_attr(feature = "serde", serde(default))]
    pub hue_api: Option<HueApiData>,
}

fn default_true() -> bool {
    true
}

impl DeviceEntry {
    /// Build a `LightCapabilities` from this entry.
    pub fn capabilities(&self) -> LightCapabilities {
        LightCapabilities {
            light_type: self.light_type,
            color_modes: self.color_modes.clone(),
            min_kelvin: self.min_kelvin,
            max_kelvin: self.max_kelvin,
            gamut: self.gamut.clone(),
            min_brightness: self.min_brightness,
            supports_transition: self.supports_transition,
        }
    }
}

/// Deserialize gamut from either a string name ("A", "B", "C") or an explicit triangle object.
#[cfg(feature = "serde")]
fn deserialize_gamut<'de, D>(deserializer: D) -> Result<Option<GamutTriangle>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;

    let value: Option<serde_json::Value> = Option::deserialize(deserializer)?;
    match value {
        None => Ok(None),
        Some(serde_json::Value::String(name)) => {
            crate::gamut::named_gamut(&name)
                .ok_or_else(|| D::Error::custom(format!("unknown gamut name: {name}")))
                .map(Some)
        }
        Some(obj @ serde_json::Value::Object(_)) => {
            serde_json::from_value(obj)
                .map(Some)
                .map_err(D::Error::custom)
        }
        Some(other) => Err(D::Error::custom(format!(
            "gamut must be a string name or object, got: {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "serde")]
    #[test]
    fn test_entry_serde_named_gamut() {
        let json = r#"{
            "manufacturer": "Signify",
            "model": "LCT016",
            "name": "Hue White and Color Ambiance",
            "light_type": "extended_color",
            "color_modes": ["xy", "color_temperature"],
            "min_kelvin": 2000,
            "max_kelvin": 6500,
            "gamut": "C",
            "supports_transition": true,
            "aliases": []
        }"#;
        let entry: DeviceEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.gamut, Some(crate::gamut::gamut_c()));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_entry_serde_no_gamut() {
        let json = r#"{
            "manufacturer": "IKEA",
            "model": "LED1545G12",
            "name": "TRADFRI bulb",
            "light_type": "color_temperature",
            "color_modes": ["color_temperature"],
            "min_kelvin": 2200,
            "max_kelvin": 4000
        }"#;
        let entry: DeviceEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.gamut, None);
        assert!(entry.supports_transition); // default true
    }

    #[test]
    fn test_capabilities_from_entry() {
        let entry = DeviceEntry {
            manufacturer: "Test".to_string(),
            model: "T001".to_string(),
            name: "Test Light".to_string(),
            light_type: LightType::ExtendedColor,
            color_modes: vec![ColorMode::Xy, ColorMode::ColorTemperature],
            min_kelvin: Some(2000),
            max_kelvin: Some(6500),
            gamut: Some(crate::gamut::gamut_c()),
            min_brightness: Some(2),
            supports_transition: true,
            aliases: vec![],
            zigbee: None,
            hue_api: None,
        };

        let caps = entry.capabilities();
        assert!(caps.supports_xy_color());
        assert!(caps.supports_color_temp());
        assert_eq!(caps.min_kelvin, Some(2000));
    }
}
