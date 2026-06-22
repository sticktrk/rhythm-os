//! Neutral runtime-plan bridge.
//!
//! This module translates between Rhythm's existing engine plans and the
//! shared `rhythm-runtime-api` contract used by external light runtimes.

use crate::controller::LightController;
use crate::lighting::LightingCommand;
use crate::primitives::{ManualActionPlan, ManualDispatchPlan, PeriodicTickPlan};
use crate::room::LightNodeKind;
use crate::runtime::error::{RuntimeError, RuntimeResult};
use crate::runtime::events::{ButtonAction, InputEvent};
use crate::runtime::{DeviceRegistry, RhythmRuntime, Scheduler, TimeProvider};
use rhythm_runtime_api::{
    DispatchCommand, DispatchTarget, InputAction, RuntimePlan, StateWrite, TickContext,
};

/// Runtime-specific metadata the existing Rhythm engine still needs after a
/// neutral plan dispatch succeeds.
///
/// `RuntimePlan` deliberately stays host/app-neutral, so this sidecar records
/// Rhythm's periodic de-dupe bookkeeping inputs without leaking them into the
/// shared app contract.
#[derive(Debug, Clone, PartialEq)]
pub struct RhythmDispatchRecord {
    pub source_node_id: String,
    pub target_node_id: String,
    pub command: LightingCommand,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RhythmInputPlanOutcome {
    RequiresLightCheck,
    Plan {
        plan: RuntimePlan,
        turned_on: bool,
        dispatch_records: Vec<RhythmDispatchRecord>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum RhythmPeriodicPlanOutcome {
    RequiresLightCheck,
    Plan {
        plan: RuntimePlan,
        dispatch_records: Vec<RhythmDispatchRecord>,
    },
}

pub fn runtime_node_kind_from_light_node_kind(
    kind: LightNodeKind,
) -> rhythm_runtime_api::RuntimeNodeKind {
    match kind {
        LightNodeKind::Room => rhythm_runtime_api::RuntimeNodeKind::Area,
        LightNodeKind::LightDevice => rhythm_runtime_api::RuntimeNodeKind::Device,
        LightNodeKind::Button | LightNodeKind::SwitchDevice => {
            rhythm_runtime_api::RuntimeNodeKind::Input
        }
        LightNodeKind::MotionSensor | LightNodeKind::Sensor | LightNodeKind::OtherDevice => {
            rhythm_runtime_api::RuntimeNodeKind::Device
        }
    }
}

pub fn runtime_lighting_command_from_core(
    command: &LightingCommand,
) -> rhythm_runtime_api::LightingCommand {
    let mut runtime_command =
        rhythm_runtime_api::LightingCommand::new(command.brightness, command.kelvin);
    runtime_command.transition_ms = command.transition_ms;
    runtime_command
}

pub fn core_lighting_command_from_runtime(
    command: &rhythm_runtime_api::LightingCommand,
) -> LightingCommand {
    let mut core = LightingCommand::new(command.brightness, command.kelvin);
    core.transition_ms = command.transition_ms;
    core
}

pub fn runtime_input_from_button_action(action: ButtonAction) -> InputAction {
    match action {
        ButtonAction::OnPress => InputAction::On,
        ButtonAction::Toggle => InputAction::Toggle,
        ButtonAction::OffPress => InputAction::Named("off_press".to_string()),
        ButtonAction::LightsOff => InputAction::Off,
        ButtonAction::Reset => InputAction::Reset,
        ButtonAction::UpPress => InputAction::BrightnessUp,
        ButtonAction::DownPress => InputAction::BrightnessDown,
        ButtonAction::UpHold => InputAction::StepUp,
        ButtonAction::DownHold => InputAction::StepDown,
        ButtonAction::Stop => InputAction::Stop,
        ButtonAction::RhythmOn => InputAction::Named("rhythm_on".to_string()),
        ButtonAction::RhythmOff => InputAction::Named("rhythm_off".to_string()),
        ButtonAction::SleepOn => InputAction::Named("sleep_on".to_string()),
        ButtonAction::SleepOff => InputAction::Named("sleep_off".to_string()),
    }
}

pub fn button_action_from_runtime_input(action: &InputAction) -> Option<ButtonAction> {
    match action {
        InputAction::On => Some(ButtonAction::OnPress),
        InputAction::Off => Some(ButtonAction::LightsOff),
        InputAction::Toggle => Some(ButtonAction::Toggle),
        InputAction::Reset => Some(ButtonAction::Reset),
        InputAction::BrightnessUp => Some(ButtonAction::UpPress),
        InputAction::BrightnessDown => Some(ButtonAction::DownPress),
        InputAction::StepUp => Some(ButtonAction::UpHold),
        InputAction::StepDown => Some(ButtonAction::DownHold),
        InputAction::Stop => Some(ButtonAction::Stop),
        InputAction::Named(name) => match name.as_str() {
            "rhythm_on" => Some(ButtonAction::RhythmOn),
            "rhythm_off" => Some(ButtonAction::RhythmOff),
            "sleep_on" => Some(ButtonAction::SleepOn),
            "sleep_off" => Some(ButtonAction::SleepOff),
            "off_press" => Some(ButtonAction::OffPress),
            "lights_off" => Some(ButtonAction::LightsOff),
            _ => None,
        },
    }
}

pub(crate) fn runtime_plan_from_dispatch_command(
    dispatch: ManualDispatchPlan,
) -> (RuntimePlan, Vec<RhythmDispatchRecord>) {
    match dispatch {
        ManualDispatchPlan::TurnOn {
            source_room_id,
            target_id,
            command,
        } => {
            let runtime_command = runtime_lighting_command_from_core(&command);
            (
                RuntimePlan {
                    dispatch: vec![DispatchCommand::TurnOn {
                        target: DispatchTarget::Node {
                            node_id: target_id.clone(),
                        },
                        command: runtime_command,
                    }],
                    state_writes: Vec::new(),
                    diagnostics: Vec::new(),
                },
                vec![RhythmDispatchRecord {
                    source_node_id: source_room_id,
                    target_node_id: target_id,
                    command,
                }],
            )
        }
        ManualDispatchPlan::TurnOff {
            target_id,
            transition_ms,
        } => (
            RuntimePlan {
                dispatch: vec![DispatchCommand::TurnOff {
                    target: DispatchTarget::Node { node_id: target_id },
                    transition_ms,
                }],
                state_writes: Vec::new(),
                diagnostics: Vec::new(),
            },
            Vec::new(),
        ),
    }
}

fn noop_plan_with_marker(node_id: &str, key: &str, value: serde_json::Value) -> RuntimePlan {
    RuntimePlan {
        dispatch: Vec::new(),
        state_writes: vec![StateWrite {
            node_id: node_id.to_string(),
            key: key.to_string(),
            value,
        }],
        diagnostics: Vec::new(),
    }
}

impl<C, T, S, R> RhythmRuntime<C, T, S, R>
where
    C: LightController + Send + Sync + 'static,
    T: TimeProvider + Send + Sync + 'static,
    S: Scheduler + Send + Sync + 'static,
    R: DeviceRegistry + Send + Sync + 'static,
{
    /// Plan a Rhythm input event without executing controller I/O.
    ///
    /// This is the neutral-plan twin of `handle_event`. Actions that need a
    /// live power check return `RequiresLightCheck`; the host should query
    /// `any_lights_on` and call again with `lights_on = Some(value)`.
    pub fn plan_input_event(
        &self,
        event: &InputEvent,
        lights_on: Option<bool>,
    ) -> RuntimeResult<RhythmInputPlanOutcome> {
        let current_hour = self.current_hour();
        let action_plan = {
            let mut engine = self
                .engine()
                .write()
                .map_err(|e| RuntimeError::Internal(format!("Failed to lock engine: {}", e)))?;
            engine.plan_button_action(&event.room_id, event.action, current_hour, lights_on)
        };

        match action_plan {
            ManualActionPlan::RequiresLightCheck => Ok(RhythmInputPlanOutcome::RequiresLightCheck),
            ManualActionPlan::Noop { turned_on } => Ok(RhythmInputPlanOutcome::Plan {
                plan: noop_plan_with_marker(
                    &event.room_id,
                    "rhythm_input_seen",
                    serde_json::json!({ "action": event.action, "device_id": event.device_id }),
                ),
                turned_on,
                dispatch_records: Vec::new(),
            }),
            ManualActionPlan::Dispatch {
                dispatch,
                turned_on,
            } => {
                let (plan, dispatch_records) = runtime_plan_from_dispatch_command(dispatch);
                Ok(RhythmInputPlanOutcome::Plan {
                    plan,
                    turned_on,
                    dispatch_records,
                })
            }
        }
    }

    /// Plan a Rhythm periodic node tick without executing controller I/O.
    pub fn plan_periodic_node_tick(
        &self,
        tick: &TickContext,
        source_node_id: &str,
        lights_on: Option<bool>,
    ) -> RuntimeResult<RhythmPeriodicPlanOutcome> {
        let periodic_plan = {
            let mut engine = self
                .engine()
                .write()
                .map_err(|e| RuntimeError::Internal(format!("Failed to lock engine: {}", e)))?;
            engine.plan_periodic_tick_node(
                &tick.node_id,
                source_node_id,
                tick.hour as f32,
                lights_on,
            )
        };

        match periodic_plan {
            PeriodicTickPlan::RequiresLightCheck => {
                Ok(RhythmPeriodicPlanOutcome::RequiresLightCheck)
            }
            PeriodicTickPlan::Skipped => Ok(RhythmPeriodicPlanOutcome::Plan {
                plan: RuntimePlan::noop(),
                dispatch_records: Vec::new(),
            }),
            PeriodicTickPlan::Dispatch { command, .. } => {
                let runtime_command = runtime_lighting_command_from_core(&command);
                Ok(RhythmPeriodicPlanOutcome::Plan {
                    plan: RuntimePlan {
                        dispatch: vec![DispatchCommand::TurnOn {
                            target: DispatchTarget::Node {
                                node_id: tick.node_id.clone(),
                            },
                            command: runtime_command,
                        }],
                        state_writes: Vec::new(),
                        diagnostics: Vec::new(),
                    },
                    dispatch_records: vec![RhythmDispatchRecord {
                        source_node_id: source_node_id.to_string(),
                        target_node_id: tick.node_id.clone(),
                        command,
                    }],
                })
            }
        }
    }

    /// Apply Rhythm-specific bookkeeping after a neutral plan dispatch succeeds.
    pub fn record_rhythm_dispatches(&self, records: &[RhythmDispatchRecord]) -> RuntimeResult<()> {
        if records.is_empty() {
            return Ok(());
        }
        let mut engine = self
            .engine()
            .write()
            .map_err(|e| RuntimeError::Internal(format!("Failed to lock engine: {}", e)))?;
        for record in records {
            engine.record_turn_on_dispatch(
                &record.source_node_id,
                &record.target_node_id,
                record.command.clone(),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::NoOpController;
    use crate::runtime::registry::SimpleDeviceRegistry;
    use crate::runtime::scheduler::NoOpScheduler;
    use crate::runtime::time::MockTimeProvider;
    use crate::runtime::RuntimeConfig;
    use std::sync::Arc;

    fn test_runtime(
    ) -> RhythmRuntime<NoOpController, MockTimeProvider, NoOpScheduler, SimpleDeviceRegistry> {
        RhythmRuntime::new(
            Arc::new(NoOpController::new()),
            MockTimeProvider::default(),
            NoOpScheduler::new(),
            SimpleDeviceRegistry::new(),
            RuntimeConfig::default(),
        )
    }

    #[test]
    fn button_action_maps_to_neutral_input_action() {
        assert_eq!(
            runtime_input_from_button_action(ButtonAction::UpPress),
            InputAction::BrightnessUp
        );
        assert_eq!(
            runtime_input_from_button_action(ButtonAction::OffPress),
            InputAction::Named("off_press".to_string())
        );
        assert_eq!(
            runtime_input_from_button_action(ButtonAction::LightsOff),
            InputAction::Off
        );
        assert_eq!(
            button_action_from_runtime_input(&InputAction::Named("off_press".to_string())),
            Some(ButtonAction::OffPress)
        );
        assert_eq!(
            button_action_from_runtime_input(&InputAction::Off),
            Some(ButtonAction::LightsOff)
        );
        assert_eq!(
            button_action_from_runtime_input(&InputAction::Named("sleep_on".to_string())),
            Some(ButtonAction::SleepOn)
        );
    }

    #[test]
    fn plans_input_as_neutral_dispatch() {
        let runtime = test_runtime();
        let event = InputEvent::new("room-a", ButtonAction::OnPress);
        let outcome = runtime.plan_input_event(&event, None).unwrap();

        let RhythmInputPlanOutcome::Plan {
            plan,
            turned_on,
            dispatch_records,
        } = outcome
        else {
            panic!("expected concrete plan");
        };
        assert!(turned_on);
        assert_eq!(dispatch_records.len(), 1);
        assert!(matches!(
            plan.dispatch.first(),
            Some(DispatchCommand::TurnOn {
                target: DispatchTarget::Node { node_id },
                ..
            }) if node_id == "room-a"
        ));
    }
}
