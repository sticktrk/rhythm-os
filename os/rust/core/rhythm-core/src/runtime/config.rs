//! Runtime configuration.
//!
//! This module defines the configuration options for the runtime,
//! including update intervals and solar midnight settings.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Default update interval in seconds.
pub const DEFAULT_UPDATE_INTERVAL_SECS: u64 = 60;

/// Runtime configuration.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct RuntimeConfig {
    /// Interval between periodic updates in seconds.
    /// Default: 60 seconds
    pub update_interval_secs: u64,

    /// Whether to reset offsets at solar midnight.
    /// Default: true
    pub solar_midnight_reset: bool,

    /// Solar noon hour (0-24) for solar midnight calculation.
    /// Default: 12.5 (12:30)
    pub solar_noon_hour: f32,

    /// Latitude for solar calculations.
    /// Optional, used for sunrise/sunset times.
    pub latitude: Option<f64>,

    /// Longitude for solar calculations.
    /// Optional, used for sunrise/sunset times.
    pub longitude: Option<f64>,

    /// UTC offset in hours (e.g., -5.0 for EST).
    /// Used when timezone lookup isn't available.
    pub utc_offset_hours: Option<f32>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            update_interval_secs: DEFAULT_UPDATE_INTERVAL_SECS,
            solar_midnight_reset: true,
            solar_noon_hour: 12.5,
            latitude: None,
            longitude: None,
            utc_offset_hours: None,
        }
    }
}

impl RuntimeConfig {
    /// Create a new runtime config with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the update interval.
    pub fn with_update_interval(mut self, secs: u64) -> Self {
        self.update_interval_secs = secs;
        self
    }

    /// Set whether to reset at solar midnight.
    pub fn with_solar_midnight_reset(mut self, enabled: bool) -> Self {
        self.solar_midnight_reset = enabled;
        self
    }

    /// Set the solar noon hour.
    pub fn with_solar_noon(mut self, hour: f32) -> Self {
        self.solar_noon_hour = hour;
        self
    }

    /// Set the location for solar calculations.
    pub fn with_location(mut self, latitude: f64, longitude: f64) -> Self {
        self.latitude = Some(latitude);
        self.longitude = Some(longitude);
        self
    }

    /// Set the UTC offset.
    pub fn with_utc_offset(mut self, hours: f32) -> Self {
        self.utc_offset_hours = Some(hours);
        self
    }

    /// Calculate solar midnight hour from solar noon.
    pub fn solar_midnight_hour(&self) -> f32 {
        (self.solar_noon_hour + 12.0) % 24.0
    }
}

/// Room configuration for multi-room support.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct RoomConfig {
    /// Unique room identifier.
    pub id: String,

    /// Human-readable room name.
    pub name: String,

    /// Entity IDs for lights in this room.
    pub entity_ids: Vec<String>,
}

impl RoomConfig {
    /// Create a new room config.
    pub fn new(id: impl Into<String>, name: impl Into<String>, entity_ids: Vec<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            entity_ids,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = RuntimeConfig::default();
        assert_eq!(config.update_interval_secs, 60);
        assert!(config.solar_midnight_reset);
        assert_eq!(config.solar_noon_hour, 12.5);
    }

    #[test]
    fn test_solar_midnight_calculation() {
        let config = RuntimeConfig::default().with_solar_noon(13.0);
        assert_eq!(config.solar_midnight_hour(), 1.0); // 13:00 + 12 = 25:00 = 1:00

        let config2 = RuntimeConfig::default().with_solar_noon(11.0);
        assert_eq!(config2.solar_midnight_hour(), 23.0); // 11:00 + 12 = 23:00
    }

    #[test]
    fn test_builder_pattern() {
        let config = RuntimeConfig::new()
            .with_update_interval(120)
            .with_solar_midnight_reset(false)
            .with_location(35.0, -78.0)
            .with_utc_offset(-5.0);

        assert_eq!(config.update_interval_secs, 120);
        assert!(!config.solar_midnight_reset);
        assert_eq!(config.latitude, Some(35.0));
        assert_eq!(config.longitude, Some(-78.0));
        assert_eq!(config.utc_offset_hours, Some(-5.0));
    }
}
