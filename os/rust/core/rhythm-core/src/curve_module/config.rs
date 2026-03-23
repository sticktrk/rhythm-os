//! Configuration types for curve modules.
//!
//! This module provides module-specific configuration enums.
//! The common [`CommonCurveConfig`] is defined in [`rhythm_curve`] and
//! re-exported via the parent module.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::config::CurveConfig;
use rhythm_curve::CommonCurveConfig;

impl From<&CurveConfig> for CommonCurveConfig {
    fn from(config: &CurveConfig) -> Self {
        Self {
            min_color_temp: config.min_color_temp,
            max_color_temp: config.max_color_temp,
            min_brightness: config.min_brightness,
            max_brightness: config.max_brightness,
            max_dim_steps: config.max_dim_steps,
        }
    }
}

/// Configuration enum for different curve modules.
///
/// This allows storing module-specific configuration in a type-safe way.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "type"))]
pub enum CurveModuleConfig {
    /// Rhythm curve module (original algorithm)
    #[cfg_attr(feature = "serde", serde(rename = "rhythm"))]
    Rhythm(CurveConfig),
}

impl Default for CurveModuleConfig {
    fn default() -> Self {
        Self::Rhythm(CurveConfig::default())
    }
}

impl CurveModuleConfig {
    /// Get the module type identifier.
    pub fn module_id(&self) -> &str {
        match self {
            CurveModuleConfig::Rhythm(_) => "rhythm",
        }
    }

    /// Get the common curve configuration.
    pub fn common(&self) -> CommonCurveConfig {
        match self {
            CurveModuleConfig::Rhythm(config) => CommonCurveConfig::from(config),
        }
    }

    /// Get the inner CurveConfig if this is a Rhythm module.
    pub fn as_rhythm(&self) -> Option<&CurveConfig> {
        match self {
            CurveModuleConfig::Rhythm(config) => Some(config),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_common_config_from_curve_config() {
        let curve_config = CurveConfig {
            min_color_temp: 600,
            max_color_temp: 5500,
            min_brightness: 5,
            max_brightness: 95,
            max_dim_steps: 8,
            ..Default::default()
        };

        let common = CommonCurveConfig::from(&curve_config);
        assert_eq!(common.min_color_temp, 600);
        assert_eq!(common.max_color_temp, 5500);
        assert_eq!(common.min_brightness, 5);
        assert_eq!(common.max_brightness, 95);
        assert_eq!(common.max_dim_steps, 8);
    }

    #[test]
    fn test_module_config_default() {
        let config = CurveModuleConfig::default();
        assert_eq!(config.module_id(), "rhythm");
        assert!(config.as_rhythm().is_some());
    }

    #[test]
    fn test_module_config_common() {
        let config = CurveModuleConfig::Rhythm(CurveConfig {
            min_brightness: 10,
            max_brightness: 90,
            ..Default::default()
        });

        let common = config.common();
        assert_eq!(common.min_brightness, 10);
        assert_eq!(common.max_brightness, 90);
    }
}
