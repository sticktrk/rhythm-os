//! # Rhythm Core
//!
//! Core adaptive lighting algorithms for Rhythm OS.
//!
//! ## Features
//!
//! - `serde`: Enables serialization support
//!
//! ## Modules
//!
//! - [`config`]: Configuration types for curve parameters
//! - [`curves`]: Super-Gaussian (flat-topped bell) curve calculations
//! - [`color`]: Color space conversions (Kelvin to XY/RGB)
//! - [`solar`]: Solar time and sunrise/sunset calculations
//! - [`timezone`]: Timezone and DST handling (via chrono-tz)
//! - [`midpoint`]: Dynamic midpoint values (sunrise/sunset)
//! - [`adaptive`]: Lighting output types (LightingValues)
//! - [`light_profile`]: Pluggable light profile system
//! - [`steps`]: Step/dimming algorithms
//! - [`lighting`]: Light command types
//! - [`room`]: Room/area abstractions
//! - [`controller`]: Light controller trait
//! - [`primitives`]: RhythmEngine and service primitives
//! - [`groups`]: Light group management
//! - [`persistence`]: State persistence trait

pub mod actions;
pub mod adaptive;
pub mod color;
pub mod composite_controller;
pub mod config;
pub mod controller;
pub mod curves;
#[cfg(feature = "serde")]
pub mod defaults;
pub mod device;
pub mod groups;
pub mod light_profile;
pub mod lighting;
pub mod midpoint;
pub mod persistence;
pub mod primitives;
pub mod room;
pub mod runtime;
pub mod solar;
#[cfg(any(test, feature = "test-support"))]
pub mod spy_controller;
pub mod steps;
pub mod timezone;

// Re-export main types for convenience
pub use actions::{process_action, ActionResult, RoomAction, RoomActionState};
pub use adaptive::LightingValues;
pub use color::{
    kelvin_to_mireds, kelvin_to_rgb, kelvin_to_xy, mireds_to_kelvin, rgb_to_xy, Rgb, XyColor,
};
pub use config::{CurveConfig, SolarContext};
pub use controller::{LightControlError, LightControlResult, LightController, NoOpController};
pub use curves::{inverse_super_gaussian, map_super_gaussian};
#[cfg(feature = "serde")]
pub use defaults::default_config;
pub use groups::{
    group_name_for_area, is_light_entity, is_rhythm_group, GroupController, GroupError,
    GroupResult, LightGroup, NoOpGroupController, GROUP_PREFIX,
};
pub use light_profile::{
    default_idle_profile, default_rhythm_profile, default_sleep_profile, CommonCurveConfig,
    CurveContext, LightCurveShape, LightDirectColor, LightPaletteKeyframe, LightProfile,
    LightProfileConfig, LightProfileModule, LightProfileModuleConfig, LightProfileRegistry,
    IDLE_PROFILE_ID, IDLE_PROFILE_NAME, RHYTHM_PROFILE_ID, RHYTHM_PROFILE_NAME,
    SLEEP_DEFAULT_COLOR_TEMP, SLEEP_DEFAULT_MAX_BRIGHTNESS, SLEEP_DEFAULT_MIN_BRIGHTNESS,
    SLEEP_PROFILE_ID, SLEEP_PROFILE_NAME, SLEEP_XY_X, SLEEP_XY_Y,
};
pub use lighting::LightingCommand;
pub use midpoint::MidpointValue;
pub use persistence::{
    NoOpPersistenceProvider, PersistenceError, PersistenceProvider, PersistenceResult,
};
pub use primitives::{crossed_solar_midnight, PeriodicTickResult, RhythmEngine};
pub use room::{Room, RoomManager};
pub use solar::{
    calculate_solar_noon, calculate_solar_noon_from_offset, calculate_sun_times, calculate_sunrise,
    calculate_sunset, calculate_twilight_times, solar_time_from_location, SolarTime, SunTimes,
    TwilightPhase, TwilightTimes,
};
pub use steps::{CurveBoundaries, StepAction, StepResult};
pub use timezone::{default_timezone, lookup_timezone, Timezone, DEFAULT_TIMEZONE};

// Runtime re-exports
pub use runtime::{
    ButtonAction, DeviceRegistry, InputEvent, MockTimeProvider, NoOpRoomStateStore, NoOpScheduler,
    RhythmRuntime, RoomConfig, RoomSnapshot, RoomStateStore, RuntimeConfig, RuntimeError,
    RuntimeHandle, RuntimeResult, ScheduleHandle, Scheduler, SimpleDeviceRegistry, StorageError,
    StorageResult, TimeProvider, ZhaEventArgs,
};

#[cfg(feature = "serde")]
pub use runtime::{DeviceType, HubRegistry};

pub use composite_controller::CompositeController;
#[cfg(feature = "tokio")]
pub use runtime::{TokioScheduler, TokioTimeProvider};
#[cfg(any(test, feature = "test-support"))]
pub use spy_controller::SpyLightController;

#[cfg(feature = "blocking")]
pub use runtime::{BlockingScheduler, BlockingTimeProvider};
