//! Pluggable light profile system.
//!
//! Profiles are config-driven: a [`LightProfileConfig`] provides the
//! curve shape, output ranges, and timing settings, and [`LightProfile`]
//! implements the runtime behavior from that config.
//!
//! # Architecture
//!
//! - `LightProfileModule`: Trait defining the interface for profile calculations (from `rhythm_curve`)
//! - `LightProfileConfig`: Full JSON-serializable profile configuration (from `rhythm_curve`)
//! - `LightCurveShape`: Tagged enum for curve types (from `rhythm_curve`)
//! - `CurveContext`: Context data passed to calculations (from `rhythm_curve`)
//! - `LightProfileRegistry`: Registry for managing available profiles
//! - `LightProfile`: Generic config-driven implementation

mod config;
mod defaults;
mod profile;
mod registry;

pub use config::LightProfileModuleConfig;
pub use defaults::{
    default_idle_profile, default_rhythm_profile, default_sleep_profile, IDLE_PROFILE_ID,
    IDLE_PROFILE_NAME, RHYTHM_PROFILE_ID, RHYTHM_PROFILE_NAME, SLEEP_DEFAULT_COLOR_TEMP,
    SLEEP_DEFAULT_MAX_BRIGHTNESS, SLEEP_DEFAULT_MIN_BRIGHTNESS, SLEEP_PROFILE_ID,
    SLEEP_PROFILE_NAME, SLEEP_XY_X, SLEEP_XY_Y,
};
pub use profile::LightProfile;
pub use registry::LightProfileRegistry;

// Re-export from rhythm-curve
pub use rhythm_curve::{
    CommonCurveConfig, CurveContext, LightCurveShape, LightDirectColor, LightPaletteKeyframe,
    LightProfileConfig, LightProfileModule,
};
