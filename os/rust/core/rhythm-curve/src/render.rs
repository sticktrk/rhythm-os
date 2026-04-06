//! Curve rendering: sample any `LightProfileModule` into visualization data.
//!
//! These functions work with any curve module implementation — the module
//! determines what to do with the solar context (or ignore it entirely).

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::color::{Rgb, XyColor};
use crate::context::CurveContext;
use crate::module::LightProfileModule;
use crate::solar::{SolarTime, SunTimes};
use crate::steps::StepAction;

/// Sampled curve data for visualization.
///
/// Contains parallel arrays of hours, brightness, and color temperature
/// values sampled at regular intervals over 24 hours.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct CurveData {
    /// Sample hours (0.0–24.0)
    pub hours: Vec<f32>,
    /// Brightness percentage at each sample (1–100)
    pub brightness: Vec<u8>,
    /// Color temperature in Kelvin at each sample
    pub kelvin: Vec<u16>,
}

/// A single step point for dimming visualization.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct StepPoint {
    /// Target hour on the curve
    pub hour: f32,
    /// Brightness at this step
    pub brightness: u8,
    /// Color temperature at this step
    pub kelvin: u16,
    /// RGB color at this step
    pub rgb: Rgb,
    /// CIE xy color at this step
    pub xy: XyColor,
}

/// Step sequences showing where dim/brighten steps land on the curve.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct StepSequences {
    /// Starting hour for the sequences
    pub start_hour: f32,
    /// Steps toward brighter/cooler (toward solar noon)
    pub step_up: Vec<StepPoint>,
    /// Steps toward dimmer/warmer (toward sunrise/sunset)
    pub step_down: Vec<StepPoint>,
}

/// Sample a curve module at regular intervals over 24 hours.
///
/// # Arguments
///
/// * `module` - Any `LightProfileModule` implementation
/// * `solar` - Solar time reference (used by the module if it cares about the sun)
/// * `sun_times` - Optional sunrise/sunset data
/// * `samples_per_hour` - Resolution: 1 = hourly (24 points), 4 = 15-min (96 points), 10 = 6-min (240 points). Clamped to 1–60.
pub fn generate_curve_data(
    module: &dyn LightProfileModule,
    solar: SolarTime,
    sun_times: Option<SunTimes>,
    samples_per_hour: u32,
) -> CurveData {
    let samples_per_hour = samples_per_hour.clamp(1, 60) as usize;
    let total = 24 * samples_per_hour;
    let step = 1.0 / samples_per_hour as f32;

    let mut hours = Vec::with_capacity(total);
    let mut brightness = Vec::with_capacity(total);
    let mut kelvin = Vec::with_capacity(total);

    for i in 0..total {
        let hour = i as f32 * step;
        let ctx = CurveContext::new(hour, solar, sun_times);
        let values = module.calculate(&ctx);
        hours.push(hour);
        brightness.push(values.brightness);
        kelvin.push(values.kelvin);
    }

    CurveData {
        hours,
        brightness,
        kelvin,
    }
}

/// Calculate step sequences from a starting hour.
///
/// Iteratively calls `calculate_step` in each direction until a boundary
/// is reached, producing the points a user would visit by pressing
/// step-up or step-down repeatedly.
///
/// # Arguments
///
/// * `module` - Any `LightProfileModule` implementation
/// * `solar` - Solar time reference
/// * `sun_times` - Optional sunrise/sunset data
/// * `start_hour` - Starting position on the curve
/// * `max_steps` - Maximum number of steps in each direction (clamped 1–255)
pub fn generate_step_sequences(
    module: &dyn LightProfileModule,
    solar: SolarTime,
    sun_times: Option<SunTimes>,
    start_hour: f32,
    max_steps: u8,
) -> StepSequences {
    let max_steps = max_steps.max(1) as usize;

    // Step up (brighten)
    let mut step_up = Vec::with_capacity(max_steps);
    let mut current_hour = start_hour;

    for _ in 0..max_steps {
        let ctx = CurveContext::new(current_hour, solar, sun_times);
        let result = module.calculate_step(&ctx, StepAction::Brighten);

        if result.at_boundary {
            break;
        }

        let target_hour = current_hour + result.time_offset_minutes / 60.0;
        let v = &result.values;
        step_up.push(StepPoint {
            hour: target_hour,
            brightness: v.brightness,
            kelvin: v.kelvin,
            rgb: v.rgb,
            xy: v.xy,
        });
        current_hour = target_hour;
    }

    // Step down (dim)
    let mut step_down = Vec::with_capacity(max_steps);
    current_hour = start_hour;

    for _ in 0..max_steps {
        let ctx = CurveContext::new(current_hour, solar, sun_times);
        let result = module.calculate_step(&ctx, StepAction::Dim);

        if result.at_boundary {
            break;
        }

        let target_hour = current_hour + result.time_offset_minutes / 60.0;
        let v = &result.values;
        step_down.push(StepPoint {
            hour: target_hour,
            brightness: v.brightness,
            kelvin: v.kelvin,
            rgb: v.rgb,
            xy: v.xy,
        });
        current_hour = target_hour;
    }

    StepSequences {
        start_hour,
        step_up,
        step_down,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::LightProfileModule;
    use crate::steps::StepResult;
    use crate::values::LightingValues;

    /// Constant curve for testing — ignores solar context entirely.
    struct ConstantCurve;

    impl LightProfileModule for ConstantCurve {
        fn id(&self) -> &str {
            "constant"
        }
        fn name(&self) -> &str {
            "Constant"
        }

        fn calculate(&self, ctx: &CurveContext) -> LightingValues {
            LightingValues::new(4000, 80, ctx.solar_time(), 0.0, 500, 600)
        }

        fn calculate_brightness(&self, _ctx: &CurveContext) -> u8 {
            80
        }
        fn calculate_color_temperature(&self, _ctx: &CurveContext) -> u16 {
            4000
        }

        fn calculate_step(&self, ctx: &CurveContext, _action: StepAction) -> StepResult {
            StepResult {
                values: self.calculate(ctx),
                time_offset_minutes: 0.0,
                at_boundary: true,
            }
        }

        fn is_at_maximum(&self, _ctx: &CurveContext) -> bool {
            true
        }
        fn is_at_minimum(&self, _ctx: &CurveContext) -> bool {
            false
        }
        fn min_brightness(&self) -> u8 {
            80
        }
        fn max_brightness(&self) -> u8 {
            80
        }
        fn min_color_temp(&self) -> u16 {
            4000
        }
        fn max_color_temp(&self) -> u16 {
            4000
        }
    }

    #[test]
    fn generate_curve_data_hourly() {
        let data = generate_curve_data(&ConstantCurve, SolarTime::default(), None, 1);
        assert_eq!(data.hours.len(), 24);
        assert_eq!(data.brightness.len(), 24);
        assert_eq!(data.kelvin.len(), 24);
        assert_eq!(data.brightness[0], 80);
        assert_eq!(data.kelvin[0], 4000);
        assert!((data.hours[0] - 0.0).abs() < 0.001);
        assert!((data.hours[23] - 23.0).abs() < 0.001);
    }

    #[test]
    fn generate_curve_data_high_res() {
        let data = generate_curve_data(&ConstantCurve, SolarTime::default(), None, 4);
        assert_eq!(data.hours.len(), 96);
        assert!((data.hours[1] - 0.25).abs() < 0.001);
    }

    #[test]
    fn generate_curve_data_clamps_samples() {
        let data = generate_curve_data(&ConstantCurve, SolarTime::default(), None, 0);
        assert_eq!(data.hours.len(), 24); // clamped to 1

        let data = generate_curve_data(&ConstantCurve, SolarTime::default(), None, 100);
        assert_eq!(data.hours.len(), 24 * 60); // clamped to 60
    }

    #[test]
    fn step_sequences_constant_curve_hits_boundary_immediately() {
        let seq = generate_step_sequences(&ConstantCurve, SolarTime::default(), None, 12.0, 6);
        assert!(
            seq.step_up.is_empty(),
            "constant curve should hit boundary immediately"
        );
        assert!(seq.step_down.is_empty());
        assert_eq!(seq.start_hour, 12.0);
    }

    #[test]
    fn generate_curve_data_with_sun_times() {
        let sun = SunTimes {
            sunrise: 6.0,
            sunset: 20.0,
            day_length: 14.0,
        };
        let data = generate_curve_data(&ConstantCurve, SolarTime::default(), Some(sun), 1);
        assert_eq!(data.hours.len(), 24);
        // Constant curve ignores sun_times, but the function should still work
        assert_eq!(data.brightness[12], 80);
    }
}
