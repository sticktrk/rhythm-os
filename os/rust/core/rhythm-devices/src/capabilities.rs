//! Light device capability types.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::gamut::GamutTriangle;

/// High-level classification of a light device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum LightType {
    /// Full CIE xy color + color temperature (e.g., Hue White and Color Ambiance).
    ExtendedColor,
    /// Color temperature only, no full RGB/XY (e.g., Hue White Ambiance).
    ColorTemperature,
    /// Brightness only (e.g., Hue White).
    Dimmable,
    /// On/off only (e.g., smart plug).
    OnOff,
}

/// A color control mode supported by a light.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ColorMode {
    /// Hue and saturation control.
    HueSaturation,
    /// Full CIE xy color gamut.
    Xy,
    /// Color temperature (mirek/kelvin range).
    ColorTemperature,
    /// Brightness only.
    Dimmable,
    /// On/off only.
    OnOff,
}

/// Normalized capabilities of a light device.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct LightCapabilities {
    /// Classification of this light.
    pub light_type: LightType,

    /// Supported color modes, best to worst.
    pub color_modes: Vec<ColorMode>,

    /// Minimum color temperature in Kelvin (None if not color-temp capable).
    #[cfg_attr(feature = "serde", serde(default))]
    pub min_kelvin: Option<u16>,

    /// Maximum color temperature in Kelvin.
    #[cfg_attr(feature = "serde", serde(default))]
    pub max_kelvin: Option<u16>,

    /// CIE xy color gamut triangle (None if not color capable).
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub gamut: Option<GamutTriangle>,

    /// Minimum brightness the light can physically produce (1-100).
    #[cfg_attr(feature = "serde", serde(default))]
    pub min_brightness: Option<u8>,

    /// Whether this light supports transitions/dynamics.
    #[cfg_attr(feature = "serde", serde(default = "default_true"))]
    pub supports_transition: bool,
}

fn default_true() -> bool {
    true
}

impl LightCapabilities {
    /// Default capabilities for a given light type.
    pub fn defaults_for(light_type: LightType) -> Self {
        match light_type {
            LightType::ExtendedColor => Self {
                light_type,
                color_modes: vec![ColorMode::Xy, ColorMode::ColorTemperature],
                min_kelvin: Some(2000),
                max_kelvin: Some(6500),
                gamut: None,
                min_brightness: None,
                supports_transition: true,
            },
            LightType::ColorTemperature => Self {
                light_type,
                color_modes: vec![ColorMode::ColorTemperature],
                min_kelvin: Some(2200),
                max_kelvin: Some(6500),
                gamut: None,
                min_brightness: None,
                supports_transition: true,
            },
            LightType::Dimmable => Self {
                light_type,
                color_modes: vec![ColorMode::Dimmable],
                min_kelvin: None,
                max_kelvin: None,
                gamut: None,
                min_brightness: None,
                supports_transition: true,
            },
            LightType::OnOff => Self {
                light_type,
                color_modes: vec![ColorMode::OnOff],
                min_kelvin: None,
                max_kelvin: None,
                gamut: None,
                min_brightness: None,
                supports_transition: false,
            },
        }
    }

    /// Whether this light supports color temperature control.
    pub fn supports_color_temp(&self) -> bool {
        self.color_modes.contains(&ColorMode::ColorTemperature)
    }

    /// Whether this light supports full xy color.
    pub fn supports_xy_color(&self) -> bool {
        self.color_modes.contains(&ColorMode::Xy)
    }

    /// Whether this light supports dimming.
    pub fn supports_dimming(&self) -> bool {
        !self.color_modes.contains(&ColorMode::OnOff) || self.color_modes.len() > 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults_extended_color() {
        let caps = LightCapabilities::defaults_for(LightType::ExtendedColor);
        assert!(caps.supports_xy_color());
        assert!(caps.supports_color_temp());
        assert!(caps.supports_dimming());
        assert!(caps.supports_transition);
        assert_eq!(caps.min_kelvin, Some(2000));
        assert_eq!(caps.max_kelvin, Some(6500));
    }

    #[test]
    fn test_defaults_color_temp() {
        let caps = LightCapabilities::defaults_for(LightType::ColorTemperature);
        assert!(!caps.supports_xy_color());
        assert!(caps.supports_color_temp());
        assert!(caps.supports_dimming());
    }

    #[test]
    fn test_defaults_dimmable() {
        let caps = LightCapabilities::defaults_for(LightType::Dimmable);
        assert!(!caps.supports_xy_color());
        assert!(!caps.supports_color_temp());
        assert!(caps.supports_dimming());
    }

    #[test]
    fn test_defaults_on_off() {
        let caps = LightCapabilities::defaults_for(LightType::OnOff);
        assert!(!caps.supports_xy_color());
        assert!(!caps.supports_color_temp());
        assert!(!caps.supports_dimming());
        assert!(!caps.supports_transition);
    }
}
