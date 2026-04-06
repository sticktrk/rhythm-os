//! # Rhythm Curve
//!
//! Pluggable lighting profile contract for Rhythm OS.
//!
//! This crate defines the trait and types needed to implement a custom
//! lighting profile. It has minimal dependencies (`libm` + optional `serde`)
//! so third-party developers can create light profiles without pulling in
//! the full `rhythm-core` runtime.
//!
//! ## Implementing a Custom Profile
//!
//! ```ignore
//! use rhythm_curve::{CurveContext, LightProfileModule, LightingValues, StepAction, StepResult};
//!
//! pub struct MyProfile { /* your config */ }
//!
//! impl LightProfileModule for MyProfile {
//!     fn id(&self) -> &str { "my-profile" }
//!     fn name(&self) -> &str { "My Custom Profile" }
//!     fn calculate(&self, ctx: &CurveContext) -> LightingValues {
//!         // Your math here
//!         LightingValues::new(4000, 80, ctx.solar_time(), 0.0, 500, 600)
//!     }
//!     // ... implement other required methods
//! }
//! ```

pub mod color;
pub mod config;
pub mod context;
pub mod curve_shape;
pub mod module;
pub mod profile_config;
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
pub use curve_shape::{LightCurveShape, LightDirectColor, LightPaletteKeyframe};
pub use module::LightProfileModule;
pub use profile_config::LightProfileConfig;
pub use render::{
    generate_curve_data, generate_step_sequences, CurveData, StepPoint, StepSequences,
};
pub use solar::{SolarTime, SunTimes};
pub use steps::{CurveBoundaries, StepAction, StepResult};
pub use values::LightingValues;
