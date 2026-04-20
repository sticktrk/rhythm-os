//! Light profile configuration.
//!
//! A [`LightProfileConfig`] is a complete, serializable profile definition.
//! Copy the JSON, tweak values, and you have a new profile.
//!
//! Timer settings ([`TimerSetting`]) support three modes:
//! - **Auto**: system-computed (curve math, time-of-day, future AI)
//! - **Fixed**: user-specified constant value
//! - **Scheduled**: user-defined hour-based breakpoints

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::config::{
    DEFAULT_MAX_BRIGHTNESS, DEFAULT_MAX_COLOR_TEMP, DEFAULT_MAX_DIM_STEPS, DEFAULT_MIN_BRIGHTNESS,
    DEFAULT_MIN_COLOR_TEMP,
};
use crate::curve_shape::LightCurveShape;

/// Default fade duration in milliseconds.
pub const DEFAULT_FADE_MS: u16 = 500;

// ── TimerSetting ────────────────────────────────────────────────────

/// A single hour-based breakpoint for scheduled timer settings.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct HourBreakpoint {
    /// Hour of day (0.0–24.0).
    pub hour: f32,
    /// Value at this breakpoint.
    pub value: u32,
}

/// Timer setting with three modes: auto, fixed, or scheduled.
///
/// Used for `fade_ms`, `motion_timeout_secs`, and `rhythm_interval_secs`
/// on [`LightProfileConfig`].
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "mode"))]
pub enum TimerSetting {
    /// System-computed. Behavior depends on which field uses this setting:
    /// - `rhythm_interval_secs`: curve rate-of-change prediction, future AI
    /// - `motion_timeout_secs`: time-of-day-aware calculation, future AI
    /// - `fade_ms`: constant default (500 ms) for now
    #[cfg_attr(feature = "serde", serde(rename = "auto"))]
    Auto,

    /// User-specified constant value.
    #[cfg_attr(feature = "serde", serde(rename = "fixed"))]
    Fixed { value: u32 },

    /// User-defined hour-based schedule.
    ///
    /// Value steps to the breakpoint's value at each hour (step function).
    /// Breakpoints should be sorted by hour ascending.
    #[cfg_attr(feature = "serde", serde(rename = "scheduled"))]
    Scheduled { breakpoints: Vec<HourBreakpoint> },
}

impl TimerSetting {
    /// Resolve the setting at the given hour.
    ///
    /// Returns `None` for [`Auto`](TimerSetting::Auto) — the caller must
    /// supply context-specific auto logic.
    /// Returns `Some(value)` for [`Fixed`](TimerSetting::Fixed) and
    /// [`Scheduled`](TimerSetting::Scheduled).
    pub fn resolve(&self, hour: f32) -> Option<u32> {
        match self {
            Self::Auto => None,
            Self::Fixed { value } => Some(*value),
            Self::Scheduled { breakpoints } => {
                if breakpoints.is_empty() {
                    return None; // degenerate, treat as auto
                }
                // Step function: find the last breakpoint with hour <= current_hour.
                // If before the first breakpoint, wrap to the last (previous day).
                let mut resolved = breakpoints.last().unwrap().value;
                for bp in breakpoints {
                    if bp.hour <= hour {
                        resolved = bp.value;
                    }
                }
                Some(resolved)
            }
        }
    }

    /// Returns `true` if this setting is [`Auto`](TimerSetting::Auto).
    pub fn is_auto(&self) -> bool {
        matches!(self, Self::Auto)
    }
}

/// Complete light profile configuration — the full JSON-serializable config.
///
/// A profile combines a curve shape with output ranges and timer settings.
/// All color-generation behavior lives inside the `curve` variant itself.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct LightProfileConfig {
    /// Unique identifier (e.g., "rhythm", "sleep", "day_idle")
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
    #[cfg_attr(feature = "serde", serde(default = "default_auto"))]
    pub fade_ms: TimerSetting,

    /// Motion timeout in seconds.
    #[cfg_attr(feature = "serde", serde(default = "default_auto"))]
    pub motion_timeout_secs: TimerSetting,

    /// Periodic rhythm tick interval in seconds.
    #[cfg_attr(feature = "serde", serde(default = "default_auto"))]
    pub rhythm_interval_secs: TimerSetting,
}

#[cfg(feature = "serde")]
fn default_auto() -> TimerSetting {
    TimerSetting::Auto
}

impl LightProfileConfig {
    /// Calculate the brightness step size based on max_dim_steps.
    pub fn brightness_step_size(&self) -> f32 {
        let range = (self.max_brightness - self.min_brightness) as f32;
        range / self.max_dim_steps.max(1) as f32
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
            fade_ms: TimerSetting::Auto,
            motion_timeout_secs: TimerSetting::Auto,
            rhythm_interval_secs: TimerSetting::Auto,
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

    // ── TimerSetting resolve tests ──────────────────────────────

    #[test]
    fn test_timer_setting_auto_resolves_none() {
        assert_eq!(TimerSetting::Auto.resolve(12.0), None);
    }

    #[test]
    fn test_timer_setting_fixed_resolves_value() {
        let ts = TimerSetting::Fixed { value: 300 };
        assert_eq!(ts.resolve(0.0), Some(300));
        assert_eq!(ts.resolve(12.0), Some(300));
        assert_eq!(ts.resolve(23.9), Some(300));
    }

    #[test]
    fn test_timer_setting_scheduled_step_function() {
        let ts = TimerSetting::Scheduled {
            breakpoints: vec![
                HourBreakpoint {
                    hour: 6.0,
                    value: 1200,
                },
                HourBreakpoint {
                    hour: 9.0,
                    value: 300,
                },
                HourBreakpoint {
                    hour: 15.0,
                    value: 1200,
                },
                HourBreakpoint {
                    hour: 19.0,
                    value: 300,
                },
            ],
        };
        // Before first breakpoint: wraps to last (300)
        assert_eq!(ts.resolve(3.0), Some(300));
        // At first breakpoint
        assert_eq!(ts.resolve(6.0), Some(1200));
        // Between first and second
        assert_eq!(ts.resolve(7.5), Some(1200));
        // At second breakpoint
        assert_eq!(ts.resolve(9.0), Some(300));
        // Between second and third
        assert_eq!(ts.resolve(12.0), Some(300));
        // At third breakpoint
        assert_eq!(ts.resolve(15.0), Some(1200));
        // At last breakpoint
        assert_eq!(ts.resolve(19.0), Some(300));
        // After last breakpoint
        assert_eq!(ts.resolve(22.0), Some(300));
    }

    #[test]
    fn test_timer_setting_scheduled_single_breakpoint() {
        let ts = TimerSetting::Scheduled {
            breakpoints: vec![HourBreakpoint {
                hour: 12.0,
                value: 600,
            }],
        };
        assert_eq!(ts.resolve(0.0), Some(600));
        assert_eq!(ts.resolve(12.0), Some(600));
        assert_eq!(ts.resolve(23.0), Some(600));
    }

    #[test]
    fn test_timer_setting_scheduled_empty_is_auto() {
        let ts = TimerSetting::Scheduled {
            breakpoints: vec![],
        };
        assert_eq!(ts.resolve(12.0), None);
    }

    #[test]
    fn test_timer_setting_is_auto() {
        assert!(TimerSetting::Auto.is_auto());
        assert!(!TimerSetting::Fixed { value: 300 }.is_auto());
        assert!(!TimerSetting::Scheduled {
            breakpoints: vec![]
        }
        .is_auto());
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
            assert!(config.fade_ms.is_auto());
            assert!(config.motion_timeout_secs.is_auto());
            assert!(config.rhythm_interval_secs.is_auto());
        }

        #[test]
        fn test_palette_profile_roundtrip() {
            let config = LightProfileConfig {
                id: "day_idle".into(),
                name: "Day Idle".into(),
                curve: LightCurveShape::default_idle_palette(),
                min_brightness: 1,
                max_brightness: 1,
                min_color_temp: 0,
                max_color_temp: 0,
                max_dim_steps: 1,
                fade_ms: TimerSetting::Auto,
                motion_timeout_secs: TimerSetting::Auto,
                rhythm_interval_secs: TimerSetting::Auto,
            };
            let json = serde_json::to_string_pretty(&config).unwrap();
            let back: LightProfileConfig = serde_json::from_str(&json).unwrap();
            assert_eq!(config, back);
        }

        // ── TimerSetting serde tests ────────────────────────────

        #[test]
        fn test_timer_setting_auto_roundtrip() {
            let ts = TimerSetting::Auto;
            let json = serde_json::to_string(&ts).unwrap();
            assert_eq!(json, r#"{"mode":"auto"}"#);
            let back: TimerSetting = serde_json::from_str(&json).unwrap();
            assert_eq!(back, TimerSetting::Auto);
        }

        #[test]
        fn test_timer_setting_fixed_roundtrip() {
            let ts = TimerSetting::Fixed { value: 300 };
            let json = serde_json::to_string(&ts).unwrap();
            assert_eq!(json, r#"{"mode":"fixed","value":300}"#);
            let back: TimerSetting = serde_json::from_str(&json).unwrap();
            assert_eq!(back, ts);
        }

        #[test]
        fn test_timer_setting_scheduled_roundtrip() {
            let ts = TimerSetting::Scheduled {
                breakpoints: vec![
                    HourBreakpoint {
                        hour: 6.0,
                        value: 1200,
                    },
                    HourBreakpoint {
                        hour: 9.0,
                        value: 300,
                    },
                ],
            };
            let json = serde_json::to_string(&ts).unwrap();
            let back: TimerSetting = serde_json::from_str(&json).unwrap();
            assert_eq!(back, ts);
        }

        #[test]
        fn test_new_format_profile_config() {
            let json = r#"{
                "id": "test",
                "name": "Test",
                "curve": {"type": "super-gaussian"},
                "fade_ms": {"mode": "auto"},
                "motion_timeout_secs": {"mode": "scheduled", "breakpoints": [
                    {"hour": 6.0, "value": 1200},
                    {"hour": 9.0, "value": 300}
                ]},
                "rhythm_interval_secs": {"mode": "fixed", "value": 120}
            }"#;
            let config: LightProfileConfig = serde_json::from_str(json).unwrap();
            assert_eq!(config.fade_ms, TimerSetting::Auto);
            assert_eq!(
                config.motion_timeout_secs,
                TimerSetting::Scheduled {
                    breakpoints: vec![
                        HourBreakpoint {
                            hour: 6.0,
                            value: 1200
                        },
                        HourBreakpoint {
                            hour: 9.0,
                            value: 300
                        },
                    ],
                }
            );
            assert_eq!(
                config.rhythm_interval_secs,
                TimerSetting::Fixed { value: 120 }
            );
        }
    }
}
