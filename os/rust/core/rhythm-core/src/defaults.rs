//! Default configuration presets loaded from embedded JSON.
//!
//! This module provides the default configuration values for adaptive lighting
//! curves. The defaults are loaded from `defaults.json` at compile time.

use std::collections::HashMap;

#[cfg(feature = "serde")]
use serde::Deserialize;

use crate::config::CurveConfig;

/// Embedded defaults JSON file.
const DEFAULTS_JSON: &str = include_str!("../defaults.json");

/// A named preset containing a configuration.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Deserialize))]
pub struct Preset {
    /// Display name for this preset
    pub name: String,
    /// The configuration values
    pub config: CurveConfig,
}

/// Structure of the defaults.json file.
#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(Deserialize))]
struct DefaultsFile {
    /// Which preset is active by default
    active: String,
    /// Map of preset ID to preset data
    presets: HashMap<String, Preset>,
}

/// Returns the default CurveConfig from embedded defaults.json.
///
/// This reads the preset specified by the "active" field in defaults.json.
///
/// # Panics
///
/// Panics if defaults.json is malformed or the active preset is not found.
#[cfg(feature = "serde")]
pub fn default_config() -> CurveConfig {
    let defaults: DefaultsFile =
        serde_json::from_str(DEFAULTS_JSON).expect("Invalid defaults.json format");

    defaults
        .presets
        .get(&defaults.active)
        .unwrap_or_else(|| {
            panic!(
                "Active preset '{}' not found in defaults.json",
                defaults.active
            )
        })
        .config
        .clone()
}

/// Returns the name of the active default preset.
#[cfg(feature = "serde")]
pub fn default_preset_name() -> String {
    let defaults: DefaultsFile =
        serde_json::from_str(DEFAULTS_JSON).expect("Invalid defaults.json format");

    defaults
        .presets
        .get(&defaults.active)
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "Default".to_string())
}

/// Returns all available presets from defaults.json.
#[cfg(feature = "serde")]
pub fn all_presets() -> HashMap<String, Preset> {
    let defaults: DefaultsFile =
        serde_json::from_str(DEFAULTS_JSON).expect("Invalid defaults.json format");

    defaults.presets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "serde")]
    fn test_default_config_loads() {
        let config = default_config();
        // Verify some expected values from defaults.json
        assert_eq!(config.min_brightness, 2);
        assert_eq!(config.max_brightness, 100);
        assert_eq!(config.min_color_temp, 1800);
        assert_eq!(config.max_color_temp, 5500);
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_default_preset_name() {
        let name = default_preset_name();
        assert_eq!(name, "Default");
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_all_presets() {
        let presets = all_presets();
        assert!(presets.contains_key("default"));
    }
}
