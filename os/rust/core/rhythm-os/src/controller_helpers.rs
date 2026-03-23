//! Shared helpers for `LightController` implementations.
//!
//! Eliminates boilerplate for registry locking, room lookup, and error
//! mapping that every integration controller would otherwise duplicate.

use std::sync::{Arc, Mutex};

use rhythm_core::controller::{LightControlError, LightControlResult};
use rhythm_core::room::Room;

use crate::registry::HubDeviceRegistry;

/// Get rooms from registry. Call this from `LightController::get_rooms()`.
pub fn rooms_from_registry(
    registry: &Arc<Mutex<HubDeviceRegistry>>,
) -> LightControlResult<Vec<Room>> {
    let registry = registry
        .lock()
        .map_err(|e| LightControlError::Internal(format!("Failed to lock registry: {}", e)))?;
    Ok(registry.rooms())
}

/// Look up the grouped_light_id (or equivalent target) for a room.
///
/// Call this from `turn_on`/`turn_off`/`any_lights_on` before making API calls.
/// Returns the target resource ID, or `RoomNotFound` if the room isn't registered.
pub fn resolve_room_target(
    registry: &Arc<Mutex<HubDeviceRegistry>>,
    room_id: &str,
) -> LightControlResult<String> {
    let registry = registry
        .lock()
        .map_err(|e| LightControlError::Internal(format!("Failed to lock registry: {}", e)))?;
    registry
        .get_grouped_light_id(room_id)
        .ok_or_else(|| LightControlError::RoomNotFound(format!("No target for room {}", room_id)))
}
