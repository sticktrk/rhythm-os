//! Common configuration types shared by light profiles.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Default color temperature range
pub const DEFAULT_MIN_COLOR_TEMP: u16 = 1800;
pub const DEFAULT_MAX_COLOR_TEMP: u16 = 5500;

/// Default brightness range (percentage)
pub const DEFAULT_MIN_BRIGHTNESS: u8 = 20;
pub const DEFAULT_MAX_BRIGHTNESS: u8 = 100;

/// Default dimming steps
pub const DEFAULT_MAX_DIM_STEPS: u8 = 6;

/// Common curve configuration shared by all modules.
///
/// This contains the basic min/max ranges that apply to any light profile.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct CommonCurveConfig {
    /// Minimum color temperature in Kelvin
    #[cfg_attr(feature = "serde", serde(default = "default_min_color_temp"))]
    pub min_color_temp: u16,

    /// Maximum color temperature in Kelvin
    #[cfg_attr(feature = "serde", serde(default = "default_max_color_temp"))]
    pub max_color_temp: u16,

    /// Minimum brightness percentage (1-100)
    #[cfg_attr(feature = "serde", serde(default = "default_min_brightness"))]
    pub min_brightness: u8,

    /// Maximum brightness percentage (1-100)
    #[cfg_attr(feature = "serde", serde(default = "default_max_brightness"))]
    pub max_brightness: u8,

    /// Maximum number of dimming steps
    #[cfg_attr(feature = "serde", serde(default = "default_max_dim_steps"))]
    pub max_dim_steps: u8,
}

impl Default for CommonCurveConfig {
    fn default() -> Self {
        Self {
            min_color_temp: DEFAULT_MIN_COLOR_TEMP,
            max_color_temp: DEFAULT_MAX_COLOR_TEMP,
            min_brightness: DEFAULT_MIN_BRIGHTNESS,
            max_brightness: DEFAULT_MAX_BRIGHTNESS,
            max_dim_steps: DEFAULT_MAX_DIM_STEPS,
        }
    }
}

impl CommonCurveConfig {
    /// Calculate the brightness step size based on max_dim_steps.
    pub fn brightness_step_size(&self) -> f32 {
        let range = (self.max_brightness - self.min_brightness) as f32;
        range / self.max_dim_steps.max(1) as f32
    }
}

// Serde default functions
#[cfg(feature = "serde")]
fn default_min_color_temp() -> u16 {
    DEFAULT_MIN_COLOR_TEMP
}

#[cfg(feature = "serde")]
fn default_max_color_temp() -> u16 {
    DEFAULT_MAX_COLOR_TEMP
}

#[cfg(feature = "serde")]
fn default_min_brightness() -> u8 {
    DEFAULT_MIN_BRIGHTNESS
}

#[cfg(feature = "serde")]
fn default_max_brightness() -> u8 {
    DEFAULT_MAX_BRIGHTNESS
}

#[cfg(feature = "serde")]
fn default_max_dim_steps() -> u8 {
    DEFAULT_MAX_DIM_STEPS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_common_config_default() {
        let config = CommonCurveConfig::default();
        assert_eq!(config.min_color_temp, DEFAULT_MIN_COLOR_TEMP);
        assert_eq!(config.max_color_temp, DEFAULT_MAX_COLOR_TEMP);
        assert_eq!(config.min_brightness, DEFAULT_MIN_BRIGHTNESS);
        assert_eq!(config.max_brightness, DEFAULT_MAX_BRIGHTNESS);
        assert_eq!(config.max_dim_steps, DEFAULT_MAX_DIM_STEPS);
    }

    #[test]
    fn test_common_config_brightness_step_size() {
        let config = CommonCurveConfig {
            min_brightness: 1,
            max_brightness: 100,
            max_dim_steps: 10,
            ..Default::default()
        };

        let step_size = config.brightness_step_size();
        assert!((step_size - 9.9).abs() < 0.1);
    }

    #[test]
    fn test_common_config_step_size_zero_steps() {
        let config = CommonCurveConfig {
            min_brightness: 2,
            max_brightness: 100,
            max_dim_steps: 0,
            ..Default::default()
        };
        // Should not panic; max(1) guard
        let step_size = config.brightness_step_size();
        assert!((step_size - 98.0).abs() < 0.1);
    }

    #[test]
    fn test_common_config_step_size_one_step() {
        let config = CommonCurveConfig {
            min_brightness: 10,
            max_brightness: 90,
            max_dim_steps: 1,
            ..Default::default()
        };
        let step_size = config.brightness_step_size();
        assert!((step_size - 80.0).abs() < 0.1);
    }

    #[test]
    fn test_common_config_clone_eq() {
        let config = CommonCurveConfig {
            min_color_temp: 2000,
            max_color_temp: 6000,
            min_brightness: 5,
            max_brightness: 95,
            max_dim_steps: 8,
        };
        let cloned = config.clone();
        assert_eq!(config, cloned);
    }

    #[test]
    fn test_constants_match_defaults() {
        assert_eq!(DEFAULT_MIN_BRIGHTNESS, 20);
        let config = CommonCurveConfig::default();
        assert_eq!(config.min_color_temp, DEFAULT_MIN_COLOR_TEMP);
        assert_eq!(config.max_color_temp, DEFAULT_MAX_COLOR_TEMP);
        assert_eq!(config.min_brightness, DEFAULT_MIN_BRIGHTNESS);
        assert_eq!(config.max_brightness, DEFAULT_MAX_BRIGHTNESS);
        assert_eq!(config.max_dim_steps, DEFAULT_MAX_DIM_STEPS);
    }
}
