//! Color space conversions for adaptive lighting.
//!
//! This module provides conversions between:
//! - Color temperature (Kelvin) and CIE 1931 xy coordinates
//! - Color temperature (Kelvin) and RGB values
//! - RGB and CIE 1931 xy coordinates
//!
//! The Kelvin to xy conversion uses Krystek polynomial approximations
//! for the Planckian locus, which provides excellent accuracy from 1000K to 25000K.

use libm::powf;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// RGB color value (8-bit per channel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    /// Create a new RGB color.
    pub fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Convert to a tuple (r, g, b).
    pub fn to_tuple(self) -> (u8, u8, u8) {
        (self.r, self.g, self.b)
    }
}

impl From<(u8, u8, u8)> for Rgb {
    fn from((r, g, b): (u8, u8, u8)) -> Self {
        Self { r, g, b }
    }
}

/// CIE 1931 xy chromaticity coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct XyColor {
    pub x: f32,
    pub y: f32,
}

impl XyColor {
    /// Create new xy coordinates.
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// Convert to a tuple (x, y).
    pub fn to_tuple(self) -> (f32, f32) {
        (self.x, self.y)
    }
}

impl From<(f32, f32)> for XyColor {
    fn from((x, y): (f32, f32)) -> Self {
        Self { x, y }
    }
}

/// Convert color temperature to CIE 1931 xy coordinates using Krystek polynomials.
///
/// Uses the improved Krystek & Moritz (1982) polynomial approximations for the
/// Planckian locus. These provide excellent accuracy from 1000K to 25000K.
///
/// Reference: Krystek, M. (1985). "An algorithm to calculate correlated colour
/// temperature". Color Research & Application, 10(1), 38-40.
///
/// # Arguments
///
/// * `kelvin` - Color temperature in Kelvin (clamped to 1000-25000)
///
/// # Returns
///
/// CIE 1931 xy coordinates for the color temperature.
pub fn kelvin_to_xy(kelvin: u16) -> XyColor {
    // Clamp to valid range
    let t = (kelvin as f32).clamp(1000.0, 25000.0);

    // Use reciprocal temperature for better numerical stability
    let inv_t = 1000.0 / t; // T in thousands of Kelvin

    // Calculate x coordinate using Krystek's polynomial
    let x = if t <= 4000.0 {
        // Low temperature range (1000-4000K)
        -0.2661239 * inv_t * inv_t * inv_t - 0.2343589 * inv_t * inv_t
            + 0.8776956 * inv_t
            + 0.179910
    } else {
        // High temperature range (4000-25000K)
        -3.025_847 * inv_t * inv_t * inv_t
            + 2.1070379 * inv_t * inv_t
            + 0.2226347 * inv_t
            + 0.240390
    };

    // Calculate y coordinate using Krystek's polynomial
    let y = if t <= 2222.0 {
        // Very low temperature
        -1.1063814 * x * x * x - 1.348_110_2 * x * x + 2.185_558_3 * x - 0.20219683
    } else if t <= 4000.0 {
        // Low-mid temperature
        -0.9549476 * x * x * x - 1.374_185_9 * x * x + 2.091_37 * x - 0.16748867
    } else {
        // High temperature
        3.081_758 * x * x * x - 5.873_387 * x * x + 3.751_13 * x - 0.37001483
    };

    XyColor::new(x, y)
}

/// Convert color temperature to RGB values.
///
/// This uses the Krystek polynomial approach to get xy coordinates,
/// then converts through XYZ to RGB color space.
///
/// # Arguments
///
/// * `kelvin` - Color temperature in Kelvin
///
/// # Returns
///
/// RGB color value (8-bit per channel).
pub fn kelvin_to_rgb(kelvin: u16) -> Rgb {
    // First get x,y coordinates using Krystek polynomials
    let xy = kelvin_to_xy(kelvin);

    // Convert x,y to XYZ (assuming Y=1 for relative luminance)
    let y_luminance = 1.0;
    let (x, y) = (xy.x as f64, xy.y as f64);

    if y.abs() < f64::EPSILON {
        return Rgb::new(255, 255, 255);
    }

    let xyz_x = (x * y_luminance) / y;
    let xyz_z = ((1.0 - x - y) * y_luminance) / y;

    // Convert XYZ to linear RGB (sRGB primaries)
    let r = 3.2404542 * xyz_x - 1.5371385 * y_luminance - 0.4985314 * xyz_z;
    let g = -0.9692660 * xyz_x + 1.8760108 * y_luminance + 0.0415560 * xyz_z;
    let b = 0.0556434 * xyz_x - 0.2040259 * y_luminance + 1.0572252 * xyz_z;

    // Clamp negative values
    let r = r.max(0.0);
    let g = g.max(0.0);
    let b = b.max(0.0);

    // Normalize if any component > 1 (preserve color ratios)
    let max_val = r.max(g).max(b);
    let (r, g, b) = if max_val > 1.0 {
        (r / max_val, g / max_val, b / max_val)
    } else {
        (r, g, b)
    };

    // Apply gamma correction (linear to sRGB)
    let r = linear_to_srgb(r);
    let g = linear_to_srgb(g);
    let b = linear_to_srgb(b);

    // Convert to 8-bit values
    Rgb::new(
        (r * 255.0).round().clamp(0.0, 255.0) as u8,
        (g * 255.0).round().clamp(0.0, 255.0) as u8,
        (b * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

/// Convert RGB to CIE 1931 xy coordinates.
///
/// # Arguments
///
/// * `rgb` - RGB color value
///
/// # Returns
///
/// CIE 1931 xy coordinates.
pub fn rgb_to_xy(rgb: Rgb) -> XyColor {
    // Normalize to 0-1 range
    let r = rgb.r as f64 / 255.0;
    let g = rgb.g as f64 / 255.0;
    let b = rgb.b as f64 / 255.0;

    // Apply gamma correction (sRGB to linear)
    let r = srgb_to_linear(r);
    let g = srgb_to_linear(g);
    let b = srgb_to_linear(b);

    // Convert to XYZ
    let xyz_x = r * 0.4124564 + g * 0.3575761 + b * 0.1804375;
    let xyz_y = r * 0.2126729 + g * 0.7151522 + b * 0.0721750;
    let xyz_z = r * 0.0193339 + g * 0.1191920 + b * 0.9503041;

    let sum = xyz_x + xyz_y + xyz_z;
    if sum.abs() < f64::EPSILON {
        return XyColor::new(0.0, 0.0);
    }

    XyColor::new((xyz_x / sum) as f32, (xyz_y / sum) as f32)
}

/// Convert Kelvin to mireds (micro reciprocal degrees).
///
/// Mireds provide perceptually uniform color temperature steps.
#[inline]
pub fn kelvin_to_mireds(kelvin: u16) -> u16 {
    let kelvin = kelvin.clamp(500, 6500) as f32;
    (1_000_000.0 / kelvin).round() as u16
}

/// Convert mireds to Kelvin.
#[inline]
pub fn mireds_to_kelvin(mireds: u16) -> u16 {
    if mireds == 0 {
        return 6500;
    }
    (1_000_000.0 / mireds as f32).round().clamp(500.0, 6500.0) as u16
}

/// Apply sRGB gamma correction (linear to sRGB).
#[inline]
fn linear_to_srgb(c: f64) -> f64 {
    if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Apply inverse sRGB gamma correction (sRGB to linear).
#[inline]
fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Apply perceptual gamma to brightness value.
///
/// Converts linear brightness percentage to perceptual brightness.
///
/// # Arguments
///
/// * `brightness` - Linear brightness (0-100)
/// * `gamma` - Gamma value (typically 0.62 for perceptual uniformity)
///
/// # Returns
///
/// Perceptual brightness value.
pub fn apply_brightness_gamma(brightness: f32, gamma: f32) -> f32 {
    let normalized = (brightness / 100.0).clamp(0.0, 1.0);
    powf(normalized, gamma)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kelvin_to_xy_daylight() {
        let xy = kelvin_to_xy(6500);
        assert!((xy.x - 0.313).abs() < 0.02, "x={}, expected ~0.313", xy.x);
        assert!((xy.y - 0.329).abs() < 0.02, "y={}, expected ~0.329", xy.y);
    }

    #[test]
    fn test_kelvin_to_xy_warm() {
        let xy = kelvin_to_xy(2700);
        assert!(xy.x > 0.4, "Warm light should have x > 0.4, got {}", xy.x);
    }

    #[test]
    fn test_kelvin_to_rgb_warm() {
        let rgb = kelvin_to_rgb(2700);
        assert!(
            rgb.r > rgb.b,
            "Warm light: expected r > b, got r={}, b={}",
            rgb.r,
            rgb.b
        );
    }

    #[test]
    fn test_kelvin_to_rgb_cool() {
        let rgb = kelvin_to_rgb(6500);
        assert!(rgb.r > 200, "Cool light should have high red");
        assert!(rgb.g > 200, "Cool light should have high green");
        assert!(rgb.b > 200, "Cool light should have high blue");
    }

    #[test]
    fn test_kelvin_mireds_roundtrip() {
        let kelvin = 4000u16;
        let mireds = kelvin_to_mireds(kelvin);
        let recovered = mireds_to_kelvin(mireds);
        assert!(
            (kelvin as i32 - recovered as i32).abs() < 10,
            "Expected {}, got {}",
            kelvin,
            recovered
        );
    }

    #[test]
    fn test_brightness_gamma() {
        assert!((apply_brightness_gamma(50.0, 1.0) - 0.5).abs() < 0.001);
        let gamma_adjusted = apply_brightness_gamma(50.0, 0.62);
        assert!(gamma_adjusted > 0.5, "Gamma 0.62 should boost mid values");
    }

    #[test]
    fn test_brightness_gamma_boundaries() {
        assert!((apply_brightness_gamma(0.0, 0.62) - 0.0).abs() < 0.001);
        assert!((apply_brightness_gamma(100.0, 0.62) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_kelvin_to_xy_extreme_warm() {
        let xy = kelvin_to_xy(1800);
        assert!(
            xy.x > 0.5,
            "Very warm light should have x > 0.5, got {}",
            xy.x
        );
    }

    #[test]
    fn test_kelvin_to_xy_extreme_cool() {
        let xy = kelvin_to_xy(10000);
        assert!(
            xy.x < 0.35,
            "Very cool light should have x < 0.35, got {}",
            xy.x
        );
    }

    #[test]
    fn test_kelvin_to_rgb_bounds() {
        for kelvin in [1800, 2700, 4000, 5000, 6500, 10000] {
            let rgb = kelvin_to_rgb(kelvin);
            let _ = (rgb.r, rgb.g, rgb.b);
        }
    }

    #[test]
    fn test_kelvin_to_mireds_common_values() {
        let mireds_2700 = kelvin_to_mireds(2700);
        assert!(
            (mireds_2700 as i32 - 370).abs() <= 2,
            "2700K should be ~370 mireds, got {}",
            mireds_2700
        );

        let mireds_4000 = kelvin_to_mireds(4000);
        assert!(
            (mireds_4000 as i32 - 250).abs() <= 2,
            "4000K should be ~250 mireds, got {}",
            mireds_4000
        );

        let mireds_6500 = kelvin_to_mireds(6500);
        assert!(
            (mireds_6500 as i32 - 154).abs() <= 2,
            "6500K should be ~154 mireds, got {}",
            mireds_6500
        );
    }

    #[test]
    fn test_mireds_to_kelvin_common_values() {
        let kelvin_153 = mireds_to_kelvin(153);
        assert!(
            (kelvin_153 as i32 - 6500).abs() <= 50,
            "153 mireds should be ~6500K, got {}",
            kelvin_153
        );

        let kelvin_250 = mireds_to_kelvin(250);
        assert!(
            (kelvin_250 as i32 - 4000).abs() <= 50,
            "250 mireds should be ~4000K, got {}",
            kelvin_250
        );

        let kelvin_500 = mireds_to_kelvin(500);
        assert!(
            (kelvin_500 as i32 - 2000).abs() <= 50,
            "500 mireds should be ~2000K, got {}",
            kelvin_500
        );
    }

    #[test]
    fn test_mireds_to_kelvin_zero() {
        assert_eq!(mireds_to_kelvin(0), 6500);
    }

    #[test]
    fn test_xy_values_in_valid_range() {
        for kelvin in [1800, 2700, 4000, 5000, 6500, 10000] {
            let xy = kelvin_to_xy(kelvin);
            assert!(
                xy.x >= 0.0 && xy.x <= 1.0,
                "x out of range at {}K: {}",
                kelvin,
                xy.x
            );
            assert!(
                xy.y >= 0.0 && xy.y <= 1.0,
                "y out of range at {}K: {}",
                kelvin,
                xy.y
            );
        }
    }

    #[test]
    fn test_rgb_to_xy_roundtrip() {
        let direct_xy = kelvin_to_xy(4000);
        let rgb = kelvin_to_rgb(4000);
        let roundtrip_xy = rgb_to_xy(rgb);

        assert!(
            (direct_xy.x - roundtrip_xy.x).abs() < 0.05,
            "x mismatch: direct={}, roundtrip={}",
            direct_xy.x,
            roundtrip_xy.x
        );
        assert!(
            (direct_xy.y - roundtrip_xy.y).abs() < 0.05,
            "y mismatch: direct={}, roundtrip={}",
            direct_xy.y,
            roundtrip_xy.y
        );
    }

    #[test]
    fn test_rgb_to_xy_pure_colors() {
        let red_xy = rgb_to_xy(Rgb { r: 255, g: 0, b: 0 });
        assert!(red_xy.x > 0.6, "Pure red should have high x");

        let green_xy = rgb_to_xy(Rgb { r: 0, g: 255, b: 0 });
        assert!(green_xy.y > 0.5, "Pure green should have high y");

        let blue_xy = rgb_to_xy(Rgb { r: 0, g: 0, b: 255 });
        assert!(blue_xy.x < 0.2, "Pure blue should have low x");
    }

    #[test]
    fn test_rgb_to_xy_black() {
        let xy = rgb_to_xy(Rgb::new(0, 0, 0));
        assert_eq!(xy.x, 0.0);
        assert_eq!(xy.y, 0.0);
    }

    #[test]
    fn test_kelvin_monotonicity() {
        let xy_warm = kelvin_to_xy(2700);
        let xy_neutral = kelvin_to_xy(4000);
        let xy_cool = kelvin_to_xy(6500);

        assert!(xy_warm.x > xy_neutral.x, "Warmer should have higher x");
        assert!(
            xy_neutral.x > xy_cool.x,
            "Neutral should have higher x than cool"
        );
    }

    #[test]
    fn test_kelvin_to_xy_clamping() {
        // Below minimum should not panic
        let xy_low = kelvin_to_xy(500);
        assert!(xy_low.x > 0.0);

        // Above maximum should not panic
        let xy_high = kelvin_to_xy(30000);
        assert!(xy_high.x > 0.0);
    }

    #[test]
    fn test_rgb_from_tuple() {
        let rgb: Rgb = (128, 64, 32).into();
        assert_eq!(rgb.r, 128);
        assert_eq!(rgb.g, 64);
        assert_eq!(rgb.b, 32);
        assert_eq!(rgb.to_tuple(), (128, 64, 32));
    }

    #[test]
    fn test_xy_from_tuple() {
        let xy: XyColor = (0.5, 0.3).into();
        assert_eq!(xy.x, 0.5);
        assert_eq!(xy.y, 0.3);
        assert_eq!(xy.to_tuple(), (0.5, 0.3));
    }
}
