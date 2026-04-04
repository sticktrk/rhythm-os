//! Sleep curve module.
//!
//! A curve tuned for nighttime motion activation — moderate warm light
//! instead of the near-zero brightness the rhythm curve produces at night.
//! Uses the same super-Gaussian math as [`RhythmCurveModule`] but with
//! sleep-appropriate defaults (lower brightness range, warmer color temps).

use crate::adaptive::LightingValues;
use crate::color::kelvin_to_rgb;
use crate::config::CurveConfig;
use crate::steps::{StepAction, StepResult};

use super::{CurveContext, LightCurveModule, RhythmCurveModule};

/// Default minimum brightness for sleep mode.
pub const SLEEP_DEFAULT_MIN_BRIGHTNESS: u8 = 10;
/// Default maximum brightness for sleep mode.
pub const SLEEP_DEFAULT_MAX_BRIGHTNESS: u8 = 40;
/// Fixed color temperature for sleep mode curve math (very warm).
pub const SLEEP_DEFAULT_COLOR_TEMP: u16 = 500;
/// Target XY chromaticity for sleep mode (warm red/amber).
pub const SLEEP_XY_X: f32 = 0.6750;
pub const SLEEP_XY_Y: f32 = 0.3220;

/// Sleep curve module — warm, moderate-brightness lighting for nighttime.
///
/// Wraps [`RhythmCurveModule`] with different defaults suitable for sleep:
/// lower brightness range and warmer color temperatures. The same
/// super-Gaussian shape means step/dim operations still work naturally.
#[derive(Debug, Clone)]
pub struct SleepCurveModule {
    inner: RhythmCurveModule,
}

impl SleepCurveModule {
    /// Module identifier.
    pub const ID: &'static str = "sleep";

    /// Module display name.
    pub const NAME: &'static str = "Sleep Curve";

    /// Create a new SleepCurveModule with the given configuration.
    pub fn new(config: CurveConfig) -> Self {
        Self {
            inner: RhythmCurveModule::new(config),
        }
    }

    /// Create with sleep-appropriate defaults.
    pub fn with_defaults() -> Self {
        Self::new(CurveConfig {
            min_brightness: SLEEP_DEFAULT_MIN_BRIGHTNESS,
            max_brightness: SLEEP_DEFAULT_MAX_BRIGHTNESS,
            min_color_temp: SLEEP_DEFAULT_COLOR_TEMP,
            max_color_temp: SLEEP_DEFAULT_COLOR_TEMP,
            ..Default::default()
        })
    }

    /// Get a reference to the inner configuration.
    pub fn config(&self) -> &CurveConfig {
        self.inner.config()
    }
}

impl Default for SleepCurveModule {
    fn default() -> Self {
        Self::with_defaults()
    }
}

impl LightCurveModule for SleepCurveModule {
    fn id(&self) -> &str {
        Self::ID
    }

    fn name(&self) -> &str {
        Self::NAME
    }

    fn calculate(&self, ctx: &CurveContext) -> LightingValues {
        let mut values = self.inner.calculate(ctx);
        // Fixed XY target — kelvin conversion is too fuzzy at these extremes
        values.xy = crate::color::XyColor {
            x: SLEEP_XY_X,
            y: SLEEP_XY_Y,
        };
        values.rgb = kelvin_to_rgb(SLEEP_DEFAULT_COLOR_TEMP);
        values.is_direct_color = true;
        values
    }

    fn calculate_brightness(&self, ctx: &CurveContext) -> u8 {
        self.inner.calculate_brightness(ctx)
    }

    fn calculate_color_temperature(&self, ctx: &CurveContext) -> u16 {
        self.inner.calculate_color_temperature(ctx)
    }

    fn calculate_with_offset(&self, ctx: &CurveContext, offset_minutes: f32) -> LightingValues {
        let mut values = self.inner.calculate_with_offset(ctx, offset_minutes);
        values.xy = crate::color::XyColor {
            x: SLEEP_XY_X,
            y: SLEEP_XY_Y,
        };
        values.rgb = kelvin_to_rgb(SLEEP_DEFAULT_COLOR_TEMP);
        values.is_direct_color = true;
        values
    }

    fn calculate_step(&self, ctx: &CurveContext, action: StepAction) -> StepResult {
        self.inner.calculate_step(ctx, action)
    }

    fn is_at_maximum(&self, ctx: &CurveContext) -> bool {
        self.inner.is_at_maximum(ctx)
    }

    fn is_at_minimum(&self, ctx: &CurveContext) -> bool {
        self.inner.is_at_minimum(ctx)
    }

    fn min_brightness(&self) -> u8 {
        self.inner.min_brightness()
    }

    fn max_brightness(&self) -> u8 {
        self.inner.max_brightness()
    }

    fn min_color_temp(&self) -> u16 {
        self.inner.min_color_temp()
    }

    fn max_color_temp(&self) -> u16 {
        self.inner.max_color_temp()
    }

    fn suggested_tick_interval(&self, ctx: &CurveContext) -> Option<u16> {
        self.inner.suggested_tick_interval(ctx)
    }

    fn calculate_idle(&self, ctx: &CurveContext) -> LightingValues {
        self.inner.calculate_idle(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solar::{SolarTime, SunTimes};

    fn test_context(hour: f32) -> CurveContext {
        CurveContext::new(
            hour,
            SolarTime::new(12.0, 35.0, 172),
            Some(SunTimes {
                sunrise: 6.0,
                sunset: 18.0,
                day_length: 12.0,
            }),
        )
    }

    #[test]
    fn test_id_and_name() {
        let module = SleepCurveModule::with_defaults();
        assert_eq!(module.id(), "sleep");
        assert_eq!(module.name(), "Sleep Curve");
    }

    #[test]
    fn test_defaults_are_warm_and_moderate() {
        let module = SleepCurveModule::with_defaults();
        assert_eq!(module.min_brightness(), SLEEP_DEFAULT_MIN_BRIGHTNESS);
        assert_eq!(module.max_brightness(), SLEEP_DEFAULT_MAX_BRIGHTNESS);
        assert_eq!(module.min_color_temp(), SLEEP_DEFAULT_COLOR_TEMP);
        assert_eq!(module.max_color_temp(), SLEEP_DEFAULT_COLOR_TEMP);
    }

    #[test]
    fn test_brightness_capped_at_sleep_max() {
        let module = SleepCurveModule::with_defaults();
        let ctx = test_context(12.0); // noon — rhythm curve would give 100%
        let brightness = module.calculate_brightness(&ctx);
        assert!(
            brightness <= SLEEP_DEFAULT_MAX_BRIGHTNESS,
            "Sleep curve brightness at noon should be <= {}, got {}",
            SLEEP_DEFAULT_MAX_BRIGHTNESS,
            brightness
        );
    }

    #[test]
    fn test_sends_direct_color_xy() {
        let module = SleepCurveModule::with_defaults();
        let ctx = test_context(12.0);
        let values = module.calculate(&ctx);
        assert!(
            values.is_direct_color,
            "Sleep curve should send direct color"
        );
        assert!((values.xy.x - SLEEP_XY_X).abs() < 0.001);
        assert!((values.xy.y - SLEEP_XY_Y).abs() < 0.001);
    }

    #[test]
    fn test_step_still_works() {
        let module = SleepCurveModule::with_defaults();
        let ctx = test_context(8.0);
        let result = module.calculate_step(&ctx, crate::steps::StepAction::Brighten);
        // Should move forward (toward noon) without panicking
        assert!(result.time_offset_minutes > 0.0 || result.at_boundary);
    }

    #[test]
    fn test_idle_returns_one_percent() {
        let module = SleepCurveModule::with_defaults();
        let ctx = test_context(12.0);
        let idle = module.calculate_idle(&ctx);
        assert_eq!(idle.brightness, 1, "Idle should still be 1%");
    }

    #[test]
    fn test_custom_config() {
        let module = SleepCurveModule::new(CurveConfig {
            min_brightness: 5,
            max_brightness: 60,
            min_color_temp: 2200,
            max_color_temp: 3000,
            ..Default::default()
        });
        assert_eq!(module.min_brightness(), 5);
        assert_eq!(module.max_brightness(), 60);
        assert_eq!(module.min_color_temp(), 2200);
        assert_eq!(module.max_color_temp(), 3000);
    }
}
