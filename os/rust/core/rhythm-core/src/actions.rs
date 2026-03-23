//! Pure-functional room action processor.
//!
//! Provides a stateless action processing function that any consumer
//! (including `RhythmEngine`) can share. No async, no controller, no side
//! effects — the caller is responsible for executing any resulting commands.

use crate::adaptive::LightingValues;
use crate::curve_module::{CurveContext, LightCurveModule};
use crate::solar::SolarTime;
use crate::steps::StepAction;

/// Minimal room state needed for action processing.
#[derive(Debug, Clone, PartialEq)]
pub struct RoomActionState {
    pub rhythm_enabled: bool,
    pub lights_on: bool,
    pub time_offset_minutes: f32,
    pub brightness_offset: f32,
}

/// Result of processing an action.
#[derive(Debug, Clone)]
pub struct ActionResult {
    /// Updated room state.
    pub new_state: RoomActionState,
    /// Lighting values to apply (None = no light command).
    pub lighting: Option<LightingValues>,
    /// Whether to turn off lights.
    pub should_turn_off: bool,
    /// Whether to turn on lights (with lighting values).
    pub should_turn_on: bool,
    /// Whether any state actually changed (for persistence).
    pub state_changed: bool,
}

/// Available room actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomAction {
    /// Toggle: if on, turn off; if off, turn on with rhythm.
    OnPress,
    /// Turn off lights (rhythm state unchanged).
    OffPress,
    /// Reset offsets and recalculate at current time.
    Reset,
    /// Enable rhythm mode only (no light commands).
    RhythmOn,
    /// Disable rhythm mode only (lights unchanged).
    RhythmOff,
    /// Step up along curve (brighten + cool).
    StepUp,
    /// Step down along curve (dim + warm).
    StepDown,
    /// Dim up (brightness only, +10).
    DimUp,
    /// Dim down (brightness only, -10).
    DimDown,
}

/// Default brightness delta for dim up/down actions.
const DIM_DELTA: f32 = 10.0;

/// Process a room action and return the new state and commands.
///
/// This is the pure-functional kernel shared by `RhythmEngine` and
/// any external consumer. It takes the current room state and returns
/// what should happen, without executing any side effects.
///
/// # Arguments
///
/// * `module` - The curve module to use for lighting calculations
/// * `solar` - Solar time context
/// * `current_hour` - Current time in hours (0-24)
/// * `action` - The action to process
/// * `state` - Current room state
pub fn process_action(
    module: &dyn LightCurveModule,
    solar: SolarTime,
    current_hour: f32,
    action: RoomAction,
    state: &RoomActionState,
) -> ActionResult {
    let effective_hour = (current_hour + state.time_offset_minutes / 60.0).rem_euclid(24.0);

    match action {
        RoomAction::OnPress => {
            if state.lights_on {
                // Turn off (rhythm state unchanged)
                ActionResult {
                    new_state: RoomActionState {
                        lights_on: false,
                        ..state.clone()
                    },
                    lighting: None,
                    should_turn_off: true,
                    should_turn_on: false,
                    state_changed: true,
                }
            } else {
                // Turn on with rhythm at current time (reset offsets)
                let ctx = CurveContext::new(current_hour, solar, None);
                let values = module.calculate(&ctx);
                ActionResult {
                    new_state: RoomActionState {
                        rhythm_enabled: true,
                        lights_on: true,
                        time_offset_minutes: 0.0,
                        brightness_offset: 0.0,
                    },
                    lighting: Some(values),
                    should_turn_off: false,
                    should_turn_on: true,
                    state_changed: true,
                }
            }
        }

        RoomAction::OffPress => ActionResult {
            new_state: RoomActionState {
                lights_on: false,
                ..state.clone()
            },
            lighting: None,
            should_turn_off: true,
            should_turn_on: false,
            state_changed: state.lights_on,
        },

        RoomAction::Reset => {
            let ctx = CurveContext::new(current_hour, solar, None);
            let values = module.calculate(&ctx);
            ActionResult {
                new_state: RoomActionState {
                    rhythm_enabled: true,
                    lights_on: true,
                    time_offset_minutes: 0.0,
                    brightness_offset: 0.0,
                },
                lighting: Some(values),
                should_turn_off: false,
                should_turn_on: true,
                state_changed: true,
            }
        }

        RoomAction::RhythmOn => ActionResult {
            new_state: RoomActionState {
                rhythm_enabled: true,
                ..state.clone()
            },
            lighting: None,
            should_turn_off: false,
            should_turn_on: false,
            state_changed: !state.rhythm_enabled,
        },

        RoomAction::RhythmOff => ActionResult {
            new_state: RoomActionState {
                rhythm_enabled: false,
                ..state.clone()
            },
            lighting: None,
            should_turn_off: false,
            should_turn_on: false,
            state_changed: state.rhythm_enabled,
        },

        RoomAction::StepUp | RoomAction::StepDown => {
            let step_action = match action {
                RoomAction::StepUp => StepAction::Brighten,
                _ => StepAction::Dim,
            };

            let ctx = CurveContext::new(effective_hour, solar, None);
            let step_result = module.calculate_step(&ctx, step_action);

            let new_offset = state.time_offset_minutes + step_result.time_offset_minutes;
            let new_effective = (current_hour + new_offset / 60.0).rem_euclid(24.0);
            let new_ctx = CurveContext::new(new_effective, solar, None);
            let values = module.calculate(&new_ctx);

            ActionResult {
                new_state: RoomActionState {
                    lights_on: true,
                    time_offset_minutes: new_offset,
                    ..state.clone()
                },
                lighting: Some(values),
                should_turn_off: false,
                should_turn_on: true,
                state_changed: true,
            }
        }

        RoomAction::DimUp | RoomAction::DimDown => {
            let delta = if action == RoomAction::DimUp {
                DIM_DELTA
            } else {
                -DIM_DELTA
            };
            let new_brightness_offset = (state.brightness_offset + delta).clamp(-100.0, 100.0);

            let ctx = CurveContext::new(effective_hour, solar, None);
            let mut values = module.calculate(&ctx);
            let adjusted =
                (values.brightness as f32 + new_brightness_offset).clamp(1.0, 100.0) as u8;
            values.brightness = adjusted;

            ActionResult {
                new_state: RoomActionState {
                    lights_on: true,
                    brightness_offset: new_brightness_offset,
                    ..state.clone()
                },
                lighting: Some(values),
                should_turn_off: false,
                should_turn_on: true,
                state_changed: true,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CurveConfig, RhythmCurveModule};

    fn test_module() -> RhythmCurveModule {
        RhythmCurveModule::new(CurveConfig::default())
    }

    fn test_solar() -> SolarTime {
        SolarTime::new(12.0, 35.0, 172)
    }

    fn default_state() -> RoomActionState {
        RoomActionState {
            rhythm_enabled: false,
            lights_on: false,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
        }
    }

    #[test]
    fn on_press_when_off_turns_on() {
        let result = process_action(
            &test_module(),
            test_solar(),
            12.0,
            RoomAction::OnPress,
            &default_state(),
        );
        assert!(result.should_turn_on);
        assert!(!result.should_turn_off);
        assert!(result.new_state.lights_on);
        assert!(result.new_state.rhythm_enabled);
        assert!(result.lighting.is_some());
    }

    #[test]
    fn on_press_when_on_turns_off() {
        let state = RoomActionState {
            lights_on: true,
            rhythm_enabled: true,
            ..default_state()
        };
        let result = process_action(
            &test_module(),
            test_solar(),
            12.0,
            RoomAction::OnPress,
            &state,
        );
        assert!(result.should_turn_off);
        assert!(!result.should_turn_on);
        assert!(!result.new_state.lights_on);
        // Rhythm state preserved
        assert!(result.new_state.rhythm_enabled);
    }

    #[test]
    fn off_press_turns_off() {
        let state = RoomActionState {
            lights_on: true,
            ..default_state()
        };
        let result = process_action(
            &test_module(),
            test_solar(),
            12.0,
            RoomAction::OffPress,
            &state,
        );
        assert!(result.should_turn_off);
        assert!(result.state_changed);
    }

    #[test]
    fn off_press_when_already_off() {
        let result = process_action(
            &test_module(),
            test_solar(),
            12.0,
            RoomAction::OffPress,
            &default_state(),
        );
        assert!(!result.state_changed);
    }

    #[test]
    fn rhythm_on_off_toggle() {
        let result = process_action(
            &test_module(),
            test_solar(),
            12.0,
            RoomAction::RhythmOn,
            &default_state(),
        );
        assert!(result.new_state.rhythm_enabled);
        assert!(result.state_changed);
        assert!(result.lighting.is_none());

        let result = process_action(
            &test_module(),
            test_solar(),
            12.0,
            RoomAction::RhythmOff,
            &result.new_state,
        );
        assert!(!result.new_state.rhythm_enabled);
        assert!(result.state_changed);
    }

    #[test]
    fn step_up_changes_offset() {
        let state = RoomActionState {
            lights_on: true,
            rhythm_enabled: true,
            ..default_state()
        };
        // Use 18:00 (evening) where stepping has room to move
        let result = process_action(
            &test_module(),
            test_solar(),
            18.0,
            RoomAction::StepUp,
            &state,
        );
        assert!(result.lighting.is_some());
        assert!(result.should_turn_on);
        assert!(result.state_changed);
    }

    #[test]
    fn dim_up_increases_brightness_offset() {
        let state = RoomActionState {
            lights_on: true,
            ..default_state()
        };
        let result = process_action(
            &test_module(),
            test_solar(),
            12.0,
            RoomAction::DimUp,
            &state,
        );
        assert_eq!(result.new_state.brightness_offset, 10.0);
        assert!(result.lighting.is_some());
    }

    #[test]
    fn dim_down_decreases_brightness_offset() {
        let state = RoomActionState {
            lights_on: true,
            ..default_state()
        };
        let result = process_action(
            &test_module(),
            test_solar(),
            12.0,
            RoomAction::DimDown,
            &state,
        );
        assert_eq!(result.new_state.brightness_offset, -10.0);
    }

    #[test]
    fn brightness_offset_clamps() {
        let state = RoomActionState {
            lights_on: true,
            brightness_offset: 95.0,
            ..default_state()
        };
        let result = process_action(
            &test_module(),
            test_solar(),
            12.0,
            RoomAction::DimUp,
            &state,
        );
        assert_eq!(result.new_state.brightness_offset, 100.0);
    }

    #[test]
    fn reset_clears_offsets() {
        let state = RoomActionState {
            lights_on: true,
            rhythm_enabled: true,
            time_offset_minutes: 30.0,
            brightness_offset: 20.0,
        };
        let result = process_action(
            &test_module(),
            test_solar(),
            12.0,
            RoomAction::Reset,
            &state,
        );
        assert_eq!(result.new_state.time_offset_minutes, 0.0);
        assert_eq!(result.new_state.brightness_offset, 0.0);
        assert!(result.new_state.rhythm_enabled);
        assert!(result.lighting.is_some());
    }
}
