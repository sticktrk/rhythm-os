//! Light profile configuration.
//!
//! A [`LightProfileConfig`] is a complete, serializable profile definition.
//! Copy the JSON, tweak values, and you have a new profile.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::config::{
    DEFAULT_MAX_BRIGHTNESS, DEFAULT_MAX_COLOR_TEMP, DEFAULT_MAX_DIM_STEPS, DEFAULT_MIN_BRIGHTNESS,
    DEFAULT_MIN_COLOR_TEMP,
};
use crate::curve_shape::{LightCurveShape, LightDirectColor};

/// Default fade duration in milliseconds.
pub const DEFAULT_FADE_MS: u16 = 500;

/// Complete light profile configuration — the full JSON-serializable config.
///
/// A profile combines a curve shape with output ranges, timer settings,
/// and optional color overrides. All profiles use the same struct — the
/// curve shape variant determines the mathematical curve type.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct LightProfileConfig {
    /// Unique identifier (e.g., "rhythm", "sleep", "idle")
    pub id: String,

    /// Human-readable display name
    pub name: String,

    /// The curve shape and its parameters
    #[cfg_attr(feature = "serde", serde(default))]
    pub curve: LightCurveShape,

    // ── Output ranges ────────────────────────────────────────────
    /// Minimum brightness percentage (1–100)
    #[cfg_attr(feature = "serde", serde(default = "default_min_brightness"))]
    pub min_brightness: u8,

    /// Maximum brightness percentage (1–100)
    #[cfg_attr(feature = "serde", serde(default = "default_max_brightness"))]
    pub max_brightness: u8,

    /// Minimum color temperature in Kelvin
    #[cfg_attr(feature = "serde", serde(default = "default_min_color_temp"))]
    pub min_color_temp: u16,

    /// Maximum color temperature in Kelvin
    #[cfg_attr(feature = "serde", serde(default = "default_max_color_temp"))]
    pub max_color_temp: u16,

    // ── Step / timer settings ────────────────────────────────────
    /// Maximum number of dimming steps
    #[cfg_attr(feature = "serde", serde(default = "default_max_dim_steps"))]
    pub max_dim_steps: u8,

    /// Light transition fade duration in milliseconds.
    /// `None` = auto (default 500ms).
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub fade_ms: Option<u16>,

    /// Motion timeout in seconds.
    /// `None` = auto (varies by time of day for super-gaussian).
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub motion_timeout_secs: Option<u16>,

    // ── Color override ───────────────────────────────────────────
    /// When set, the profile outputs this fixed color instead of
    /// deriving color from Kelvin. Used for modes like sleep.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub direct_color: Option<LightDirectColor>,
}

impl LightProfileConfig {
    /// Calculate the brightness step size based on max_dim_steps.
    pub fn brightness_step_size(&self) -> f32 {
        let range = (self.max_brightness - self.min_brightness) as f32;
        range / self.max_dim_steps.max(1) as f32
    }

    /// Get the effective fade duration in milliseconds.
    pub fn effective_fade_ms(&self) -> u16 {
        self.fade_ms.unwrap_or(DEFAULT_FADE_MS)
    }
}

// Serde default functions
#[cfg(feature = "serde")]
fn default_min_brightness() -> u8 {
    DEFAULT_MIN_BRIGHTNESS
}
#[cfg(feature = "serde")]
fn default_max_brightness() -> u8 {
    DEFAULT_MAX_BRIGHTNESS
}
#[cfg(feature = "serde")]
fn default_min_color_temp() -> u16 {
    DEFAULT_MIN_COLOR_TEMP
}
#[cfg(feature = "serde")]
fn default_max_color_temp() -> u16 {
    DEFAULT_MAX_COLOR_TEMP
}
#[cfg(feature = "serde")]
fn default_max_dim_steps() -> u8 {
    DEFAULT_MAX_DIM_STEPS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve_shape::LightCurveShape;

    fn test_config() -> LightProfileConfig {
        LightProfileConfig {
            id: "test".into(),
            name: "Test Profile".into(),
            curve: LightCurveShape::default(),
            min_brightness: DEFAULT_MIN_BRIGHTNESS,
            max_brightness: DEFAULT_MAX_BRIGHTNESS,
            min_color_temp: DEFAULT_MIN_COLOR_TEMP,
            max_color_temp: DEFAULT_MAX_COLOR_TEMP,
            max_dim_steps: DEFAULT_MAX_DIM_STEPS,
            fade_ms: None,
            motion_timeout_secs: None,
            direct_color: None,
        }
    }

    #[test]
    fn test_brightness_step_size() {
        let mut config = test_config();
        config.min_brightness = 1;
        config.max_brightness = 100;
        config.max_dim_steps = 10;
        assert!((config.brightness_step_size() - 9.9).abs() < 0.1);
    }

    #[test]
    fn test_effective_fade_ms() {
        let mut config = test_config();
        assert_eq!(config.effective_fade_ms(), 500);
        config.fade_ms = Some(300);
        assert_eq!(config.effective_fade_ms(), 300);
    }

    #[cfg(feature = "serde")]
    mod serde_tests {
        use super::*;

        #[test]
        fn test_full_roundtrip() {
            let config = test_config();
            let json = serde_json::to_string(&config).unwrap();
            let back: LightProfileConfig = serde_json::from_str(&json).unwrap();
            assert_eq!(config, back);
        }

        #[test]
        fn test_minimal_json_uses_defaults() {
            let json = r#"{"id": "test", "name": "Test", "curve": {"type": "super-gaussian"}}"#;
            let config: LightProfileConfig = serde_json::from_str(json).unwrap();
            assert_eq!(config.min_brightness, DEFAULT_MIN_BRIGHTNESS);
            assert_eq!(config.max_brightness, DEFAULT_MAX_BRIGHTNESS);
            assert_eq!(config.max_dim_steps, DEFAULT_MAX_DIM_STEPS);
        }

        #[test]
        fn test_palette_profile_roundtrip() {
            let config = LightProfileConfig {
                id: "idle".into(),
                name: "Idle Profile".into(),
                curve: LightCurveShape::default_idle_palette(),
                min_brightness: 1,
                max_brightness: 1,
                min_color_temp: 0,
                max_color_temp: 0,
                max_dim_steps: 1,
                fade_ms: None,
                motion_timeout_secs: None,
                direct_color: None,
            };
            let json = serde_json::to_string_pretty(&config).unwrap();
            let back: LightProfileConfig = serde_json::from_str(&json).unwrap();
            assert_eq!(config, back);
        }
    }
}
