//! Default light profile configurations.
//!
//! Factory functions that produce the built-in profile configs:
//! `rhythm` (adaptive lighting), `sleep` (nighttime motion), `idle` (soft-off palette).

use rhythm_curve::curve_shape::{LightCurveShape, LightDirectColor};
use rhythm_curve::profile_config::LightProfileConfig;
use rhythm_curve::{Rgb, XyColor};

use crate::config::{
    DEFAULT_MAX_BRIGHTNESS, DEFAULT_MAX_COLOR_TEMP, DEFAULT_MAX_DIM_STEPS, DEFAULT_MIN_BRIGHTNESS,
    DEFAULT_MIN_COLOR_TEMP,
};

pub const RHYTHM_PROFILE_ID: &str = "rhythm";
pub const RHYTHM_PROFILE_NAME: &str = "Rhythm Profile";
pub const SLEEP_PROFILE_ID: &str = "sleep";
pub const SLEEP_PROFILE_NAME: &str = "Sleep Profile";
pub const IDLE_PROFILE_ID: &str = "idle";
pub const IDLE_PROFILE_NAME: &str = "Idle Profile";
pub const SLEEP_DEFAULT_MIN_BRIGHTNESS: u8 = 10;
pub const SLEEP_DEFAULT_MAX_BRIGHTNESS: u8 = 40;
pub const SLEEP_DEFAULT_COLOR_TEMP: u16 = 500;
pub const SLEEP_XY_X: f32 = 0.6750;
pub const SLEEP_XY_Y: f32 = 0.3220;

/// Create the default rhythm (adaptive lighting) profile config.
pub fn default_rhythm_profile() -> LightProfileConfig {
    LightProfileConfig {
        id: RHYTHM_PROFILE_ID.into(),
        name: RHYTHM_PROFILE_NAME.into(),
        curve: LightCurveShape::default_super_gaussian(),
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

/// Create the default sleep profile config.
pub fn default_sleep_profile() -> LightProfileConfig {
    LightProfileConfig {
        id: SLEEP_PROFILE_ID.into(),
        name: SLEEP_PROFILE_NAME.into(),
        curve: LightCurveShape::default_super_gaussian(),
        min_brightness: SLEEP_DEFAULT_MIN_BRIGHTNESS,
        max_brightness: SLEEP_DEFAULT_MAX_BRIGHTNESS,
        min_color_temp: SLEEP_DEFAULT_COLOR_TEMP,
        max_color_temp: SLEEP_DEFAULT_COLOR_TEMP,
        max_dim_steps: DEFAULT_MAX_DIM_STEPS,
        fade_ms: None,
        motion_timeout_secs: None,
        direct_color: Some(LightDirectColor {
            xy: XyColor {
                x: SLEEP_XY_X,
                y: SLEEP_XY_Y,
            },
            rgb: Rgb::new(255, 147, 41),
        }),
    }
}

/// Create the default idle (soft-off) profile config.
pub fn default_idle_profile() -> LightProfileConfig {
    LightProfileConfig {
        id: IDLE_PROFILE_ID.into(),
        name: IDLE_PROFILE_NAME.into(),
        curve: LightCurveShape::default_idle_palette(),
        min_brightness: 1,
        max_brightness: 1,
        min_color_temp: 0,
        max_color_temp: 0,
        max_dim_steps: 1,
        fade_ms: None,
        motion_timeout_secs: None,
        direct_color: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rhythm_profile_defaults() {
        let p = default_rhythm_profile();
        assert_eq!(p.id, "rhythm");
        assert!(matches!(p.curve, LightCurveShape::SuperGaussian { .. }));
        assert_eq!(p.min_brightness, DEFAULT_MIN_BRIGHTNESS);
        assert_eq!(p.max_brightness, DEFAULT_MAX_BRIGHTNESS);
        assert!(p.direct_color.is_none());
    }

    #[test]
    fn test_sleep_profile_defaults() {
        let p = default_sleep_profile();
        assert_eq!(p.id, "sleep");
        assert!(p.direct_color.is_some());
        assert_eq!(p.min_brightness, SLEEP_DEFAULT_MIN_BRIGHTNESS);
        assert_eq!(p.max_brightness, SLEEP_DEFAULT_MAX_BRIGHTNESS);
    }

    #[test]
    fn test_idle_profile_defaults() {
        let p = default_idle_profile();
        assert_eq!(p.id, "idle");
        assert!(matches!(p.curve, LightCurveShape::Palette { .. }));
        assert_eq!(p.min_brightness, 1);
        assert_eq!(p.max_brightness, 1);
    }
}
