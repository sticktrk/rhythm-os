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
    /// Color temperature in Kelvin
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
}

impl LightingValues {
    /// Create new lighting values.
    ///
    /// Automatically calculates RGB and XY color representations
    /// from the Kelvin temperature.
    pub fn new(kelvin: u16, brightness: u8, solar_time: f32, sun_position: f32) -> Self {
        let rgb = kelvin_to_rgb(kelvin);
        let xy = kelvin_to_xy(kelvin);

        Self {
            kelvin,
            brightness,
            rgb,
            xy,
            solar_time,
            sun_position,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lighting_values_creation() {
        let values = LightingValues::new(4000, 80, 10.5, 0.5);
        assert_eq!(values.kelvin, 4000);
        assert_eq!(values.brightness, 80);
        assert_eq!(values.solar_time, 10.5);
        assert_eq!(values.sun_position, 0.5);
        assert!(values.rgb.r > 0);
        assert!(values.xy.x > 0.0);
    }

    #[test]
    fn test_lighting_values_warm() {
        let values = LightingValues::new(2700, 50, 6.0, -0.5);
        assert!(values.rgb.r > values.rgb.b);
    }

    #[test]
    fn test_lighting_values_cool() {
        let values = LightingValues::new(6500, 100, 12.0, 1.0);
        let diff = (values.rgb.r as i16 - values.rgb.b as i16).abs();
        assert!(diff < 50, "At 6500K, RGB should be relatively balanced");
    }

    #[test]
    fn test_lighting_values_xy_populated() {
        let values = LightingValues::new(4000, 50, 10.0, 0.3);
        assert!(values.xy.x > 0.0, "XY x should be positive");
        assert!(values.xy.y > 0.0, "XY y should be positive");
        assert!(values.xy.x <= 1.0, "XY x should be <= 1.0");
        assert!(values.xy.y <= 1.0, "XY y should be <= 1.0");
    }

    #[test]
    fn test_lighting_values_rgb_consistency() {
        // Warm: more red
        let warm = LightingValues::new(2000, 100, 6.0, -0.5);
        assert!(warm.rgb.r > warm.rgb.b);

        // Neutral: roughly balanced
        let neutral = LightingValues::new(5000, 100, 12.0, 1.0);
        let diff = (neutral.rgb.r as i16 - neutral.rgb.b as i16).abs();
        assert!(diff < 80, "5000K should be roughly balanced");
    }

    #[test]
    fn test_lighting_values_clone_eq() {
        let a = LightingValues::new(3000, 75, 8.0, 0.2);
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn test_lighting_values_min_brightness() {
        let values = LightingValues::new(2700, 1, 0.0, -1.0);
        assert_eq!(values.brightness, 1);
    }

    #[test]
    fn test_lighting_values_max_brightness() {
        let values = LightingValues::new(5500, 100, 12.0, 1.0);
        assert_eq!(values.brightness, 100);
    }

    #[test]
    fn test_lighting_values_extreme_kelvin() {
        // Should not panic at boundary Kelvin values
        let low = LightingValues::new(1000, 50, 6.0, 0.0);
        assert!(low.rgb.r > 0);

        let high = LightingValues::new(10000, 50, 12.0, 0.0);
        assert!(high.rgb.r > 0);
    }
}
