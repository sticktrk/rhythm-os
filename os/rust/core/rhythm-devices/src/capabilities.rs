//! Light device capability types.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::gamut::GamutTriangle;
use crate::ControlCorrections;

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
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub gamut: Option<GamutTriangle>,

    /// Minimum brightness the light can physically produce (1-100).
    #[cfg_attr(feature = "serde", serde(default))]
    pub min_brightness: Option<u8>,

    /// Whether this light supports transitions/dynamics.
    #[cfg_attr(feature = "serde", serde(default = "default_true"))]
    pub supports_transition: bool,

    /// Optional Hue-relative command corrections for this exact device model.
    #[cfg_attr(feature = "serde", serde(default))]
    pub control_corrections: ControlCorrections,
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
                control_corrections: ControlCorrections::default(),
            },
            LightType::ColorTemperature => Self {
                light_type,
                color_modes: vec![ColorMode::ColorTemperature],
                min_kelvin: Some(2200),
                max_kelvin: Some(6500),
                gamut: None,
                min_brightness: None,
                supports_transition: true,
                control_corrections: ControlCorrections::default(),
            },
            LightType::Dimmable => Self {
                light_type,
                color_modes: vec![ColorMode::Dimmable],
                min_kelvin: None,
                max_kelvin: None,
                gamut: None,
                min_brightness: None,
                supports_transition: true,
                control_corrections: ControlCorrections::default(),
            },
            LightType::OnOff => Self {
                light_type,
                color_modes: vec![ColorMode::OnOff],
                min_kelvin: None,
                max_kelvin: None,
                gamut: None,
                min_brightness: None,
                supports_transition: false,
                control_corrections: ControlCorrections::default(),
            },
        }
    }

    /// Build the common room-level capabilities for multiple light devices.
    ///
    /// The result is the least-capable command surface that can be safely sent
    /// to every device in the set. Unknown devices should be supplied with a
    /// sensible fallback (for example `defaults_for(ExtendedColor)`) before
    /// calling this helper.
    pub fn common_for<'a>(
        caps: impl IntoIterator<Item = &'a LightCapabilities>,
    ) -> Option<LightCapabilities> {
        let mut caps = caps.into_iter();
        let first = caps.next()?.clone();

        let mut supports_dimming = first.supports_dimming();
        let mut supports_color_temp = first.supports_color_temp();
        let mut supports_hue_saturation = first.supports_hue_saturation();
        let mut supports_xy = first.supports_xy_color();
        let mut min_kelvin = first.min_kelvin;
        let mut max_kelvin = first.max_kelvin;
        let mut gamut = first.gamut.clone();
        let mut min_brightness = first.min_brightness;
        let mut supports_transition = first.supports_transition;

        for cap in caps {
            supports_dimming &= cap.supports_dimming();
            supports_color_temp &= cap.supports_color_temp();
            supports_hue_saturation &= cap.supports_hue_saturation();
            supports_xy &= cap.supports_xy_color();
            supports_transition &= cap.supports_transition;
            min_brightness = max_option(min_brightness, cap.min_brightness);

            if supports_color_temp {
                min_kelvin = max_option(min_kelvin, cap.min_kelvin);
                max_kelvin = min_option(max_kelvin, cap.max_kelvin);
            } else {
                min_kelvin = None;
                max_kelvin = None;
            }

            if supports_xy {
                if gamut != cap.gamut {
                    gamut = None;
                }
            } else {
                gamut = None;
            }
        }

        if let (Some(min_k), Some(max_k)) = (min_kelvin, max_kelvin) {
            if min_k > max_k {
                supports_color_temp = false;
                min_kelvin = None;
                max_kelvin = None;
            }
        }

        if !supports_xy {
            gamut = None;
        }

        let light_type = if supports_xy || supports_hue_saturation {
            LightType::ExtendedColor
        } else if supports_color_temp {
            LightType::ColorTemperature
        } else if supports_dimming {
            LightType::Dimmable
        } else {
            LightType::OnOff
        };

        let color_modes = match light_type {
            LightType::ExtendedColor => {
                let mut modes = Vec::new();
                if supports_hue_saturation {
                    modes.push(ColorMode::HueSaturation);
                }
                if supports_xy {
                    modes.push(ColorMode::Xy);
                }
                if supports_color_temp {
                    modes.push(ColorMode::ColorTemperature);
                }
                modes
            }
            LightType::ColorTemperature => vec![ColorMode::ColorTemperature],
            LightType::Dimmable => vec![ColorMode::Dimmable],
            LightType::OnOff => vec![ColorMode::OnOff],
        };

        Some(Self {
            light_type,
            color_modes,
            min_kelvin,
            max_kelvin,
            gamut,
            min_brightness,
            supports_transition,
            // Room-level commands can span unlike devices. Per-device
            // corrections must be applied only after the command is fanned
            // out to an exact model.
            control_corrections: ControlCorrections::default(),
        })
    }

    /// Whether this light supports color temperature control.
    pub fn supports_color_temp(&self) -> bool {
        self.color_modes.contains(&ColorMode::ColorTemperature)
    }

    /// Whether this light supports hue/saturation color.
    pub fn supports_hue_saturation(&self) -> bool {
        self.color_modes.contains(&ColorMode::HueSaturation)
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

fn min_option(lhs: Option<u16>, rhs: Option<u16>) -> Option<u16> {
    match (lhs, rhs) {
        (Some(lhs), Some(rhs)) => Some(lhs.min(rhs)),
        (Some(lhs), None) => Some(lhs),
        (None, Some(rhs)) => Some(rhs),
        (None, None) => None,
    }
}

fn max_option<T: Ord>(lhs: Option<T>, rhs: Option<T>) -> Option<T> {
    match (lhs, rhs) {
        (Some(lhs), Some(rhs)) => Some(lhs.max(rhs)),
        (Some(lhs), None) => Some(lhs),
        (None, Some(rhs)) => Some(rhs),
        (None, None) => None,
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

    #[test]
    fn common_for_narrows_room_to_ct() {
        let extended = LightCapabilities {
            min_kelvin: Some(2000),
            max_kelvin: Some(6500),
            min_brightness: Some(2),
            ..LightCapabilities::defaults_for(LightType::ExtendedColor)
        };
        let ct = LightCapabilities {
            min_kelvin: Some(2700),
            max_kelvin: Some(5000),
            min_brightness: Some(5),
            ..LightCapabilities::defaults_for(LightType::ColorTemperature)
        };

        let room = LightCapabilities::common_for([&extended, &ct]).unwrap();

        assert_eq!(room.light_type, LightType::ColorTemperature);
        assert_eq!(room.color_modes, vec![ColorMode::ColorTemperature]);
        assert_eq!(room.min_kelvin, Some(2700));
        assert_eq!(room.max_kelvin, Some(5000));
        assert_eq!(room.min_brightness, Some(5));
    }

    #[test]
    fn common_for_drops_to_on_off_if_any_device_is_on_off_only() {
        let extended = LightCapabilities::defaults_for(LightType::ExtendedColor);
        let on_off = LightCapabilities::defaults_for(LightType::OnOff);

        let room = LightCapabilities::common_for([&extended, &on_off]).unwrap();

        assert_eq!(room.light_type, LightType::OnOff);
        assert_eq!(room.color_modes, vec![ColorMode::OnOff]);
        assert_eq!(room.min_kelvin, None);
        assert_eq!(room.max_kelvin, None);
    }

    #[test]
    fn common_for_drops_mixed_gamuts() {
        let gamut_a = crate::gamut::gamut_a();
        let gamut_c = crate::gamut::gamut_c();
        let caps_a = LightCapabilities {
            gamut: Some(gamut_a),
            ..LightCapabilities::defaults_for(LightType::ExtendedColor)
        };
        let caps_c = LightCapabilities {
            gamut: Some(gamut_c),
            ..LightCapabilities::defaults_for(LightType::ExtendedColor)
        };

        let room = LightCapabilities::common_for([&caps_a, &caps_c]).unwrap();

        assert_eq!(room.light_type, LightType::ExtendedColor);
        assert_eq!(room.gamut, None);
    }
}
