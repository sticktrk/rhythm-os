//! Timezone handling using chrono-tz.
//!
//! Thin wrapper around chrono-tz for IANA timezone support with DST handling.

use chrono::{
    DateTime, Datelike, FixedOffset, NaiveDate, NaiveDateTime, Offset, TimeZone, Timelike, Utc,
};
use chrono_tz::Tz;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Default timezone (US Eastern).
pub const DEFAULT_TIMEZONE: &str = "America/New_York";

/// Timezone configuration wrapper.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Timezone {
    /// IANA timezone name (e.g., "America/New_York")
    name: String,

    /// Parsed chrono-tz timezone
    #[cfg_attr(feature = "serde", serde(skip))]
    tz: Option<Tz>,
}

impl Timezone {
    /// Create a new timezone from an IANA name.
    ///
    /// Falls back to America/New_York if the name is invalid.
    pub fn new(name: &str) -> Self {
        let tz = name.parse::<Tz>().ok();
        Self {
            name: if tz.is_some() {
                name.to_string()
            } else {
                DEFAULT_TIMEZONE.to_string()
            },
            tz: tz.or_else(|| DEFAULT_TIMEZONE.parse().ok()),
        }
    }

    /// Get the timezone name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the chrono-tz timezone.
    fn tz(&self) -> Tz {
        self.tz.unwrap_or_else(|| DEFAULT_TIMEZONE.parse().unwrap())
    }

    /// Check if DST is active for the given date.
    pub fn is_dst_active(&self, year: i32, month: u32, day: u32, hour: u32) -> bool {
        let tz = self.tz();

        // Create a datetime in this timezone
        if let Some(dt) = NaiveDate::from_ymd_opt(year, month, day)
            .and_then(|d| d.and_hms_opt(hour, 0, 0))
            .and_then(|ndt| tz.from_local_datetime(&ndt).single())
        {
            // Check if the offset differs from standard time
            // by comparing to a known non-DST date (January 1)
            if let Some(jan1) = NaiveDate::from_ymd_opt(year, 1, 1)
                .and_then(|d| d.and_hms_opt(12, 0, 0))
                .and_then(|ndt| tz.from_local_datetime(&ndt).single())
            {
                return dt.offset().fix() != jan1.offset().fix();
            }
        }
        false
    }

    /// Get the current UTC offset in hours.
    pub fn utc_offset(&self, year: i32, month: u32, day: u32, hour: u32) -> f32 {
        let tz = self.tz();

        if let Some(dt) = NaiveDate::from_ymd_opt(year, month, day)
            .and_then(|d| d.and_hms_opt(hour, 0, 0))
            .and_then(|ndt| tz.from_local_datetime(&ndt).single())
        {
            dt.offset().fix().local_minus_utc() as f32 / 3600.0
        } else {
            // Fallback: use current offset
            -5.0 // EST default
        }
    }

    /// Convert UTC hour to local hour for a given date.
    pub fn utc_to_local(&self, utc_hour: f32, year: i32, month: u32, day: u32) -> f32 {
        let offset = self.utc_offset(year, month, day, 12); // Use noon as reference
        let local = utc_hour + offset;
        ((local % 24.0) + 24.0) % 24.0
    }

    /// Convert local hour to UTC hour for a given date.
    pub fn local_to_utc(&self, local_hour: f32, year: i32, month: u32, day: u32) -> f32 {
        let offset = self.utc_offset(year, month, day, local_hour as u32);
        let utc = local_hour - offset;
        ((utc % 24.0) + 24.0) % 24.0
    }

    /// Convert a UTC naive datetime into the corresponding local naive datetime.
    pub fn local_datetime_from_utc(&self, utc: NaiveDateTime) -> NaiveDateTime {
        let utc_dt = chrono::DateTime::<Utc>::from_naive_utc_and_offset(utc, Utc);
        utc_dt.with_timezone(&self.tz()).naive_local()
    }

    /// Convert a UTC naive datetime into the corresponding local datetime with
    /// its explicit UTC offset preserved.
    pub fn local_datetime_with_offset_from_utc(&self, utc: NaiveDateTime) -> DateTime<FixedOffset> {
        let utc_dt = chrono::DateTime::<Utc>::from_naive_utc_and_offset(utc, Utc);
        utc_dt.with_timezone(&self.tz()).fixed_offset()
    }

    /// Convert a local naive datetime into UTC when the local time is unambiguous.
    pub fn utc_datetime_from_local(&self, local: NaiveDateTime) -> Option<NaiveDateTime> {
        self.tz()
            .from_local_datetime(&local)
            .single()
            .map(|dt| dt.with_timezone(&Utc).naive_utc())
    }

    /// Get local date components for a UTC naive datetime.
    pub fn local_date_from_utc(&self, utc: NaiveDateTime) -> (i32, u32, u32) {
        let local = self.local_datetime_from_utc(utc);
        (
            local.date().year(),
            local.date().month(),
            local.date().day(),
        )
    }

    /// Get local date and hour components for a UTC naive datetime.
    pub fn local_date_hour_from_utc(&self, utc: NaiveDateTime) -> (i32, u32, u32, u32) {
        let local = self.local_datetime_from_utc(utc);
        (
            local.date().year(),
            local.date().month(),
            local.date().day(),
            local.time().hour(),
        )
    }
}

impl Default for Timezone {
    fn default() -> Self {
        Self::new(DEFAULT_TIMEZONE)
    }
}

/// Look up a timezone by name.
///
/// Supports IANA names (e.g., "America/New_York") and common aliases.
pub fn lookup_timezone(name: &str) -> Option<Timezone> {
    // Try direct parse first
    if name.parse::<Tz>().is_ok() {
        return Some(Timezone::new(name));
    }

    // Handle common aliases
    let iana_name = match name.to_uppercase().as_str() {
        "EST" | "EDT" | "EASTERN" => "America/New_York",
        "CST" | "CDT" | "CENTRAL" => "America/Chicago",
        "MST" | "MDT" | "MOUNTAIN" => "America/Denver",
        "PST" | "PDT" | "PACIFIC" => "America/Los_Angeles",
        "GMT" | "UTC" => "UTC",
        _ => return None,
    };

    Some(Timezone::new(iana_name))
}

/// Get the default timezone (America/New_York).
pub fn default_timezone() -> Timezone {
    Timezone::default()
}

/// Calculate day of year (1-366).
pub fn day_of_year(year: i32, month: u32, day: u32) -> u32 {
    NaiveDate::from_ymd_opt(year, month, day)
        .map(|d| d.ordinal())
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timezone_new() {
        let tz = Timezone::new("America/New_York");
        assert_eq!(tz.name(), "America/New_York");

        // Invalid timezone falls back to default
        let tz = Timezone::new("Invalid/Timezone");
        assert_eq!(tz.name(), DEFAULT_TIMEZONE);
    }

    #[test]
    fn test_lookup_timezone() {
        assert!(lookup_timezone("America/New_York").is_some());
        assert!(lookup_timezone("EST").is_some());
        assert!(lookup_timezone("America/Chicago").is_some());
        assert!(lookup_timezone("Invalid").is_none());
    }

    #[test]
    fn test_utc_offset_est() {
        let tz = Timezone::new("America/New_York");

        // Winter (EST = UTC-5)
        let offset = tz.utc_offset(2024, 1, 15, 12);
        assert_eq!(offset, -5.0);

        // Summer (EDT = UTC-4)
        let offset = tz.utc_offset(2024, 7, 15, 12);
        assert_eq!(offset, -4.0);
    }

    #[test]
    fn test_is_dst_active() {
        let tz = Timezone::new("America/New_York");

        // Winter - no DST
        assert!(!tz.is_dst_active(2024, 1, 15, 12));

        // Summer - DST active
        assert!(tz.is_dst_active(2024, 7, 15, 12));
    }

    #[test]
    fn test_day_of_year() {
        assert_eq!(day_of_year(2024, 1, 1), 1);
        assert_eq!(day_of_year(2024, 12, 31), 366); // Leap year
        assert_eq!(day_of_year(2023, 12, 31), 365);
    }

    #[test]
    fn test_utc_to_local() {
        let tz = Timezone::new("America/New_York");

        // Winter: UTC 17:00 = EST 12:00
        let local = tz.utc_to_local(17.0, 2024, 1, 15);
        assert!((local - 12.0).abs() < 0.1);

        // Summer: UTC 16:00 = EDT 12:00
        let local = tz.utc_to_local(16.0, 2024, 7, 15);
        assert!((local - 12.0).abs() < 0.1);
    }

    #[test]
    fn test_local_date_from_utc_uses_local_calendar_day() {
        let tz = Timezone::new("America/New_York");
        let utc = NaiveDate::from_ymd_opt(2026, 4, 9)
            .unwrap()
            .and_hms_opt(0, 40, 0)
            .unwrap();

        let (year, month, day, hour) = tz.local_date_hour_from_utc(utc);

        assert_eq!((year, month, day, hour), (2026, 4, 8, 20));
    }

    #[test]
    fn test_local_datetime_with_offset_from_utc_preserves_dst_offset() {
        let tz = Timezone::new("America/New_York");
        let utc = NaiveDate::from_ymd_opt(2026, 4, 23)
            .unwrap()
            .and_hms_opt(19, 32, 47)
            .unwrap();

        let local = tz.local_datetime_with_offset_from_utc(utc);

        assert_eq!(
            local.format("%Y-%m-%dT%H:%M:%S%:z").to_string(),
            "2026-04-23T15:32:47-04:00"
        );
    }

    /// Verify that utc_offset() changes across the March DST boundary.
    ///
    /// In 2025, clocks spring forward on March 9 at 2:00 AM EST → 3:00 AM EDT.
    /// March 8 should be EST (-5), March 10 should be EDT (-4).
    #[test]
    fn test_dst_spring_forward_boundary_2025() {
        let tz = Timezone::new("America/New_York");

        // March 8, 2025 — still EST
        let before = tz.utc_offset(2025, 3, 8, 12);
        assert_eq!(before, -5.0, "March 8 should be EST (-5)");

        // March 10, 2025 — now EDT
        let after = tz.utc_offset(2025, 3, 10, 12);
        assert_eq!(after, -4.0, "March 10 should be EDT (-4)");

        // The offset differs by exactly 1 hour
        assert_eq!(
            after - before,
            1.0,
            "DST spring forward should shift by +1 hour"
        );
    }

    /// Verify that utc_offset() changes across the November DST boundary.
    ///
    /// In 2025, clocks fall back on November 2 at 2:00 AM EDT → 1:00 AM EST.
    #[test]
    fn test_dst_fall_back_boundary_2025() {
        let tz = Timezone::new("America/New_York");

        // November 1, 2025 — still EDT
        let before = tz.utc_offset(2025, 11, 1, 12);
        assert_eq!(before, -4.0, "November 1 should be EDT (-4)");

        // November 3, 2025 — now EST
        let after = tz.utc_offset(2025, 11, 3, 12);
        assert_eq!(after, -5.0, "November 3 should be EST (-5)");

        assert_eq!(before - after, 1.0, "DST fall back should shift by -1 hour");
    }

    /// Verify solar noon shifts by ~1 hour across DST boundary.
    #[test]
    fn test_solar_noon_shifts_with_dst() {
        let tz = Timezone::new("America/New_York");
        let lon = -74.006; // New York City

        // Winter solar noon (EST)
        let winter_noon = crate::calculate_solar_noon(lon, 2025, 1, 15, &tz);
        // Summer solar noon (EDT)
        let summer_noon = crate::calculate_solar_noon(lon, 2025, 7, 15, &tz);

        // Both should be near ~12:00-13:00 local time
        assert!(
            winter_noon > 11.5 && winter_noon < 13.5,
            "Winter solar noon should be near 12:00-13:00, got {:.2}",
            winter_noon
        );
        assert!(
            summer_noon > 11.5 && summer_noon < 13.5,
            "Summer solar noon should be near 12:00-13:00, got {:.2}",
            summer_noon
        );

        // The difference should be roughly 1 hour (DST shift) ± equation-of-time variation
        let diff = (summer_noon - winter_noon).abs();
        assert!(
            diff < 1.5,
            "Solar noon difference between winter and summer should be < 1.5h, got {:.2}",
            diff
        );
    }
}
