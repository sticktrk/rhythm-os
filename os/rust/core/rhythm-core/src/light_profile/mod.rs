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
    default_builtin_profiles, default_day_idle_profile, default_rhythm_profile,
    default_sleep_idle_profile, default_sleep_profile, is_builtin_state_profile_id,
    normalize_builtin_state_profile_config, DAY_IDLE_PROFILE_ID, DAY_IDLE_PROFILE_NAME,
    RHYTHM_PROFILE_ID, RHYTHM_PROFILE_NAME, SLEEP_IDLE_PROFILE_ID, SLEEP_IDLE_PROFILE_NAME,
    SLEEP_PROFILE_ID, SLEEP_PROFILE_NAME,
};
pub use profile::LightProfile;
pub use registry::LightProfileRegistry;

// Re-export from rhythm-profile
pub use rhythm_profile::{
    CommonCurveConfig, CurveContext, HourBreakpoint, LightCurveShape, LightDirectColor,
    LightPaletteKeyframe, LightProfileConfig, LightProfileModule, TimerSetting,
};
