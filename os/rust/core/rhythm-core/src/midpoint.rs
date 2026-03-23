//! Dynamic midpoint values for adaptive lighting curves.
//!
//! This module provides the `MidpointValue` type which can represent either
//! a fixed hour value or a dynamic value based on sunrise/sunset.

use core::fmt;

#[cfg(feature = "serde")]
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use crate::solar::SunTimes;

/// A midpoint value that can be fixed or dynamic (sunrise/sunset).
///
/// Midpoint values control when the lighting curve reaches its 50% point.
/// They can be specified as:
/// - A fixed decimal hour (e.g., `6.5` = 6:30 from solar midnight/noon)
/// - `"sunrise"` - dynamically use the sunrise time
/// - `"sunset"` - dynamically use the sunset time (adjusted for evening curve)
#[derive(Debug, Clone, PartialEq, Default)]
pub enum MidpointValue {
    /// Fixed hour value (0-12 typically)
    Fixed(f32),
    /// Use sunrise time dynamically
    #[default]
    Sunrise,
    /// Use sunset time dynamically
    Sunset,
}

impl MidpointValue {
    /// Create a fixed midpoint value.
    pub fn fixed(hours: f32) -> Self {
        MidpointValue::Fixed(hours)
    }

    /// Resolve to actual hour value given sun times.
    ///
    /// For morning curves (0-12 scale):
    /// - Fixed values are returned as-is
    /// - Sunrise returns the sunrise hour directly
    /// - Sunset returns sunset - 12 (to fit 0-12 scale)
    ///
    /// For evening curves (called with times relative to solar noon):
    /// - Fixed values are returned as-is
    /// - Sunrise returns sunrise (rarely used for evening)
    /// - Sunset returns sunset - 12 (hours past noon)
    ///
    /// # Arguments
    ///
    /// * `sun_times` - The current day's sunrise and sunset times
    ///
    /// # Returns
    ///
    /// The resolved hour value (typically 0-12 range)
    pub fn resolve(&self, sun_times: &SunTimes) -> f32 {
        match self {
            MidpointValue::Fixed(v) => *v,
            MidpointValue::Sunrise => sun_times.sunrise,
            MidpointValue::Sunset => {
                // For evening curves, sunset is relative to solar noon
                // Sunset at 20:00 (8 PM) = 8 hours past noon
                sun_times.sunset - 12.0
            }
        }
    }

    /// Resolve with a fallback value if sun_times is not available.
    ///
    /// # Arguments
    ///
    /// * `sun_times` - Optional sun times (may be None if location unknown)
    /// * `fallback` - Value to use if sun_times is None and this is a dynamic value
    pub fn resolve_or(&self, sun_times: Option<&SunTimes>, fallback: f32) -> f32 {
        match self {
            MidpointValue::Fixed(v) => *v,
            MidpointValue::Sunrise | MidpointValue::Sunset => {
                sun_times.map(|st| self.resolve(st)).unwrap_or(fallback)
            }
        }
    }

    /// Check if this is a dynamic value (sunrise or sunset).
    pub fn is_dynamic(&self) -> bool {
        !matches!(self, MidpointValue::Fixed(_))
    }

    /// Check if this is a fixed value.
    pub fn is_fixed(&self) -> bool {
        matches!(self, MidpointValue::Fixed(_))
    }
}

impl fmt::Display for MidpointValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MidpointValue::Fixed(v) => {
                // Format with precision if specified
                if let Some(precision) = f.precision() {
                    write!(f, "{:.prec$}", v, prec = precision)
                } else {
                    write!(f, "{}", v)
                }
            }
            MidpointValue::Sunrise => write!(f, "sunrise"),
            MidpointValue::Sunset => write!(f, "sunset"),
        }
    }
}

impl From<f32> for MidpointValue {
    fn from(value: f32) -> Self {
        MidpointValue::Fixed(value)
    }
}

// Custom serde implementation to allow both numbers and strings

#[cfg(feature = "serde")]
impl Serialize for MidpointValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            MidpointValue::Fixed(v) => serializer.serialize_f32(*v),
            MidpointValue::Sunrise => serializer.serialize_str("sunrise"),
            MidpointValue::Sunset => serializer.serialize_str("sunset"),
        }
    }
}

#[cfg(feature = "serde")]
impl<'de> Deserialize<'de> for MidpointValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct MidpointValueVisitor;

        impl<'de> de::Visitor<'de> for MidpointValueVisitor {
            type Value = MidpointValue;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a number or \"sunrise\"/\"sunset\"")
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(MidpointValue::Fixed(value as f32))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(MidpointValue::Fixed(value as f32))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(MidpointValue::Fixed(value as f32))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match value.to_lowercase().as_str() {
                    "sunrise" => Ok(MidpointValue::Sunrise),
                    "sunset" => Ok(MidpointValue::Sunset),
                    _ => Err(de::Error::custom(format!(
                        "unknown midpoint value: '{}', expected 'sunrise' or 'sunset'",
                        value
                    ))),
                }
            }
        }

        deserializer.deserialize_any(MidpointValueVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_sun_times() -> SunTimes {
        SunTimes {
            sunrise: 6.5, // 6:30 AM
            sunset: 20.0, // 8:00 PM
            day_length: 13.5,
        }
    }

    #[test]
    fn test_fixed_value() {
        let midpoint = MidpointValue::Fixed(7.0);
        let sun_times = test_sun_times();

        assert_eq!(midpoint.resolve(&sun_times), 7.0);
        assert!(midpoint.is_fixed());
        assert!(!midpoint.is_dynamic());
    }

    #[test]
    fn test_sunrise_value() {
        let midpoint = MidpointValue::Sunrise;
        let sun_times = test_sun_times();

        assert_eq!(midpoint.resolve(&sun_times), 6.5);
        assert!(midpoint.is_dynamic());
        assert!(!midpoint.is_fixed());
    }

    #[test]
    fn test_sunset_value() {
        let midpoint = MidpointValue::Sunset;
        let sun_times = test_sun_times();

        // Sunset at 20:00 = 8 hours past noon
        assert_eq!(midpoint.resolve(&sun_times), 8.0);
    }

    #[test]
    fn test_resolve_or_with_none() {
        let fixed = MidpointValue::Fixed(5.0);
        let sunrise = MidpointValue::Sunrise;

        // Fixed should return its value even without sun_times
        assert_eq!(fixed.resolve_or(None, 6.0), 5.0);

        // Dynamic should return fallback without sun_times
        assert_eq!(sunrise.resolve_or(None, 6.0), 6.0);
    }

    #[test]
    fn test_resolve_or_with_some() {
        let sunrise = MidpointValue::Sunrise;
        let sun_times = test_sun_times();

        assert_eq!(sunrise.resolve_or(Some(&sun_times), 6.0), 6.5);
    }

    #[test]
    fn test_from_f32() {
        let midpoint: MidpointValue = 7.5.into();
        assert_eq!(midpoint, MidpointValue::Fixed(7.5));
    }

    #[test]
    fn test_default() {
        let midpoint = MidpointValue::default();
        assert_eq!(midpoint, MidpointValue::Sunrise);
    }

    #[cfg(feature = "serde")]
    mod serde_tests {
        use super::*;

        #[test]
        fn test_serialize_fixed() {
            let midpoint = MidpointValue::Fixed(6.5);
            let json = serde_json::to_string(&midpoint).unwrap();
            assert_eq!(json, "6.5");
        }

        #[test]
        fn test_serialize_sunrise() {
            let midpoint = MidpointValue::Sunrise;
            let json = serde_json::to_string(&midpoint).unwrap();
            assert_eq!(json, "\"sunrise\"");
        }

        #[test]
        fn test_serialize_sunset() {
            let midpoint = MidpointValue::Sunset;
            let json = serde_json::to_string(&midpoint).unwrap();
            assert_eq!(json, "\"sunset\"");
        }

        #[test]
        fn test_deserialize_number() {
            let midpoint: MidpointValue = serde_json::from_str("7.5").unwrap();
            assert_eq!(midpoint, MidpointValue::Fixed(7.5));
        }

        #[test]
        fn test_deserialize_integer() {
            let midpoint: MidpointValue = serde_json::from_str("6").unwrap();
            assert_eq!(midpoint, MidpointValue::Fixed(6.0));
        }

        #[test]
        fn test_deserialize_sunrise() {
            let midpoint: MidpointValue = serde_json::from_str("\"sunrise\"").unwrap();
            assert_eq!(midpoint, MidpointValue::Sunrise);
        }

        #[test]
        fn test_deserialize_sunset() {
            let midpoint: MidpointValue = serde_json::from_str("\"sunset\"").unwrap();
            assert_eq!(midpoint, MidpointValue::Sunset);
        }

        #[test]
        fn test_deserialize_case_insensitive() {
            let midpoint: MidpointValue = serde_json::from_str("\"SUNRISE\"").unwrap();
            assert_eq!(midpoint, MidpointValue::Sunrise);

            let midpoint: MidpointValue = serde_json::from_str("\"Sunset\"").unwrap();
            assert_eq!(midpoint, MidpointValue::Sunset);
        }

        #[test]
        fn test_deserialize_invalid_string() {
            let result: Result<MidpointValue, _> = serde_json::from_str("\"invalid\"");
            assert!(result.is_err());
        }

        #[test]
        fn test_roundtrip() {
            let values = vec![
                MidpointValue::Fixed(6.8),
                MidpointValue::Sunrise,
                MidpointValue::Sunset,
            ];

            for original in values {
                let json = serde_json::to_string(&original).unwrap();
                let deserialized: MidpointValue = serde_json::from_str(&json).unwrap();
                assert_eq!(original, deserialized);
            }
        }
    }
}
