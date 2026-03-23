//! Step/dimming types for adaptive lighting.
//!
//! This module provides types for step-based dimming along the
//! adaptive lighting curve.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::values::LightingValues;

/// Step action direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum StepAction {
    /// Step up (brighten and cool)
    Brighten,
    /// Step down (dim and warm)
    Dim,
}

impl StepAction {
    /// Get the direction multiplier (+1 for brighten, -1 for dim).
    pub fn direction(&self) -> i8 {
        match self {
            StepAction::Brighten => 1,
            StepAction::Dim => -1,
        }
    }
}

/// Result of a step calculation.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct StepResult {
    /// Target lighting values after the step
    pub values: LightingValues,

    /// Time offset from current position (in minutes)
    pub time_offset_minutes: f32,

    /// Whether the step reached a boundary (plateau)
    pub at_boundary: bool,
}

/// Curve boundary points.
///
/// These are the solar times where the curves plateau at min/max values.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct CurveBoundaries {
    /// Morning: solar time where brightness first reaches minimum
    pub min_brightness_morning: f32,
    /// Evening: solar time where brightness reaches minimum
    pub min_brightness_evening: f32,
    /// Morning: solar time where brightness reaches maximum
    pub max_brightness_morning: f32,
    /// Evening: solar time where brightness first reaches maximum
    pub max_brightness_evening: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_step_action_direction() {
        assert_eq!(StepAction::Brighten.direction(), 1);
        assert_eq!(StepAction::Dim.direction(), -1);
    }

    #[test]
    fn test_step_result_creation() {
        let values = LightingValues::new(4000, 80, 10.0, 0.5);
        let result = StepResult {
            values,
            time_offset_minutes: 30.0,
            at_boundary: false,
        };

        assert_eq!(result.time_offset_minutes, 30.0);
        assert!(!result.at_boundary);
    }

    #[test]
    fn test_curve_boundaries_creation() {
        let boundaries = CurveBoundaries {
            min_brightness_morning: 2.0,
            min_brightness_evening: 22.0,
            max_brightness_morning: 10.0,
            max_brightness_evening: 14.0,
        };

        assert_eq!(boundaries.min_brightness_morning, 2.0);
        assert_eq!(boundaries.min_brightness_evening, 22.0);
    }

    #[test]
    fn test_step_result_at_boundary() {
        let values = LightingValues::new(5500, 100, 12.0, 1.0);
        let result = StepResult {
            values,
            time_offset_minutes: 0.0,
            at_boundary: true,
        };
        assert!(result.at_boundary);
        assert_eq!(result.time_offset_minutes, 0.0);
    }

    #[test]
    fn test_step_action_clone_copy() {
        let action = StepAction::Brighten;
        let copy = action;
        assert_eq!(action, copy);
    }

    #[test]
    fn test_step_result_clone_eq() {
        let values = LightingValues::new(3000, 60, 9.0, 0.3);
        let result = StepResult {
            values,
            time_offset_minutes: 45.0,
            at_boundary: false,
        };
        let cloned = result.clone();
        assert_eq!(result, cloned);
    }

    #[test]
    fn test_curve_boundaries_clone_eq() {
        let boundaries = CurveBoundaries {
            min_brightness_morning: 3.0,
            min_brightness_evening: 21.0,
            max_brightness_morning: 9.0,
            max_brightness_evening: 15.0,
        };
        let cloned = boundaries.clone();
        assert_eq!(boundaries, cloned);
    }
}
