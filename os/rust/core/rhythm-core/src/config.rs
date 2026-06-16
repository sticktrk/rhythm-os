//! Shared runtime configuration helpers.
//!
//! The profile-native runtime no longer exposes a separate profile config type.
//! This module now only carries common timing constants and the public
//! `SolarContext` helper type.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};
use rhythm_profile::CurveContext;

use crate::solar::{SunTimes, TwilightTimes};
use crate::{calculate_sun_times, SolarTime, Timezone};

/// Default motion timeout in seconds (20 minutes).
pub const DEFAULT_MOTION_TIMEOUT_SECS: u16 = 1200;

/// Fallback value for sunrise if sun times are unavailable.
pub const FALLBACK_SUNRISE_HOUR: f32 = 6.0;

/// Fallback value for sunset if sun times are unavailable.
pub const FALLBACK_SUNSET_HOUR: f32 = 20.0;

/// Solar context for a specific day.
///
/// Contains the solar timing information used for lighting calculations
/// and API display.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct SolarContext {
    /// Sunrise time in local decimal hours (0-24).
    pub sunrise: f32,
    /// Sunset time in local decimal hours (0-24).
    pub sunset: f32,
    /// Solar noon time in local decimal hours (0-24).
    pub solar_noon: f32,
    /// Solar midnight time in local decimal hours (0-24).
    pub solar_midnight: f32,
    /// Day length in hours.
    pub day_length: f32,
    /// Twilight times (dawn and dusk for civil, nautical, astronomical).
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub twilight: Option<TwilightTimes>,
}

impl SolarContext {
    /// Create a new solar context.
    pub fn new(sunrise: f32, sunset: f32, solar_noon: f32) -> Self {
        Self {
            sunrise,
            sunset,
            solar_noon,
            solar_midnight: (solar_noon + 12.0) % 24.0,
            day_length: sunset - sunrise,
            twilight: None,
        }
    }

    /// Create from `SunTimes` and solar noon.
    pub fn from_sun_times(sun_times: &SunTimes, solar_noon: f32) -> Self {
        Self {
            sunrise: sun_times.sunrise,
            sunset: sun_times.sunset,
            solar_noon,
            solar_midnight: (solar_noon + 12.0) % 24.0,
            day_length: sun_times.day_length,
            twilight: None,
        }
    }

    /// Create a default solar context when location is not configured.
    pub fn default_context() -> Self {
        Self {
            sunrise: FALLBACK_SUNRISE_HOUR,
            sunset: FALLBACK_SUNSET_HOUR,
            solar_noon: 12.0,
            solar_midnight: 0.0,
            day_length: 14.0,
            twilight: None,
        }
    }

    /// Add twilight times to this context.
    pub fn with_twilight(mut self, twilight: TwilightTimes) -> Self {
        self.twilight = Some(twilight);
        self
    }
}

impl Default for SolarContext {
    fn default() -> Self {
        Self::default_context()
    }
}

/// Resolve sun times for a specific local date when full location data is available.
pub fn resolve_sun_times_for_local_date(
    latitude: Option<f32>,
    longitude: Option<f32>,
    timezone_name: Option<&str>,
    sample_date: NaiveDate,
) -> Option<SunTimes> {
    let lat = latitude?;
    let lon = longitude?;
    let tz_name = timezone_name?;
    let tz = Timezone::new(tz_name);

    Some(calculate_sun_times(
        lat,
        lon,
        sample_date.year(),
        sample_date.month(),
        sample_date.day(),
        &tz,
    ))
}

/// Build a curve context for a specific local date and decimal hour.
pub fn curve_context_for_local_date_and_hour(
    solar_noon: f32,
    latitude: Option<f32>,
    longitude: Option<f32>,
    timezone_name: Option<&str>,
    sample_date: NaiveDate,
    sample_hour: f32,
) -> CurveContext {
    let day_of_year =
        crate::timezone::day_of_year(sample_date.year(), sample_date.month(), sample_date.day());
    let solar = SolarTime::new(solar_noon, latitude.unwrap_or(35.0), day_of_year);
    let sun_times =
        resolve_sun_times_for_local_date(latitude, longitude, timezone_name, sample_date);

    CurveContext::new(sample_hour, solar, sun_times)
}

/// Build a curve context for a specific local datetime.
pub fn curve_context_for_local_datetime(
    solar_noon: f32,
    latitude: Option<f32>,
    longitude: Option<f32>,
    timezone_name: Option<&str>,
    sample_at: NaiveDateTime,
) -> CurveContext {
    let sample_time = sample_at.time();
    let sample_hour = sample_time.hour() as f32
        + sample_time.minute() as f32 / 60.0
        + sample_time.second() as f32 / 3600.0;

    curve_context_for_local_date_and_hour(
        solar_noon,
        latitude,
        longitude,
        timezone_name,
        sample_at.date(),
        sample_hour,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_context_uses_fallback_hours() {
        let ctx = SolarContext::default_context();
        assert_eq!(ctx.sunrise, FALLBACK_SUNRISE_HOUR);
        assert_eq!(ctx.sunset, FALLBACK_SUNSET_HOUR);
        assert_eq!(ctx.solar_noon, 12.0);
        assert_eq!(ctx.solar_midnight, 0.0);
        assert_eq!(ctx.day_length, 14.0);
        assert!(ctx.twilight.is_none());
    }

    #[test]
    fn new_sets_midnight_and_day_length() {
        let ctx = SolarContext::new(5.5, 19.5, 12.5);
        assert_eq!(ctx.solar_midnight, 0.5);
        assert_eq!(ctx.day_length, 14.0);
    }

    #[test]
    fn from_sun_times_preserves_values() {
        let sun_times = SunTimes {
            sunrise: 6.0,
            sunset: 20.0,
            day_length: 14.0,
        };
        let ctx = SolarContext::from_sun_times(&sun_times, 13.0);
        assert_eq!(ctx.sunrise, 6.0);
        assert_eq!(ctx.sunset, 20.0);
        assert_eq!(ctx.solar_noon, 13.0);
        assert_eq!(ctx.solar_midnight, 1.0);
    }
}
