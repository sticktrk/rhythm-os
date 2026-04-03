//! Adaptive lighting output types.
//!
//! This module provides the [`LightingValues`] struct which is the standard
//! output type for all curve modules.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::color::{kelvin_to_rgb, kelvin_to_xy, Rgb, XyColor};

/// Output lighting values from adaptive calculations.
///
/// This struct is the standard output format for all curve modules.
/// It contains the calculated lighting parameters along with
/// solar time information.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct LightingValues {
    /// Color temperature in Kelvin (0 when is_direct_color is true)
    pub kelvin: u16,

    /// Brightness percentage (1-100)
    pub brightness: u8,

    /// RGB color representation
    pub rgb: Rgb,

    /// CIE xy color coordinates
    pub xy: XyColor,

    /// Current solar time (0-24)
    pub solar_time: f32,

    /// Sun position (-1 to +1)
    pub sun_position: f32,

    /// When true, rgb/xy are authoritative (not derived from kelvin).
    /// Used for curves that output arbitrary colors outside the CCT spectrum.
    #[cfg_attr(feature = "serde", serde(default))]
    pub is_direct_color: bool,

    /// Transition time in milliseconds for this lighting state.
    /// Set by the curve module to control how fast lights fade to these values.
    pub transition_ms: u32,

    /// Default motion timeout in seconds for this lighting state.
    /// Set by the curve module; per-room overrides take precedence.
    pub motion_timeout_secs: u16,

    /// Suggested tick interval in seconds based on curve rate of change.
    /// None means use the configured default interval.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub suggested_tick_interval_secs: Option<u16>,
}

impl LightingValues {
    /// Create new lighting values.
    ///
    /// Automatically calculates RGB and XY color representations
    /// from the Kelvin temperature.
    pub fn new(
        kelvin: u16,
        brightness: u8,
        solar_time: f32,
        sun_position: f32,
        transition_ms: u32,
        motion_timeout_secs: u16,
    ) -> Self {
        let rgb = kelvin_to_rgb(kelvin);
        let xy = kelvin_to_xy(kelvin);

        Self {
            kelvin,
            brightness,
            rgb,
            xy,
            solar_time,
            sun_position,
            is_direct_color: false,
            transition_ms,
            motion_timeout_secs,
            suggested_tick_interval_secs: None,
        }
    }

    /// Create lighting values from arbitrary color (not kelvin-derived).
    ///
    /// Used by curves that output colors outside the normal CCT spectrum.
    /// The `kelvin` field is set to 0 since it's not meaningful for direct colors.
    pub fn from_color(
        rgb: Rgb,
        xy: XyColor,
        brightness: u8,
        solar_time: f32,
        sun_position: f32,
        transition_ms: u32,
        motion_timeout_secs: u16,
    ) -> Self {
        Self {
            kelvin: 0,
            brightness,
            rgb,
            xy,
            solar_time,
            sun_position,
            is_direct_color: true,
            transition_ms,
            motion_timeout_secs,
            suggested_tick_interval_secs: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lighting_values_creation() {
        let values = LightingValues::new(4000, 80, 10.5, 0.5, 500, 600);
        assert_eq!(values.kelvin, 4000);
        assert_eq!(values.brightness, 80);
        assert_eq!(values.solar_time, 10.5);
        assert_eq!(values.sun_position, 0.5);
        assert!(values.rgb.r > 0);
        assert!(values.xy.x > 0.0);
    }

    #[test]
    fn test_lighting_values_warm() {
        let values = LightingValues::new(2700, 50, 6.0, -0.5, 500, 600);
        assert!(values.rgb.r > values.rgb.b);
    }

    #[test]
    fn test_lighting_values_cool() {
        let values = LightingValues::new(6500, 100, 12.0, 1.0, 500, 600);
        let diff = (values.rgb.r as i16 - values.rgb.b as i16).abs();
        assert!(diff < 50, "At 6500K, RGB should be relatively balanced");
    }

    #[test]
    fn test_lighting_values_xy_populated() {
        let values = LightingValues::new(4000, 50, 10.0, 0.3, 500, 600);
        assert!(values.xy.x > 0.0, "XY x should be positive");
        assert!(values.xy.y > 0.0, "XY y should be positive");
        assert!(values.xy.x <= 1.0, "XY x should be <= 1.0");
        assert!(values.xy.y <= 1.0, "XY y should be <= 1.0");
    }

    #[test]
    fn test_lighting_values_rgb_consistency() {
        // Warm: more red
        let warm = LightingValues::new(2000, 100, 6.0, -0.5, 500, 600);
        assert!(warm.rgb.r > warm.rgb.b);

        // Neutral: roughly balanced
        let neutral = LightingValues::new(5000, 100, 12.0, 1.0, 500, 600);
        let diff = (neutral.rgb.r as i16 - neutral.rgb.b as i16).abs();
        assert!(diff < 80, "5000K should be roughly balanced");
    }

    #[test]
    fn test_lighting_values_clone_eq() {
        let a = LightingValues::new(3000, 75, 8.0, 0.2, 500, 600);
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn test_lighting_values_min_brightness() {
        let values = LightingValues::new(2700, 1, 0.0, -1.0, 500, 600);
        assert_eq!(values.brightness, 1);
    }

    #[test]
    fn test_lighting_values_max_brightness() {
        let values = LightingValues::new(5500, 100, 12.0, 1.0, 500, 600);
        assert_eq!(values.brightness, 100);
    }

    #[test]
    fn test_from_color_sets_direct_color() {
        let rgb = Rgb::new(255, 40, 150);
        let xy = XyColor { x: 0.45, y: 0.25 };
        let values = LightingValues::from_color(rgb, xy, 1, 12.0, 0.0, 500, 600);

        assert!(values.is_direct_color);
        assert_eq!(values.kelvin, 0);
        assert_eq!(values.rgb, rgb);
        assert_eq!(values.xy, xy);
        assert_eq!(values.brightness, 1);
    }

    #[test]
    fn test_lighting_values_extreme_kelvin() {
        // Should not panic at boundary Kelvin values
        let low = LightingValues::new(1000, 50, 6.0, 0.0, 500, 600);
        assert!(low.rgb.r > 0);

        let high = LightingValues::new(10000, 50, 12.0, 0.0, 500, 600);
        assert!(high.rgb.r > 0);
    }
}
