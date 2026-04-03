//! Pluggable light curve module trait.
//!
//! Implement [`LightCurveModule`] to create a custom lighting curve algorithm.
//! The trait defines the contract for calculating lighting values from solar context.

use crate::context::CurveContext;
use crate::steps::{StepAction, StepResult};
use crate::values::LightingValues;

/// Trait for pluggable light curve modules.
///
/// Implement this trait to create a new curve algorithm. Each module
/// can define its own configuration type and calculation logic.
///
/// # Example
///
/// ```ignore
/// use rhythm_curve::{CurveContext, LightCurveModule, LightingValues, StepAction, StepResult};
///
/// struct ConstantCurve { brightness: u8, kelvin: u16 }
///
/// impl LightCurveModule for ConstantCurve {
///     fn id(&self) -> &str { "constant" }
///     fn name(&self) -> &str { "Constant Output" }
///     fn calculate(&self, ctx: &CurveContext) -> LightingValues {
///         LightingValues::new(self.kelvin, self.brightness, ctx.solar_time(), 0.0, 500, 600)
///     }
///     // ... implement other methods
/// }
/// ```
pub trait LightCurveModule: Send + Sync {
    /// Unique identifier for this module (e.g., "rhythm", "linear").
    fn id(&self) -> &str;

    /// Human-readable name for this module.
    fn name(&self) -> &str;

    /// Calculate complete lighting values for the given context.
    fn calculate(&self, ctx: &CurveContext) -> LightingValues;

    /// Calculate brightness only for the given context.
    fn calculate_brightness(&self, ctx: &CurveContext) -> u8;

    /// Calculate color temperature only for the given context.
    fn calculate_color_temperature(&self, ctx: &CurveContext) -> u16;

    /// Calculate lighting values with a time offset.
    ///
    /// This is useful for step up/down operations where we want to
    /// preview what the lighting would be at a different time.
    fn calculate_with_offset(&self, ctx: &CurveContext, offset_minutes: f32) -> LightingValues {
        let offset_ctx = ctx.with_offset(offset_minutes);
        self.calculate(&offset_ctx)
    }

    /// Calculate the result of a step operation (brighten or dim).
    ///
    /// # Arguments
    ///
    /// * `ctx` - Current context
    /// * `action` - Step direction (Brighten or Dim)
    ///
    /// # Returns
    ///
    /// StepResult with target values and time offset.
    fn calculate_step(&self, ctx: &CurveContext, action: StepAction) -> StepResult;

    /// Check if lighting is at maximum values (on the bright plateau).
    fn is_at_maximum(&self, ctx: &CurveContext) -> bool;

    /// Check if lighting is at minimum values (on the dim plateau).
    fn is_at_minimum(&self, ctx: &CurveContext) -> bool;

    /// Get the minimum brightness for this module.
    fn min_brightness(&self) -> u8;

    /// Get the maximum brightness for this module.
    fn max_brightness(&self) -> u8;

    /// Get the minimum color temperature for this module.
    fn min_color_temp(&self) -> u16;

    /// Get the maximum color temperature for this module.
    fn max_color_temp(&self) -> u16;

    /// Suggest a tick interval in seconds based on current curve rate of change.
    ///
    /// Returns `None` to use the configured default interval.
    /// Curve modules that know their own rate of change can return `Some(secs)`
    /// to make the periodic loop tick faster during transitions and slower
    /// during plateaus.
    fn suggested_tick_interval(&self, _ctx: &CurveContext) -> Option<u16> {
        None
    }

    /// Calculate values for idle (soft_off) mode.
    ///
    /// The returned brightness is authoritative (e.g. 1% for soft-off).
    ///
    /// Default: returns the same as `calculate()` (normal curve values).
    fn calculate_idle(&self, ctx: &CurveContext) -> LightingValues {
        self.calculate(ctx)
    }
}

#[cfg(test)]
extern crate alloc;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solar::SolarTime;
    use alloc::sync::Arc;

    /// A minimal constant curve for testing the trait contract.
    struct ConstantCurve {
        brightness: u8,
        kelvin: u16,
    }

    impl LightCurveModule for ConstantCurve {
        fn id(&self) -> &str {
            "constant"
        }
        fn name(&self) -> &str {
            "Constant Output"
        }

        fn calculate(&self, ctx: &CurveContext) -> LightingValues {
            LightingValues::new(
                self.kelvin,
                self.brightness,
                ctx.solar_time(),
                0.0,
                500,
                600,
            )
        }

        fn calculate_brightness(&self, _ctx: &CurveContext) -> u8 {
            self.brightness
        }

        fn calculate_color_temperature(&self, _ctx: &CurveContext) -> u16 {
            self.kelvin
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
            self.brightness
        }
        fn max_brightness(&self) -> u8 {
            self.brightness
        }
        fn min_color_temp(&self) -> u16 {
            self.kelvin
        }
        fn max_color_temp(&self) -> u16 {
            self.kelvin
        }
    }

    #[test]
    fn test_constant_curve_identity() {
        let curve = ConstantCurve {
            brightness: 80,
            kelvin: 4000,
        };
        assert_eq!(curve.id(), "constant");
        assert_eq!(curve.name(), "Constant Output");
    }

    #[test]
    fn test_constant_curve_calculate() {
        let curve = ConstantCurve {
            brightness: 80,
            kelvin: 4000,
        };
        let ctx = CurveContext::default();
        let values = curve.calculate(&ctx);
        assert_eq!(values.brightness, 80);
        assert_eq!(values.kelvin, 4000);
    }

    #[test]
    fn test_constant_curve_individual_accessors() {
        let curve = ConstantCurve {
            brightness: 50,
            kelvin: 3000,
        };
        let ctx = CurveContext::default();
        assert_eq!(curve.calculate_brightness(&ctx), 50);
        assert_eq!(curve.calculate_color_temperature(&ctx), 3000);
    }

    #[test]
    fn test_constant_curve_min_max() {
        let curve = ConstantCurve {
            brightness: 75,
            kelvin: 5000,
        };
        assert_eq!(curve.min_brightness(), 75);
        assert_eq!(curve.max_brightness(), 75);
        assert_eq!(curve.min_color_temp(), 5000);
        assert_eq!(curve.max_color_temp(), 5000);
    }

    #[test]
    fn test_constant_curve_boundaries() {
        let curve = ConstantCurve {
            brightness: 100,
            kelvin: 5500,
        };
        let ctx = CurveContext::default();
        assert!(curve.is_at_maximum(&ctx));
        assert!(!curve.is_at_minimum(&ctx));
    }

    #[test]
    fn test_constant_curve_step_is_noop() {
        let curve = ConstantCurve {
            brightness: 80,
            kelvin: 4000,
        };
        let ctx = CurveContext::default();
        let result = curve.calculate_step(&ctx, StepAction::Brighten);
        assert!(result.at_boundary);
        assert_eq!(result.time_offset_minutes, 0.0);
        assert_eq!(result.values.brightness, 80);
    }

    #[test]
    fn test_default_calculate_with_offset() {
        let curve = ConstantCurve {
            brightness: 80,
            kelvin: 4000,
        };
        let ctx = CurveContext::new(12.0, SolarTime::default(), None);

        // Default impl calls calculate with offset context
        let values = curve.calculate_with_offset(&ctx, 60.0);
        // Constant curve always returns same values
        assert_eq!(values.brightness, 80);
        assert_eq!(values.kelvin, 4000);
    }

    #[test]
    fn test_trait_object_via_arc() {
        let curve: Arc<dyn LightCurveModule> = Arc::new(ConstantCurve {
            brightness: 60,
            kelvin: 3500,
        });
        let ctx = CurveContext::default();
        let values = curve.calculate(&ctx);
        assert_eq!(values.brightness, 60);
        assert_eq!(values.kelvin, 3500);
    }

    #[test]
    fn test_trait_object_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ConstantCurve>();
    }

    #[test]
    fn test_calculate_across_day() {
        let curve = ConstantCurve {
            brightness: 80,
            kelvin: 4000,
        };
        // Constant curve should produce same output at every hour
        for h in 0..24 {
            let ctx = CurveContext::new(h as f32, SolarTime::default(), None);
            let values = curve.calculate(&ctx);
            assert_eq!(values.brightness, 80);
            assert_eq!(values.kelvin, 4000);
        }
    }
}
