//! Solar time types for adaptive lighting.
//!
//! This module provides the solar time structs used as input to curve
//! calculations. The astronomical calculation functions (solar noon,
//! sunrise/sunset) that depend on chrono/chrono-tz live in rhythm-core.

use libm::cosf;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Pi constant for calculations.
const PI: f32 = core::f32::consts::PI;

/// Sunrise and sunset times for a location and date.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct SunTimes {
    /// Sunrise time in local decimal hours (0-24)
    pub sunrise: f32,
    /// Sunset time in local decimal hours (0-24)
    pub sunset: f32,
    /// Day length in hours
    pub day_length: f32,
}

/// Solar time state for a location.
///
/// Contains pre-calculated solar reference points that are used
/// to determine the current position in the solar day cycle.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct SolarTime {
    /// Solar noon time in hours (0-24, local time)
    pub solar_noon_hour: f32,

    /// Latitude in degrees (for sun elevation calculations)
    pub latitude: f32,

    /// Day of year (1-366) for seasonal adjustments
    pub day_of_year: u32,
}

impl SolarTime {
    /// Create a new SolarTime with the given solar noon hour.
    ///
    /// # Arguments
    ///
    /// * `solar_noon_hour` - Hour of solar noon in local time (0-24)
    /// * `latitude` - Latitude in degrees
    /// * `day_of_year` - Day of year (1-366)
    pub fn new(solar_noon_hour: f32, latitude: f32, day_of_year: u32) -> Self {
        Self {
            solar_noon_hour,
            latitude,
            day_of_year,
        }
    }

    /// Create SolarTime with default values (noon at 12:00).
    ///
    /// Useful for testing or when actual solar data isn't available.
    pub fn default_noon() -> Self {
        Self {
            solar_noon_hour: 12.0,
            latitude: 35.0,
            day_of_year: 172, // Summer solstice (June 21)
        }
    }

    /// Get solar midnight hour (12 hours before/after solar noon).
    pub fn solar_midnight_hour(&self) -> f32 {
        (self.solar_noon_hour + 12.0) % 24.0
    }

    /// Calculate the current solar time (0-24 scale where 12 = solar noon).
    ///
    /// # Arguments
    ///
    /// * `current_hour` - Current time in hours (0-24, local time)
    ///
    /// # Returns
    ///
    /// Solar time on a 0-24 scale where:
    /// - 0 = solar midnight
    /// - 6 = roughly "solar morning"
    /// - 12 = solar noon
    /// - 18 = roughly "solar evening"
    pub fn get_solar_time(&self, current_hour: f32) -> f32 {
        // Calculate hours from solar midnight
        let solar_midnight = self.solar_midnight_hour();

        // Calculate difference from solar midnight
        let mut diff = current_hour - solar_midnight;

        // Wrap to 0-24 range
        while diff < 0.0 {
            diff += 24.0;
        }
        while diff >= 24.0 {
            diff -= 24.0;
        }

        diff
    }

    /// Calculate sun position using time-based cosine wave.
    ///
    /// Returns a value from -1 (solar midnight) to +1 (solar noon).
    ///
    /// # Arguments
    ///
    /// * `current_hour` - Current time in hours (0-24, local time)
    pub fn get_sun_position(&self, current_hour: f32) -> f32 {
        let solar_time = self.get_solar_time(current_hour);

        // -cos(2π * solar_hour / 24) gives:
        // - solar midnight (0h): -1
        // - 6h: 0
        // - solar noon (12h): +1
        // - 18h: 0
        -cosf(2.0 * PI * solar_time / 24.0)
    }

    /// Check if the current solar time is in the morning half (before solar noon).
    pub fn is_morning(&self, current_hour: f32) -> bool {
        self.get_solar_time(current_hour) < 12.0
    }

    /// Estimate sun elevation angle based on time and latitude.
    ///
    /// This is a simplified calculation suitable for adaptive lighting.
    /// For precise astronomical calculations, use a dedicated library.
    ///
    /// # Arguments
    ///
    /// * `current_hour` - Current time in hours (0-24, local time)
    ///
    /// # Returns
    ///
    /// Approximate sun elevation in degrees (-90 to +90).
    pub fn estimate_elevation(&self, current_hour: f32) -> f32 {
        use libm::sinf;

        let solar_time = self.get_solar_time(current_hour);

        // Calculate declination angle (simplified)
        // Declination varies from -23.44° to +23.44° throughout the year
        let declination = 23.44 * sinf(2.0 * PI * (self.day_of_year as f32 - 81.0) / 365.0);

        // Hour angle: 0° at solar noon, 15° per hour
        let hour_angle = (solar_time - 12.0) * 15.0;

        // Convert to radians
        let lat_rad = self.latitude * PI / 180.0;
        let dec_rad = declination * PI / 180.0;
        let ha_rad = hour_angle * PI / 180.0;

        // Calculate elevation (simplified solar position equation)
        let sin_elevation =
            sinf(lat_rad) * sinf(dec_rad) + cosf(lat_rad) * cosf(dec_rad) * cosf(ha_rad);

        // Convert back to degrees
        libm::asinf(sin_elevation.clamp(-1.0, 1.0)) * 180.0 / PI
    }
}

impl Default for SolarTime {
    fn default() -> Self {
        Self::default_noon()
    }
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
    fn test_elevation_midnight() {
        let solar = SolarTime::new(12.0, 35.0, 172);
        let elev = solar.estimate_elevation(0.0);
        assert!(
            elev < 0.0,
            "Midnight elevation should be negative, got {}",
            elev
        );
    }

    #[test]
    fn test_elevation_winter_noon() {
        let solar = SolarTime::new(12.0, 35.0, 355); // Winter solstice
        let elev = solar.estimate_elevation(12.0);
        // At 35°N winter solstice, noon elevation ~31.5°
        assert!(
            elev > 25.0 && elev < 40.0,
            "Winter noon elevation at 35°N should be ~31°, got {}",
            elev
        );
    }

    #[test]
    fn test_solar_midnight_hour() {
        let solar = SolarTime::new(12.0, 35.0, 172);
        assert!((solar.solar_midnight_hour() - 0.0).abs() < 0.01);

        let solar = SolarTime::new(13.0, 35.0, 172);
        assert!((solar.solar_midnight_hour() - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_solar_time_symmetry() {
        let solar = SolarTime::new(12.0, 35.0, 172);
        // Solar times equidistant from noon should give mirrored sun positions
        let pos_morning = solar.get_sun_position(9.0);
        let pos_evening = solar.get_sun_position(15.0);
        assert!(
            (pos_morning - pos_evening).abs() < 0.01,
            "Symmetric hours should have equal positions: morning={}, evening={}",
            pos_morning,
            pos_evening
        );
    }

    #[test]
    fn test_default_noon() {
        let solar = SolarTime::default_noon();
        assert_eq!(solar.solar_noon_hour, 12.0);
        assert_eq!(solar.latitude, 35.0);
        assert_eq!(solar.day_of_year, 172);
    }

    #[test]
    fn test_default_equals_default_noon() {
        let a = SolarTime::default();
        let b = SolarTime::default_noon();
        assert_eq!(a, b);
    }

    #[test]
    fn test_sun_times_struct() {
        let times = SunTimes {
            sunrise: 6.5,
            sunset: 20.0,
            day_length: 13.5,
        };
        assert_eq!(times.sunrise, 6.5);
        assert_eq!(times.sunset, 20.0);
        assert_eq!(times.day_length, 13.5);
    }

    #[test]
    fn test_sun_times_copy() {
        let times = SunTimes {
            sunrise: 6.0,
            sunset: 18.0,
            day_length: 12.0,
        };
        let copy = times;
        assert_eq!(times, copy);
    }

    #[test]
    fn test_solar_time_get_solar_time_range() {
        let solar = SolarTime::new(12.0, 35.0, 172);
        // Solar time should always be in 0-24 range
        for h in 0..24 {
            let st = solar.get_solar_time(h as f32);
            assert!(
                (0.0..24.0).contains(&st),
                "Solar time out of range at hour {}: {}",
                h,
                st
            );
        }
    }

    #[test]
    fn test_sun_position_range() {
        let solar = SolarTime::new(12.0, 35.0, 172);
        for h in 0..24 {
            let pos = solar.get_sun_position(h as f32);
            assert!(
                (-1.01..=1.01).contains(&pos),
                "Sun position out of range at hour {}: {}",
                h,
                pos
            );
        }
    }

    #[test]
    fn test_is_morning_at_noon_boundary() {
        let solar = SolarTime::new(12.0, 35.0, 172);
        // Exactly at solar noon (solar_time = 12.0), not morning
        assert!(!solar.is_morning(12.0));
    }
}
