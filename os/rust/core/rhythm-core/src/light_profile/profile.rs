//! Generic config-driven light profile implementation.
//!
//! [`LightProfile`] implements [`LightProfileModule`] using a
//! [`LightProfileConfig`]. The curve shape variant determines the
//! mathematical algorithm; the profile config provides output ranges
//! and timer settings.

use rhythm_profile::color::rgb_to_xy;
use rhythm_profile::context::CurveContext;
use rhythm_profile::curve_shape::{LightCurveShape, LightDirectColor};
use rhythm_profile::module::LightProfileModule;
use rhythm_profile::profile_config::{LightProfileConfig, DEFAULT_FADE_MS};
use rhythm_profile::steps::{StepAction, StepResult};
use rhythm_profile::values::LightingValues;
use rhythm_profile::SunTimes;

use crate::config::{DEFAULT_MOTION_TIMEOUT_SECS, FALLBACK_SUNRISE_HOUR, FALLBACK_SUNSET_HOUR};
use crate::curves::{inverse_super_gaussian, map_super_gaussian};

/// A generic, config-driven light profile.
///
/// All profile types (rhythm, sleep, idle, custom) use this single struct.
/// The [`LightCurveShape`] in the config determines the curve math;
/// the rest of the config provides output ranges and timer settings.
#[derive(Debug, Clone)]
pub struct LightProfile {
    config: LightProfileConfig,
}

impl LightProfile {
    /// Create a new profile from a config.
    pub fn new(config: LightProfileConfig) -> Self {
        Self { config }
    }

    /// Get a reference to the configuration.
    pub fn config(&self) -> &LightProfileConfig {
        &self.config
    }

    /// Set the configuration.
    pub fn set_config(&mut self, config: LightProfileConfig) {
        self.config = config;
    }

    // ── Helpers ──────────────────────────────────────────────────

    fn get_sunrise(sun_times: Option<&SunTimes>) -> f32 {
        sun_times
            .map(|st| st.sunrise)
            .unwrap_or(FALLBACK_SUNRISE_HOUR)
    }

    fn get_sunset(sun_times: Option<&SunTimes>) -> f32 {
        sun_times
            .map(|st| st.sunset)
            .unwrap_or(FALLBACK_SUNSET_HOUR)
    }

    /// Calculate motion timeout at the given hour.
    ///
    /// For [`TimerSetting::Fixed`] and [`TimerSetting::Scheduled`], resolves
    /// directly. For [`TimerSetting::Auto`], uses time-of-day bands based on
    /// sunrise/sunset.
    fn calculate_motion_timeout(&self, ctx: &CurveContext) -> u16 {
        if let Some(val) = self.config.motion_timeout_secs.resolve(ctx.current_hour) {
            return val as u16;
        }

        // Auto: time-of-day bands
        let sun_times_ref = ctx.sun_times.as_ref();
        let sunrise = Self::get_sunrise(sun_times_ref);
        let sunset = Self::get_sunset(sun_times_ref);
        let hour = ctx.current_hour;

        let long = DEFAULT_MOTION_TIMEOUT_SECS;
        let short = long / 4;

        let morning_end = sunrise + 3.0;
        let evening_start = sunset - 3.0;
        let night_start = sunset + 1.0;

        if hour >= sunrise && hour < morning_end {
            long
        } else if hour >= morning_end && hour < evening_start {
            short
        } else if hour >= evening_start && hour < night_start {
            long
        } else {
            short
        }
    }

    /// Find the hour that produces a target brightness (super-Gaussian only).
    fn find_hour_for_brightness(
        &self,
        target_brightness: f32,
        is_morning: bool,
        sun_times: Option<&SunTimes>,
    ) -> Option<f32> {
        if let Some((wlb, wrb, _, _, sp)) = self.config.curve.effective_sg_params() {
            let sunrise = Self::get_sunrise(sun_times);
            let sunset = Self::get_sunset(sun_times);

            inverse_super_gaussian(
                target_brightness,
                sunrise,
                sunset,
                wlb,
                wrb,
                sp,
                self.config.min_brightness as f32,
                self.config.max_brightness as f32,
                is_morning,
            )
        } else {
            None
        }
    }

    fn constant_brightness(&self) -> u8 {
        self.config.max_brightness.max(self.config.min_brightness)
    }

    fn fallback_direct_color(&self) -> LightDirectColor {
        LightDirectColor {
            xy: rhythm_profile::XyColor { x: 0.612, y: 0.356 },
            rgb: rhythm_profile::Rgb::new(255, 147, 41),
        }
    }

    fn fallback_inherit_active_values(&self, ctx: &CurveContext) -> LightingValues {
        let fade = self
            .config
            .fade_ms
            .resolve(ctx.current_hour)
            .unwrap_or(DEFAULT_FADE_MS as u32);
        let motion_timeout = self.calculate_motion_timeout(ctx);
        let direct = self.fallback_direct_color();

        LightingValues::from_color(
            direct.rgb,
            direct.xy,
            self.constant_brightness(),
            ctx.solar.get_solar_time(ctx.current_hour),
            ctx.solar.get_sun_position(ctx.current_hour),
            fade,
            motion_timeout,
        )
    }
}

impl LightProfileModule for LightProfile {
    fn id(&self) -> &str {
        &self.config.id
    }

    fn name(&self) -> &str {
        &self.config.name
    }

    fn calculate(&self, ctx: &CurveContext) -> LightingValues {
        let solar_time = ctx.solar.get_solar_time(ctx.current_hour);
        let sun_position = ctx.solar.get_sun_position(ctx.current_hour);
        let motion_timeout = self.calculate_motion_timeout(ctx);
        let fade = self
            .config
            .fade_ms
            .resolve(ctx.current_hour)
            .unwrap_or(DEFAULT_FADE_MS as u32);

        match &self.config.curve {
            LightCurveShape::SuperGaussian { .. } => {
                let brightness = self.calculate_brightness(ctx);
                let kelvin = self.calculate_color_temperature(ctx);

                let mut values = LightingValues::new(
                    kelvin,
                    brightness,
                    solar_time,
                    sun_position,
                    fade,
                    motion_timeout,
                );
                values.suggested_tick_interval_secs = self.suggested_tick_interval(ctx);

                // Apply fixed direct color on top of the super-Gaussian brightness curve.
                if let Some(dc) = self.config.curve.direct_color() {
                    values.xy = dc.xy;
                    values.rgb = dc.rgb;
                    values.is_direct_color = true;
                }

                values
            }
            LightCurveShape::Palette { .. } => {
                let rgb = self
                    .config
                    .curve
                    .sample_color(solar_time)
                    .map(|(rgb, _)| rgb)
                    .unwrap_or_else(|| rhythm_profile::Rgb::new(255, 147, 41));
                let xy = rgb_to_xy(rgb);

                LightingValues::from_color(
                    rgb,
                    xy,
                    self.config.min_brightness, // palette brightness is authoritative
                    solar_time,
                    sun_position,
                    fade,
                    motion_timeout,
                )
            }
            LightCurveShape::Constant {
                brightness,
                color_temp,
                direct_color,
            } => {
                let bri = (self.config.min_brightness as f32
                    + (self.config.max_brightness as f32 - self.config.min_brightness as f32)
                        * brightness)
                    .round()
                    .clamp(
                        self.config.min_brightness as f32,
                        self.config.max_brightness as f32,
                    ) as u8;
                let kelvin = (self.config.min_color_temp as f32
                    + (self.config.max_color_temp as f32 - self.config.min_color_temp as f32)
                        * color_temp)
                    .round()
                    .clamp(
                        self.config.min_color_temp as f32,
                        self.config.max_color_temp as f32,
                    ) as u16;

                let mut values = LightingValues::new(
                    kelvin,
                    bri,
                    solar_time,
                    sun_position,
                    fade,
                    motion_timeout,
                );

                if let Some(dc) = direct_color {
                    values.xy = dc.xy;
                    values.rgb = dc.rgb;
                    values.is_direct_color = true;
                }

                values
            }
            LightCurveShape::InheritActive => self.fallback_inherit_active_values(ctx),
        }
    }

    fn calculate_brightness(&self, ctx: &CurveContext) -> u8 {
        match &self.config.curve {
            LightCurveShape::SuperGaussian { .. } => {
                let (wlb, wrb, _, _, sp) = self.config.curve.effective_sg_params().unwrap();
                let sun_times_ref = ctx.sun_times.as_ref();
                let sunrise = Self::get_sunrise(sun_times_ref);
                let sunset = Self::get_sunset(sun_times_ref);

                let value = map_super_gaussian(
                    ctx.current_hour,
                    sunrise,
                    sunset,
                    wlb,
                    wrb,
                    sp,
                    self.config.min_brightness as f32,
                    self.config.max_brightness as f32,
                );

                value.round().clamp(
                    self.config.min_brightness as f32,
                    self.config.max_brightness as f32,
                ) as u8
            }
            LightCurveShape::Palette { .. } => self.config.min_brightness,
            LightCurveShape::InheritActive => self.constant_brightness(),
            LightCurveShape::Constant { brightness, .. } => {
                let range = self.config.max_brightness as f32 - self.config.min_brightness as f32;
                (self.config.min_brightness as f32 + range * brightness)
                    .round()
                    .clamp(
                        self.config.min_brightness as f32,
                        self.config.max_brightness as f32,
                    ) as u8
            }
        }
    }

    fn calculate_color_temperature(&self, ctx: &CurveContext) -> u16 {
        match &self.config.curve {
            LightCurveShape::SuperGaussian { .. } => {
                let (_, _, wlc, wrc, sp) = self.config.curve.effective_sg_params().unwrap();
                let sun_times_ref = ctx.sun_times.as_ref();
                let sunrise = Self::get_sunrise(sun_times_ref);
                let sunset = Self::get_sunset(sun_times_ref);

                let value = map_super_gaussian(
                    ctx.current_hour,
                    sunrise,
                    sunset,
                    wlc,
                    wrc,
                    sp,
                    self.config.min_color_temp as f32,
                    self.config.max_color_temp as f32,
                );

                value.round().clamp(
                    self.config.min_color_temp as f32,
                    self.config.max_color_temp as f32,
                ) as u16
            }
            LightCurveShape::Palette { .. } => 0,
            LightCurveShape::InheritActive => self.config.min_color_temp,
            LightCurveShape::Constant { color_temp, .. } => {
                let range = self.config.max_color_temp as f32 - self.config.min_color_temp as f32;
                (self.config.min_color_temp as f32 + range * color_temp)
                    .round()
                    .clamp(
                        self.config.min_color_temp as f32,
                        self.config.max_color_temp as f32,
                    ) as u16
            }
        }
    }

    fn calculate_with_offset(&self, ctx: &CurveContext, offset_minutes: f32) -> LightingValues {
        let offset_hours = offset_minutes / 60.0;
        let mut target_hour = ctx.current_hour + offset_hours;
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
        // Only super-Gaussian supports stepping via inverse
        if self.config.curve.effective_sg_params().is_none() {
            return StepResult {
                values: self.calculate(ctx),
                time_offset_minutes: 0.0,
                at_boundary: true,
            };
        }

        let sun_times_ref = ctx.sun_times.as_ref();
        let sunrise = Self::get_sunrise(sun_times_ref);
        let sunset = Self::get_sunset(sun_times_ref);
        let mu = (sunrise + sunset) / 2.0;
        let is_morning = ctx.current_hour < mu;

        let current_values = self.calculate(ctx);
        let current_brightness = current_values.brightness as f32;

        let step_size = self.config.brightness_step_size();
        let direction = action.direction();

        let at_boundary = match action {
            StepAction::Dim => current_brightness <= self.config.min_brightness as f32,
            StepAction::Brighten => current_brightness >= self.config.max_brightness as f32,
        };

        if at_boundary {
            return StepResult {
                values: current_values,
                time_offset_minutes: 0.0,
                at_boundary: true,
            };
        }

        let target_brightness = (current_brightness + (direction as f32 * step_size)).clamp(
            self.config.min_brightness as f32,
            self.config.max_brightness as f32,
        );

        let target_hour = self
            .find_hour_for_brightness(target_brightness, is_morning, sun_times_ref)
            .unwrap_or(ctx.current_hour);

        let mut time_offset_hours = target_hour - ctx.current_hour;
        if time_offset_hours > 12.0 {
            time_offset_hours -= 24.0;
        } else if time_offset_hours < -12.0 {
            time_offset_hours += 24.0;
        }

        let expected_direction = match (action, is_morning) {
            (StepAction::Dim, false) => 1.0,
            (StepAction::Dim, true) => -1.0,
            (StepAction::Brighten, false) => -1.0,
            (StepAction::Brighten, true) => 1.0,
        };

        if time_offset_hours * expected_direction <= 0.0 {
            return StepResult {
                values: current_values,
                time_offset_minutes: 0.0,
                at_boundary: true,
            };
        }

        let time_offset_minutes = time_offset_hours * 60.0;
        let target_ctx = CurveContext::new(target_hour, ctx.solar, ctx.sun_times);
        let target_values = self.calculate(&target_ctx);

        StepResult {
            values: target_values,
            time_offset_minutes,
            at_boundary: false,
        }
    }

    fn is_at_maximum(&self, ctx: &CurveContext) -> bool {
        match &self.config.curve {
            LightCurveShape::SuperGaussian { .. } => {
                let values = self.calculate(ctx);
                values.brightness >= self.config.max_brightness
                    && values.kelvin >= self.config.max_color_temp
            }
            LightCurveShape::Palette { .. } => true,
            LightCurveShape::InheritActive => true,
            LightCurveShape::Constant { .. } => true,
        }
    }

    fn is_at_minimum(&self, ctx: &CurveContext) -> bool {
        match &self.config.curve {
            LightCurveShape::SuperGaussian { .. } => {
                let values = self.calculate(ctx);
                values.brightness <= self.config.min_brightness
                    && values.kelvin <= self.config.min_color_temp
            }
            LightCurveShape::Palette { .. } => true,
            LightCurveShape::InheritActive => true,
            LightCurveShape::Constant { .. } => true,
        }
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
        match &self.config.curve {
            LightCurveShape::SuperGaussian { .. } => {
                let delta = 1.0 / 60.0;
                let ctx_ahead = CurveContext::new(
                    (ctx.current_hour + delta).rem_euclid(24.0),
                    ctx.solar,
                    ctx.sun_times,
                );

                let bri_now = self.calculate_brightness(ctx) as f32;
                let bri_ahead = self.calculate_brightness(&ctx_ahead) as f32;
                let rate = (bri_ahead - bri_now).abs();

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
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::light_profile::defaults::{
        default_idle_profile, default_rhythm_profile, default_sleep_profile,
    };
    use rhythm_profile::{solar::SolarTime, TimerSetting};

    fn test_sun_times() -> SunTimes {
        SunTimes {
            sunrise: 6.0,
            sunset: 18.0,
            day_length: 12.0,
        }
    }

    fn test_context(hour: f32) -> CurveContext {
        CurveContext::new(
            hour,
            SolarTime::new(12.0, 35.0, 172),
            Some(test_sun_times()),
        )
    }

    // ── Rhythm profile tests ────────────────────────────────────

    #[test]
    fn test_rhythm_brightness_at_noon() {
        let profile = LightProfile::new(default_rhythm_profile());
        let bri = profile.calculate_brightness(&test_context(12.0));
        assert!(bri > 95, "At noon, expected >95, got {}", bri);
    }

    #[test]
    fn test_rhythm_brightness_at_sunrise() {
        let profile = LightProfile::new(default_rhythm_profile());
        let bri = profile.calculate_brightness(&test_context(6.0));
        assert!(bri < 15, "At sunrise, expected <15, got {}", bri);
    }

    #[test]
    fn test_rhythm_step_brighten_morning() {
        let profile = LightProfile::new(default_rhythm_profile());
        let result = profile.calculate_step(&test_context(8.0), StepAction::Brighten);
        assert!(result.time_offset_minutes > 0.0);
        assert!(!result.at_boundary);
    }

    #[test]
    fn test_rhythm_tick_interval_varies() {
        let profile = LightProfile::new(default_rhythm_profile());
        let noon = profile
            .suggested_tick_interval(&test_context(12.0))
            .unwrap();
        let sunrise = profile.suggested_tick_interval(&test_context(7.0)).unwrap();
        assert!(
            noon > sunrise,
            "Plateau should be longer interval than ramp"
        );
    }

    // ── Sleep profile tests ─────────────────────────────────────

    #[test]
    fn test_sleep_uses_direct_color() {
        let profile = LightProfile::new(default_sleep_profile());
        let values = profile.calculate(&test_context(12.0));
        assert!(values.is_direct_color);
    }

    #[test]
    fn test_sleep_brightness_capped() {
        let profile = LightProfile::new(default_sleep_profile());
        let bri = profile.calculate_brightness(&test_context(12.0));
        assert!(bri <= 40, "Sleep max should be 40, got {}", bri);
    }

    // ── Idle profile tests ──────────────────────────────────────

    #[test]
    fn test_idle_always_1_percent() {
        let profile = LightProfile::new(default_idle_profile());
        let values = profile.calculate(&test_context(12.0));
        assert_eq!(values.brightness, 1);
        assert!(values.is_direct_color);
    }

    #[test]
    fn test_idle_uses_fallback_color_when_unresolved() {
        let profile = LightProfile::new(default_idle_profile());
        let midnight = profile.calculate(&test_context(0.0));
        let noon = profile.calculate(&test_context(12.0));
        assert_eq!(midnight.rgb, noon.rgb);
    }

    #[test]
    fn test_idle_step_at_boundary() {
        let profile = LightProfile::new(default_idle_profile());
        let result = profile.calculate_step(&test_context(12.0), StepAction::Brighten);
        assert!(result.at_boundary);
    }

    // ── Constant profile tests ──────────────────────────────────

    #[test]
    fn test_constant_fixed_output() {
        let config = LightProfileConfig {
            id: "constant".into(),
            name: "Constant".into(),
            curve: LightCurveShape::Constant {
                brightness: 0.5,
                color_temp: 0.5,
                direct_color: None,
            },
            min_brightness: 0,
            max_brightness: 100,
            min_color_temp: 2000,
            max_color_temp: 6000,
            max_dim_steps: 6,
            fade_ms: TimerSetting::Auto,
            motion_timeout_secs: TimerSetting::Auto,
            rhythm_interval_secs: TimerSetting::Auto,
        };
        let profile = LightProfile::new(config);
        let values = profile.calculate(&test_context(12.0));
        assert_eq!(values.brightness, 50);
        assert_eq!(values.kelvin, 4000);
    }

    #[test]
    fn test_constant_direct_color_fixed_output() {
        let direct = LightDirectColor {
            xy: rhythm_profile::XyColor { x: 0.45, y: 0.25 },
            rgb: rhythm_profile::Rgb::new(255, 40, 150),
        };
        let config = LightProfileConfig {
            id: "constant".into(),
            name: "Constant".into(),
            curve: LightCurveShape::Constant {
                brightness: 0.5,
                color_temp: 0.5,
                direct_color: Some(direct),
            },
            min_brightness: 0,
            max_brightness: 100,
            min_color_temp: 2000,
            max_color_temp: 6000,
            max_dim_steps: 6,
            fade_ms: TimerSetting::Auto,
            motion_timeout_secs: TimerSetting::Auto,
            rhythm_interval_secs: TimerSetting::Auto,
        };
        let profile = LightProfile::new(config);
        let values = profile.calculate(&test_context(12.0));
        assert_eq!(values.brightness, 50);
        assert_eq!(values.kelvin, 4000);
        assert!(values.is_direct_color);
        assert_eq!(values.rgb, rhythm_profile::Rgb::new(255, 40, 150));
        assert_eq!(values.xy, rhythm_profile::XyColor { x: 0.45, y: 0.25 });
    }
}
