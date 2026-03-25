//! Capability-aware command adaptation.
//!
//! Transforms a universal lighting command into a device-specific command
//! based on the device's [`LightCapabilities`]. Handles:
//!
//! - Dropping color fields for dimmable-only or on/off devices
//! - Clamping kelvin to the device's supported range
//! - Gamut-clamping xy coordinates for color devices
//! - Enforcing minimum brightness
//! - Suppressing transitions for devices that don't support them

use crate::capabilities::{LightCapabilities, LightType};

/// A lighting command adapted to a specific device's capabilities.
#[derive(Debug, Clone, PartialEq)]
pub struct AdaptedCommand {
    /// Whether the device should be on.
    pub on: bool,
    /// Brightness (1-100), or `None` if the device is on/off-only.
    pub brightness: Option<u8>,
    /// Color temperature in Kelvin, clamped to device range.
    /// `None` if the device doesn't support color temperature.
    pub kelvin: Option<u16>,
    /// CIE xy color coordinates, gamut-clamped.
    /// `None` if the device doesn't support xy color.
    pub xy: Option<(f32, f32)>,
    /// Transition time in milliseconds.
    /// `None` if the device doesn't support transitions.
    pub transition_ms: Option<u32>,
}

/// Adapt a universal lighting command to a specific device's capabilities.
///
/// # Arguments
///
/// * `caps` - The device's capabilities
/// * `brightness` - Desired brightness (1-100)
/// * `kelvin` - Desired color temperature in Kelvin
/// * `xy` - Desired CIE xy color coordinates
/// * `transition_ms` - Desired transition time in milliseconds
pub fn adapt_command(
    caps: &LightCapabilities,
    brightness: u8,
    kelvin: u16,
    xy: (f32, f32),
    transition_ms: Option<u32>,
) -> AdaptedCommand {
    let on = brightness > 0;

    let transition = if caps.supports_transition {
        transition_ms
    } else {
        None
    };

    match caps.light_type {
        LightType::OnOff => AdaptedCommand {
            on,
            brightness: None,
            kelvin: None,
            xy: None,
            transition_ms: None,
        },

        LightType::Dimmable => AdaptedCommand {
            on,
            brightness: Some(clamp_brightness(brightness, caps.min_brightness)),
            kelvin: None,
            xy: None,
            transition_ms: transition,
        },

        LightType::ColorTemperature => AdaptedCommand {
            on,
            brightness: Some(clamp_brightness(brightness, caps.min_brightness)),
            kelvin: Some(clamp_kelvin(kelvin, caps.min_kelvin, caps.max_kelvin)),
            xy: None,
            transition_ms: transition,
        },

        LightType::ExtendedColor => {
            let clamped_xy = match &caps.gamut {
                Some(gamut) => {
                    let point = crate::gamut::XyPoint::new(xy.0, xy.1);
                    let clamped = gamut.clamp_xy(point);
                    (clamped.x, clamped.y)
                }
                None => xy,
            };

            AdaptedCommand {
                on,
                brightness: Some(clamp_brightness(brightness, caps.min_brightness)),
                kelvin: Some(clamp_kelvin(kelvin, caps.min_kelvin, caps.max_kelvin)),
                xy: Some(clamped_xy),
                transition_ms: transition,
            }
        }
    }
}

/// Clamp brightness to device minimum (when on).
fn clamp_brightness(brightness: u8, min_brightness: Option<u8>) -> u8 {
    if brightness == 0 {
        return 0;
    }
    match min_brightness {
        Some(min) if brightness < min => min,
        _ => brightness,
    }
}

/// Clamp kelvin to device range.
fn clamp_kelvin(kelvin: u16, min: Option<u16>, max: Option<u16>) -> u16 {
    let kelvin = match min {
        Some(min_k) if kelvin < min_k => min_k,
        _ => kelvin,
    };
    match max {
        Some(max_k) if kelvin > max_k => max_k,
        _ => kelvin,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::ColorMode;
    use crate::gamut::{gamut_c, GamutTriangle, XyPoint};

    // ── OnOff devices ─────────────────────────────────────────────────

    #[test]
    fn on_off_device_only_gets_on_flag() {
        let caps = LightCapabilities::defaults_for(LightType::OnOff);
        let adapted = adapt_command(&caps, 80, 4000, (0.3, 0.3), Some(500));

        assert!(adapted.on);
        assert_eq!(adapted.brightness, None);
        assert_eq!(adapted.kelvin, None);
        assert_eq!(adapted.xy, None);
        assert_eq!(adapted.transition_ms, None);
    }

    #[test]
    fn on_off_device_off_when_brightness_zero() {
        let caps = LightCapabilities::defaults_for(LightType::OnOff);
        let adapted = adapt_command(&caps, 0, 4000, (0.3, 0.3), None);

        assert!(!adapted.on);
    }

    // ── Dimmable devices ──────────────────────────────────────────────

    #[test]
    fn dimmable_gets_brightness_only() {
        let caps = LightCapabilities::defaults_for(LightType::Dimmable);
        let adapted = adapt_command(&caps, 75, 4000, (0.3, 0.3), Some(500));

        assert!(adapted.on);
        assert_eq!(adapted.brightness, Some(75));
        assert_eq!(adapted.kelvin, None);
        assert_eq!(adapted.xy, None);
        assert_eq!(adapted.transition_ms, Some(500));
    }

    #[test]
    fn dimmable_respects_min_brightness() {
        let caps = LightCapabilities {
            min_brightness: Some(5),
            ..LightCapabilities::defaults_for(LightType::Dimmable)
        };
        let adapted = adapt_command(&caps, 2, 4000, (0.3, 0.3), None);

        assert_eq!(adapted.brightness, Some(5));
    }

    #[test]
    fn dimmable_zero_brightness_stays_zero() {
        let caps = LightCapabilities {
            min_brightness: Some(5),
            ..LightCapabilities::defaults_for(LightType::Dimmable)
        };
        let adapted = adapt_command(&caps, 0, 4000, (0.3, 0.3), None);

        assert!(!adapted.on);
        assert_eq!(adapted.brightness, Some(0));
    }

    // ── Color temperature devices ─────────────────────────────────────

    #[test]
    fn ct_gets_brightness_and_kelvin() {
        let caps = LightCapabilities::defaults_for(LightType::ColorTemperature);
        let adapted = adapt_command(&caps, 80, 4000, (0.3, 0.3), Some(1000));

        assert!(adapted.on);
        assert_eq!(adapted.brightness, Some(80));
        assert_eq!(adapted.kelvin, Some(4000));
        assert_eq!(adapted.xy, None);
        assert_eq!(adapted.transition_ms, Some(1000));
    }

    #[test]
    fn ct_clamps_kelvin_below_min() {
        let caps = LightCapabilities {
            min_kelvin: Some(2200),
            max_kelvin: Some(6500),
            ..LightCapabilities::defaults_for(LightType::ColorTemperature)
        };
        let adapted = adapt_command(&caps, 80, 1800, (0.3, 0.3), None);

        assert_eq!(adapted.kelvin, Some(2200));
    }

    #[test]
    fn ct_clamps_kelvin_above_max() {
        let caps = LightCapabilities {
            min_kelvin: Some(2200),
            max_kelvin: Some(4000),
            ..LightCapabilities::defaults_for(LightType::ColorTemperature)
        };
        let adapted = adapt_command(&caps, 80, 6500, (0.3, 0.3), None);

        assert_eq!(adapted.kelvin, Some(4000));
    }

    #[test]
    fn ct_kelvin_in_range_unchanged() {
        let caps = LightCapabilities {
            min_kelvin: Some(2200),
            max_kelvin: Some(6500),
            ..LightCapabilities::defaults_for(LightType::ColorTemperature)
        };
        let adapted = adapt_command(&caps, 80, 4000, (0.3, 0.3), None);

        assert_eq!(adapted.kelvin, Some(4000));
    }

    #[test]
    fn ct_no_range_passes_through() {
        let caps = LightCapabilities {
            light_type: LightType::ColorTemperature,
            color_modes: vec![ColorMode::ColorTemperature],
            min_kelvin: None,
            max_kelvin: None,
            gamut: None,
            min_brightness: None,
            supports_transition: true,
        };
        let adapted = adapt_command(&caps, 80, 4000, (0.3, 0.3), None);

        assert_eq!(adapted.kelvin, Some(4000));
    }

    // ── Extended color devices ────────────────────────────────────────

    #[test]
    fn extended_color_gets_everything() {
        let caps = LightCapabilities::defaults_for(LightType::ExtendedColor);
        let adapted = adapt_command(&caps, 80, 4000, (0.31, 0.33), Some(500));

        assert!(adapted.on);
        assert_eq!(adapted.brightness, Some(80));
        assert_eq!(adapted.kelvin, Some(4000));
        assert!(adapted.xy.is_some());
        assert_eq!(adapted.transition_ms, Some(500));
    }

    #[test]
    fn extended_color_gamut_clamps_xy() {
        let caps = LightCapabilities {
            gamut: Some(gamut_c()),
            ..LightCapabilities::defaults_for(LightType::ExtendedColor)
        };
        // Point far outside gamut C
        let adapted = adapt_command(&caps, 80, 4000, (0.0, 0.9), None);

        let (x, y) = adapted.xy.unwrap();
        let gamut = gamut_c();
        assert!(
            gamut.contains(&XyPoint::new(x, y)),
            "clamped point ({}, {}) should be inside gamut C",
            x,
            y
        );
    }

    #[test]
    fn extended_color_no_gamut_passes_xy_through() {
        let caps = LightCapabilities {
            gamut: None,
            ..LightCapabilities::defaults_for(LightType::ExtendedColor)
        };
        let adapted = adapt_command(&caps, 80, 4000, (0.45, 0.55), None);

        assert_eq!(adapted.xy, Some((0.45, 0.55)));
    }

    #[test]
    fn extended_color_clamps_kelvin() {
        let caps = LightCapabilities {
            min_kelvin: Some(2000),
            max_kelvin: Some(6500),
            ..LightCapabilities::defaults_for(LightType::ExtendedColor)
        };
        let adapted = adapt_command(&caps, 80, 1500, (0.3, 0.3), None);

        assert_eq!(adapted.kelvin, Some(2000));
    }

    // ── Transition suppression ────────────────────────────────────────

    #[test]
    fn transition_suppressed_when_not_supported() {
        let caps = LightCapabilities {
            supports_transition: false,
            ..LightCapabilities::defaults_for(LightType::Dimmable)
        };
        let adapted = adapt_command(&caps, 80, 4000, (0.3, 0.3), Some(500));

        assert_eq!(adapted.transition_ms, None);
    }

    #[test]
    fn transition_passed_when_supported() {
        let caps = LightCapabilities::defaults_for(LightType::ColorTemperature);
        let adapted = adapt_command(&caps, 80, 4000, (0.3, 0.3), Some(1000));

        assert_eq!(adapted.transition_ms, Some(1000));
    }

    #[test]
    fn no_transition_stays_none() {
        let caps = LightCapabilities::defaults_for(LightType::ExtendedColor);
        let adapted = adapt_command(&caps, 80, 4000, (0.3, 0.3), None);

        assert_eq!(adapted.transition_ms, None);
    }

    // ── Min brightness edge cases ─────────────────────────────────────

    #[test]
    fn brightness_at_min_unchanged() {
        let caps = LightCapabilities {
            min_brightness: Some(5),
            ..LightCapabilities::defaults_for(LightType::ColorTemperature)
        };
        let adapted = adapt_command(&caps, 5, 4000, (0.3, 0.3), None);

        assert_eq!(adapted.brightness, Some(5));
    }

    #[test]
    fn brightness_above_min_unchanged() {
        let caps = LightCapabilities {
            min_brightness: Some(5),
            ..LightCapabilities::defaults_for(LightType::ColorTemperature)
        };
        let adapted = adapt_command(&caps, 50, 4000, (0.3, 0.3), None);

        assert_eq!(adapted.brightness, Some(50));
    }

    #[test]
    fn no_min_brightness_passes_through() {
        let caps = LightCapabilities::defaults_for(LightType::Dimmable);
        let adapted = adapt_command(&caps, 1, 4000, (0.3, 0.3), None);

        assert_eq!(adapted.brightness, Some(1));
    }

    // ── Gamut edge cases ──────────────────────────────────────────────

    #[test]
    fn xy_inside_gamut_unchanged() {
        let gamut = gamut_c();
        // D65 white point — inside gamut C
        let caps = LightCapabilities {
            gamut: Some(gamut.clone()),
            ..LightCapabilities::defaults_for(LightType::ExtendedColor)
        };
        let adapted = adapt_command(&caps, 80, 4000, (0.3127, 0.3290), None);

        let (x, y) = adapted.xy.unwrap();
        assert!((x - 0.3127).abs() < 1e-5);
        assert!((y - 0.3290).abs() < 1e-5);
    }

    #[test]
    fn custom_gamut_triangle_clamps() {
        let narrow_gamut = GamutTriangle::new(
            XyPoint::new(0.4, 0.4),
            XyPoint::new(0.3, 0.5),
            XyPoint::new(0.3, 0.3),
        );
        let caps = LightCapabilities {
            gamut: Some(narrow_gamut.clone()),
            ..LightCapabilities::defaults_for(LightType::ExtendedColor)
        };
        // Point well outside the narrow gamut
        let adapted = adapt_command(&caps, 80, 4000, (0.1, 0.1), None);

        let (x, y) = adapted.xy.unwrap();
        assert!(
            narrow_gamut.contains(&XyPoint::new(x, y)),
            "clamped point ({}, {}) should be inside the narrow gamut",
            x,
            y
        );
    }
}
