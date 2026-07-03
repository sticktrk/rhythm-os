//! Curve calculation context.
//!
//! This module provides the [`CurveContext`] struct which carries all
//! the information a light profile needs to calculate lighting values.

use crate::solar::{wrap_hour_24, SolarTime, SunTimes};

/// Context for curve calculations.
///
/// Contains all the information needed for a light profile to calculate
/// lighting values at a given time.
#[derive(Debug, Clone)]
pub struct CurveContext {
    /// Current local hour (0-24)
    pub current_hour: f32,

    /// Solar time reference for coordinate-based calculations
    pub solar: SolarTime,

    /// Optional sunrise/sunset times for dynamic midpoint resolution
    pub sun_times: Option<SunTimes>,
}

impl CurveContext {
    /// Create a new curve context.
    pub fn new(current_hour: f32, solar: SolarTime, sun_times: Option<SunTimes>) -> Self {
        Self {
            current_hour,
            solar,
            sun_times,
        }
    }

    /// Create a context with offset applied to the current hour.
    pub fn with_offset(&self, offset_minutes: f32) -> Self {
        let offset_hours = offset_minutes / 60.0;
        // Wrapped to 0-24 range; offsets come from unvalidated API floats.
        let target_hour = wrap_hour_24(self.current_hour + offset_hours);

        Self {
            current_hour: target_hour,
            solar: self.solar,
            sun_times: self.sun_times,
        }
    }

    /// Get the solar time for the current local hour.
    pub fn solar_time(&self) -> f32 {
        self.solar.get_solar_time(self.current_hour)
    }

    /// Check if it's morning (before solar noon).
    pub fn is_morning(&self) -> bool {
        self.solar.is_morning(self.current_hour)
    }
}

impl Default for CurveContext {
    fn default() -> Self {
        Self {
            current_hour: 12.0,
            solar: SolarTime::default(),
            sun_times: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_curve_context_default() {
        let ctx = CurveContext::default();
        assert_eq!(ctx.current_hour, 12.0);
        assert!(ctx.sun_times.is_none());
    }

    #[test]
    fn test_curve_context_with_offset() {
        let ctx = CurveContext::new(12.0, SolarTime::default(), None);

        let offset_ctx = ctx.with_offset(60.0);
        assert!((offset_ctx.current_hour - 13.0).abs() < 0.01);

        let offset_ctx = ctx.with_offset(-180.0);
        assert!((offset_ctx.current_hour - 9.0).abs() < 0.01);
    }

    #[test]
    fn test_curve_context_with_offset_wrap() {
        let ctx = CurveContext::new(23.0, SolarTime::default(), None);
        let offset_ctx = ctx.with_offset(120.0);
        assert!((offset_ctx.current_hour - 1.0).abs() < 0.01);

        let ctx = CurveContext::new(1.0, SolarTime::default(), None);
        let offset_ctx = ctx.with_offset(-180.0);
        assert!((offset_ctx.current_hour - 22.0).abs() < 0.01);
    }

    #[test]
    fn test_curve_context_solar_time() {
        let solar = SolarTime::new(12.5, 35.0, 172);
        let ctx = CurveContext::new(12.0, solar, None);
        let solar_time = ctx.solar_time();
        assert!((solar_time - 11.5).abs() < 0.01);
    }

    #[test]
    fn test_curve_context_is_morning() {
        let solar = SolarTime::new(12.0, 35.0, 172);

        let morning = CurveContext::new(8.0, solar, None);
        assert!(morning.is_morning());

        let afternoon = CurveContext::new(15.0, solar, None);
        assert!(!afternoon.is_morning());
    }

    #[test]
    fn test_curve_context_with_sun_times() {
        let solar = SolarTime::new(12.0, 35.0, 172);
        let sun_times = SunTimes {
            sunrise: 6.0,
            sunset: 20.0,
            day_length: 14.0,
        };
        let ctx = CurveContext::new(10.0, solar, Some(sun_times));
        assert!(ctx.sun_times.is_some());
        assert_eq!(ctx.sun_times.unwrap().sunrise, 6.0);
    }

    #[test]
    fn test_curve_context_with_offset_preserves_solar() {
        let solar = SolarTime::new(12.5, 40.0, 180);
        let sun_times = SunTimes {
            sunrise: 5.5,
            sunset: 20.5,
            day_length: 15.0,
        };
        let ctx = CurveContext::new(10.0, solar, Some(sun_times));
        let offset = ctx.with_offset(60.0);

        // Solar and sun_times should be unchanged
        assert_eq!(offset.solar, solar);
        assert_eq!(offset.sun_times, Some(sun_times));
        assert!((offset.current_hour - 11.0).abs() < 0.01);
    }

    #[test]
    fn test_curve_context_zero_offset() {
        let ctx = CurveContext::new(10.0, SolarTime::default(), None);
        let offset = ctx.with_offset(0.0);
        assert!((offset.current_hour - 10.0).abs() < 0.01);
    }

    #[test]
    fn test_curve_context_large_offset() {
        let ctx = CurveContext::new(12.0, SolarTime::default(), None);
        // +25 hours should wrap to +1 hour
        let offset = ctx.with_offset(25.0 * 60.0);
        assert!((offset.current_hour - 13.0).abs() < 0.01);
    }

    #[test]
    fn test_curve_context_large_negative_offset() {
        let ctx = CurveContext::new(12.0, SolarTime::default(), None);
        // -25 hours should wrap to -1 hour
        let offset = ctx.with_offset(-25.0 * 60.0);
        assert!((offset.current_hour - 11.0).abs() < 0.01);
    }
}
