//! A spec-faithful fake Matter bulb for plan-builder and readback tests.
//!
//! It models the behaviour of `connectedhomeip/examples/lighting-app`, the
//! reference implementation most cheap bulbs derive from:
//!
//! - A colour command sent while the bulb is off returns Success. It changes
//!   the colour attributes only when the bulb honours `ExecuteIfOff`. The
//!   controller sets that bit on every colour command, so a bulb built with
//!   `honours_execute_if_off: false` models one that ignores the bit.
//! - `MoveToLevel` and `Step` without `WithOnOff` are ignored while off, and
//!   still return Success.
//! - A plain `On` does not touch `CurrentLevel`, so the bulb comes back at
//!   whatever level it had before it was turned off.
//! - `MoveToLevelWithOnOff` turns the bulb on and sets the level.
//!
//! `read_light_state` returns the same JSON shape as chipd so the runtime
//! readback comparison runs unchanged against it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::{json, Value};

use crate::transport::{
    CommissionedDevice, MatterColorMode, MatterCommissionRequest, MatterDeviceInfo,
    MatterLevelCommandVariant, MatterLevelStepMode, MatterTransport,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FakeColorMode {
    HueSaturation,
    Xy,
    ColorTemperature,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FakeBulbState {
    pub on: bool,
    pub level: u8,
    pub color_mode: FakeColorMode,
    pub hue: u8,
    pub saturation: u8,
    pub x: u16,
    pub y: u16,
    pub mireds: u16,
}

impl Default for FakeBulbState {
    fn default() -> Self {
        // Off, previously at full brightness, cool white: the state a bulb
        // has after the house turned it off last night.
        Self {
            on: false,
            level: 254,
            color_mode: FakeColorMode::ColorTemperature,
            hue: 0,
            saturation: 0,
            x: 0,
            y: 0,
            mireds: 153,
        }
    }
}

struct FakeTransition {
    started_at: Instant,
    duration: Duration,
    from: u16,
    to: u16,
}

pub(crate) struct FakeMatterBulb {
    node_id: u64,
    endpoint: u16,
    honours_execute_if_off: bool,
    state: Mutex<FakeBulbState>,
    clock: Option<Arc<Mutex<Instant>>>,
    transitions: Mutex<HashMap<&'static str, FakeTransition>>,
    pub(crate) reads: Mutex<Vec<(Instant, FakeBulbState)>>,
    pub(crate) fail_reads: AtomicBool,
    pub(crate) read_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl FakeMatterBulb {
    pub(crate) fn new(node_id: u64, endpoint: u16, honours_execute_if_off: bool) -> Self {
        Self {
            node_id,
            endpoint,
            honours_execute_if_off,
            state: Mutex::new(FakeBulbState::default()),
            clock: None,
            transitions: Mutex::new(HashMap::new()),
            reads: Mutex::new(Vec::new()),
            fail_reads: AtomicBool::new(false),
            read_hook: Mutex::new(None),
        }
    }

    /// With a shared clock, attribute reports progress throughout each requested
    /// transition. Existing plan-only fixtures keep their instantaneous behavior.
    pub(crate) fn with_clock(mut self, clock: Arc<Mutex<Instant>>) -> Self {
        self.clock = Some(clock);
        self
    }

    pub(crate) fn state(&self) -> FakeBulbState {
        let mut state = self.state.lock().unwrap().clone();
        if let Some(clock) = &self.clock {
            let now = *clock.lock().unwrap();
            for (field, transition) in self.transitions.lock().unwrap().iter() {
                let elapsed = now.saturating_duration_since(transition.started_at);
                let fraction = (elapsed.as_secs_f64() / transition.duration.as_secs_f64()).min(1.0);
                let value = (f64::from(transition.from)
                    + (f64::from(transition.to) - f64::from(transition.from)) * fraction)
                    .round() as u16;
                match *field {
                    "level" => state.level = value as u8,
                    "mireds" => state.mireds = value,
                    "x" => state.x = value,
                    "y" => state.y = value,
                    "hue" => state.hue = value as u8,
                    "saturation" => state.saturation = value as u8,
                    _ => unreachable!(),
                }
            }
        }
        state
    }

    fn transition(&self, field: &'static str, from: u16, to: u16, transition_ms: Option<u32>) {
        let mut transitions = self.transitions.lock().unwrap();
        transitions.remove(field);
        if let (Some(clock), Some(duration)) = (&self.clock, transition_ms.filter(|ms| *ms > 0)) {
            transitions.insert(
                field,
                FakeTransition {
                    started_at: *clock.lock().unwrap(),
                    duration: Duration::from_millis(u64::from(
                        crate::clusters::wire_transition_ms(duration),
                    )),
                    from,
                    to,
                },
            );
        }
    }

    fn check_target(&self, node_id: u64, endpoint: u16) -> Result<()> {
        if node_id != self.node_id || endpoint != self.endpoint {
            anyhow::bail!(
                "fake bulb is node {} endpoint {}",
                self.node_id,
                self.endpoint
            );
        }
        Ok(())
    }

    fn colour_command_executes(&self, state: &FakeBulbState) -> bool {
        state.on || self.honours_execute_if_off
    }

    fn device(&self) -> CommissionedDevice {
        CommissionedDevice {
            node_id: self.node_id,
            vendor_name: "Fake".to_string(),
            product_name: "Spec Bulb".to_string(),
            vendor_id: 0xFFF1,
            product_id: 0x8000,
            serial_number: None,
            light_endpoint: self.endpoint,
            color_modes: vec![
                MatterColorMode::HueSaturation,
                MatterColorMode::Xy,
                MatterColorMode::ColorTemperature,
            ],
            min_kelvin: Some(2000),
            max_kelvin: Some(6500),
        }
    }
}

fn read_value(value: impl Into<Value>) -> Value {
    json!({"ok": true, "value": value.into()})
}

impl MatterTransport for FakeMatterBulb {
    fn commission_light(&self, _request: &MatterCommissionRequest) -> Result<CommissionedDevice> {
        Ok(self.device())
    }

    fn decommission_device(&self, _node_id: u64, _force: bool) -> Result<()> {
        Ok(())
    }

    fn list_devices(&self) -> Result<Vec<MatterDeviceInfo>> {
        Ok(vec![MatterDeviceInfo {
            node_id: self.node_id,
            vendor_name: "Fake".to_string(),
            product_name: "Spec Bulb".to_string(),
            reachable: true,
        }])
    }

    fn probe_light(&self, node_id: u64) -> Result<CommissionedDevice> {
        self.check_target(node_id, self.endpoint)?;
        Ok(self.device())
    }

    fn set_on_off(&self, node_id: u64, endpoint: u16, on: bool) -> Result<()> {
        self.check_target(node_id, endpoint)?;
        // Plain On/Off leaves CurrentLevel alone: On restores the prior level.
        self.state.lock().unwrap().on = on;
        Ok(())
    }

    fn identify_light(&self, node_id: u64, endpoint: u16, _duration_secs: u16) -> Result<()> {
        self.check_target(node_id, endpoint)
    }

    fn set_brightness(
        &self,
        node_id: u64,
        endpoint: u16,
        level: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        // The runtime's `SetBrightness` step is MoveToLevelWithOnOff.
        self.run_level_command(
            node_id,
            endpoint,
            MatterLevelCommandVariant::MoveToLevelWithOnOff,
            level,
            None,
            transition_ms,
        )
    }

    fn run_level_command(
        &self,
        node_id: u64,
        endpoint: u16,
        command: MatterLevelCommandVariant,
        level_or_step: u8,
        step_mode: Option<MatterLevelStepMode>,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        self.check_target(node_id, endpoint)?;
        let current = self.state();
        let mut state = self.state.lock().unwrap();
        let with_on_off = matches!(
            command,
            MatterLevelCommandVariant::MoveToLevelWithOnOff
                | MatterLevelCommandVariant::StepWithOnOff
        );
        if !state.on && !with_on_off {
            // Level Control Options.ExecuteIfOff is clear by default: the
            // command is accepted and ignored.
            return Ok(());
        }
        let target = match command {
            MatterLevelCommandVariant::MoveToLevel
            | MatterLevelCommandVariant::MoveToLevelWithOnOff => level_or_step,
            MatterLevelCommandVariant::Step | MatterLevelCommandVariant::StepWithOnOff => {
                match step_mode {
                    Some(MatterLevelStepMode::Down) => current.level.saturating_sub(level_or_step),
                    _ => current.level.saturating_add(level_or_step).min(254),
                }
            }
        };
        state.level = target;
        if with_on_off {
            state.on = target > 0;
        }
        self.transition(
            "level",
            u16::from(current.level),
            u16::from(target),
            transition_ms,
        );
        Ok(())
    }

    fn set_color_temperature(
        &self,
        node_id: u64,
        endpoint: u16,
        kelvin: u16,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        self.check_target(node_id, endpoint)?;
        let current = self.state();
        let mut state = self.state.lock().unwrap();
        if !self.colour_command_executes(&state) {
            return Ok(());
        }
        state.color_mode = FakeColorMode::ColorTemperature;
        state.mireds = (1_000_000 / u32::from(kelvin.max(1))) as u16;
        self.transition("mireds", current.mireds, state.mireds, transition_ms);
        Ok(())
    }

    fn set_xy(
        &self,
        node_id: u64,
        endpoint: u16,
        x: f32,
        y: f32,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        self.check_target(node_id, endpoint)?;
        let current = self.state();
        let mut state = self.state.lock().unwrap();
        if !self.colour_command_executes(&state) {
            return Ok(());
        }
        state.color_mode = FakeColorMode::Xy;
        state.x = (x * 65_535.0).round() as u16;
        state.y = (y * 65_535.0).round() as u16;
        self.transition("x", current.x, state.x, transition_ms);
        self.transition("y", current.y, state.y, transition_ms);
        Ok(())
    }

    fn set_hue_saturation(
        &self,
        node_id: u64,
        endpoint: u16,
        hue: u8,
        saturation: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        self.check_target(node_id, endpoint)?;
        let current = self.state();
        let mut state = self.state.lock().unwrap();
        if !self.colour_command_executes(&state) {
            return Ok(());
        }
        state.color_mode = FakeColorMode::HueSaturation;
        state.hue = hue;
        state.saturation = saturation;
        self.transition(
            "hue",
            u16::from(current.hue),
            u16::from(state.hue),
            transition_ms,
        );
        self.transition(
            "saturation",
            u16::from(current.saturation),
            u16::from(state.saturation),
            transition_ms,
        );
        Ok(())
    }

    fn read_on_off(&self, node_id: u64, endpoint: u16) -> Result<bool> {
        self.check_target(node_id, endpoint)?;
        Ok(self.state.lock().unwrap().on)
    }

    fn read_light_state(&self, node_id: u64, endpoint: u16) -> Result<Value> {
        self.check_target(node_id, endpoint)?;
        let hook = self.read_hook.lock().unwrap().take();
        if let Some(hook) = hook {
            hook();
        }
        anyhow::ensure!(
            !self.fail_reads.load(Ordering::Relaxed),
            "synthetic read failure"
        );
        let state = self.state();
        if let Some(clock) = &self.clock {
            self.reads
                .lock()
                .unwrap()
                .push((*clock.lock().unwrap(), state.clone()));
        }
        Ok(json!({
            "onoff": read_value(state.on),
            "current_level": read_value(state.level),
            "color_temperature_mireds": read_value(state.mireds),
            "current_x": read_value(state.x),
            "current_y": read_value(state.y),
            "current_hue": read_value(state.hue),
            "current_saturation": read_value(state.saturation),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_while_off_is_acknowledged_and_ignored_unless_execute_if_off_is_honoured() {
        let ignores = FakeMatterBulb::new(7, 1, false);
        ignores.set_color_temperature(7, 1, 2700, None).unwrap();
        assert_eq!(
            ignores.state().mireds,
            153,
            "colour must not change while off"
        );

        let honours = FakeMatterBulb::new(7, 1, true);
        honours.set_color_temperature(7, 1, 2700, None).unwrap();
        assert_eq!(honours.state().mireds, 370);
        assert!(!honours.state().on, "ExecuteIfOff never turns the bulb on");
    }

    #[test]
    fn move_to_level_without_on_off_is_ignored_while_off() {
        let bulb = FakeMatterBulb::new(7, 1, true);
        bulb.run_level_command(7, 1, MatterLevelCommandVariant::MoveToLevel, 76, None, None)
            .unwrap();
        assert_eq!(bulb.state().level, 254);
        assert!(!bulb.state().on);

        bulb.set_brightness(7, 1, 76, None).unwrap();
        assert_eq!(bulb.state().level, 76);
        assert!(bulb.state().on);
    }

    #[test]
    fn plain_on_restores_the_previous_level() {
        let bulb = FakeMatterBulb::new(7, 1, true);
        bulb.set_brightness(7, 1, 200, None).unwrap();
        bulb.set_on_off(7, 1, false).unwrap();
        bulb.set_on_off(7, 1, true).unwrap();
        assert_eq!(bulb.state().level, 200);
        assert!(bulb.state().on);
    }
}
