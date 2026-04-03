//! Pluggable light curve module system.
//!
//! This module provides a trait-based strategy pattern for different
//! lighting curve algorithms. The default implementation is `RhythmCurveModule`
//! which provides the original adaptive lighting algorithm.
//!
//! # Architecture
//!
//! - `LightCurveModule`: Trait defining the interface for curve calculations (from `rhythm_curve`)
//! - `CurveContext`: Context data passed to calculations (from `rhythm_curve`)
//! - `CurveModuleRegistry`: Registry for managing available modules
//! - `RhythmCurveModule`: Default implementation using existing algorithm

mod config;
mod idle;
mod registry;
mod rhythm;

pub use config::CurveModuleConfig;
pub use idle::{IdleCurveConfig, IdleCurveModule};
pub use registry::CurveModuleRegistry;
pub use rhythm::RhythmCurveModule;

// Re-export from rhythm-curve
pub use rhythm_curve::{CommonCurveConfig, CurveContext, LightCurveModule};
