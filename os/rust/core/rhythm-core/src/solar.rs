//! Solar time calculations for adaptive lighting.
//!
//! This module provides solar time calculations used to determine
//! the position in the day cycle for adaptive lighting.
//!
//! Solar noon is calculated astronomically from longitude.
//! Timezone/DST handling is done via chrono-tz.
//!
//! The core types [`SolarTime`] and [`SunTimes`] are defined in
//! [`rhythm_profile`] and re-exported here.

use crate::timezone::{day_of_year, Timezone};
use libm::{cosf, sinf};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

// Re-export core solar types from rhythm-profile
pub use rhythm_profile::solar::{SolarTime, SunTimes};

/// Pi constant for calculations.
const PI: f32 = core::f32::consts::PI;

// ============================================================================
// Astronomical solar noon calculation
// ============================================================================

/// Calculate solar noon in local time for a given location and date.
///
/// Uses the Equation of Time to account for Earth's orbital eccentricity
/// and axial tilt.
///
/// # Arguments
///
/// * `longitude` - Longitude in degrees (negative = west)
/// * `year` - Full year
/// * `month` - Month (1-12)
/// * `day` - Day of month
/// * `tz` - Timezone for local time conversion
///
/// # Returns
///
/// Solar noon as decimal hours in local time (0-24)
pub fn calculate_solar_noon(longitude: f32, year: i32, month: u32, day: u32, tz: &Timezone) -> f32 {
    let doy = day_of_year(year, month, day);

    // Equation of Time (minutes) - accounts for orbital eccentricity and axial tilt
    let eot = equation_of_time(doy);

    // Solar noon in UTC:
    // At longitude 0°, solar noon is 12:00 UTC (minus EoT correction)
    // Each degree of longitude shifts solar noon by 4 minutes (360° / 24h = 15°/h)
    let longitude_correction_minutes = longitude * 4.0; // West longitude = later solar noon

    // Solar noon in UTC (in hours)
    let solar_noon_utc = 12.0 - (eot / 60.0) - (longitude_correction_minutes / 60.0);

    // Convert to local time
    tz.utc_to_local(solar_noon_utc, year, month, day)
}

/// Calculate solar noon using a simple float UTC offset (no timezone lookup).
///
/// This is a lightweight alternative to [`calculate_solar_noon`] for platforms
/// that don't have (or don't need) a full IANA timezone database. The offset-based
/// approach cannot account for DST transitions — pass the correct current offset.
///
/// # Arguments
///
/// * `longitude` - Longitude in degrees (negative = west)
/// * `utc_offset_hours` - UTC offset as float hours (e.g., -5.0 for EST, -4.0 for EDT)
/// * `day_of_year` - Day of year (1-366)
///
/// # Returns
///
/// Solar noon as decimal hours in local time (0-24)
pub fn calculate_solar_noon_from_offset(
    longitude: f32,
    utc_offset_hours: f32,
    day_of_year: u32,
) -> f32 {
    let eot = equation_of_time(day_of_year);
    let lng_correction_min = longitude * 4.0;
    let noon_utc = 12.0 - (eot / 60.0) - (lng_correction_min / 60.0);
    (noon_utc + utc_offset_hours).rem_euclid(24.0)
}

/// Equation of Time - the difference between apparent solar time and mean solar time.
///
/// Returns correction in minutes. Positive = sundial ahead of clock.
///
/// Uses the simplified formula accurate to within ~1 minute.
pub fn equation_of_time(day_of_year: u32) -> f32 {
    // B = 360/365 * (day - 81) in radians
    let b = 2.0 * PI * (day_of_year as f32 - 81.0) / 365.0;

    // EoT in minutes (Spencer's formula, simplified)
    9.87 * sinf(2.0 * b) - 7.53 * cosf(b) - 1.5 * sinf(b)
}

// ============================================================================
// SolarTime construction from location (requires chrono/chrono-tz)
// ============================================================================

/// Create a [`SolarTime`] from geographic coordinates and date.
///
/// This is the preferred constructor when you have geographic coordinates.
/// Solar noon is calculated using the Equation of Time, accounting for
/// Earth's orbital eccentricity and axial tilt.
///
/// # Arguments
///
/// * `latitude` - Latitude in degrees (positive = north)
/// * `longitude` - Longitude in degrees (negative = west)
/// * `year` - Full year (e.g., 2024)
/// * `month` - Month (1-12)
/// * `day` - Day of month (1-31)
/// * `tz` - Timezone for local time conversion
///
/// # Example
///
/// ```
/// use rhythm_core::solar::{solar_time_from_location, SolarTime};
/// use rhythm_core::timezone::Timezone;
///
/// // New York City on June 21, 2024
/// let tz = Timezone::new("America/New_York");
/// let solar = solar_time_from_location(40.7128, -74.0060, 2024, 6, 21, &tz);
/// // Solar noon will be approximately 12:58 PM EDT
/// ```
pub fn solar_time_from_location(
    latitude: f32,
    longitude: f32,
    year: i32,
    month: u32,
    day: u32,
    tz: &Timezone,
) -> SolarTime {
    let solar_noon_hour = calculate_solar_noon(longitude, year, month, day, tz);
    let doy = day_of_year(year, month, day);

    SolarTime {
        solar_noon_hour,
        latitude,
        day_of_year: doy,
    }
}

// ============================================================================
// Sunrise / Sunset calculations (using `sunrise` crate)
// ============================================================================

/// Calculate sunrise and sunset times for a given location and date.
///
/// # Arguments
///
/// * `latitude` - Latitude in degrees (positive = north)
/// * `longitude` - Longitude in degrees (negative = west)
/// * `year` - Full year
/// * `month` - Month (1-12)
/// * `day` - Day of month
/// * `tz` - Timezone for local time conversion
///
/// # Returns
///
/// `SunTimes` with sunrise, sunset, and day length in local time.
///
/// # Example
///
/// ```
/// use rhythm_core::solar::calculate_sun_times;
/// use rhythm_core::timezone::Timezone;
///
/// let tz = Timezone::new("America/New_York");
/// let times = calculate_sun_times(40.7128, -74.006, 2024, 6, 21, &tz);
/// // Summer solstice in NYC: sunrise ~5:25 AM, sunset ~8:31 PM
/// ```
pub fn calculate_sun_times(
    latitude: f32,
    longitude: f32,
    year: i32,
    month: u32,
    day: u32,
    tz: &Timezone,
) -> SunTimes {
    use chrono::NaiveDate;
    use sunrise::{Coordinates, SolarDay, SolarEvent};

    // Get UTC offset for this date (in seconds)
    let utc_offset_hours = tz.utc_offset(year, month, day, 12);
    let utc_offset_seconds = (utc_offset_hours * 3600.0) as i64;

    // Create coordinates and date. Never panic here: lat/lon come from
    // user-supplied, persisted location data, and this runs on every periodic
    // tick — a panic would kill the scheduler thread and permanently stop
    // adaptive lighting. Clamp junk into range instead.
    let lat = if latitude.is_finite() {
        (latitude as f64).clamp(-90.0, 90.0)
    } else {
        0.0
    };
    let lon = if longitude.is_finite() {
        (longitude as f64).clamp(-180.0, 180.0)
    } else {
        0.0
    };
    let coords = Coordinates::new(lat, lon)
        .or_else(|| Coordinates::new(0.0, 0.0))
        .expect("Coordinates::new(0,0) is always valid");
    let date = NaiveDate::from_ymd_opt(year, month, day)
        .or_else(|| NaiveDate::from_ymd_opt(2000, 1, 1))
        .expect("2000-01-01 is a valid date");

    let solar_day = SolarDay::new(coords, date);

    // Get sunrise/sunset as UTC timestamps
    let sunrise_ts = solar_day
        .event_time(SolarEvent::Sunrise)
        .map(|dt| dt.timestamp())
        .unwrap_or(0);
    let sunset_ts = solar_day
        .event_time(SolarEvent::Sunset)
        .map(|dt| dt.timestamp())
        .unwrap_or(0);

    // Convert timestamps to local decimal hours
    // timestamp is seconds since Unix epoch, we need hour of day
    let sunrise_local_ts = sunrise_ts + utc_offset_seconds;
    let sunset_local_ts = sunset_ts + utc_offset_seconds;

    // Extract hour of day from timestamp
    let sunrise = ((sunrise_local_ts % 86400) as f32) / 3600.0;
    let sunset = ((sunset_local_ts % 86400) as f32) / 3600.0;

    // Wrap to 0-24 range (handle negative values)
    let sunrise = ((sunrise % 24.0) + 24.0) % 24.0;
    let sunset = ((sunset % 24.0) + 24.0) % 24.0;

    let day_length = if sunset > sunrise {
        sunset - sunrise
    } else {
        // Handles midnight crossing (rare at extreme latitudes)
        24.0 - sunrise + sunset
    };

    SunTimes {
        sunrise,
        sunset,
        day_length,
    }
}

/// Calculate sunrise time in local decimal hours.
pub fn calculate_sunrise(
    latitude: f32,
    longitude: f32,
    year: i32,
    month: u32,
    day: u32,
    tz: &Timezone,
) -> f32 {
    calculate_sun_times(latitude, longitude, year, month, day, tz).sunrise
}

/// Calculate sunset time in local decimal hours.
pub fn calculate_sunset(
    latitude: f32,
    longitude: f32,
    year: i32,
    month: u32,
    day: u32,
    tz: &Timezone,
) -> f32 {
    calculate_sun_times(latitude, longitude, year, month, day, tz).sunset
}

/// Twilight phase times (dawn or dusk).
///
/// Times are in local decimal hours (0-24).
/// `None` indicates the event doesn't occur (polar regions).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct TwilightPhase {
    /// Civil twilight: sun 6° below horizon
    /// Dawn: sufficient light for outdoor activities
    /// Dusk: sufficient light ends
    pub civil: Option<f32>,
    /// Nautical twilight: sun 12° below horizon
    /// Dawn: horizon becomes visible at sea
    /// Dusk: horizon no longer visible
    pub nautical: Option<f32>,
    /// Astronomical twilight: sun 18° below horizon
    /// Dawn: sky no longer completely dark
    /// Dusk: sky becomes completely dark
    pub astronomical: Option<f32>,
}

/// Full twilight times for a day (dawn and dusk).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct TwilightTimes {
    /// Dawn twilight times (morning, before sunrise)
    pub dawn: TwilightPhase,
    /// Dusk twilight times (evening, after sunset)
    pub dusk: TwilightPhase,
}

/// Calculate twilight times for a given location and date.
///
/// Returns dawn (morning) and dusk (evening) times for:
/// - Civil twilight (6° below horizon)
/// - Nautical twilight (12° below horizon)
/// - Astronomical twilight (18° below horizon)
///
/// Times are returned as `Option<f32>` in local decimal hours (0-24).
/// `None` indicates the twilight event doesn't occur (polar regions).
///
/// # Arguments
///
/// * `latitude` - Latitude in degrees (positive = north)
/// * `longitude` - Longitude in degrees (negative = west)
/// * `year` - Full year
/// * `month` - Month (1-12)
/// * `day` - Day of month
/// * `tz` - Timezone for local time conversion
///
/// # Returns
///
/// `TwilightTimes` with dawn and dusk phases.
///
/// # Example
///
/// ```
/// use rhythm_core::solar::calculate_twilight_times;
/// use rhythm_core::timezone::Timezone;
///
/// let tz = Timezone::new("America/New_York");
/// let twilight = calculate_twilight_times(40.7128, -74.006, 2024, 6, 21, &tz);
/// // Dawn civil twilight ~4:52 AM, before sunrise
/// // Dusk civil twilight ~9:01 PM, after sunset
/// ```
pub fn calculate_twilight_times(
    latitude: f32,
    longitude: f32,
    year: i32,
    month: u32,
    day: u32,
    tz: &Timezone,
) -> TwilightTimes {
    use chrono::NaiveDate;
    use sunrise::{Coordinates, DawnType, SolarDay, SolarEvent};

    // Get UTC offset for this date (in seconds)
    let utc_offset_hours = tz.utc_offset(year, month, day, 12);
    let utc_offset_seconds = (utc_offset_hours * 3600.0) as i64;

    // Create coordinates and date. Never panic here: lat/lon come from
    // user-supplied, persisted location data, and this runs on every periodic
    // tick — a panic would kill the scheduler thread and permanently stop
    // adaptive lighting. Clamp junk into range instead.
    let lat = if latitude.is_finite() {
        (latitude as f64).clamp(-90.0, 90.0)
    } else {
        0.0
    };
    let lon = if longitude.is_finite() {
        (longitude as f64).clamp(-180.0, 180.0)
    } else {
        0.0
    };
    let coords = Coordinates::new(lat, lon)
        .or_else(|| Coordinates::new(0.0, 0.0))
        .expect("Coordinates::new(0,0) is always valid");
    let date = NaiveDate::from_ymd_opt(year, month, day)
        .or_else(|| NaiveDate::from_ymd_opt(2000, 1, 1))
        .expect("2000-01-01 is a valid date");

    let solar_day = SolarDay::new(coords, date);

    // Helper to convert timestamp to local decimal hours
    let ts_to_hours = |ts: i64| -> f32 {
        let local_ts = ts + utc_offset_seconds;
        let hour = ((local_ts % 86400) as f32) / 3600.0;
        ((hour % 24.0) + 24.0) % 24.0
    };

    // Calculate dawn times (before sunrise)
    let dawn_civil = solar_day
        .event_time(SolarEvent::Dawn(DawnType::Civil))
        .map(|dt| ts_to_hours(dt.timestamp()));
    let dawn_nautical = solar_day
        .event_time(SolarEvent::Dawn(DawnType::Nautical))
        .map(|dt| ts_to_hours(dt.timestamp()));
    let dawn_astronomical = solar_day
        .event_time(SolarEvent::Dawn(DawnType::Astronomical))
        .map(|dt| ts_to_hours(dt.timestamp()));

    // Calculate dusk times (after sunset)
    let dusk_civil = solar_day
        .event_time(SolarEvent::Dusk(DawnType::Civil))
        .map(|dt| ts_to_hours(dt.timestamp()));
    let dusk_nautical = solar_day
        .event_time(SolarEvent::Dusk(DawnType::Nautical))
        .map(|dt| ts_to_hours(dt.timestamp()));
    let dusk_astronomical = solar_day
        .event_time(SolarEvent::Dusk(DawnType::Astronomical))
        .map(|dt| ts_to_hours(dt.timestamp()));

    TwilightTimes {
        dawn: TwilightPhase {
            civil: dawn_civil,
            nautical: dawn_nautical,
            astronomical: dawn_astronomical,
        },
        dusk: TwilightPhase {
            civil: dusk_civil,
            nautical: dusk_nautical,
            astronomical: dusk_astronomical,
        },
    }
}

/// Convert hours and minutes to decimal hours.
#[inline]
pub fn hm_to_decimal(hours: u8, minutes: u8) -> f32 {
    hours as f32 + minutes as f32 / 60.0
}

/// Convert decimal hours to hours and minutes.
#[inline]
pub fn decimal_to_hm(decimal: f32) -> (u8, u8) {
    let hours = decimal.floor() as u8;
    let minutes = ((decimal - hours as f32) * 60.0).round() as u8;
    (hours, minutes.min(59))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_solar_time_at_noon() {
        let solar = SolarTime::new(12.0, 35.0, 172);
        let st = solar.get_solar_time(12.0);
        assert!(
            (st - 12.0).abs() < 0.01,
            "At solar noon, expected 12.0, got {}",
            st
        );
    }

    #[test]
    fn test_solar_time_at_midnight() {
        let solar = SolarTime::new(12.0, 35.0, 172);
        let st = solar.get_solar_time(0.0);
        assert!(
            st.abs() < 0.01 || (st - 24.0).abs() < 0.01,
            "At solar midnight, expected ~0 or ~24, got {}",
            st
        );
    }

    #[test]
    fn test_sun_position_extremes() {
        let solar = SolarTime::new(12.0, 35.0, 172);

        let pos_noon = solar.get_sun_position(12.0);
        assert!(
            (pos_noon - 1.0).abs() < 0.01,
            "At solar noon, expected +1, got {}",
            pos_noon
        );

        let pos_midnight = solar.get_sun_position(0.0);
        assert!(
            (pos_midnight - (-1.0)).abs() < 0.01,
            "At solar midnight, expected -1, got {}",
            pos_midnight
        );
    }

    #[test]
    fn test_is_morning() {
        let solar = SolarTime::new(12.0, 35.0, 172);

        assert!(solar.is_morning(6.0), "6am should be morning");
        assert!(solar.is_morning(11.0), "11am should be morning");
        assert!(!solar.is_morning(13.0), "1pm should not be morning");
        assert!(!solar.is_morning(18.0), "6pm should not be morning");
    }

    #[test]
    fn test_offset_solar_noon() {
        let solar = SolarTime::new(12.5, 35.0, 172);

        let pos = solar.get_sun_position(12.5);
        assert!(
            (pos - 1.0).abs() < 0.01,
            "At offset solar noon, expected +1, got {}",
            pos
        );
    }

    #[test]
    fn test_elevation_noon_summer() {
        let solar = SolarTime::new(12.0, 35.0, 172);

        let elev = solar.estimate_elevation(12.0);

        assert!(
            elev > 70.0 && elev < 85.0,
            "Summer noon elevation at 35°N should be ~78°, got {}",
            elev
        );
    }

    #[test]
    fn test_hm_conversion() {
        assert!((hm_to_decimal(12, 30) - 12.5).abs() < 0.01);
        assert!((hm_to_decimal(6, 45) - 6.75).abs() < 0.01);

        let (h, m) = decimal_to_hm(12.5);
        assert_eq!(h, 12);
        assert_eq!(m, 30);

        let (h, m) = decimal_to_hm(6.75);
        assert_eq!(h, 6);
        assert_eq!(m, 45);
    }

    #[test]
    fn test_equation_of_time() {
        let eot_feb = equation_of_time(42);
        assert!(
            eot_feb < -10.0 && eot_feb > -18.0,
            "Feb EoT should be ~-14 min, got {}",
            eot_feb
        );

        let eot_nov = equation_of_time(307);
        assert!(
            eot_nov > 10.0 && eot_nov < 20.0,
            "Nov EoT should be ~+16 min, got {}",
            eot_nov
        );

        let eot_apr = equation_of_time(105);
        assert!(
            eot_apr.abs() < 3.0,
            "Apr EoT should be ~0 min, got {}",
            eot_apr
        );
    }

    #[test]
    fn test_calculate_solar_noon_nyc() {
        use crate::timezone::Timezone;

        let tz = Timezone::new("America/New_York");

        let noon = calculate_solar_noon(-74.006, 2024, 6, 21, &tz);

        assert!(
            noon > 12.8 && noon < 13.2,
            "NYC summer solar noon should be ~12:58, got {:.2}",
            noon
        );
    }

    #[test]
    fn test_calculate_solar_noon_la() {
        use crate::timezone::Timezone;

        let tz = Timezone::new("America/Los_Angeles");

        let noon = calculate_solar_noon(-118.2437, 2024, 6, 21, &tz);

        assert!(
            noon > 12.75 && noon < 13.1,
            "LA summer solar noon should be ~12:53, got {:.2}",
            noon
        );
    }

    #[test]
    fn test_from_location() {
        use crate::timezone::Timezone;

        let tz = Timezone::new("America/New_York");

        let solar = solar_time_from_location(40.7128, -74.006, 2024, 6, 21, &tz);

        assert_eq!(solar.day_of_year, 173);
        assert!((solar.latitude - 40.7128).abs() < 0.01);
        assert!(
            solar.solar_noon_hour > 12.8 && solar.solar_noon_hour < 13.2,
            "Solar noon should be ~12:58, got {:.2}",
            solar.solar_noon_hour
        );
    }

    #[test]
    fn test_dst_shift() {
        use crate::timezone::Timezone;

        let tz = Timezone::new("America/New_York");

        let noon_before = calculate_solar_noon(-74.006, 2024, 3, 1, &tz);
        let noon_after = calculate_solar_noon(-74.006, 2024, 3, 15, &tz);

        let diff = noon_after - noon_before;
        assert!(
            diff > 0.8 && diff < 1.2,
            "DST shift should move solar noon ~1 hour later, got diff of {:.2}",
            diff
        );
    }

    #[test]
    fn test_sun_times_nyc_summer() {
        use crate::timezone::Timezone;

        let tz = Timezone::new("America/New_York");

        let times = calculate_sun_times(40.7128, -74.006, 2024, 6, 21, &tz);

        assert!(
            times.sunrise > 5.2 && times.sunrise < 5.6,
            "NYC summer sunrise should be ~5:25 AM, got {:.2}",
            times.sunrise
        );

        assert!(
            times.sunset > 20.3 && times.sunset < 20.7,
            "NYC summer sunset should be ~8:31 PM, got {:.2}",
            times.sunset
        );

        assert!(
            times.day_length > 14.5 && times.day_length < 15.5,
            "NYC summer day length should be ~15h, got {:.2}",
            times.day_length
        );
    }

    #[test]
    fn test_sun_times_nyc_winter() {
        use crate::timezone::Timezone;

        let tz = Timezone::new("America/New_York");

        let times = calculate_sun_times(40.7128, -74.006, 2024, 12, 21, &tz);

        assert!(
            times.sunrise > 7.0 && times.sunrise < 7.5,
            "NYC winter sunrise should be ~7:16 AM, got {:.2}",
            times.sunrise
        );

        assert!(
            times.sunset > 16.3 && times.sunset < 16.8,
            "NYC winter sunset should be ~4:32 PM, got {:.2}",
            times.sunset
        );

        assert!(
            times.day_length > 9.0 && times.day_length < 9.5,
            "NYC winter day length should be ~9.25h, got {:.2}",
            times.day_length
        );
    }

    #[test]
    fn test_sunrise_sunset_helpers() {
        use crate::timezone::Timezone;

        let tz = Timezone::new("America/New_York");

        let sunrise = calculate_sunrise(40.7128, -74.006, 2024, 6, 21, &tz);
        let sunset = calculate_sunset(40.7128, -74.006, 2024, 6, 21, &tz);

        assert!(sunrise > 5.0 && sunrise < 6.0);
        assert!(sunset > 20.0 && sunset < 21.0);
        assert!(sunset > sunrise);
    }

    // ========================================================================
    // Twilight tests
    // ========================================================================

    #[test]
    fn test_twilight_times_nyc_summer() {
        use crate::timezone::Timezone;

        let tz = Timezone::new("America/New_York");

        let twilight = calculate_twilight_times(40.7128, -74.006, 2024, 6, 21, &tz);
        let sun_times = calculate_sun_times(40.7128, -74.006, 2024, 6, 21, &tz);

        assert!(
            twilight.dawn.civil.is_some(),
            "Dawn civil twilight should be present"
        );
        assert!(
            twilight.dawn.nautical.is_some(),
            "Dawn nautical twilight should be present"
        );
        assert!(
            twilight.dawn.astronomical.is_some(),
            "Dawn astronomical twilight should be present"
        );
        assert!(
            twilight.dusk.civil.is_some(),
            "Dusk civil twilight should be present"
        );
        assert!(
            twilight.dusk.nautical.is_some(),
            "Dusk nautical twilight should be present"
        );
        assert!(
            twilight.dusk.astronomical.is_some(),
            "Dusk astronomical twilight should be present"
        );

        let dawn_astro = twilight.dawn.astronomical.unwrap();
        let dawn_naut = twilight.dawn.nautical.unwrap();
        let dawn_civil = twilight.dawn.civil.unwrap();
        assert!(
            dawn_astro < dawn_naut,
            "Dawn astronomical ({:.2}) should be before nautical ({:.2})",
            dawn_astro,
            dawn_naut
        );
        assert!(
            dawn_naut < dawn_civil,
            "Dawn nautical ({:.2}) should be before civil ({:.2})",
            dawn_naut,
            dawn_civil
        );
        assert!(
            dawn_civil < sun_times.sunrise,
            "Dawn civil ({:.2}) should be before sunrise ({:.2})",
            dawn_civil,
            sun_times.sunrise
        );

        let dusk_civil = twilight.dusk.civil.unwrap();
        let dusk_naut = twilight.dusk.nautical.unwrap();
        let dusk_astro = twilight.dusk.astronomical.unwrap();
        assert!(
            sun_times.sunset < dusk_civil,
            "Sunset ({:.2}) should be before dusk civil ({:.2})",
            sun_times.sunset,
            dusk_civil
        );
        assert!(
            dusk_civil < dusk_naut,
            "Dusk civil ({:.2}) should be before nautical ({:.2})",
            dusk_civil,
            dusk_naut
        );
        assert!(
            dusk_naut < dusk_astro,
            "Dusk nautical ({:.2}) should be before astronomical ({:.2})",
            dusk_naut,
            dusk_astro
        );
    }

    #[test]
    fn test_twilight_times_polar_summer() {
        use crate::timezone::Timezone;

        let tz = Timezone::new("Europe/Oslo");

        let twilight = calculate_twilight_times(69.6492, 18.9553, 2024, 6, 21, &tz);

        if let Some(dawn_civil) = twilight.dawn.civil {
            assert!(
                (0.0..24.0).contains(&dawn_civil),
                "Dawn civil twilight should be valid hour, got {}",
                dawn_civil
            );
        }
        if let Some(dusk_civil) = twilight.dusk.civil {
            assert!(
                (0.0..24.0).contains(&dusk_civil),
                "Dusk civil twilight should be valid hour, got {}",
                dusk_civil
            );
        }
    }

    #[test]
    fn test_twilight_default() {
        let phase = TwilightPhase::default();
        assert!(phase.civil.is_none());
        assert!(phase.nautical.is_none());
        assert!(phase.astronomical.is_none());

        let times = TwilightTimes::default();
        assert!(times.dawn.civil.is_none());
        assert!(times.dusk.civil.is_none());
    }

    // ========================================================================
    // Edge-case regression tests
    // ========================================================================

    #[test]
    fn solar_noon_is_finite_at_arctic_circle_winter() {
        // 70°N on the winter solstice — above the Arctic circle the sun may
        // not rise, but solar noon itself is always a well-defined clock
        // time. The calculation must return a finite value (no NaN / inf).
        let tz = Timezone::new("Europe/Oslo");
        let noon = calculate_solar_noon(15.0, 2026, 12, 21, &tz);
        assert!(
            noon.is_finite(),
            "polar-winter solar noon must be finite, got {}",
            noon
        );
        assert!(
            (0.0..=24.0).contains(&noon),
            "solar noon must be within 0..24h, got {}",
            noon
        );
    }

    #[test]
    fn solar_noon_is_finite_at_antarctic_summer() {
        // 70°S on the summer solstice (for southern hemisphere).
        let tz = Timezone::new("Antarctica/McMurdo");
        let noon = calculate_solar_noon(166.6, 2026, 12, 21, &tz);
        assert!(noon.is_finite());
        assert!((0.0..=24.0).contains(&noon));
    }

    #[test]
    fn solar_noon_across_dst_spring_forward_matches_standard_day() {
        // On the DST transition day (US spring-forward), solar noon should
        // jump by ~1 hour vs. the day before because local clock time
        // advances but the sun doesn't. No NaN, no negatives.
        let tz = Timezone::new("America/New_York");
        let noon_before = calculate_solar_noon(-74.0, 2026, 3, 7, &tz); // Sat before DST
        let noon_after = calculate_solar_noon(-74.0, 2026, 3, 8, &tz); // DST Sunday
        assert!(noon_before.is_finite() && noon_after.is_finite());
        // After DST, wall-clock solar noon is ~1h later than before.
        let delta = noon_after - noon_before;
        assert!(
            (0.5..=1.5).contains(&delta),
            "DST spring-forward should shift solar noon by ~1h, got {}",
            delta
        );
    }

    #[test]
    fn solar_noon_across_dst_fall_back_matches_standard_day() {
        let tz = Timezone::new("America/New_York");
        let noon_before = calculate_solar_noon(-74.0, 2026, 10, 31, &tz); // Sat before DST end
        let noon_after = calculate_solar_noon(-74.0, 2026, 11, 1, &tz); // DST end Sunday
        assert!(noon_before.is_finite() && noon_after.is_finite());
        let delta = noon_before - noon_after;
        assert!(
            (0.5..=1.5).contains(&delta),
            "DST fall-back should shift solar noon by ~-1h, got {}",
            delta
        );
    }

    #[test]
    fn solar_noon_offset_variant_is_finite_at_polar_latitudes() {
        // The offset-based (timezone-free) variant must also stay finite when
        // pointed at a polar longitude.
        let day_of_year = 355u32;
        let noon = calculate_solar_noon_from_offset(15.0, 1.0, day_of_year);
        assert!(noon.is_finite());
    }

    #[test]
    fn sun_times_do_not_return_nan_at_equator_on_equinox() {
        // Equator on equinox is the canonical easy case. Sanity check that
        // neither sunrise, sunset, nor day_length produce NaN.
        let tz = Timezone::new("Pacific/Galapagos");
        let times = calculate_sun_times(0.0, -90.0, 2026, 3, 20, &tz);
        assert!(times.sunrise.is_finite());
        assert!(times.sunset.is_finite());
        assert!(times.day_length.is_finite());
        assert!(
            times.day_length >= 0.0 && times.day_length <= 24.0,
            "day_length must be sane, got {}",
            times.day_length
        );
    }

    #[test]
    fn sun_times_polar_night_returns_finite_numbers_or_zero_length() {
        // 72°N on winter solstice — polar night. Sunrise/sunset may not
        // exist; the calculation must not crash or return NaN.
        let tz = Timezone::new("Europe/Oslo");
        let times = calculate_sun_times(72.0, 15.0, 2026, 12, 21, &tz);
        assert!(
            times.sunrise.is_finite() || times.day_length == 0.0,
            "polar night must yield either finite sunrise or zero day_length"
        );
        assert!(
            !times.day_length.is_nan(),
            "day_length must never be NaN, got {}",
            times.day_length
        );
    }

    #[test]
    fn solar_noon_consistent_across_year_bounds() {
        // Dec 31 → Jan 1 transition must not produce a discontinuity. Both
        // should be finite and differ by less than ~1 minute (same
        // longitude, consecutive days).
        let tz = Timezone::new("America/New_York");
        let dec31 = calculate_solar_noon(-74.0, 2026, 12, 31, &tz);
        let jan01 = calculate_solar_noon(-74.0, 2027, 1, 1, &tz);
        assert!(dec31.is_finite() && jan01.is_finite());
        let delta = (dec31 - jan01).abs();
        assert!(
            delta < 0.05,
            "year boundary should not cause a solar-noon jump, got delta {}",
            delta
        );
    }
}
