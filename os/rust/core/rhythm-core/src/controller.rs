//! Light controller trait for controlling lights.
//!
//! This module defines the `LightController` trait which provides a
//! platform-agnostic interface for controlling lights. Implementations
//! exist for different backends:
//!
//! - Home Assistant (WebSocket)
//! - Philips Hue (HTTP)
//! - Direct ZigBee (future)

use async_trait::async_trait;
use std::sync::Arc;
use thiserror::Error;

use crate::lighting::LightingCommand;
use crate::room::Room;

/// Errors that can occur when controlling lights.
#[derive(Error, Debug)]
pub enum LightControlError {
    /// The specified room was not found.
    #[error("Room not found: {0}")]
    RoomNotFound(String),

    /// Failed to connect to the light controller backend.
    #[error("Connection error: {0}")]
    ConnectionError(String),

    /// Failed to send a command to the lights.
    #[error("Command failed: {0}")]
    CommandFailed(String),

    /// The light controller is not authenticated.
    #[error("Authentication required")]
    AuthRequired,

    /// A timeout occurred while waiting for a response.
    #[error("Timeout: {0}")]
    Timeout(String),

    /// An internal error occurred.
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Result type for light controller operations.
pub type LightControlResult<T> = Result<T, LightControlError>;

/// Concrete dispatch target for a single hub controller.
///
/// Rhythm's runtime always addresses topology room IDs. The composite
/// controller translates those into hub-native dispatch targets:
/// grouped room commands when a hub-native room still matches topology,
/// or explicit device lists when the user has customized membership.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HubDispatchTarget {
    /// Dispatch through a hub-native group/area/room control.
    Group {
        /// Hub-native room identifier used for registry lookups/log labels.
        room_id: String,
        /// Hub-native control resource used for the actual command.
        control_id: String,
    },
    /// Dispatch directly to one or more hub-native devices.
    Devices {
        /// Hub-native device identifiers.
        native_ids: Vec<String>,
    },
}

impl HubDispatchTarget {
    /// Human-readable description for logs and errors.
    pub fn label(&self) -> String {
        match self {
            Self::Group {
                room_id,
                control_id,
            } if room_id == control_id => room_id.clone(),
            Self::Group {
                room_id,
                control_id,
            } => format!("{} ({})", room_id, control_id),
            Self::Devices { native_ids } => native_ids.join(","),
        }
    }
}

/// Trait for controlling lights.
///
/// This trait provides a platform-agnostic interface for turning lights
/// on and off, and querying available rooms. Implementations handle the
/// specifics of each backend (Home Assistant, Hue, ZigBee, etc.).
///
/// # Example
///
/// ```ignore
/// use rhythm_core::controller::LightController;
/// use rhythm_core::lighting::LightingCommand;
///
/// async fn control_lights(controller: &impl LightController) {
///     let cmd = LightingCommand::new(80, 4000);
///     controller.turn_on("living_room", cmd).await.unwrap();
/// }
/// ```
#[async_trait]
pub trait LightController: Send + Sync {
    /// Turn on lights in a room with the specified settings.
    ///
    /// # Arguments
    ///
    /// * `room_id` - The ID of the room to control
    /// * `command` - The lighting command with brightness, color temp, etc.
    async fn turn_on(&self, room_id: &str, command: LightingCommand) -> LightControlResult<()>;

    /// Turn off lights in a room.
    ///
    /// # Arguments
    ///
    /// * `room_id` - The ID of the room to control
    /// * `transition_ms` - Optional fade duration before the room turns off
    async fn turn_off(&self, room_id: &str, transition_ms: Option<u32>) -> LightControlResult<()>;

    /// Get all available rooms from the backend.
    ///
    /// This queries the backend (Home Assistant, Hue, etc.) for available
    /// rooms/areas and returns them.
    async fn get_rooms(&self) -> LightControlResult<Vec<Room>>;

    /// Check if connected to the backend.
    async fn is_connected(&self) -> bool;

    /// Check if any lights are on in a room.
    ///
    /// # Arguments
    ///
    /// * `room_id` - The ID of the room to check
    ///
    /// # Returns
    ///
    /// `true` if any lights in the room are on, `false` otherwise.
    async fn any_lights_on(&self, room_id: &str) -> LightControlResult<bool>;

    /// Get the name of this controller (for logging/debugging).
    fn name(&self) -> &str;
}

#[async_trait]
impl<T> LightController for Arc<T>
where
    T: LightController + ?Sized,
{
    async fn turn_on(&self, room_id: &str, command: LightingCommand) -> LightControlResult<()> {
        (**self).turn_on(room_id, command).await
    }

    async fn turn_off(&self, room_id: &str, transition_ms: Option<u32>) -> LightControlResult<()> {
        (**self).turn_off(room_id, transition_ms).await
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

/// Trait for dispatching hub-native light commands.
///
/// Per-hub integrations implement this trait. The engine-facing
/// [`LightController`] remains topology-based and is implemented by the
/// composite controller.
#[async_trait]
pub trait HubLightController: Send + Sync {
    /// Turn on the target lights with the specified settings.
    async fn turn_on_target(
        &self,
        target: &HubDispatchTarget,
        command: LightingCommand,
    ) -> LightControlResult<()>;

    /// Turn off the target lights.
    async fn turn_off_target(
        &self,
        target: &HubDispatchTarget,
        transition_ms: Option<u32>,
    ) -> LightControlResult<()>;

    /// Get all available rooms from the backend.
    async fn get_rooms(&self) -> LightControlResult<Vec<Room>>;

    /// Check if connected to the backend.
    async fn is_connected(&self) -> bool;

    /// Check if any lights are on for the given target.
    async fn any_lights_on_target(&self, target: &HubDispatchTarget) -> LightControlResult<bool>;

    /// Get the name of this controller (for logging/debugging).
    fn name(&self) -> &str;
}

#[async_trait]
impl<T> HubLightController for Arc<T>
where
    T: HubLightController + ?Sized,
{
    async fn turn_on_target(
        &self,
        target: &HubDispatchTarget,
        command: LightingCommand,
    ) -> LightControlResult<()> {
        (**self).turn_on_target(target, command).await
    }

    async fn turn_off_target(
        &self,
        target: &HubDispatchTarget,
        transition_ms: Option<u32>,
    ) -> LightControlResult<()> {
        (**self).turn_off_target(target, transition_ms).await
    }

    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        (**self).get_rooms().await
    }

    async fn is_connected(&self) -> bool {
        (**self).is_connected().await
    }

    async fn any_lights_on_target(&self, target: &HubDispatchTarget) -> LightControlResult<bool> {
        (**self).any_lights_on_target(target).await
    }

    fn name(&self) -> &str {
        (**self).name()
    }
}

/// A no-op light controller for testing.
///
/// This controller does nothing but can be used for testing the
/// RhythmEngine without a real backend.
#[derive(Debug, Default)]
pub struct NoOpController;

impl NoOpController {
    /// Create a new no-op controller.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl LightController for NoOpController {
    async fn turn_on(&self, _room_id: &str, _command: LightingCommand) -> LightControlResult<()> {
        Ok(())
    }

    async fn turn_off(
        &self,
        _room_id: &str,
        _transition_ms: Option<u32>,
    ) -> LightControlResult<()> {
        Ok(())
    }

    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        Ok(vec![])
    }

    async fn is_connected(&self) -> bool {
        true
    }

    async fn any_lights_on(&self, _room_id: &str) -> LightControlResult<bool> {
        Ok(false) // Default to false for testing
    }

    fn name(&self) -> &str {
        "NoOp"
    }
}

#[async_trait]
impl HubLightController for NoOpController {
    async fn turn_on_target(
        &self,
        _target: &HubDispatchTarget,
        _command: LightingCommand,
    ) -> LightControlResult<()> {
        Ok(())
    }

    async fn turn_off_target(
        &self,
        _target: &HubDispatchTarget,
        _transition_ms: Option<u32>,
    ) -> LightControlResult<()> {
        Ok(())
    }

    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        Ok(vec![])
    }

    async fn is_connected(&self) -> bool {
        true
    }

    async fn any_lights_on_target(&self, _target: &HubDispatchTarget) -> LightControlResult<bool> {
        Ok(false)
    }

    fn name(&self) -> &str {
        "NoOp"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_noop_controller() {
        let controller = NoOpController::new();

        assert!(LightController::is_connected(&controller).await);
        assert_eq!(LightController::name(&controller), "NoOp");

        let cmd = LightingCommand::new(80, 4000);
        assert!(LightController::turn_on(&controller, "test", cmd)
            .await
            .is_ok());
        assert!(LightController::turn_off(&controller, "test", None)
            .await
            .is_ok());

        let rooms = LightController::get_rooms(&controller).await.unwrap();
        assert!(rooms.is_empty());
    }

    #[tokio::test]
    async fn test_arc_controller_wrapper_delegates() {
        let controller = Arc::new(NoOpController::new());

        assert!(LightController::is_connected(&controller).await);
        assert_eq!(LightController::name(&controller), "NoOp");

        let cmd = LightingCommand::new(42, 3200);
        assert!(LightController::turn_on(&controller, "test", cmd)
            .await
            .is_ok());
        assert!(LightController::turn_off(&controller, "test", None)
            .await
            .is_ok());
        assert!(!HubLightController::any_lights_on_target(
            &controller,
            &HubDispatchTarget::Group {
                room_id: "test".to_string(),
                control_id: "test".to_string(),
            }
        )
        .await
        .unwrap());
    }
}
