//! Spy light controller for integration testing.
//!
//! `SpyLightController` implements `LightController` and records every call,
//! allowing tests to verify what `LightingCommand` values the engine dispatches.
//!
//! Gated behind `#[cfg(any(test, feature = "test-support"))]` — never compiled
//! in production builds.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::controller::{LightControlResult, LightController};
use crate::lighting::LightingCommand;
use crate::room::Room;

/// A recorded call to the spy controller.
#[derive(Debug, Clone)]
pub enum SpyCall {
    TurnOn {
        room_id: String,
        command: LightingCommand,
    },
    TurnOff {
        room_id: String,
    },
    AnyLightsOn {
        room_id: String,
    },
}

/// A `LightController` that records all calls for test assertions.
///
/// # Example
///
/// ```ignore
/// let spy = Arc::new(SpyLightController::new());
/// // ... wire into RhythmRuntime ...
/// // ... dispatch action ...
/// let calls = spy.turn_on_calls();
/// assert_eq!(calls[0].0, "kitchen");
/// assert!(calls[0].1.brightness > 0);
/// ```
#[derive(Debug)]
pub struct SpyLightController {
    calls: Mutex<Vec<SpyCall>>,
    /// Configurable response for `any_lights_on`. Default: false.
    any_lights_on_response: Mutex<bool>,
}

impl SpyLightController {
    pub fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            any_lights_on_response: Mutex::new(false),
        }
    }

    /// Set the response for `any_lights_on` calls.
    pub fn set_any_lights_on(&self, on: bool) {
        *self.any_lights_on_response.lock().unwrap() = on;
    }

    /// Get all recorded calls.
    pub fn calls(&self) -> Vec<SpyCall> {
        self.calls.lock().unwrap().clone()
    }

    /// Get only `TurnOn` calls as `(room_id, command)` pairs.
    pub fn turn_on_calls(&self) -> Vec<(String, LightingCommand)> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter_map(|c| match c {
                SpyCall::TurnOn { room_id, command } => Some((room_id.clone(), command.clone())),
                _ => None,
            })
            .collect()
    }

    /// Get only `TurnOff` calls as room IDs.
    pub fn turn_off_calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter_map(|c| match c {
                SpyCall::TurnOff { room_id } => Some(room_id.clone()),
                _ => None,
            })
            .collect()
    }

    /// Get the last `TurnOn` command for a specific room.
    pub fn last_command_for(&self, room_id: &str) -> Option<LightingCommand> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find_map(|c| match c {
                SpyCall::TurnOn {
                    room_id: rid,
                    command,
                } if rid == room_id => Some(command.clone()),
                _ => None,
            })
    }

    /// Count total `TurnOn` calls.
    pub fn turn_on_count(&self) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| matches!(c, SpyCall::TurnOn { .. }))
            .count()
    }

    /// Count total `TurnOff` calls.
    pub fn turn_off_count(&self) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| matches!(c, SpyCall::TurnOff { .. }))
            .count()
    }

    /// Clear all recorded calls.
    pub fn reset(&self) {
        self.calls.lock().unwrap().clear();
    }
}

impl Default for SpyLightController {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LightController for SpyLightController {
    async fn turn_on(&self, room_id: &str, command: LightingCommand) -> LightControlResult<()> {
        self.calls.lock().unwrap().push(SpyCall::TurnOn {
            room_id: room_id.to_string(),
            command,
        });
        Ok(())
    }

    async fn turn_off(&self, room_id: &str) -> LightControlResult<()> {
        self.calls.lock().unwrap().push(SpyCall::TurnOff {
            room_id: room_id.to_string(),
        });
        Ok(())
    }

    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        Ok(vec![])
    }

    async fn is_connected(&self) -> bool {
        true
    }

    async fn any_lights_on(&self, room_id: &str) -> LightControlResult<bool> {
        self.calls.lock().unwrap().push(SpyCall::AnyLightsOn {
            room_id: room_id.to_string(),
        });
        Ok(*self.any_lights_on_response.lock().unwrap())
    }

    fn name(&self) -> &str {
        "Spy"
    }
}

/// `LightController` impl for `Arc<SpyLightController>` so the runtime can
/// hold a shared reference while tests hold another for inspection.
#[async_trait]
impl LightController for Arc<SpyLightController> {
    async fn turn_on(&self, room_id: &str, command: LightingCommand) -> LightControlResult<()> {
        (**self).turn_on(room_id, command).await
    }
    async fn turn_off(&self, room_id: &str) -> LightControlResult<()> {
        (**self).turn_off(room_id).await
    }
    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        (**self).get_rooms().await
    }
    async fn is_connected(&self) -> bool {
        (**self).is_connected().await
    }
    async fn any_lights_on(&self, room_id: &str) -> LightControlResult<bool> {
        (**self).any_lights_on(room_id).await
    }
    fn name(&self) -> &str {
        (**self).name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn records_turn_on() {
        let spy = SpyLightController::new();
        let cmd = LightingCommand::new(80, 4000);
        spy.turn_on("kitchen", cmd.clone()).await.unwrap();

        let calls = spy.turn_on_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "kitchen");
        assert_eq!(calls[0].1.brightness, 80);
        assert_eq!(calls[0].1.kelvin, 4000);
    }

    #[tokio::test]
    async fn records_turn_off() {
        let spy = SpyLightController::new();
        spy.turn_off("bedroom").await.unwrap();

        let calls = spy.turn_off_calls();
        assert_eq!(calls, vec!["bedroom"]);
    }

    #[tokio::test]
    async fn last_command_for_room() {
        let spy = SpyLightController::new();
        spy.turn_on("kitchen", LightingCommand::new(50, 3000))
            .await
            .unwrap();
        spy.turn_on("kitchen", LightingCommand::new(80, 4000))
            .await
            .unwrap();

        let cmd = spy.last_command_for("kitchen").unwrap();
        assert_eq!(cmd.brightness, 80);
        assert_eq!(cmd.kelvin, 4000);
    }

    #[tokio::test]
    async fn reset_clears_calls() {
        let spy = SpyLightController::new();
        spy.turn_on("kitchen", LightingCommand::new(80, 4000))
            .await
            .unwrap();
        spy.reset();
        assert_eq!(spy.turn_on_count(), 0);
    }

    #[tokio::test]
    async fn any_lights_on_configurable() {
        let spy = SpyLightController::new();
        assert!(!spy.any_lights_on("room").await.unwrap());

        spy.set_any_lights_on(true);
        assert!(spy.any_lights_on("room").await.unwrap());
    }
}
