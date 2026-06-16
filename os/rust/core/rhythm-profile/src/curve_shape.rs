//! Curve shape definitions for light profiles.
//!
//! A [`LightCurveShape`] describes the mathematical curve type and its parameters.
//! This is pure data/config — copy the JSON, tweak values, get a different curve.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::color::{rgb_to_xy, Rgb, XyColor};

/// A single color keyframe in a palette curve.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct LightPaletteKeyframe {
    /// Hour in the solar day (0.0–24.0)
    pub hour: f32,
    /// Red channel (0–255)
    pub r: u8,
    /// Green channel (0–255)
    pub g: u8,
    /// Blue channel (0–255)
    pub b: u8,
}

/// Direct color override (RGB + XY).
///
/// When present on a profile, the profile outputs this color instead of
/// deriving color from the Kelvin temperature.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct LightDirectColor {
    pub xy: XyColor,
    pub rgb: Rgb,
}

/// The mathematical curve shape used by a light profile.
///
/// This is a tagged enum — each variant carries its own parameters.
/// The profile maps the curve's normalized output through its brightness/CCT ranges.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "type"))]
pub enum LightCurveShape {
    /// Super-Gaussian (flat-topped bell curve) peaking at solar noon.
    ///
    /// Width < 1 = faster ramp, > 1 = slower ramp.
    /// shape_p: 2 = round top, 6 = flat plateau, 10 = very flat.
    /// When `direct_color` is set, brightness still follows the super-Gaussian
    /// curve, but emitted color is fixed to the provided RGB/XY value.
    #[cfg_attr(feature = "serde", serde(rename = "super-gaussian"))]
    SuperGaussian {
        /// Morning ramp speed for brightness
        #[cfg_attr(feature = "serde", serde(default = "default_width_left_bri"))]
        width_left_bri: f32,
        /// Evening ramp speed for brightness
        #[cfg_attr(feature = "serde", serde(default = "default_width_right_bri"))]
        width_right_bri: f32,
        /// Morning ramp speed for color temperature
        #[cfg_attr(feature = "serde", serde(default = "default_width_left_cct"))]
        width_left_cct: f32,
        /// Evening ramp speed for color temperature
        #[cfg_attr(feature = "serde", serde(default = "default_width_right_cct"))]
        width_right_cct: f32,
        /// Shape exponent (2 = round, 6 = flat plateau)
        #[cfg_attr(feature = "serde", serde(default = "default_shape_p"))]
        shape_p: f32,
        /// Optional fixed color output for the super-Gaussian brightness curve.
        #[cfg_attr(
            feature = "serde",
            serde(default, skip_serializing_if = "Option::is_none")
        )]
        direct_color: Option<LightDirectColor>,
    },

    /// Color palette that cycles through keyframes over 24 hours.
    ///
    /// Produces direct RGB color (not Kelvin-derived).
    /// Keyframes are interpolated with smoothstep easing.
    #[cfg_attr(feature = "serde", serde(rename = "palette"))]
    Palette {
        keyframes: Vec<LightPaletteKeyframe>,
    },

    /// Follow the active profile's current color while using this profile's
    /// brightness/timer settings.
    ///
    /// This is primarily used by the idle profile so soft-off can default to
    /// "1% of whatever the active profile looks like right now" without
    /// requiring a separate stored palette.
    #[cfg_attr(feature = "serde", serde(rename = "inherit-active"))]
    InheritActive,

    /// Constant output (ignores time of day).
    #[cfg_attr(feature = "serde", serde(rename = "constant"))]
    Constant {
        /// Fixed brightness level (0.0–1.0 normalized)
        brightness: f32,
        /// Fixed color temperature level (0.0–1.0 normalized)
        color_temp: f32,
        /// Optional fixed direct color output for this constant level.
        #[cfg_attr(
            feature = "serde",
            serde(default, skip_serializing_if = "Option::is_none")
        )]
        direct_color: Option<LightDirectColor>,
    },
}

// ── Default constants ────────────────────────────────────────────────

pub const DEFAULT_WIDTH_LEFT_BRI: f32 = 0.95;
pub const DEFAULT_WIDTH_RIGHT_BRI: f32 = 0.85;
pub const DEFAULT_WIDTH_LEFT_CCT: f32 = 0.95;
pub const DEFAULT_WIDTH_RIGHT_CCT: f32 = 1.15;
pub const DEFAULT_SHAPE_P: f32 = 6.0;

#[cfg(feature = "serde")]
fn default_width_left_bri() -> f32 {
    DEFAULT_WIDTH_LEFT_BRI
}
#[cfg(feature = "serde")]
fn default_width_right_bri() -> f32 {
    DEFAULT_WIDTH_RIGHT_BRI
}
#[cfg(feature = "serde")]
fn default_width_left_cct() -> f32 {
    DEFAULT_WIDTH_LEFT_CCT
}
#[cfg(feature = "serde")]
fn default_width_right_cct() -> f32 {
    DEFAULT_WIDTH_RIGHT_CCT
}
#[cfg(feature = "serde")]
fn default_shape_p() -> f32 {
    DEFAULT_SHAPE_P
}

// ── Palette helpers ──────────────────────────────────────────────────

/// Smoothstep for S-curved transitions between keyframes.
#[inline]
fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Interpolate between two keyframes with smoothstep easing.
fn lerp_keyframes(a: &LightPaletteKeyframe, b: &LightPaletteKeyframe, t: f32) -> Rgb {
    let t = smoothstep(t);
    let r = (a.r as f32 + (b.r as f32 - a.r as f32) * t).round() as u8;
    let g = (a.g as f32 + (b.g as f32 - a.g as f32) * t).round() as u8;
    let bv = (a.b as f32 + (b.b as f32 - a.b as f32) * t).round() as u8;
    Rgb::new(r, g, bv)
}

/// Sample a palette at a given solar hour (0–24).
fn sample_palette(keyframes: &[LightPaletteKeyframe], hour: f32) -> Rgb {
    if keyframes.is_empty() {
        return Rgb::new(255, 147, 41); // warm amber fallback
    }
    let h = hour.rem_euclid(24.0);
    for i in 0..keyframes.len() - 1 {
        if h >= keyframes[i].hour && h <= keyframes[i + 1].hour {
            let span = keyframes[i + 1].hour - keyframes[i].hour;
            let frac = if span > 0.0 {
                (h - keyframes[i].hour) / span
            } else {
                0.0
            };
            return lerp_keyframes(&keyframes[i], &keyframes[i + 1], frac);
        }
    }
    Rgb::new(keyframes[0].r, keyframes[0].g, keyframes[0].b)
}

// ── Width clamping ───────────────────────────────────────────────────

/// Clamp width values to valid range (0.2–2.0).
#[inline]
pub fn clamp_width(width: f32) -> f32 {
    width.clamp(0.2, 2.0)
}

/// Clamp shape_p to valid range (2.0–10.0).
#[inline]
pub fn clamp_shape_p(shape_p: f32) -> f32 {
    shape_p.clamp(2.0, 10.0)
}

// ── LightCurveShape methods ──────────────────────────────────────────

impl LightCurveShape {
    /// Whether this curve produces direct color (RGB/XY) rather than Kelvin.
    pub fn is_direct_color(&self) -> bool {
        matches!(
            self,
            LightCurveShape::Palette { .. }
                | LightCurveShape::SuperGaussian {
                    direct_color: Some(_),
                    ..
                }
                | LightCurveShape::Constant {
                    direct_color: Some(_),
                    ..
                }
        )
    }

    /// Get the fixed direct color for this curve, if it has one.
    pub fn direct_color(&self) -> Option<&LightDirectColor> {
        match self {
            LightCurveShape::SuperGaussian {
                direct_color: Some(direct_color),
                ..
            }
            | LightCurveShape::Constant {
                direct_color: Some(direct_color),
                ..
            } => Some(direct_color),
            _ => None,
        }
    }

    /// Sample the palette color at the given solar hour.
    ///
    /// Returns `None` for non-palette curves.
    pub fn sample_color(&self, solar_hour: f32) -> Option<(Rgb, XyColor)> {
        match self {
            LightCurveShape::Palette { keyframes } => {
                let rgb = sample_palette(keyframes, solar_hour);
                let xy = rgb_to_xy(rgb);
                Some((rgb, xy))
            }
            _ => None,
        }
    }

    /// Get the effective (clamped) super-Gaussian parameters.
    ///
    /// Returns `None` for non-super-gaussian curves.
    pub fn effective_sg_params(&self) -> Option<(f32, f32, f32, f32, f32)> {
        match self {
            LightCurveShape::SuperGaussian {
                width_left_bri,
                width_right_bri,
                width_left_cct,
                width_right_cct,
                shape_p,
                ..
            } => Some((
                clamp_width(*width_left_bri),
                clamp_width(*width_right_bri),
                clamp_width(*width_left_cct),
                clamp_width(*width_right_cct),
                clamp_shape_p(*shape_p),
            )),
            _ => None,
        }
    }

    /// Create the default super-Gaussian shape.
    pub fn default_super_gaussian() -> Self {
        LightCurveShape::SuperGaussian {
            width_left_bri: DEFAULT_WIDTH_LEFT_BRI,
            width_right_bri: DEFAULT_WIDTH_RIGHT_BRI,
            width_left_cct: DEFAULT_WIDTH_LEFT_CCT,
            width_right_cct: DEFAULT_WIDTH_RIGHT_CCT,
            shape_p: DEFAULT_SHAPE_P,
            direct_color: None,
        }
    }

    /// Create the default idle palette shape.
    pub fn default_idle_palette() -> Self {
        LightCurveShape::Palette {
            keyframes: default_idle_keyframes(),
        }
    }
}

/// The default 24-hour idle palette keyframes.
///
/// Colors rotate around the spectrum so every 2-3 hour segment is
/// visually distinct, even at 1% brightness on real bulbs.
pub fn default_idle_keyframes() -> Vec<LightPaletteKeyframe> {
    vec![
        LightPaletteKeyframe {
            hour: 0.0,
            r: 10,
            g: 10,
            b: 80,
        },
        LightPaletteKeyframe {
            hour: 3.0,
            r: 5,
            g: 50,
            b: 80,
        },
        LightPaletteKeyframe {
            hour: 5.0,
            r: 5,
            g: 80,
            b: 60,
        },
        LightPaletteKeyframe {
            hour: 7.0,
            r: 40,
            g: 80,
            b: 10,
        },
        LightPaletteKeyframe {
            hour: 9.0,
            r: 180,
            g: 100,
            b: 5,
        },
        LightPaletteKeyframe {
            hour: 11.0,
            r: 220,
            g: 60,
            b: 5,
        },
        LightPaletteKeyframe {
            hour: 13.0,
            r: 220,
            g: 30,
            b: 30,
        },
        LightPaletteKeyframe {
            hour: 15.0,
            r: 200,
            g: 20,
            b: 100,
        },
        LightPaletteKeyframe {
            hour: 17.0,
            r: 160,
            g: 15,
            b: 180,
        },
        LightPaletteKeyframe {
            hour: 19.0,
            r: 80,
            g: 10,
            b: 200,
        },
        LightPaletteKeyframe {
            hour: 21.0,
            r: 40,
            g: 10,
            b: 160,
        },
        LightPaletteKeyframe {
            hour: 23.0,
            r: 15,
            g: 10,
            b: 100,
        },
        LightPaletteKeyframe {
            hour: 24.0,
            r: 10,
            g: 10,
            b: 80,
        },
    ]
}

impl Default for LightCurveShape {
    fn default() -> Self {
        Self::default_super_gaussian()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_super_gaussian() {
        let shape = LightCurveShape::default();
        assert!(matches!(shape, LightCurveShape::SuperGaussian { .. }));
    }

    #[test]
    fn test_palette_is_direct_color() {
        let palette = LightCurveShape::default_idle_palette();
        assert!(palette.is_direct_color());

        let sg = LightCurveShape::default_super_gaussian();
        assert!(!sg.is_direct_color());

        let sg_direct = LightCurveShape::SuperGaussian {
            width_left_bri: DEFAULT_WIDTH_LEFT_BRI,
            width_right_bri: DEFAULT_WIDTH_RIGHT_BRI,
            width_left_cct: DEFAULT_WIDTH_LEFT_CCT,
            width_right_cct: DEFAULT_WIDTH_RIGHT_CCT,
            shape_p: DEFAULT_SHAPE_P,
            direct_color: Some(LightDirectColor {
                xy: XyColor { x: 0.5, y: 0.3 },
                rgb: Rgb::new(255, 140, 40),
            }),
        };
        assert!(sg_direct.is_direct_color());
    }

    #[test]
    fn test_palette_sample_color() {
        let palette = LightCurveShape::default_idle_palette();
        let (rgb, xy) = palette.sample_color(12.0).unwrap();
        assert!(rgb.r > 0);
        assert!(xy.x > 0.0);
    }

    #[test]
    fn test_palette_midnight_wrap() {
        let palette = LightCurveShape::default_idle_palette();
        let (before, _) = palette.sample_color(23.9).unwrap();
        let (after, _) = palette.sample_color(0.1).unwrap();

        let dr = (before.r as i16 - after.r as i16).unsigned_abs();
        let dg = (before.g as i16 - after.g as i16).unsigned_abs();
        let db = (before.b as i16 - after.b as i16).unsigned_abs();

        assert!(dr < 10, "red jump at midnight: {}", dr);
        assert!(dg < 5, "green jump at midnight: {}", dg);
        assert!(db < 10, "blue jump at midnight: {}", db);
    }

    #[test]
    fn test_sg_effective_params_clamping() {
        let shape = LightCurveShape::SuperGaussian {
            width_left_bri: 0.1,  // below min
            width_right_bri: 3.0, // above max
            width_left_cct: 0.5,
            width_right_cct: 1.0,
            shape_p: 1.0, // below min
            direct_color: None,
        };
        let (wlb, wrb, wlc, wrc, sp) = shape.effective_sg_params().unwrap();
        assert_eq!(wlb, 0.2);
        assert_eq!(wrb, 2.0);
        assert_eq!(wlc, 0.5);
        assert_eq!(wrc, 1.0);
        assert_eq!(sp, 2.0);
    }

    #[test]
    fn test_constant_not_direct_color() {
        let c = LightCurveShape::Constant {
            brightness: 0.5,
            color_temp: 0.5,
            direct_color: None,
        };
        assert!(!c.is_direct_color());
        assert!(c.sample_color(12.0).is_none());
    }

    #[test]
    fn test_constant_with_direct_color_is_direct_color() {
        let direct = LightDirectColor {
            xy: XyColor { x: 0.45, y: 0.25 },
            rgb: Rgb::new(255, 40, 150),
        };
        let c = LightCurveShape::Constant {
            brightness: 0.5,
            color_temp: 0.5,
            direct_color: Some(direct.clone()),
        };

        assert!(c.is_direct_color());
        assert_eq!(c.direct_color(), Some(&direct));
    }

    #[cfg(feature = "serde")]
    mod serde_tests {
        use super::*;

        #[test]
        fn test_super_gaussian_roundtrip() {
            let shape = LightCurveShape::default_super_gaussian();
            let json = serde_json::to_string(&shape).unwrap();
            let back: LightCurveShape = serde_json::from_str(&json).unwrap();
            assert_eq!(shape, back);
        }

        #[test]
        fn test_palette_roundtrip() {
            let shape = LightCurveShape::default_idle_palette();
            let json = serde_json::to_string(&shape).unwrap();
            let back: LightCurveShape = serde_json::from_str(&json).unwrap();
            assert_eq!(shape, back);
        }

        #[test]
        fn test_super_gaussian_with_direct_color_roundtrip() {
            let shape = LightCurveShape::SuperGaussian {
                width_left_bri: DEFAULT_WIDTH_LEFT_BRI,
                width_right_bri: DEFAULT_WIDTH_RIGHT_BRI,
                width_left_cct: DEFAULT_WIDTH_LEFT_CCT,
                width_right_cct: DEFAULT_WIDTH_RIGHT_CCT,
                shape_p: DEFAULT_SHAPE_P,
                direct_color: Some(LightDirectColor {
                    xy: XyColor { x: 0.45, y: 0.25 },
                    rgb: Rgb::new(255, 40, 150),
                }),
            };
            let json = serde_json::to_string(&shape).unwrap();
            let back: LightCurveShape = serde_json::from_str(&json).unwrap();
            assert_eq!(shape, back);
        }

        #[test]
        fn test_constant_roundtrip() {
            let shape = LightCurveShape::Constant {
                brightness: 0.8,
                color_temp: 0.5,
                direct_color: None,
            };
            let json = serde_json::to_string(&shape).unwrap();
            let back: LightCurveShape = serde_json::from_str(&json).unwrap();
            assert_eq!(shape, back);
        }

        #[test]
        fn test_constant_with_direct_color_roundtrip() {
            let shape = LightCurveShape::Constant {
                brightness: 0.8,
                color_temp: 0.5,
                direct_color: Some(LightDirectColor {
                    xy: XyColor { x: 0.45, y: 0.25 },
                    rgb: Rgb::new(255, 40, 150),
                }),
            };
            let json = serde_json::to_string(&shape).unwrap();
            let back: LightCurveShape = serde_json::from_str(&json).unwrap();
            assert_eq!(shape, back);
        }

        #[test]
        fn test_super_gaussian_from_partial_json() {
            // Only type + shape_p — other fields use defaults
            let json = r#"{"type": "super-gaussian", "shape_p": 4.0}"#;
            let shape: LightCurveShape = serde_json::from_str(json).unwrap();
            match shape {
                LightCurveShape::SuperGaussian {
                    width_left_bri,
                    shape_p,
                    ..
                } => {
                    assert_eq!(shape_p, 4.0);
                    assert_eq!(width_left_bri, DEFAULT_WIDTH_LEFT_BRI);
                }
                _ => panic!("expected SuperGaussian"),
            }
        }
    }
}
