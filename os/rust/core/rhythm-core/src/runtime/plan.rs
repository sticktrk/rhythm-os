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
    use crate::runtime::handle::{RestoredRoomState, RuntimeHandle};
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
    fn node_kind_and_command_conversions_preserve_runtime_contract_values() {
        assert_eq!(
            runtime_node_kind_from_light_node_kind(LightNodeKind::Room),
            rhythm_runtime_api::RuntimeNodeKind::Area
        );
        assert_eq!(
            runtime_node_kind_from_light_node_kind(LightNodeKind::LightDevice),
            rhythm_runtime_api::RuntimeNodeKind::Device
        );
        assert_eq!(
            runtime_node_kind_from_light_node_kind(LightNodeKind::Button),
            rhythm_runtime_api::RuntimeNodeKind::Input
        );
        assert_eq!(
            runtime_node_kind_from_light_node_kind(LightNodeKind::SwitchDevice),
            rhythm_runtime_api::RuntimeNodeKind::Input
        );
        assert_eq!(
            runtime_node_kind_from_light_node_kind(LightNodeKind::MotionSensor),
            rhythm_runtime_api::RuntimeNodeKind::Device
        );
        assert_eq!(
            runtime_node_kind_from_light_node_kind(LightNodeKind::Sensor),
            rhythm_runtime_api::RuntimeNodeKind::Device
        );
        assert_eq!(
            runtime_node_kind_from_light_node_kind(LightNodeKind::OtherDevice),
            rhythm_runtime_api::RuntimeNodeKind::Device
        );

        let core = LightingCommand::with_transition(42, 2700, 1500);
        let runtime = runtime_lighting_command_from_core(&core);
        assert_eq!(runtime.brightness, 42);
        assert_eq!(runtime.kelvin, 2700);
        assert_eq!(runtime.transition_ms, Some(1500));
        assert_eq!(core_lighting_command_from_runtime(&runtime), core);
    }

    #[test]
    fn all_runtime_inputs_map_back_or_reject_unknown_names() {
        let cases = [
            (InputAction::On, Some(ButtonAction::OnPress)),
            (InputAction::Off, Some(ButtonAction::LightsOff)),
            (InputAction::Toggle, Some(ButtonAction::Toggle)),
            (InputAction::Reset, Some(ButtonAction::Reset)),
            (InputAction::BrightnessUp, Some(ButtonAction::UpPress)),
            (InputAction::BrightnessDown, Some(ButtonAction::DownPress)),
            (InputAction::StepUp, Some(ButtonAction::UpHold)),
            (InputAction::StepDown, Some(ButtonAction::DownHold)),
            (InputAction::Stop, Some(ButtonAction::Stop)),
            (
                InputAction::Named("rhythm_on".to_string()),
                Some(ButtonAction::RhythmOn),
            ),
            (
                InputAction::Named("rhythm_off".to_string()),
                Some(ButtonAction::RhythmOff),
            ),
            (
                InputAction::Named("sleep_off".to_string()),
                Some(ButtonAction::SleepOff),
            ),
            (InputAction::Named("unknown".to_string()), None),
        ];

        for (input, expected) in cases {
            assert_eq!(button_action_from_runtime_input(&input), expected);
        }
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

    #[test]
    fn noop_input_becomes_visible_state_write_not_dispatch() {
        let runtime = test_runtime();
        let event = InputEvent::new("room-a", ButtonAction::Stop);
        let outcome = runtime.plan_input_event(&event, None).unwrap();

        let RhythmInputPlanOutcome::Plan {
            plan,
            turned_on,
            dispatch_records,
        } = outcome
        else {
            panic!("expected concrete noop plan");
        };

        assert!(!turned_on);
        assert!(dispatch_records.is_empty());
        assert!(plan.dispatch.is_empty());
        assert_eq!(plan.state_writes.len(), 1);
        assert_eq!(plan.state_writes[0].node_id, "room-a");
        assert_eq!(plan.state_writes[0].key, "rhythm_input_seen");
        assert_eq!(plan.state_writes[0].value["action"], "stop");
    }

    #[test]
    fn off_input_with_lights_on_plans_turn_off_without_dispatch_record() {
        let runtime = test_runtime();
        let event = InputEvent::new("room-a", ButtonAction::OffPress);
        let outcome = runtime.plan_input_event(&event, Some(true)).unwrap();

        let RhythmInputPlanOutcome::Plan {
            plan,
            turned_on,
            dispatch_records,
        } = outcome
        else {
            panic!("expected concrete dispatch plan");
        };

        assert!(!turned_on);
        assert!(dispatch_records.is_empty());
        assert!(matches!(
            plan.dispatch.first(),
            Some(DispatchCommand::TurnOff {
                target: DispatchTarget::Node { node_id },
                transition_ms: None,
            }) if node_id == "room-a"
        ));
    }

    #[test]
    fn periodic_plan_requires_light_check_then_dispatches_in_order() {
        let runtime = test_runtime();
        runtime.add_room("room-a", "Room A");
        runtime.restore_room_state(
            "room-a",
            RestoredRoomState {
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
                soft_off: false,
                mood_active: false,
                standby_enabled: true,
                hard_off: false,
                profile_settings: Default::default(),
            },
        );
        let tick = TickContext {
            node_id: "room-a".to_string(),
            hour: 14.0,
            epoch_ms: Some(1_700_000_000_000),
            metadata: Default::default(),
        };

        assert_eq!(
            runtime
                .plan_periodic_node_tick(&tick, "room-a", None)
                .unwrap(),
            RhythmPeriodicPlanOutcome::RequiresLightCheck
        );

        let outcome = runtime
            .plan_periodic_node_tick(&tick, "room-a", Some(true))
            .unwrap();
        let RhythmPeriodicPlanOutcome::Plan {
            plan,
            dispatch_records,
        } = outcome
        else {
            panic!("expected periodic dispatch plan");
        };

        assert_eq!(dispatch_records.len(), 1);
        assert_eq!(dispatch_records[0].source_node_id, "room-a");
        assert_eq!(dispatch_records[0].target_node_id, "room-a");
        assert!(matches!(
            plan.dispatch.first(),
            Some(DispatchCommand::TurnOn {
                target: DispatchTarget::Node { node_id },
                ..
            }) if node_id == "room-a"
        ));
    }

    #[test]
    fn record_rhythm_dispatches_accepts_empty_and_multiple_records() {
        let runtime = test_runtime();
        runtime.add_room("room-a", "Room A");
        runtime.add_room("room-b", "Room B");

        runtime.record_rhythm_dispatches(&[]).unwrap();
        runtime
            .record_rhythm_dispatches(&[
                RhythmDispatchRecord {
                    source_node_id: "room-a".to_string(),
                    target_node_id: "room-a".to_string(),
                    command: LightingCommand::new(55, 3000),
                },
                RhythmDispatchRecord {
                    source_node_id: "room-a".to_string(),
                    target_node_id: "room-b".to_string(),
                    command: LightingCommand::with_transition(44, 2700, 500),
                },
            ])
            .unwrap();

        let tick = TickContext {
            node_id: "room-b".to_string(),
            hour: 14.0,
            epoch_ms: None,
            metadata: Default::default(),
        };
        let planned = runtime
            .plan_periodic_node_tick(&tick, "room-a", Some(true))
            .unwrap();
        assert!(matches!(planned, RhythmPeriodicPlanOutcome::Plan { .. }));
    }
}
