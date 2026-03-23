//! Light command types for controlling lights.
//!
//! This module provides the `LightingCommand` struct which represents
//! a command to send to lights. This is distinct from `LightingValues`
//! which represents calculated output from the adaptive lighting engine.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::adaptive::LightingValues;
use crate::color::{kelvin_to_mireds, kelvin_to_rgb, kelvin_to_xy, Rgb, XyColor};

/// A command to send to lights.
///
/// Contains all the values needed to control a light, with multiple
/// color representations for compatibility with different protocols.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct LightingCommand {
    /// Brightness percentage (1-100)
    pub brightness: u8,

    /// Color temperature in Kelvin
    pub kelvin: u16,

    /// RGB color representation (derived from kelvin)
    pub rgb: Rgb,

    /// CIE xy color coordinates (derived from kelvin)
    pub xy: XyColor,

    /// Transition time in milliseconds (optional)
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub transition_ms: Option<u32>,
}

impl LightingCommand {
    /// Create a new lighting command with the given brightness and color temperature.
    ///
    /// RGB and XY values are automatically calculated from the kelvin value.
    pub fn new(brightness: u8, kelvin: u16) -> Self {
        Self {
            brightness,
            kelvin,
            rgb: kelvin_to_rgb(kelvin),
            xy: kelvin_to_xy(kelvin),
            transition_ms: None,
        }
    }

    /// Create a lighting command with a transition time.
    pub fn with_transition(brightness: u8, kelvin: u16, transition_ms: u32) -> Self {
        Self {
            brightness,
            kelvin,
            rgb: kelvin_to_rgb(kelvin),
            xy: kelvin_to_xy(kelvin),
            transition_ms: Some(transition_ms),
        }
    }

    /// Create a lighting command from calculated lighting values.
    ///
    /// This converts the output of the adaptive lighting engine into
    /// a command that can be sent to lights.
    pub fn from_values(values: &LightingValues) -> Self {
        Self {
            brightness: values.brightness,
            kelvin: values.kelvin,
            rgb: values.rgb,
            xy: values.xy,
            transition_ms: None,
        }
    }

    /// Create a lighting command from lighting values with a transition.
    pub fn from_values_with_transition(values: &LightingValues, transition_ms: u32) -> Self {
        Self {
            brightness: values.brightness,
            kelvin: values.kelvin,
            rgb: values.rgb,
            xy: values.xy,
            transition_ms: Some(transition_ms),
        }
    }

    /// Convert color temperature to mireds (micro reciprocal degrees).
    ///
    /// Mireds are used by Philips Hue, ZHA, and other systems for
    /// perceptually uniform color temperature steps.
    ///
    /// Formula: mireds = 1,000,000 / kelvin
    #[inline]
    pub fn to_mired(&self) -> u16 {
        kelvin_to_mireds(self.kelvin)
    }

    /// Set the transition time and return self (builder pattern).
    pub fn transition(mut self, ms: u32) -> Self {
        self.transition_ms = Some(ms);
        self
    }

    /// Create a command that turns off the light (brightness 0).
    pub fn off() -> Self {
        Self::new(0, 2700) // Warm white, but will be off anyway
    }
}

impl From<LightingValues> for LightingCommand {
    fn from(values: LightingValues) -> Self {
        Self::from_values(&values)
    }
}

impl From<&LightingValues> for LightingCommand {
    fn from(values: &LightingValues) -> Self {
        Self::from_values(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_command() {
        let cmd = LightingCommand::new(80, 4000);
        assert_eq!(cmd.brightness, 80);
        assert_eq!(cmd.kelvin, 4000);
        assert!(cmd.transition_ms.is_none());
    }

    #[test]
    fn test_with_transition() {
        let cmd = LightingCommand::with_transition(80, 4000, 500);
        assert_eq!(cmd.brightness, 80);
        assert_eq!(cmd.kelvin, 4000);
        assert_eq!(cmd.transition_ms, Some(500));
    }

    #[test]
    fn test_from_values() {
        let values = LightingValues::new(4000, 80, 10.0, 0.5);
        let cmd = LightingCommand::from_values(&values);
        assert_eq!(cmd.brightness, 80);
        assert_eq!(cmd.kelvin, 4000);
    }

    #[test]
    fn test_to_mired() {
        let cmd = LightingCommand::new(80, 4000);
        let mired = cmd.to_mired();
        // 1,000,000 / 4000 = 250 mireds
        assert_eq!(mired, 250);
    }

    #[test]
    fn test_to_mired_warm() {
        let cmd = LightingCommand::new(80, 2700);
        let mired = cmd.to_mired();
        // 1,000,000 / 2700 ≈ 370 mireds
        assert!(mired > 360 && mired < 380);
    }

    #[test]
    fn test_to_mired_cool() {
        let cmd = LightingCommand::new(80, 6500);
        let mired = cmd.to_mired();
        // 1,000,000 / 6500 ≈ 154 mireds
        assert!(mired > 150 && mired < 160);
    }

    #[test]
    fn test_builder_pattern() {
        let cmd = LightingCommand::new(80, 4000).transition(500);
        assert_eq!(cmd.transition_ms, Some(500));
    }

    #[test]
    fn test_off_command() {
        let cmd = LightingCommand::off();
        assert_eq!(cmd.brightness, 0);
    }

    #[test]
    fn test_from_trait() {
        let values = LightingValues::new(4000, 80, 10.0, 0.5);
        let cmd: LightingCommand = values.into();
        assert_eq!(cmd.brightness, 80);
        assert_eq!(cmd.kelvin, 4000);
    }
}
