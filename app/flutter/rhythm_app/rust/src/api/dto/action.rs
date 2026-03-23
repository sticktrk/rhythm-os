//! Legacy action DTOs for backwards compatibility.

use rhythm_core::StepAction;

use super::curve::LightingValuesDto;

/// Step direction for dimming.
#[derive(Debug, Clone, Copy)]
pub enum StepDirection {
    Up,
    Down,
}

impl From<StepDirection> for StepAction {
    fn from(dir: StepDirection) -> Self {
        match dir {
            StepDirection::Up => StepAction::Brighten,
            StepDirection::Down => StepAction::Dim,
        }
    }
}

/// Room state for calculating action results (legacy).
#[derive(Debug, Clone)]
pub struct RoomStateDto {
    /// Whether rhythm mode is enabled for this room
    pub rhythm_enabled: bool,
    /// Whether lights are currently on
    pub lights_on: bool,
    /// Current time offset in minutes (from stepping)
    pub time_offset_minutes: f64,
    /// Brightness offset (from dim up/down, -100 to 100)
    pub brightness_offset: f64,
}

impl Default for RoomStateDto {
    fn default() -> Self {
        Self {
            rhythm_enabled: false,
            lights_on: false,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
        }
    }
}

/// Result of processing an action (legacy).
#[derive(Debug, Clone)]
pub struct ActionResultDto {
    /// The lighting values to apply (None if lights should be off)
    pub lighting: Option<LightingValuesDto>,
    /// Updated room state after the action
    pub new_state: RoomStateDto,
    /// Whether lights should be turned off
    pub should_turn_off: bool,
    /// Whether lights should be turned on
    pub should_turn_on: bool,
    /// Whether state changed (for persistence)
    pub state_changed: bool,
}
