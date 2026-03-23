//! # Rhythm Curve
//!
//! Pluggable lighting curve contract for Rhythm OS.
//!
//! This crate defines the trait and types needed to implement a custom
//! lighting curve algorithm. It has minimal dependencies (`libm` + optional `serde`)
//! so third-party developers can create curve modules without pulling in
//! the full `rhythm-core` runtime.
//!
//! ## Implementing a Custom Curve
//!
//! ```ignore
//! use rhythm_curve::{CurveContext, LightCurveModule, LightingValues, StepAction, StepResult};
//!
//! pub struct MyCurve { /* your config */ }
//!
//! impl LightCurveModule for MyCurve {
//!     fn id(&self) -> &str { "my-curve" }
//!     fn name(&self) -> &str { "My Custom Curve" }
//!     fn calculate(&self, ctx: &CurveContext) -> LightingValues {
//!         // Your math here
//!         LightingValues::new(4000, 80, ctx.solar_time(), 0.0)
//!     }
//!     // ... implement other required methods
//! }
//! ```

pub mod color;
pub mod config;
pub mod context;
pub mod module;
pub mod render;
pub mod solar;
pub mod steps;
pub mod values;

// Re-export main types for convenience
pub use color::{
    apply_brightness_gamma, kelvin_to_mireds, kelvin_to_rgb, kelvin_to_xy, mireds_to_kelvin,
    rgb_to_xy, Rgb, XyColor,
};
pub use config::{
    CommonCurveConfig, DEFAULT_MAX_BRIGHTNESS, DEFAULT_MAX_COLOR_TEMP, DEFAULT_MAX_DIM_STEPS,
    DEFAULT_MIN_BRIGHTNESS, DEFAULT_MIN_COLOR_TEMP,
};
pub use context::CurveContext;
pub use module::LightCurveModule;
pub use render::{
    generate_curve_data, generate_step_sequences, CurveData, StepPoint, StepSequences,
};
pub use solar::{SolarTime, SunTimes};
pub use steps::{CurveBoundaries, StepAction, StepResult};
pub use values::LightingValues;
