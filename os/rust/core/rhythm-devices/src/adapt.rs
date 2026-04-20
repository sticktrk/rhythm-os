//! Capability-aware command adaptation.
//!
//! Transforms a universal lighting request into a device-specific command
//! based on the device's [`LightCapabilities`]. Handles:
//!
//! - Dropping unsupported color fields
//! - Clamping kelvin to the device's supported range
//! - Gamut-clamping xy coordinates for color devices
//! - Enforcing minimum brightness
//! - Suppressing transitions for devices that don't support them
//! - Selecting the final color representation based on protocol preference

use crate::capabilities::LightCapabilities;

/// Requested color data from the engine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColorRequest {
    /// The engine requested a kelvin target plus an xy fallback derived from it.
    ColorTemperature { kelvin: u16, xy: (f32, f32) },
    /// The engine requested direct color with both xy and hue/saturation forms.
    DirectColor {
        xy: (f32, f32),
        hue_saturation: (u8, u8),
    },
    /// The engine requested direct xy color.
    Xy((f32, f32)),
}

/// Protocol preference for which normalized color representation to emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorPreference {
    /// Prefer color temperature when supported, otherwise fall back to xy.
    PreferColorTemperature,
    /// Prefer xy when supported, otherwise fall back to color temperature.
    PreferXy,
}

/// A lighting command adapted to a specific device's capabilities.
#[derive(Debug, Clone, PartialEq)]
pub struct AdaptedCommand {
    /// Whether the device should be on.
    pub on: bool,
    /// Brightness (1-100), or `None` if the device is on/off-only.
    pub brightness: Option<u8>,
    /// Color temperature in Kelvin, clamped to device range.
    /// Mutually exclusive with `xy`.
    pub kelvin: Option<u16>,
    /// CIE xy color coordinates, gamut-clamped.
    /// Mutually exclusive with `kelvin`.
    pub xy: Option<(f32, f32)>,
    /// Hue/saturation coordinates in Matter's 0-254 range.
    /// Mutually exclusive with `kelvin` and `xy`.
    pub hue_saturation: Option<(u8, u8)>,
    /// Transition time in milliseconds.
    /// `None` if the device doesn't support transitions.
    pub transition_ms: Option<u32>,
}

/// Adapt a lighting request to a specific device's capabilities.
pub fn adapt_command(
    caps: &LightCapabilities,
    brightness: u8,
    color: ColorRequest,
    transition_ms: Option<u32>,
    preference: ColorPreference,
) -> AdaptedCommand {
    let on = brightness > 0;
    let transition_ms = if caps.supports_transition {
        transition_ms
    } else {
        None
    };

    if !caps.supports_dimming() {
        return AdaptedCommand {
            on,
            brightness: None,
            kelvin: None,
            xy: None,
            hue_saturation: None,
            transition_ms: None,
        };
    }

    let brightness = Some(clamp_brightness(brightness, caps.min_brightness));
    let (kelvin, xy, hue_saturation) = select_color(caps, color, preference);

    AdaptedCommand {
        on,
        brightness,
        kelvin,
        xy,
        hue_saturation,
        transition_ms,
    }
}

fn select_color(
    caps: &LightCapabilities,
    color: ColorRequest,
    preference: ColorPreference,
) -> (Option<u16>, Option<(f32, f32)>, Option<(u8, u8)>) {
    let supports_ct = caps.supports_color_temp();
    let supports_hs = caps.supports_hue_saturation();
    let supports_xy = caps.supports_xy_color();

    if !supports_ct && !supports_hs && !supports_xy {
        return (None, None, None);
    }

    match color {
        ColorRequest::DirectColor { xy, hue_saturation } => {
            if supports_hs {
                (None, None, Some(hue_saturation))
            } else if supports_xy {
                (None, Some(clamp_xy(caps, xy)), None)
            } else {
                (None, None, None)
            }
        }
        ColorRequest::Xy(xy) => {
            if supports_xy {
                (None, Some(clamp_xy(caps, xy)), None)
            } else {
                (None, None, None)
            }
        }
        ColorRequest::ColorTemperature { kelvin, xy } => {
            let clamped_kelvin = clamp_kelvin(kelvin, caps.min_kelvin, caps.max_kelvin);
            let clamped_xy = clamp_xy(caps, xy);

            match preference {
                ColorPreference::PreferColorTemperature if supports_ct => {
                    (Some(clamped_kelvin), None, None)
                }
                ColorPreference::PreferXy if supports_xy => (None, Some(clamped_xy), None),
                _ if supports_ct => (Some(clamped_kelvin), None, None),
                _ if supports_xy => (None, Some(clamped_xy), None),
                _ => (None, None, None),
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

fn clamp_xy(caps: &LightCapabilities, xy: (f32, f32)) -> (f32, f32) {
    match &caps.gamut {
        Some(gamut) => {
            let point = crate::gamut::XyPoint::new(xy.0, xy.1);
            let clamped = gamut.clamp_xy(point);
            (clamped.x, clamped.y)
        }
        None => xy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::{ColorMode, LightType};
    use crate::gamut::GamutTriangle;

    #[test]
    fn on_off_device_only_gets_on_flag() {
        let caps = LightCapabilities::defaults_for(LightType::OnOff);
        let adapted = adapt_command(
            &caps,
            80,
            ColorRequest::ColorTemperature {
                kelvin: 4000,
                xy: (0.3, 0.3),
            },
            Some(500),
            ColorPreference::PreferColorTemperature,
        );

        assert!(adapted.on);
        assert_eq!(adapted.brightness, None);
        assert_eq!(adapted.kelvin, None);
        assert_eq!(adapted.xy, None);
        assert_eq!(adapted.hue_saturation, None);
        assert_eq!(adapted.transition_ms, None);
    }

    #[test]
    fn dimmable_gets_brightness_only() {
        let caps = LightCapabilities::defaults_for(LightType::Dimmable);
        let adapted = adapt_command(
            &caps,
            75,
            ColorRequest::ColorTemperature {
                kelvin: 4000,
                xy: (0.3, 0.3),
            },
            Some(500),
            ColorPreference::PreferColorTemperature,
        );

        assert!(adapted.on);
        assert_eq!(adapted.brightness, Some(75));
        assert_eq!(adapted.kelvin, None);
        assert_eq!(adapted.xy, None);
        assert_eq!(adapted.hue_saturation, None);
        assert_eq!(adapted.transition_ms, Some(500));
    }

    #[test]
    fn dimmable_respects_min_brightness() {
        let caps = LightCapabilities {
            min_brightness: Some(5),
            ..LightCapabilities::defaults_for(LightType::Dimmable)
        };
        let adapted = adapt_command(
            &caps,
            2,
            ColorRequest::ColorTemperature {
                kelvin: 4000,
                xy: (0.3, 0.3),
            },
            None,
            ColorPreference::PreferColorTemperature,
        );

        assert_eq!(adapted.brightness, Some(5));
    }

    #[test]
    fn zero_brightness_stays_zero() {
        let caps = LightCapabilities {
            min_brightness: Some(5),
            ..LightCapabilities::defaults_for(LightType::Dimmable)
        };
        let adapted = adapt_command(
            &caps,
            0,
            ColorRequest::ColorTemperature {
                kelvin: 4000,
                xy: (0.3, 0.3),
            },
            None,
            ColorPreference::PreferColorTemperature,
        );

        assert!(!adapted.on);
        assert_eq!(adapted.brightness, Some(0));
    }

    #[test]
    fn ct_gets_brightness_and_kelvin() {
        let caps = LightCapabilities::defaults_for(LightType::ColorTemperature);
        let adapted = adapt_command(
            &caps,
            80,
            ColorRequest::ColorTemperature {
                kelvin: 4000,
                xy: (0.3, 0.3),
            },
            Some(1000),
            ColorPreference::PreferColorTemperature,
        );

        assert!(adapted.on);
        assert_eq!(adapted.brightness, Some(80));
        assert_eq!(adapted.kelvin, Some(4000));
        assert_eq!(adapted.xy, None);
        assert_eq!(adapted.hue_saturation, None);
        assert_eq!(adapted.transition_ms, Some(1000));
    }

    #[test]
    fn ct_direct_color_drops_unsupported_xy() {
        let caps = LightCapabilities::defaults_for(LightType::ColorTemperature);
        let adapted = adapt_command(
            &caps,
            50,
            ColorRequest::Xy((0.45, 0.25)),
            Some(500),
            ColorPreference::PreferColorTemperature,
        );

        assert_eq!(adapted.brightness, Some(50));
        assert_eq!(adapted.kelvin, None);
        assert_eq!(adapted.xy, None);
        assert_eq!(adapted.hue_saturation, None);
    }

    #[test]
    fn ct_clamps_kelvin_to_range() {
        let caps = LightCapabilities {
            min_kelvin: Some(2200),
            max_kelvin: Some(4000),
            ..LightCapabilities::defaults_for(LightType::ColorTemperature)
        };
        let adapted = adapt_command(
            &caps,
            80,
            ColorRequest::ColorTemperature {
                kelvin: 6500,
                xy: (0.3, 0.3),
            },
            None,
            ColorPreference::PreferColorTemperature,
        );

        assert_eq!(adapted.kelvin, Some(4000));
    }

    #[test]
    fn extended_color_prefers_kelvin_for_group_protocols() {
        let caps = LightCapabilities::defaults_for(LightType::ExtendedColor);
        let adapted = adapt_command(
            &caps,
            80,
            ColorRequest::ColorTemperature {
                kelvin: 4000,
                xy: (0.31, 0.33),
            },
            Some(500),
            ColorPreference::PreferColorTemperature,
        );

        assert_eq!(adapted.brightness, Some(80));
        assert_eq!(adapted.kelvin, Some(4000));
        assert_eq!(adapted.xy, None);
        assert_eq!(adapted.hue_saturation, None);
        assert_eq!(adapted.transition_ms, Some(500));
    }

    #[test]
    fn extended_color_prefers_xy_for_protocols_that_want_it() {
        let caps = LightCapabilities::defaults_for(LightType::ExtendedColor);
        let adapted = adapt_command(
            &caps,
            80,
            ColorRequest::ColorTemperature {
                kelvin: 4000,
                xy: (0.31, 0.33),
            },
            Some(500),
            ColorPreference::PreferXy,
        );

        assert_eq!(adapted.brightness, Some(80));
        assert_eq!(adapted.kelvin, None);
        assert_eq!(adapted.xy, Some((0.31, 0.33)));
        assert_eq!(adapted.hue_saturation, None);
    }

    #[test]
    fn extended_color_direct_color_uses_xy() {
        let caps = LightCapabilities::defaults_for(LightType::ExtendedColor);
        let adapted = adapt_command(
            &caps,
            80,
            ColorRequest::DirectColor {
                xy: (0.45, 0.25),
                hue_saturation: (32, 254),
            },
            None,
            ColorPreference::PreferColorTemperature,
        );

        assert_eq!(adapted.kelvin, None);
        assert_eq!(adapted.xy, Some((0.45, 0.25)));
        assert_eq!(adapted.hue_saturation, None);
    }

    #[test]
    fn extended_color_direct_color_prefers_hue_saturation_when_supported() {
        let caps = LightCapabilities {
            color_modes: vec![ColorMode::HueSaturation, ColorMode::Xy],
            ..LightCapabilities::defaults_for(LightType::ExtendedColor)
        };
        let adapted = adapt_command(
            &caps,
            80,
            ColorRequest::DirectColor {
                xy: (0.45, 0.25),
                hue_saturation: (32, 254),
            },
            None,
            ColorPreference::PreferColorTemperature,
        );

        assert_eq!(adapted.kelvin, None);
        assert_eq!(adapted.xy, None);
        assert_eq!(adapted.hue_saturation, Some((32, 254)));
    }

    #[test]
    fn extended_color_gamut_clamps_xy() {
        let caps = LightCapabilities {
            gamut: Some(GamutTriangle {
                red: crate::gamut::XyPoint {
                    x: 0.6915,
                    y: 0.3083,
                },
                green: crate::gamut::XyPoint { x: 0.17, y: 0.7 },
                blue: crate::gamut::XyPoint {
                    x: 0.1532,
                    y: 0.0475,
                },
            }),
            ..LightCapabilities::defaults_for(LightType::ExtendedColor)
        };
        let adapted = adapt_command(
            &caps,
            80,
            ColorRequest::Xy((0.1, 0.1)),
            None,
            ColorPreference::PreferXy,
        );

        let (x, y) = adapted.xy.unwrap();
        assert!(x >= 0.1532);
        assert!(y >= 0.0475);
        assert_eq!(adapted.hue_saturation, None);
    }

    #[test]
    fn transitions_are_suppressed_when_not_supported() {
        let caps = LightCapabilities {
            supports_transition: false,
            ..LightCapabilities::defaults_for(LightType::Dimmable)
        };
        let adapted = adapt_command(
            &caps,
            80,
            ColorRequest::ColorTemperature {
                kelvin: 4000,
                xy: (0.3, 0.3),
            },
            Some(500),
            ColorPreference::PreferColorTemperature,
        );

        assert_eq!(adapted.transition_ms, None);
    }

    #[test]
    fn hs_only_device_uses_hue_saturation_for_direct_color() {
        let caps = LightCapabilities {
            light_type: LightType::ExtendedColor,
            color_modes: vec![ColorMode::HueSaturation],
            min_kelvin: None,
            max_kelvin: None,
            gamut: None,
            min_brightness: None,
            supports_transition: true,
        };
        let adapted = adapt_command(
            &caps,
            80,
            ColorRequest::DirectColor {
                xy: (0.3, 0.3),
                hue_saturation: (90, 200),
            },
            None,
            ColorPreference::PreferXy,
        );

        assert_eq!(adapted.brightness, Some(80));
        assert_eq!(adapted.kelvin, None);
        assert_eq!(adapted.xy, None);
        assert_eq!(adapted.hue_saturation, Some((90, 200)));
    }
}
