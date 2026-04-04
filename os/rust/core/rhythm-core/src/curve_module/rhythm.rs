//! Rhythm curve module implementation.
//!
//! This module implements the rhythm lighting algorithm using a super-Gaussian
//! (flat-topped bell curve) for smooth brightness and color temperature
//! transitions based on solar time.

extern crate alloc;

use tracing::{debug, info};

use crate::adaptive::LightingValues;
use crate::config::{
    CurveConfig, DEFAULT_FADE_MS, DEFAULT_MOTION_TIMEOUT_SECS, FALLBACK_SUNRISE_HOUR,
    FALLBACK_SUNSET_HOUR,
};
use crate::curves::{inverse_super_gaussian, map_super_gaussian};
use crate::solar::SunTimes;
use crate::steps::{StepAction, StepResult};

use super::{CurveContext, IdleCurveModule, LightCurveModule};

/// Rhythm curve module using super-Gaussian adaptive lighting algorithm.
///
/// This module implements the rhythm lighting algorithm with a flat-topped
/// bell curve that peaks at solar noon and reaches minimum at sunrise/sunset.
///
/// # Features
///
/// - Super-Gaussian (flat-topped bell) curve shape
/// - Configurable peak flatness via shape_p
/// - Asymmetric morning/evening ramp speeds via width parameters
/// - Separate brightness and CCT width controls
/// - Step-based dimming along the curve
#[derive(Debug, Clone)]
pub struct RhythmCurveModule {
    config: CurveConfig,
    idle: IdleCurveModule,
}

impl RhythmCurveModule {
    /// Module identifier.
    pub const ID: &'static str = "rhythm";

    /// Module display name.
    pub const NAME: &'static str = "Rhythm Curve";

    /// Create a new RhythmCurveModule with the given configuration.
    pub fn new(config: CurveConfig) -> Self {
        Self {
            config,
            idle: IdleCurveModule::with_defaults(),
        }
    }

    /// Create with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(CurveConfig::default())
    }

    /// Get a reference to the configuration.
    pub fn config(&self) -> &CurveConfig {
        &self.config
    }

    /// Get a mutable reference to the configuration.
    pub fn config_mut(&mut self) -> &mut CurveConfig {
        &mut self.config
    }

    /// Set the configuration.
    pub fn set_config(&mut self, config: CurveConfig) {
        self.config = config;
    }

    /// Update sun times in the configuration context.
    ///
    /// Note: Sun times are passed via CurveContext, so this is mainly
    /// for compatibility with the old AdaptiveLighting API.
    pub fn update_sun_times(&mut self, _sun_times: Option<SunTimes>) {
        // Sun times are now passed via CurveContext, not stored in module
    }

    /// Get sunrise hour from context or use fallback.
    fn get_sunrise(&self, sun_times: Option<&SunTimes>) -> f32 {
        sun_times
            .map(|st| st.sunrise)
            .unwrap_or(FALLBACK_SUNRISE_HOUR)
    }

    /// Get sunset hour from context or use fallback.
    fn get_sunset(&self, sun_times: Option<&SunTimes>) -> f32 {
        sun_times
            .map(|st| st.sunset)
            .unwrap_or(FALLBACK_SUNSET_HOUR)
    }

    /// Calculate motion timeout based on time of day.
    ///
    /// The config value (`motion_timeout_secs`) is the "long" timeout (default 20 min).
    /// Morning and evening get the full value (getting ready, winding down).
    /// Midday and night get 1/4 (passing through rooms, brief trips — default 5 min).
    ///
    /// Periods (solar time):
    ///   sunrise → sunrise+3h : morning (long)
    ///   sunrise+3h → sunset-3h : midday (short)
    ///   sunset-3h → sunset+1h : evening (long)
    ///   sunset+1h → sunrise : night (short)
    fn calculate_motion_timeout(&self, ctx: &CurveContext) -> u16 {
        // Manual override: return the user's fixed value
        if let Some(manual) = self.config.motion_timeout_secs {
            return manual;
        }

        // Auto mode: vary by time of day using internal baseline
        let sun_times_ref = ctx.sun_times.as_ref();
        let sunrise = self.get_sunrise(sun_times_ref);
        let sunset = self.get_sunset(sun_times_ref);
        let hour = ctx.current_hour;

        let long = DEFAULT_MOTION_TIMEOUT_SECS;
        let short = long / 4;

        let morning_end = sunrise + 3.0;
        let evening_start = sunset - 3.0;
        let night_start = sunset + 1.0;

        if hour >= sunrise && hour < morning_end {
            long // morning: getting ready
        } else if hour >= morning_end && hour < evening_start {
            short // midday: passing through
        } else if hour >= evening_start && hour < night_start {
            long // evening: winding down
        } else {
            short // night: brief trips
        }
    }

    /// Find the hour that produces a target brightness.
    ///
    /// Uses the inverse super-Gaussian function to find the time.
    ///
    /// # Arguments
    ///
    /// * `target_brightness` - Target brightness percentage
    /// * `is_morning` - Whether to search morning or evening side
    /// * `sun_times` - Optional sun times for curve computation
    ///
    /// # Returns
    ///
    /// Hour that produces the target brightness, or None if not found.
    fn find_hour_for_brightness(
        &self,
        target_brightness: f32,
        is_morning: bool,
        sun_times: Option<&SunTimes>,
    ) -> Option<f32> {
        let sunrise = self.get_sunrise(sun_times);
        let sunset = self.get_sunset(sun_times);

        inverse_super_gaussian(
            target_brightness,
            sunrise,
            sunset,
            self.config.effective_width_left_bri(),
            self.config.effective_width_right_bri(),
            self.config.effective_shape_p(),
            self.config.min_brightness as f32,
            self.config.max_brightness as f32,
            is_morning,
        )
    }
}

impl Default for RhythmCurveModule {
    fn default() -> Self {
        Self::with_defaults()
    }
}

impl LightCurveModule for RhythmCurveModule {
    fn id(&self) -> &str {
        Self::ID
    }

    fn name(&self) -> &str {
        Self::NAME
    }

    fn calculate(&self, ctx: &CurveContext) -> LightingValues {
        let kelvin = self.calculate_color_temperature(ctx);
        let brightness = self.calculate_brightness(ctx);
        let solar_time = ctx.solar.get_solar_time(ctx.current_hour);
        let sun_position = ctx.solar.get_sun_position(ctx.current_hour);

        debug!(
            "Curve: local={:.2}h solar={:.2}h sun={:.2} -> {}% {}K",
            ctx.current_hour, solar_time, sun_position, brightness, kelvin
        );

        let motion_timeout = self.calculate_motion_timeout(ctx);
        let fade = self.config.fade_ms.unwrap_or(DEFAULT_FADE_MS) as u32;
        let mut values = LightingValues::new(
            kelvin,
            brightness,
            solar_time,
            sun_position,
            fade,
            motion_timeout,
        );
        values.suggested_tick_interval_secs = self.suggested_tick_interval(ctx);
        values
    }

    fn calculate_brightness(&self, ctx: &CurveContext) -> u8 {
        let sun_times_ref = ctx.sun_times.as_ref();
        let sunrise = self.get_sunrise(sun_times_ref);
        let sunset = self.get_sunset(sun_times_ref);

        let value = map_super_gaussian(
            ctx.current_hour,
            sunrise,
            sunset,
            self.config.effective_width_left_bri(),
            self.config.effective_width_right_bri(),
            self.config.effective_shape_p(),
            self.config.min_brightness as f32,
            self.config.max_brightness as f32,
        );

        value.round().clamp(
            self.config.min_brightness as f32,
            self.config.max_brightness as f32,
        ) as u8
    }

    fn calculate_color_temperature(&self, ctx: &CurveContext) -> u16 {
        let sun_times_ref = ctx.sun_times.as_ref();
        let sunrise = self.get_sunrise(sun_times_ref);
        let sunset = self.get_sunset(sun_times_ref);

        let value = map_super_gaussian(
            ctx.current_hour,
            sunrise,
            sunset,
            self.config.effective_width_left_cct(),
            self.config.effective_width_right_cct(),
            self.config.effective_shape_p(),
            self.config.min_color_temp as f32,
            self.config.max_color_temp as f32,
        );

        value.round().clamp(
            self.config.min_color_temp as f32,
            self.config.max_color_temp as f32,
        ) as u16
    }

    fn calculate_with_offset(&self, ctx: &CurveContext, offset_minutes: f32) -> LightingValues {
        let offset_hours = offset_minutes / 60.0;
        let mut target_hour = ctx.current_hour + offset_hours;

        // Wrap to 0-24 range
        while target_hour < 0.0 {
            target_hour += 24.0;
        }
        while target_hour >= 24.0 {
            target_hour -= 24.0;
        }

        let offset_ctx = CurveContext::new(target_hour, ctx.solar, ctx.sun_times);
        self.calculate(&offset_ctx)
    }

    fn calculate_step(&self, ctx: &CurveContext, action: StepAction) -> StepResult {
        let sun_times_ref = ctx.sun_times.as_ref();
        let sunrise = self.get_sunrise(sun_times_ref);
        let sunset = self.get_sunset(sun_times_ref);
        let mu = (sunrise + sunset) / 2.0;

        // Determine if we're on the morning (ascending) or evening (descending) side
        let is_morning = ctx.current_hour < mu;

        // Get current values
        let current_values = self.calculate(ctx);
        let current_brightness = current_values.brightness as f32;
        let current_kelvin = current_values.kelvin;

        // Calculate step size
        let step_size = self.config.brightness_step_size();
        let direction = action.direction();

        // Check if at curve boundary
        // For super-Gaussian: max is at mu (solar noon), min is in the flat tails
        let at_boundary = match action {
            StepAction::Dim => {
                // At minimum when brightness has reached the floor
                current_brightness <= self.config.min_brightness as f32
            }
            StepAction::Brighten => {
                // At maximum when we've reached mu (the peak)
                current_brightness >= self.config.max_brightness as f32
            }
        };

        if at_boundary {
            return StepResult {
                values: current_values,
                time_offset_minutes: 0.0,
                at_boundary: true,
            };
        }

        // Calculate target brightness
        let target_brightness = (current_brightness + (direction as f32 * step_size)).clamp(
            self.config.min_brightness as f32,
            self.config.max_brightness as f32,
        );

        // Find hour that produces this brightness
        let target_hour = self
            .find_hour_for_brightness(target_brightness, is_morning, sun_times_ref)
            .unwrap_or(ctx.current_hour);

        // Calculate time offset
        let mut time_offset_hours = target_hour - ctx.current_hour;

        // Handle midnight wraparound for time offset
        // If offset is > 12 hours, we probably wrapped the wrong way
        if time_offset_hours > 12.0 {
            time_offset_hours -= 24.0;
        } else if time_offset_hours < -12.0 {
            time_offset_hours += 24.0;
        }

        // Verify the step is moving in the correct direction
        // Dim: evening moves forward (+), morning moves backward (-)
        // Brighten: evening moves backward (-), morning moves forward (+)
        let expected_direction = match (action, is_morning) {
            (StepAction::Dim, false) => 1.0, // Evening dim: forward toward midnight
            (StepAction::Dim, true) => -1.0, // Morning dim: backward toward midnight
            (StepAction::Brighten, false) => -1.0, // Evening brighten: backward toward noon
            (StepAction::Brighten, true) => 1.0, // Morning brighten: forward toward noon
        };

        // If moving in wrong direction or not moving, we're at boundary
        if time_offset_hours * expected_direction <= 0.0 {
            return StepResult {
                values: current_values,
                time_offset_minutes: 0.0,
                at_boundary: true,
            };
        }
        let time_offset_minutes = time_offset_hours * 60.0;

        // Calculate target values at the new hour
        let target_ctx = CurveContext::new(target_hour, ctx.solar, ctx.sun_times);
        let target_values = self.calculate(&target_ctx);

        info!(
            "Step {:?}: {}% {}K -> {}% {}K (offset {:.1}min)",
            action,
            current_brightness as u8,
            current_kelvin,
            target_values.brightness,
            target_values.kelvin,
            time_offset_minutes
        );

        StepResult {
            values: target_values,
            time_offset_minutes,
            at_boundary: false,
        }
    }

    fn is_at_maximum(&self, ctx: &CurveContext) -> bool {
        let values = self.calculate(ctx);
        values.brightness >= self.config.max_brightness
            && values.kelvin >= self.config.max_color_temp
    }

    fn is_at_minimum(&self, ctx: &CurveContext) -> bool {
        let values = self.calculate(ctx);
        values.brightness <= self.config.min_brightness
            && values.kelvin <= self.config.min_color_temp
    }

    fn min_brightness(&self) -> u8 {
        self.config.min_brightness
    }

    fn max_brightness(&self) -> u8 {
        self.config.max_brightness
    }

    fn min_color_temp(&self) -> u16 {
        self.config.min_color_temp
    }

    fn max_color_temp(&self) -> u16 {
        self.config.max_color_temp
    }

    fn suggested_tick_interval(&self, ctx: &CurveContext) -> Option<u16> {
        // Compute rate of change (brightness per minute) via numerical differentiation
        let delta = 1.0 / 60.0; // 1 minute in hours
        let ctx_ahead = CurveContext::new(
            (ctx.current_hour + delta).rem_euclid(24.0),
            ctx.solar,
            ctx.sun_times,
        );

        let bri_now = self.calculate_brightness(ctx) as f32;
        let bri_ahead = self.calculate_brightness(&ctx_ahead) as f32;
        let rate = (bri_ahead - bri_now).abs(); // brightness change per minute

        // Map rate to interval:
        //   rate < 0.01  → 180s (plateau, barely changing)
        //   rate < 0.1   → 120s (gentle slope)
        //   rate < 0.3   →  60s (moderate transition)
        //   rate < 0.5   →  30s (active ramp)
        //   rate >= 0.5  →  15s (steep sunrise/sunset)
        let secs = if rate < 0.01 {
            180
        } else if rate < 0.1 {
            120
        } else if rate < 0.3 {
            60
        } else if rate < 0.5 {
            30
        } else {
            15
        };

        Some(secs)
    }

    fn calculate_idle(&self, ctx: &CurveContext) -> LightingValues {
        let mut values = self.idle.calculate(ctx);
        // Inherit from the main curve when idle config doesn't override
        if self.idle.config().fade_ms.is_none() {
            values.transition_ms = self.config.fade_ms.unwrap_or(DEFAULT_FADE_MS) as u32;
        }
        if self.idle.config().motion_timeout_secs.is_none() {
            values.motion_timeout_secs = self.calculate_motion_timeout(ctx);
        }
        values
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solar::SolarTime;

    fn test_sun_times() -> SunTimes {
        SunTimes {
            sunrise: 6.0,
            sunset: 18.0,
            day_length: 12.0,
        }
    }

    fn test_config() -> CurveConfig {
        CurveConfig::default()
    }

    fn test_module() -> RhythmCurveModule {
        RhythmCurveModule::new(test_config())
    }

    fn test_context(hour: f32) -> CurveContext {
        CurveContext::new(
            hour,
            SolarTime::new(12.0, 35.0, 172),
            Some(test_sun_times()),
        )
    }

    #[test]
    fn test_module_id_and_name() {
        let module = test_module();
        assert_eq!(module.id(), "rhythm");
        assert_eq!(module.name(), "Rhythm Curve");
    }

    #[test]
    fn test_brightness_at_noon() {
        let module = test_module();
        let ctx = test_context(12.0);

        let brightness = module.calculate_brightness(&ctx);

        // At noon (mu), brightness should be at maximum
        assert!(
            brightness > 95,
            "At noon, expected brightness > 95, got {}",
            brightness
        );
    }

    #[test]
    fn test_brightness_at_sunrise() {
        let module = test_module();
        let ctx = test_context(6.0);

        let brightness = module.calculate_brightness(&ctx);

        // At sunrise, brightness should be near minimum
        assert!(
            brightness < 15,
            "At sunrise, expected brightness < 15, got {}",
            brightness
        );
    }

    #[test]
    fn test_brightness_at_sunset() {
        let module = test_module();
        let ctx = test_context(18.0);

        let brightness = module.calculate_brightness(&ctx);

        // At sunset, brightness should be near minimum
        assert!(
            brightness < 30,
            "At sunset, expected brightness < 30, got {}",
            brightness
        );
    }

    #[test]
    fn test_brightness_at_midnight() {
        let module = test_module();
        let ctx = test_context(0.0);

        let brightness = module.calculate_brightness(&ctx);

        // At midnight, brightness should be at absolute minimum
        assert!(
            brightness <= 2,
            "At midnight, expected brightness <= 2, got {}",
            brightness
        );
    }

    #[test]
    fn test_color_temp_at_noon() {
        use crate::config::DEFAULT_MAX_COLOR_TEMP;
        let module = test_module();
        let ctx = test_context(12.0);

        let kelvin = module.calculate_color_temperature(&ctx);

        // At noon, color temp should be near maximum (cool/daylight)
        assert!(
            kelvin >= DEFAULT_MAX_COLOR_TEMP - 100,
            "At noon, expected kelvin >= {}, got {}",
            DEFAULT_MAX_COLOR_TEMP - 100,
            kelvin
        );
    }

    #[test]
    fn test_color_temp_at_sunrise() {
        use crate::config::DEFAULT_MIN_COLOR_TEMP;
        let module = test_module();
        let ctx = test_context(6.0);

        let kelvin = module.calculate_color_temperature(&ctx);

        // At sunrise, color temp should be near minimum (warm)
        assert!(
            kelvin <= DEFAULT_MIN_COLOR_TEMP + 300,
            "At sunrise, expected kelvin near min ({}), got {}",
            DEFAULT_MIN_COLOR_TEMP,
            kelvin
        );
    }

    #[test]
    fn test_complete_calculation() {
        use crate::config::DEFAULT_MAX_COLOR_TEMP;
        let module = test_module();
        let ctx = test_context(12.0);

        let values = module.calculate(&ctx);

        assert!(values.brightness > 95);
        assert!(
            values.kelvin >= DEFAULT_MAX_COLOR_TEMP - 100,
            "At noon, expected kelvin >= {}, got {}",
            DEFAULT_MAX_COLOR_TEMP - 100,
            values.kelvin
        );
    }

    #[test]
    fn test_calculate_with_offset() {
        let module = test_module();
        let ctx = test_context(12.0);

        // At 12:00 with +60 minute offset should give same as 13:00
        let offset_values = module.calculate_with_offset(&ctx, 60.0);
        let ctx_13 = test_context(13.0);
        let direct_values = module.calculate(&ctx_13);

        assert_eq!(offset_values.brightness, direct_values.brightness);
        assert_eq!(offset_values.kelvin, direct_values.kelvin);
    }

    #[test]
    fn test_plateau_detection() {
        let module = test_module();

        // At noon should be at or near maximum
        let ctx_noon = test_context(12.0);
        assert!(
            module.is_at_maximum(&ctx_noon),
            "Should be at maximum at solar noon"
        );

        // At midnight should be at or near minimum
        let ctx_midnight = test_context(0.0);
        assert!(
            module.is_at_minimum(&ctx_midnight),
            "Should be at minimum at midnight"
        );
    }

    #[test]
    fn test_step_brighten_morning() {
        let module = test_module();
        let ctx = test_context(8.0); // Morning, before noon

        let result = module.calculate_step(&ctx, StepAction::Brighten);

        // Should move forward in time (toward noon)
        assert!(
            result.time_offset_minutes > 0.0,
            "Brightening in morning should increase time offset, got {}",
            result.time_offset_minutes
        );
        assert!(!result.at_boundary, "Should not be at boundary at 8am");
    }

    #[test]
    fn test_step_dim_morning() {
        let module = test_module();
        let ctx = test_context(8.0);

        let result = module.calculate_step(&ctx, StepAction::Dim);

        // Should move backward in time (toward sunrise)
        assert!(
            result.time_offset_minutes < 0.0,
            "Dimming in morning should decrease time offset, got {}",
            result.time_offset_minutes
        );
    }

    #[test]
    fn test_step_brighten_evening() {
        let module = test_module();
        let ctx = test_context(16.0); // Evening, after noon

        let result = module.calculate_step(&ctx, StepAction::Brighten);

        // Should move backward toward noon
        assert!(
            result.time_offset_minutes < 0.0,
            "Brightening in evening should move toward noon, got {}",
            result.time_offset_minutes
        );
    }

    #[test]
    fn test_step_dim_evening() {
        let module = test_module();
        let ctx = test_context(16.0);

        let result = module.calculate_step(&ctx, StepAction::Dim);

        // Should move forward toward sunset
        assert!(
            result.time_offset_minutes > 0.0,
            "Dimming in evening should move toward sunset, got {}",
            result.time_offset_minutes
        );
    }

    #[test]
    fn test_step_at_maximum_boundary() {
        let module = test_module();
        let ctx = test_context(12.0);

        let result = module.calculate_step(&ctx, StepAction::Brighten);

        assert!(
            result.at_boundary,
            "At noon, brightening should hit boundary"
        );
        assert!(
            result.time_offset_minutes.abs() < 0.1,
            "At boundary, time offset should be ~0"
        );
    }

    #[test]
    fn test_step_at_minimum_boundary() {
        let module = test_module();
        let ctx = test_context(0.0); // At midnight - actual minimum for super-Gaussian

        let result = module.calculate_step(&ctx, StepAction::Dim);

        assert!(
            result.at_boundary,
            "At midnight (minimum brightness), dimming should hit boundary"
        );
    }

    #[test]
    fn test_min_max_accessors() {
        use crate::config::{
            DEFAULT_MAX_BRIGHTNESS, DEFAULT_MAX_COLOR_TEMP, DEFAULT_MIN_BRIGHTNESS,
            DEFAULT_MIN_COLOR_TEMP,
        };
        let module = test_module();

        assert_eq!(module.min_brightness(), DEFAULT_MIN_BRIGHTNESS);
        assert_eq!(module.max_brightness(), DEFAULT_MAX_BRIGHTNESS);
        assert_eq!(module.min_color_temp(), DEFAULT_MIN_COLOR_TEMP);
        assert_eq!(module.max_color_temp(), DEFAULT_MAX_COLOR_TEMP);
    }

    #[test]
    fn test_config_modification() {
        let mut module = test_module();

        module.config_mut().min_brightness = 10;
        module.config_mut().max_brightness = 90;

        assert_eq!(module.min_brightness(), 10);
        assert_eq!(module.max_brightness(), 90);
    }

    #[test]
    fn test_context_without_sun_times() {
        use crate::config::DEFAULT_MAX_COLOR_TEMP;
        let module = test_module();
        // Without sun_times, uses fallback (6:00 sunrise, 20:00 sunset)
        let ctx = CurveContext::new(13.0, SolarTime::new(12.0, 35.0, 172), None);

        // Should still work with fallback values
        // mu = (6 + 20) / 2 = 13, so at 13:00 we're at peak
        let values = module.calculate(&ctx);
        assert!(
            values.brightness > 95,
            "At peak with fallback, expected >95, got {}",
            values.brightness
        );
        assert!(
            values.kelvin >= DEFAULT_MAX_COLOR_TEMP - 100,
            "At peak, expected kelvin >= {}, got {}",
            DEFAULT_MAX_COLOR_TEMP - 100,
            values.kelvin
        );
    }

    #[test]
    fn test_width_affects_ramp_speed() {
        // Faster width (< 1) should result in higher brightness at the same hour
        let mut fast_config = test_config();
        fast_config.width_left_bri = 0.5;

        let mut slow_config = test_config();
        slow_config.width_left_bri = 1.5;

        let fast_module = RhythmCurveModule::new(fast_config);
        let slow_module = RhythmCurveModule::new(slow_config);

        let ctx = test_context(8.0); // Morning

        let fast_bri = fast_module.calculate_brightness(&ctx);
        let slow_bri = slow_module.calculate_brightness(&ctx);

        assert!(
            fast_bri > slow_bri,
            "Faster ramp should give higher brightness: fast={}, slow={}",
            fast_bri,
            slow_bri
        );
    }

    #[test]
    fn test_shape_p_affects_flatness() {
        // Higher shape_p = flatter top = higher brightness near peak
        let mut round_config = test_config();
        round_config.shape_p = 2.0;

        let mut flat_config = test_config();
        flat_config.shape_p = 10.0;

        let round_module = RhythmCurveModule::new(round_config);
        let flat_module = RhythmCurveModule::new(flat_config);

        let ctx = test_context(10.0); // Near noon but not at peak

        let round_bri = round_module.calculate_brightness(&ctx);
        let flat_bri = flat_module.calculate_brightness(&ctx);

        assert!(
            flat_bri > round_bri,
            "Flatter curve should give higher brightness near peak: flat={}, round={}",
            flat_bri,
            round_bri
        );
    }

    #[test]
    fn test_suggested_tick_interval_plateau_vs_ramp() {
        let module = test_module();

        // At noon (plateau) — brightness barely changes, should get long interval
        let ctx_noon = test_context(12.0);
        let interval_noon = module.suggested_tick_interval(&ctx_noon).unwrap();
        assert!(
            interval_noon >= 120,
            "At noon plateau, expected >= 120s, got {}",
            interval_noon
        );

        // Near sunrise (steep ramp) — brightness changing fast, should get short interval
        let ctx_sunrise = test_context(7.0);
        let interval_sunrise = module.suggested_tick_interval(&ctx_sunrise).unwrap();
        assert!(
            interval_sunrise <= 60,
            "Near sunrise ramp, expected <= 60s, got {}",
            interval_sunrise
        );

        // Sunrise interval should be shorter than noon interval
        assert!(
            interval_sunrise < interval_noon,
            "Ramp interval ({}) should be shorter than plateau interval ({})",
            interval_sunrise,
            interval_noon
        );
    }

    #[test]
    fn test_suggested_tick_interval_midnight_is_long() {
        let module = test_module();
        let ctx = test_context(0.0);
        let interval = module.suggested_tick_interval(&ctx).unwrap();
        assert!(
            interval >= 120,
            "At midnight, expected >= 120s, got {}",
            interval
        );
    }

    #[test]
    fn test_calculate_includes_suggested_tick() {
        let module = test_module();
        let ctx = test_context(12.0);
        let values = module.calculate(&ctx);
        assert!(
            values.suggested_tick_interval_secs.is_some(),
            "RhythmCurveModule should always provide a suggested tick interval"
        );
    }

    #[test]
    fn test_motion_timeout_auto_varies_by_time() {
        let mut config = test_config();
        config.motion_timeout_secs = None; // auto mode
        let module = RhythmCurveModule::new(config);

        // Morning (7am, sunrise=6) → long timeout (20 min)
        let morning = module.calculate(&test_context(7.0));
        assert_eq!(
            morning.motion_timeout_secs, 1200,
            "morning should get full timeout"
        );

        // Midday (12pm) → short timeout (5 min)
        let midday = module.calculate(&test_context(12.0));
        assert_eq!(
            midday.motion_timeout_secs, 300,
            "midday should get 1/4 timeout"
        );

        // Evening (16pm, sunset=18) → long timeout (20 min)
        let evening = module.calculate(&test_context(16.0));
        assert_eq!(
            evening.motion_timeout_secs, 1200,
            "evening should get full timeout"
        );

        // Night (23pm) → short timeout (5 min)
        let night = module.calculate(&test_context(23.0));
        assert_eq!(
            night.motion_timeout_secs, 300,
            "night should get 1/4 timeout"
        );
    }

    #[test]
    fn test_motion_timeout_manual_fixed() {
        let mut config = test_config();
        config.motion_timeout_secs = Some(600); // manual 10 min
        let module = RhythmCurveModule::new(config);

        // Manual value returned at all times of day
        assert_eq!(
            module.calculate(&test_context(7.0)).motion_timeout_secs,
            600
        );
        assert_eq!(
            module.calculate(&test_context(12.0)).motion_timeout_secs,
            600
        );
        assert_eq!(
            module.calculate(&test_context(16.0)).motion_timeout_secs,
            600
        );
        assert_eq!(
            module.calculate(&test_context(23.0)).motion_timeout_secs,
            600
        );
    }
}
