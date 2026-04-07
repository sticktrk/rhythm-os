//! Pluggable light profile system.
//!
//! Profiles are config-driven: a [`LightProfileConfig`] provides the
//! curve shape, output ranges, and timing settings, and [`LightProfile`]
//! implements the runtime behavior from that config.
//!
//! # Architecture
//!
//! - `LightProfileModule`: Trait defining the interface for profile calculations (from `rhythm_profile`)
//! - `LightProfileConfig`: Full JSON-serializable profile configuration (from `rhythm_profile`)
//! - `LightCurveShape`: Tagged enum for curve types (from `rhythm_profile`)
//! - `CurveContext`: Context data passed to calculations (from `rhythm_profile`)
//! - `LightProfileRegistry`: Registry for managing available profiles
//! - `LightProfile`: Generic config-driven implementation

mod defaults;
mod profile;
mod registry;

pub use defaults::{
    default_idle_profile, default_rhythm_profile, default_sleep_profile, IDLE_PROFILE_ID,
    IDLE_PROFILE_NAME, RHYTHM_PROFILE_ID, RHYTHM_PROFILE_NAME, SLEEP_DEFAULT_COLOR_TEMP,
    SLEEP_DEFAULT_MAX_BRIGHTNESS, SLEEP_DEFAULT_MIN_BRIGHTNESS, SLEEP_PROFILE_ID,
    SLEEP_PROFILE_NAME, SLEEP_XY_X, SLEEP_XY_Y,
};
pub use profile::LightProfile;
pub use registry::LightProfileRegistry;

// Re-export from rhythm-profile
pub use rhythm_profile::{
    CommonCurveConfig, CurveContext, HourBreakpoint, LightCurveShape, LightDirectColor,
    LightPaletteKeyframe, LightProfileConfig, LightProfileModule, TimerSetting,
};
