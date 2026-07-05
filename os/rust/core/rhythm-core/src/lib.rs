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
//! - [`config`]: Shared timing constants and solar context helpers
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
pub use config::{
    curve_context_for_local_date_and_hour, curve_context_for_local_datetime,
    resolve_sun_times_for_local_date, SolarContext,
};
pub use controller::{
    HubDispatchTarget, HubLightController, LightControlError, LightControlResult, LightController,
    NoOpController,
};
pub use curves::{inverse_super_gaussian, map_super_gaussian};
pub use groups::{
    group_name_for_area, is_light_entity, is_rhythm_group, GroupController, GroupError,
    GroupResult, LightGroup, NoOpGroupController, GROUP_PREFIX,
};
pub use light_profile::{
    default_builtin_profiles, default_day_idle_profile, default_rhythm_profile,
    default_sleep_idle_profile, default_sleep_profile, is_builtin_state_profile_id,
    normalize_builtin_state_profile_config, CommonCurveConfig, CurveContext, HourBreakpoint,
    LightCurvePosition, LightCurveShape, LightCurveTarget, LightDirectColor, LightPaletteKeyframe,
    LightProfile, LightProfileConfig, LightProfileModule, LightProfileRegistry, TimerSetting,
    DAY_IDLE_PROFILE_ID, DAY_IDLE_PROFILE_NAME, RHYTHM_PROFILE_ID, RHYTHM_PROFILE_NAME,
    SLEEP_IDLE_PROFILE_ID, SLEEP_IDLE_PROFILE_NAME, SLEEP_PROFILE_ID, SLEEP_PROFILE_NAME,
};
pub use lighting::LightingCommand;
pub use midpoint::MidpointValue;
pub use persistence::{
    NoOpPersistenceProvider, PersistenceError, PersistenceProvider, PersistenceResult,
};
pub use primitives::{crossed_solar_midnight, PeriodicTickResult, RhythmEngine};
pub use room::{
    default_mode_configs, default_mode_transition_configs, normalize_mode_transition_configs,
    EffectiveRoomState, LightNodeKind, LightProfileNodeOverride, ModeChangeCause, ModeConfig,
    ModeTransitionConfig, ModeTransitionTime, ModeTransitionTrigger, RhythmMode, Room, RoomManager,
    RoomModeDefault, RoomModeState, RoomProfileSettings, DEFAULT_MODE_TRANSITION_DURATION_MS,
};
pub use solar::{
    calculate_solar_noon, calculate_solar_noon_from_offset, calculate_sun_times, calculate_sunrise,
    calculate_sunset, calculate_twilight_times, solar_time_from_location, SolarTime, SunTimes,
    TwilightPhase, TwilightTimes,
};
pub use steps::{CurveBoundaries, StepAction, StepResult};
pub use timezone::{default_timezone, lookup_timezone, Timezone, DEFAULT_TIMEZONE};

// Runtime re-exports
pub use runtime::{
    button_action_from_runtime_input, core_lighting_command_from_runtime,
    runtime_input_from_button_action, runtime_lighting_command_from_core,
    runtime_node_kind_from_light_node_kind, ButtonAction, DeviceRegistry, InputEvent,
    MockTimeProvider, NoOpRoomStateStore, NoOpScheduler, NodeSnapshot, RestoredNodeState,
    RestoredRoomState, RhythmDispatchRecord, RhythmInputPlanOutcome, RhythmPeriodicPlanOutcome,
    RhythmRuntime, RoomConfig, RoomSnapshot, RoomStateStore, RuntimeConfig, RuntimeError,
    RuntimeHandle, RuntimeResult, ScheduleHandle, Scheduler, SimpleDeviceRegistry, StorageError,
    StorageResult, TimeProvider, ZhaEventArgs,
};

#[cfg(feature = "serde")]
pub use runtime::{DeviceType, HubRegistry};

pub use composite_controller::{
    CompositeController, HubDispatchKind, HubDispatchMetadata, HubDispatchOutcome,
    HubDispatchOutcomeListener, HubDispatchPolicy, HubDispatchQueued, HubDispatchQueuedListener,
    HubDispatchStatus, HubDispatchTimeoutScope, HubRateLimit,
};
pub use runtime::{SystemTimeProvider, ThreadScheduler};
#[cfg(any(test, feature = "test-support"))]
pub use spy_controller::SpyLightController;
