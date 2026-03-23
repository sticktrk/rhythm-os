//! Time provider trait for platform-agnostic time access.
//!
//! This module defines the `TimeProvider` trait which abstracts how the runtime
//! obtains the current time. Different platforms can implement this differently:
//! - Tokio-based systems can use chrono::Local
//! - ESP32 can use NTP-synced system time
//! - Test environments can use mock time

/// Trait for providing current time information.
///
/// Implementations should provide local time (not UTC) for correct
/// adaptive lighting calculations.
pub trait TimeProvider: Send + Sync {
    /// Get the current hour as a float (0.0 - 24.0).
    ///
    /// For example, 14:30 would be returned as 14.5.
    fn current_hour(&self) -> f32;

    /// Get the current day of the year (1-366).
    fn day_of_year(&self) -> u32;

    /// Get the current year.
    fn year(&self) -> i32;
}

/// A mock time provider for testing.
#[derive(Clone)]
pub struct MockTimeProvider {
    hour: f32,
    day_of_year: u32,
    year: i32,
}

impl MockTimeProvider {
    /// Create a new mock time provider.
    pub fn new(hour: f32, day_of_year: u32, year: i32) -> Self {
        Self {
            hour,
            day_of_year,
            year,
        }
    }

    /// Set the current hour.
    pub fn set_hour(&mut self, hour: f32) {
        self.hour = hour;
    }

    /// Set the day of year.
    pub fn set_day_of_year(&mut self, day: u32) {
        self.day_of_year = day;
    }
}

impl Default for MockTimeProvider {
    fn default() -> Self {
        Self {
            hour: 12.0,
            day_of_year: 172, // Summer solstice
            year: 2024,
        }
    }
}

impl TimeProvider for MockTimeProvider {
    fn current_hour(&self) -> f32 {
        self.hour
    }

    fn day_of_year(&self) -> u32 {
        self.day_of_year
    }

    fn year(&self) -> i32 {
        self.year
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_time_provider() {
        let provider = MockTimeProvider::new(14.5, 180, 2024);
        assert_eq!(provider.current_hour(), 14.5);
        assert_eq!(provider.day_of_year(), 180);
        assert_eq!(provider.year(), 2024);
    }

    #[test]
    fn test_mock_time_provider_set_hour() {
        let mut provider = MockTimeProvider::default();
        provider.set_hour(6.0);
        assert_eq!(provider.current_hour(), 6.0);
    }
}
