//! Default light profile configurations.
//!
//! Factory functions that produce the built-in profile configs:
//! `rhythm` (adaptive lighting), `sleep` (constant night light),
//! `day_idle` (inherit-active soft-off), and `sleep_idle` (inherit-active soft-off).

use rhythm_profile::curve_shape::LightCurveShape;
use rhythm_profile::profile_config::{LightProfileConfig, TimerSetting};
use rhythm_profile::{
    DEFAULT_MAX_BRIGHTNESS, DEFAULT_MAX_COLOR_TEMP, DEFAULT_MAX_DIM_STEPS, DEFAULT_MIN_BRIGHTNESS,
    DEFAULT_MIN_COLOR_TEMP,
};

pub const RHYTHM_PROFILE_ID: &str = "rhythm";
pub const RHYTHM_PROFILE_NAME: &str = "Day";
pub const SLEEP_PROFILE_ID: &str = "sleep";
pub const SLEEP_PROFILE_NAME: &str = "Sleep";
pub const DAY_IDLE_PROFILE_ID: &str = "day_idle";
pub const DAY_IDLE_PROFILE_NAME: &str = "Day Idle";
pub const SLEEP_IDLE_PROFILE_ID: &str = "sleep_idle";
pub const SLEEP_IDLE_PROFILE_NAME: &str = "Sleep Idle";

fn inherit_active_idle_profile(id: &str, name: &str) -> LightProfileConfig {
    LightProfileConfig {
        id: id.into(),
        name: name.into(),
        curve: LightCurveShape::InheritActive,
        min_brightness: 1,
        max_brightness: 1,
        min_color_temp: 0,
        max_color_temp: 0,
        max_dim_steps: 1,
        fade_ms: TimerSetting::Auto,
        motion_timeout_secs: TimerSetting::Auto,
        rhythm_interval_secs: TimerSetting::Auto,
    }
}

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
        fade_ms: TimerSetting::Auto,
        motion_timeout_secs: TimerSetting::Auto,
        rhythm_interval_secs: TimerSetting::Auto,
    }
}

/// Create the default sleep profile config.
pub fn default_sleep_profile() -> LightProfileConfig {
    let wake = default_rhythm_profile();

    LightProfileConfig {
        id: SLEEP_PROFILE_ID.into(),
        name: SLEEP_PROFILE_NAME.into(),
        curve: LightCurveShape::Constant {
            brightness: 0.0,
            color_temp: 0.0,
            direct_color: None,
        },
        min_brightness: wake.min_brightness,
        max_brightness: wake.min_brightness,
        min_color_temp: wake.min_color_temp,
        max_color_temp: wake.min_color_temp,
        max_dim_steps: 1,
        fade_ms: TimerSetting::Auto,
        motion_timeout_secs: TimerSetting::Auto,
        rhythm_interval_secs: TimerSetting::Auto,
    }
}

/// Create the default day idle (soft-off) profile config.
///
/// Day idle inherits the active profile's current color while
/// forcing output to 1% brightness. Saving an explicit idle palette replaces
/// this fallback behavior.
pub fn default_day_idle_profile() -> LightProfileConfig {
    inherit_active_idle_profile(DAY_IDLE_PROFILE_ID, DAY_IDLE_PROFILE_NAME)
}

/// Create the default sleep idle (soft-off) profile config.
pub fn default_sleep_idle_profile() -> LightProfileConfig {
    inherit_active_idle_profile(SLEEP_IDLE_PROFILE_ID, SLEEP_IDLE_PROFILE_NAME)
}

/// Create the full built-in profile set.
pub fn default_builtin_profiles() -> [LightProfileConfig; 4] {
    [
        default_rhythm_profile(),
        default_sleep_profile(),
        default_day_idle_profile(),
        default_sleep_idle_profile(),
    ]
}

/// Built-in state profiles cannot be selected as active profiles directly.
pub fn is_builtin_state_profile_id(id: &str) -> bool {
    matches!(id, DAY_IDLE_PROFILE_ID | SLEEP_IDLE_PROFILE_ID)
}

/// Normalize built-in state-profile configs into the fixed-brightness shape
/// expected by the runtime.
///
/// Idle-state profiles always use a single absolute brightness value via
/// `min_brightness == max_brightness`. When generic profile defaults leak into
/// these configs during editing, coerce them back to the idle default of 1%.
pub fn normalize_builtin_state_profile_config(config: &mut LightProfileConfig) {
    if !is_builtin_state_profile_id(&config.id) {
        return;
    }

    let reset_to_idle_default = config.min_brightness == DEFAULT_MIN_BRIGHTNESS
        && config.max_brightness == DEFAULT_MAX_BRIGHTNESS;

    let fixed_brightness = if reset_to_idle_default {
        1
    } else {
        config.max_brightness.max(config.min_brightness).max(1)
    };
    config.min_brightness = fixed_brightness;
    config.max_brightness = fixed_brightness;

    if let LightCurveShape::Constant {
        brightness,
        color_temp,
        ..
    } = &mut config.curve
    {
        *brightness = 1.0;
        *color_temp = color_temp.clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhythm_profile::color::rgb_to_xy;
    use rhythm_profile::curve_shape::LightDirectColor;
    use rhythm_profile::Rgb;

    #[test]
    fn test_rhythm_profile_defaults() {
        let p = default_rhythm_profile();
        assert_eq!(p.id, "rhythm");
        assert!(matches!(
            p.curve,
            LightCurveShape::SuperGaussian {
                direct_color: None,
                ..
            }
        ));
        assert_eq!(p.min_brightness, DEFAULT_MIN_BRIGHTNESS);
        assert_eq!(p.max_brightness, DEFAULT_MAX_BRIGHTNESS);
        assert!(p.rhythm_interval_secs.is_auto());
    }

    #[test]
    fn test_sleep_profile_defaults() {
        let p = default_sleep_profile();
        let wake = default_rhythm_profile();
        assert_eq!(p.id, "sleep");
        assert!(matches!(
            p.curve,
            LightCurveShape::Constant {
                brightness,
                color_temp,
                direct_color: None,
            } if brightness.abs() < f32::EPSILON
                && color_temp.abs() < f32::EPSILON
        ));
        assert_eq!(p.min_brightness, wake.min_brightness);
        assert_eq!(p.max_brightness, wake.min_brightness);
        assert_eq!(p.min_color_temp, wake.min_color_temp);
        assert_eq!(p.max_color_temp, wake.min_color_temp);
        assert!(p.rhythm_interval_secs.is_auto());
    }

    #[test]
    fn test_day_idle_profile_defaults() {
        let p = default_day_idle_profile();
        assert_eq!(p.id, DAY_IDLE_PROFILE_ID);
        assert!(matches!(p.curve, LightCurveShape::InheritActive));
        assert_eq!(p.min_brightness, 1);
        assert_eq!(p.max_brightness, 1);
        assert!(p.rhythm_interval_secs.is_auto());
    }

    #[test]
    fn test_sleep_idle_profile_defaults() {
        let p = default_sleep_idle_profile();
        assert_eq!(p.id, SLEEP_IDLE_PROFILE_ID);
        assert!(matches!(p.curve, LightCurveShape::InheritActive));
        assert_eq!(p.min_brightness, 1);
        assert_eq!(p.max_brightness, 1);
        assert!(p.rhythm_interval_secs.is_auto());
    }

    #[test]
    fn test_builtin_state_profile_ids() {
        assert!(is_builtin_state_profile_id(DAY_IDLE_PROFILE_ID));
        assert!(is_builtin_state_profile_id(SLEEP_IDLE_PROFILE_ID));
        assert!(!is_builtin_state_profile_id(RHYTHM_PROFILE_ID));
    }

    #[test]
    fn normalize_idle_profile_resets_generic_defaults_to_one_percent() {
        let mut config = LightProfileConfig {
            id: DAY_IDLE_PROFILE_ID.into(),
            name: DAY_IDLE_PROFILE_NAME.into(),
            curve: LightCurveShape::Palette { keyframes: vec![] },
            min_brightness: DEFAULT_MIN_BRIGHTNESS,
            max_brightness: DEFAULT_MAX_BRIGHTNESS,
            min_color_temp: 0,
            max_color_temp: 0,
            max_dim_steps: 1,
            fade_ms: TimerSetting::Auto,
            motion_timeout_secs: TimerSetting::Auto,
            rhythm_interval_secs: TimerSetting::Auto,
        };

        normalize_builtin_state_profile_config(&mut config);

        assert_eq!(config.min_brightness, 1);
        assert_eq!(config.max_brightness, 1);
    }

    #[test]
    fn normalize_idle_profile_preserves_explicit_custom_brightness() {
        let mut config = LightProfileConfig {
            id: DAY_IDLE_PROFILE_ID.into(),
            name: DAY_IDLE_PROFILE_NAME.into(),
            curve: LightCurveShape::Constant {
                brightness: 0.25,
                color_temp: 0.0,
                direct_color: Some(LightDirectColor {
                    xy: rgb_to_xy(Rgb::new(38, 191, 255)),
                    rgb: Rgb::new(38, 191, 255),
                }),
            },
            min_brightness: 20,
            max_brightness: 20,
            min_color_temp: 0,
            max_color_temp: 0,
            max_dim_steps: 1,
            fade_ms: TimerSetting::Auto,
            motion_timeout_secs: TimerSetting::Auto,
            rhythm_interval_secs: TimerSetting::Auto,
        };

        normalize_builtin_state_profile_config(&mut config);

        assert_eq!(config.min_brightness, 20);
        assert_eq!(config.max_brightness, 20);
        assert!(matches!(
            config.curve,
            LightCurveShape::Constant {
                brightness: 1.0,
                ..
            }
        ));
    }
}
