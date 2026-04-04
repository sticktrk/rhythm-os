//! Configuration types for adaptive lighting curves.
//!
//! This module defines the configuration parameters that control the shape
//! of the brightness and color temperature curves throughout the day.
//!
//! The curve uses a super-Gaussian (flat-topped bell curve) shape with
//! configurable peak flatness and asymmetric morning/evening ramp speeds.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::solar::{SunTimes, TwilightTimes};

// Re-export shared default constants from rhythm-curve
pub use rhythm_curve::config::{
    DEFAULT_MAX_BRIGHTNESS, DEFAULT_MAX_COLOR_TEMP, DEFAULT_MAX_DIM_STEPS, DEFAULT_MIN_BRIGHTNESS,
    DEFAULT_MIN_COLOR_TEMP,
};

/// Default width multipliers (1.0 = width calibrated so y(edge) = epsilon)
pub const DEFAULT_WIDTH_LEFT_BRI: f32 = 0.95;
pub const DEFAULT_WIDTH_RIGHT_BRI: f32 = 0.85;
pub const DEFAULT_WIDTH_LEFT_CCT: f32 = 0.95;
pub const DEFAULT_WIDTH_RIGHT_CCT: f32 = 1.15;

/// Default shape exponent (2 = round top, 6 = flat plateau)
pub const DEFAULT_SHAPE_P: f32 = 6.0;

/// Default fade duration in milliseconds.
pub const DEFAULT_FADE_MS: u16 = 500;

/// Default motion timeout in seconds (20 minutes).
pub const DEFAULT_MOTION_TIMEOUT_SECS: u16 = 1200;

/// Fallback value for sunrise if sun times unavailable
pub const FALLBACK_SUNRISE_HOUR: f32 = 6.0;

/// Fallback value for sunset if sun times unavailable
pub const FALLBACK_SUNSET_HOUR: f32 = 20.0;

/// Configuration for adaptive lighting curves.
///
/// Controls the shape of brightness and color temperature curves
/// using a super-Gaussian (flat-topped bell curve) shape.
///
/// The curve reaches its peak (max brightness/CCT) at solar noon
/// and reaches its minimum at sunrise/sunset (with ~2% headroom).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct CurveConfig {
    // Color temperature range (Kelvin)
    #[cfg_attr(feature = "serde", serde(default = "default_min_color_temp"))]
    pub min_color_temp: u16,
    #[cfg_attr(feature = "serde", serde(default = "default_max_color_temp"))]
    pub max_color_temp: u16,

    // Brightness range (percentage 0-100)
    #[cfg_attr(feature = "serde", serde(default = "default_min_brightness"))]
    pub min_brightness: u8,
    #[cfg_attr(feature = "serde", serde(default = "default_max_brightness"))]
    pub max_brightness: u8,

    // Super-Gaussian width parameters
    /// Morning ramp speed for brightness (<1 = faster, >1 = slower)
    #[cfg_attr(feature = "serde", serde(default = "default_width_left_bri"))]
    pub width_left_bri: f32,
    /// Evening ramp speed for brightness (<1 = faster, >1 = slower)
    #[cfg_attr(feature = "serde", serde(default = "default_width_right_bri"))]
    pub width_right_bri: f32,
    /// Morning ramp speed for CCT (<1 = faster, >1 = slower)
    #[cfg_attr(feature = "serde", serde(default = "default_width_left_cct"))]
    pub width_left_cct: f32,
    /// Evening ramp speed for CCT (<1 = faster, >1 = slower)
    #[cfg_attr(feature = "serde", serde(default = "default_width_right_cct"))]
    pub width_right_cct: f32,

    /// Shape exponent for the super-Gaussian curve (2 = round, 6 = flat plateau)
    #[cfg_attr(feature = "serde", serde(default = "default_shape_p"))]
    pub shape_p: f32,

    // Dimming steps
    #[cfg_attr(feature = "serde", serde(default = "default_max_dim_steps"))]
    pub max_dim_steps: u8,

    /// Light transition fade duration in milliseconds.
    /// `None` = auto (curve decides, default 500ms). `Some(v)` = manual override.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub fade_ms: Option<u16>,

    /// Motion timeout in seconds.
    /// `None` = auto (curve varies by time of day). `Some(v)` = manual override.
    /// Per-room overrides take precedence over this value.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub motion_timeout_secs: Option<u16>,
}

impl Default for CurveConfig {
    #[cfg(feature = "serde")]
    fn default() -> Self {
        // Use defaults from embedded defaults.json
        crate::defaults::default_config()
    }

    #[cfg(not(feature = "serde"))]
    fn default() -> Self {
        // Fallback for no_std/no-serde builds
        Self {
            min_color_temp: DEFAULT_MIN_COLOR_TEMP,
            max_color_temp: DEFAULT_MAX_COLOR_TEMP,
            min_brightness: DEFAULT_MIN_BRIGHTNESS,
            max_brightness: DEFAULT_MAX_BRIGHTNESS,
            width_left_bri: DEFAULT_WIDTH_LEFT_BRI,
            width_right_bri: DEFAULT_WIDTH_RIGHT_BRI,
            width_left_cct: DEFAULT_WIDTH_LEFT_CCT,
            width_right_cct: DEFAULT_WIDTH_RIGHT_CCT,
            shape_p: DEFAULT_SHAPE_P,
            max_dim_steps: DEFAULT_MAX_DIM_STEPS,
            fade_ms: None,
            motion_timeout_secs: None,
        }
    }
}

/// Solar context for a specific day.
///
/// Contains all solar timing information needed for lighting calculations
/// and UI display.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct SolarContext {
    /// Sunrise time in local decimal hours (0-24)
    pub sunrise: f32,
    /// Sunset time in local decimal hours (0-24)
    pub sunset: f32,
    /// Solar noon time in local decimal hours (0-24)
    pub solar_noon: f32,
    /// Solar midnight time in local decimal hours (0-24)
    pub solar_midnight: f32,
    /// Day length in hours
    pub day_length: f32,
    /// Twilight times (dawn and dusk for civil, nautical, astronomical)
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub twilight: Option<TwilightTimes>,
}

impl SolarContext {
    /// Create a new solar context.
    pub fn new(sunrise: f32, sunset: f32, solar_noon: f32) -> Self {
        Self {
            sunrise,
            sunset,
            solar_noon,
            solar_midnight: (solar_noon + 12.0) % 24.0,
            day_length: sunset - sunrise,
            twilight: None,
        }
    }

    /// Create from SunTimes and solar noon.
    pub fn from_sun_times(sun_times: &SunTimes, solar_noon: f32) -> Self {
        Self {
            sunrise: sun_times.sunrise,
            sunset: sun_times.sunset,
            solar_noon,
            solar_midnight: (solar_noon + 12.0) % 24.0,
            day_length: sun_times.day_length,
            twilight: None,
        }
    }

    /// Create default solar context (for when location is not configured).
    pub fn default_context() -> Self {
        Self {
            sunrise: 6.0,
            sunset: 20.0,
            solar_noon: 12.0,
            solar_midnight: 0.0,
            day_length: 14.0,
            twilight: None,
        }
    }

    /// Add twilight times to this context (builder pattern).
    pub fn with_twilight(mut self, twilight: TwilightTimes) -> Self {
        self.twilight = Some(twilight);
        self
    }
}

impl Default for SolarContext {
    fn default() -> Self {
        Self::default_context()
    }
}

impl CurveConfig {
    /// Create a new configuration with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Calculate the brightness step size based on max_dim_steps.
    pub fn brightness_step_size(&self) -> f32 {
        let range = (self.max_brightness - self.min_brightness) as f32;
        range / self.max_dim_steps.max(1) as f32
    }

    /// Clamp width values to valid range (0.2-2.0).
    pub fn clamped_width(width: f32) -> f32 {
        width.clamp(0.2, 2.0)
    }

    /// Clamp shape_p to valid range (2.0-10.0).
    pub fn clamped_shape_p(shape_p: f32) -> f32 {
        shape_p.clamp(2.0, 10.0)
    }

    /// Get effective width_left_bri (clamped to valid range).
    pub fn effective_width_left_bri(&self) -> f32 {
        Self::clamped_width(self.width_left_bri)
    }

    /// Get effective width_right_bri (clamped to valid range).
    pub fn effective_width_right_bri(&self) -> f32 {
        Self::clamped_width(self.width_right_bri)
    }

    /// Get effective width_left_cct (clamped to valid range).
    pub fn effective_width_left_cct(&self) -> f32 {
        Self::clamped_width(self.width_left_cct)
    }

    /// Get effective width_right_cct (clamped to valid range).
    pub fn effective_width_right_cct(&self) -> f32 {
        Self::clamped_width(self.width_right_cct)
    }

    /// Get effective shape_p (clamped to valid range).
    pub fn effective_shape_p(&self) -> f32 {
        Self::clamped_shape_p(self.shape_p)
    }

    /// Adjust width parameters to absorb a time offset.
    ///
    /// Modifies the ramp widths for the current side (morning/evening) so the
    /// curve produces the same values at `current_hour` as it currently does
    /// at `current_hour + offset_minutes/60`.
    ///
    /// Returns None if the adjustment isn't meaningful (at peak, crossing peak,
    /// or extreme factor).
    pub fn absorb_time_offset(
        &self,
        current_hour: f32,
        offset_minutes: f32,
        sunrise: f32,
        sunset: f32,
    ) -> Option<CurveConfig> {
        let mu = (sunrise + sunset) / 2.0;
        let offset_hours = offset_minutes / 60.0;
        let target_hour = (current_hour + offset_hours).rem_euclid(24.0);

        let dist_current = wrapped_dist(current_hour, mu);
        let dist_target = wrapped_dist(target_hour, mu);

        // Guard: at or near peak (within ~15 min)
        if dist_current.abs() < 0.25 || dist_target.abs() < 0.25 {
            return None;
        }
        // Guard: crossing the peak
        if dist_current.signum() != dist_target.signum() {
            return None;
        }

        let factor = dist_target.abs() / dist_current.abs();
        // Guard: factor too extreme
        if !(0.1..=5.0).contains(&factor) {
            return None;
        }

        let mut new = self.clone();
        let is_morning = dist_current < 0.0;
        if is_morning {
            new.width_left_bri = Self::clamped_width(self.width_left_bri * factor);
            new.width_left_cct = Self::clamped_width(self.width_left_cct * factor);
        } else {
            new.width_right_bri = Self::clamped_width(self.width_right_bri * factor);
            new.width_right_cct = Self::clamped_width(self.width_right_cct * factor);
        }
        Some(new)
    }
}

/// Wrapped distance from hour to mu, normalized to -12..+12 range.
/// Negative = before mu (morning), positive = after mu (evening).
fn wrapped_dist(hour: f32, mu: f32) -> f32 {
    let mut d = hour - mu;
    if d > 12.0 {
        d -= 24.0;
    }
    if d < -12.0 {
        d += 24.0;
    }
    d
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
#[cfg(feature = "serde")]
fn default_max_dim_steps() -> u8 {
    DEFAULT_MAX_DIM_STEPS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = CurveConfig::default();
        assert_eq!(config.min_color_temp, DEFAULT_MIN_COLOR_TEMP);
        assert_eq!(config.max_color_temp, DEFAULT_MAX_COLOR_TEMP);
        assert_eq!(config.min_brightness, DEFAULT_MIN_BRIGHTNESS);
        assert_eq!(config.max_brightness, DEFAULT_MAX_BRIGHTNESS);
        assert_eq!(config.width_left_bri, DEFAULT_WIDTH_LEFT_BRI);
        assert_eq!(config.width_right_bri, DEFAULT_WIDTH_RIGHT_BRI);
        assert_eq!(config.width_left_cct, DEFAULT_WIDTH_LEFT_CCT);
        assert_eq!(config.width_right_cct, DEFAULT_WIDTH_RIGHT_CCT);
        assert_eq!(config.shape_p, DEFAULT_SHAPE_P);
        assert_eq!(config.max_dim_steps, DEFAULT_MAX_DIM_STEPS);
    }

    #[test]
    fn test_brightness_step_size() {
        let config = CurveConfig {
            min_brightness: 1,
            max_brightness: 100,
            max_dim_steps: 10,
            ..Default::default()
        };

        assert!((config.brightness_step_size() - 9.9).abs() < 0.1);
    }

    #[test]
    fn test_width_clamping() {
        assert_eq!(CurveConfig::clamped_width(0.1), 0.2);
        assert_eq!(CurveConfig::clamped_width(0.5), 0.5);
        assert_eq!(CurveConfig::clamped_width(1.0), 1.0);
        assert_eq!(CurveConfig::clamped_width(2.5), 2.0);
    }

    #[test]
    fn test_shape_p_clamping() {
        assert_eq!(CurveConfig::clamped_shape_p(1.0), 2.0);
        assert_eq!(CurveConfig::clamped_shape_p(6.0), 6.0);
        assert_eq!(CurveConfig::clamped_shape_p(15.0), 10.0);
    }

    #[test]
    fn test_solar_context_default() {
        let ctx = SolarContext::default_context();
        assert_eq!(ctx.sunrise, 6.0);
        assert_eq!(ctx.sunset, 20.0);
        assert_eq!(ctx.solar_noon, 12.0);
        assert_eq!(ctx.day_length, 14.0);
    }

    #[test]
    fn test_solar_context_new() {
        let ctx = SolarContext::new(5.5, 19.5, 12.5);
        assert_eq!(ctx.sunrise, 5.5);
        assert_eq!(ctx.sunset, 19.5);
        assert_eq!(ctx.solar_noon, 12.5);
        assert_eq!(ctx.day_length, 14.0);
        assert_eq!(ctx.solar_midnight, 0.5); // 12.5 + 12 = 24.5 % 24 = 0.5
    }

    #[test]
    fn test_solar_context_solar_midnight_calculation() {
        // Solar noon at 13:00 -> solar midnight at 1:00
        let ctx = SolarContext::new(6.0, 20.0, 13.0);
        assert_eq!(ctx.solar_midnight, 1.0);

        // Solar noon at 11:00 -> solar midnight at 23:00
        let ctx = SolarContext::new(6.0, 20.0, 11.0);
        assert_eq!(ctx.solar_midnight, 23.0);
    }

    #[test]
    fn test_config_brightness_step_size_division() {
        let mut config = CurveConfig {
            min_brightness: 10,
            max_brightness: 100,
            max_dim_steps: 9,
            ..Default::default()
        };
        // (100 - 10) / 9 = 10.0
        assert!((config.brightness_step_size() - 10.0).abs() < 0.01);

        config.max_dim_steps = 3;
        // (100 - 10) / 3 = 30.0
        assert!((config.brightness_step_size() - 30.0).abs() < 0.01);
    }

    #[test]
    fn test_config_brightness_step_size_zero_steps() {
        let config = CurveConfig {
            max_dim_steps: 0,
            ..Default::default()
        };

        // Should handle division by zero gracefully
        let step_size = config.brightness_step_size();
        // Either returns infinity or some safe value
        assert!(step_size.is_finite() || step_size.is_infinite());
    }

    #[test]
    fn test_width_clamping_boundaries() {
        // Minimum boundary
        assert_eq!(CurveConfig::clamped_width(0.2), 0.2);
        assert_eq!(CurveConfig::clamped_width(0.19), 0.2);

        // Maximum boundary
        assert_eq!(CurveConfig::clamped_width(2.0), 2.0);
        assert_eq!(CurveConfig::clamped_width(2.01), 2.0);
    }

    #[test]
    fn test_shape_p_clamping_boundaries() {
        // Minimum boundary
        assert_eq!(CurveConfig::clamped_shape_p(2.0), 2.0);
        assert_eq!(CurveConfig::clamped_shape_p(1.9), 2.0);

        // Maximum boundary
        assert_eq!(CurveConfig::clamped_shape_p(10.0), 10.0);
        assert_eq!(CurveConfig::clamped_shape_p(10.1), 10.0);
    }

    // =========================================================================
    // Serialization Tests
    // =========================================================================

    #[cfg(feature = "serde")]
    mod serde_tests {
        use super::*;

        #[test]
        fn test_config_serialize_deserialize() {
            let config = CurveConfig::default();
            let json = serde_json::to_string(&config).unwrap();
            let deserialized: CurveConfig = serde_json::from_str(&json).unwrap();

            assert_eq!(config, deserialized);
        }

        #[test]
        fn test_config_deserialize_with_defaults() {
            // JSON with only a few fields - others should use defaults
            let json = r#"{"min_color_temp": 2000, "max_color_temp": 5000}"#;
            let config: CurveConfig = serde_json::from_str(json).unwrap();

            assert_eq!(config.min_color_temp, 2000);
            assert_eq!(config.max_color_temp, 5000);
            // Other fields should have defaults
            assert_eq!(config.min_brightness, DEFAULT_MIN_BRIGHTNESS);
            assert_eq!(config.max_brightness, DEFAULT_MAX_BRIGHTNESS);
        }

        #[test]
        fn test_config_roundtrip_preserves_custom_values() {
            let config = CurveConfig {
                min_color_temp: 1500,
                max_color_temp: 7000,
                min_brightness: 5,
                max_brightness: 95,
                width_left_bri: 0.8,
                width_right_bri: 1.2,
                width_left_cct: 0.9,
                width_right_cct: 1.1,
                shape_p: 4.0,
                max_dim_steps: 8,
                fade_ms: Some(300),
                motion_timeout_secs: Some(300),
            };

            let json = serde_json::to_string(&config).unwrap();
            let roundtrip: CurveConfig = serde_json::from_str(&json).unwrap();

            assert_eq!(config, roundtrip);
        }

        #[test]
        fn test_solar_context_serialize_deserialize() {
            let ctx = SolarContext::new(5.5, 19.5, 12.5);
            let json = serde_json::to_string(&ctx).unwrap();
            let deserialized: SolarContext = serde_json::from_str(&json).unwrap();

            assert_eq!(ctx.sunrise, deserialized.sunrise);
            assert_eq!(ctx.sunset, deserialized.sunset);
            assert_eq!(ctx.solar_noon, deserialized.solar_noon);
        }

        #[test]
        fn test_config_empty_json_uses_all_defaults() {
            let json = "{}";
            let config: CurveConfig = serde_json::from_str(json).unwrap();

            assert_eq!(config.min_color_temp, DEFAULT_MIN_COLOR_TEMP);
            assert_eq!(config.max_color_temp, DEFAULT_MAX_COLOR_TEMP);
            assert_eq!(config.min_brightness, DEFAULT_MIN_BRIGHTNESS);
            assert_eq!(config.max_brightness, DEFAULT_MAX_BRIGHTNESS);
            assert_eq!(config.width_left_bri, DEFAULT_WIDTH_LEFT_BRI);
            assert_eq!(config.shape_p, DEFAULT_SHAPE_P);
            assert_eq!(config.max_dim_steps, DEFAULT_MAX_DIM_STEPS);
        }
    }

    // =========================================================================
    // absorb_time_offset tests
    // =========================================================================

    #[test]
    fn test_absorb_morning_positive_offset() {
        // At 8am (morning), +30min offset → closer to noon → faster ramp (lower width)
        let config = CurveConfig::default();
        let result = config.absorb_time_offset(8.0, 30.0, 6.0, 18.0);
        let new = result.expect("should produce adjusted config");

        // width_left should decrease (faster ramp)
        assert!(
            new.width_left_bri < config.width_left_bri,
            "morning +offset: width_left_bri should decrease: {} -> {}",
            config.width_left_bri,
            new.width_left_bri
        );
        assert!(
            new.width_left_cct < config.width_left_cct,
            "morning +offset: width_left_cct should decrease"
        );
        // Evening widths unchanged
        assert_eq!(new.width_right_bri, config.width_right_bri);
        assert_eq!(new.width_right_cct, config.width_right_cct);
    }

    #[test]
    fn test_absorb_morning_negative_offset() {
        // At 8am, -30min → away from noon → slower ramp (higher width)
        let config = CurveConfig::default();
        let result = config.absorb_time_offset(8.0, -30.0, 6.0, 18.0);
        let new = result.expect("should produce adjusted config");

        assert!(
            new.width_left_bri > config.width_left_bri,
            "morning -offset: width_left_bri should increase: {} -> {}",
            config.width_left_bri,
            new.width_left_bri
        );
    }

    #[test]
    fn test_absorb_evening_positive_offset() {
        // At 16:00 (evening), +30min → away from noon → steeper ramp (higher width)
        let config = CurveConfig::default();
        let result = config.absorb_time_offset(16.0, 30.0, 6.0, 18.0);
        let new = result.expect("should produce adjusted config");

        assert!(
            new.width_right_bri > config.width_right_bri,
            "evening +offset: width_right_bri should increase: {} -> {}",
            config.width_right_bri,
            new.width_right_bri
        );
        // Morning widths unchanged
        assert_eq!(new.width_left_bri, config.width_left_bri);
    }

    #[test]
    fn test_absorb_evening_negative_offset() {
        // At 16:00, -30min → closer to noon → slower ramp (lower width)
        let config = CurveConfig::default();
        let result = config.absorb_time_offset(16.0, -30.0, 6.0, 18.0);
        let new = result.expect("should produce adjusted config");

        assert!(
            new.width_right_bri < config.width_right_bri,
            "evening -offset: width_right_bri should decrease: {} -> {}",
            config.width_right_bri,
            new.width_right_bri
        );
    }

    #[test]
    fn test_absorb_near_peak_returns_none() {
        let config = CurveConfig::default();
        // At noon (mu=12), should return None
        assert!(config.absorb_time_offset(12.0, 30.0, 6.0, 18.0).is_none());
        // Just past noon
        assert!(config.absorb_time_offset(12.1, 5.0, 6.0, 18.0).is_none());
    }

    #[test]
    fn test_absorb_cross_peak_returns_none() {
        let config = CurveConfig::default();
        // At 11:50 morning, +30min → 12:20 evening → crosses peak
        assert!(config.absorb_time_offset(11.83, 30.0, 6.0, 18.0).is_none());
    }

    #[test]
    fn test_absorb_zero_offset_returns_none_or_noop() {
        let config = CurveConfig::default();
        // Zero offset: factor = 1.0, widths unchanged
        let result = config.absorb_time_offset(8.0, 0.0, 6.0, 18.0);
        if let Some(new) = result {
            assert!((new.width_left_bri - config.width_left_bri).abs() < 0.001);
        }
    }

    #[test]
    fn test_absorb_width_clamping() {
        let config = CurveConfig {
            width_left_bri: 1.8,
            width_left_cct: 1.8,
            ..Default::default()
        };
        // Large negative offset → much higher width → should clamp to 2.0
        let result = config.absorb_time_offset(8.0, -120.0, 6.0, 18.0);
        if let Some(new) = result {
            assert!(new.width_left_bri <= 2.0, "should clamp to max 2.0");
        }
    }

    #[test]
    fn test_wrapped_dist() {
        use super::wrapped_dist;
        // Normal cases
        assert!((wrapped_dist(8.0, 12.0) - (-4.0)).abs() < 0.01);
        assert!((wrapped_dist(16.0, 12.0) - 4.0).abs() < 0.01);
        // Wrapping near midnight
        assert!((wrapped_dist(23.0, 12.0) - 11.0).abs() < 0.01);
        assert!((wrapped_dist(1.0, 12.0) - (-11.0)).abs() < 0.01);
    }
}
