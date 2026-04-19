//! Shared helpers for `LightController` implementations.
//!
//! Eliminates boilerplate for registry locking, room lookup, and command
//! normalization that every integration controller would otherwise duplicate.

use std::sync::{Arc, Mutex};

use rhythm_core::controller::{LightControlError, LightControlResult};
use rhythm_core::kelvin_to_xy;
use rhythm_core::lighting::LightingCommand;
use rhythm_core::room::Room;
use rhythm_devices::{
    adapt_command, builtin_db, AdaptedCommand, ColorPreference, ColorRequest, LightCapabilities,
    LightType,
};

use crate::canonical::identity::HubKey;
use crate::registry::HubDeviceRegistry;
use crate::state::SharedState;

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

/// Build a user-friendly room label for logs.
pub fn format_room_label(registry: &Arc<Mutex<HubDeviceRegistry>>, room_id: &str) -> String {
    match registry.lock() {
        Ok(registry) => match registry.room_name(room_id) {
            Some(name) if name != room_id => format!("{} ({})", name, room_id),
            Some(name) => name.to_string(),
            None => room_id.to_string(),
        },
        Err(_) => room_id.to_string(),
    }
}

/// Convert a `LightingCommand` into a capability-aware normalized command.
pub fn adapt_lighting_command(
    caps: &LightCapabilities,
    command: &LightingCommand,
    preference: ColorPreference,
) -> AdaptedCommand {
    let color = if command.is_direct_color {
        ColorRequest::Xy((command.xy.x, command.xy.y))
    } else {
        let clamped_kelvin = clamp_kelvin(command.kelvin, caps.min_kelvin, caps.max_kelvin);
        let clamped_xy = kelvin_to_xy(clamped_kelvin);
        ColorRequest::ColorTemperature {
            kelvin: clamped_kelvin,
            xy: (clamped_xy.x, clamped_xy.y),
        }
    };

    adapt_command(
        caps,
        command.brightness,
        color,
        command.transition_ms,
        preference,
    )
}

fn clamp_kelvin(kelvin: u16, min_kelvin: Option<u16>, max_kelvin: Option<u16>) -> u16 {
    let kelvin = match min_kelvin {
        Some(min_kelvin) if kelvin < min_kelvin => min_kelvin,
        _ => kelvin,
    };

    match max_kelvin {
        Some(max_kelvin) if kelvin > max_kelvin => max_kelvin,
        _ => kelvin,
    }
}

/// Resolve common room capabilities from the canonical registry.
///
/// Group-addressed integrations (Hue grouped lights, HA areas) need the least
/// capable command shape that is safe for every device in the room.
///
/// Falls back to `ExtendedColor` defaults when canonical data is unavailable.
pub fn resolve_room_capabilities(
    registry: &Arc<Mutex<HubDeviceRegistry>>,
    state: Option<&SharedState>,
    hub_key: Option<&HubKey>,
    room_id: &str,
) -> LightCapabilities {
    let device_ids = match registry.lock() {
        Ok(registry) => registry.get_light_entities(room_id),
        Err(_) => return LightCapabilities::defaults_for(LightType::ExtendedColor),
    };

    resolve_device_capabilities(state, hub_key, &device_ids)
}

/// Resolve common capabilities from a concrete list of hub-native device IDs.
///
/// Used by device-addressed dispatch when there is no hub-native room grouping.
pub fn resolve_device_capabilities(
    state: Option<&SharedState>,
    hub_key: Option<&HubKey>,
    native_ids: &[String],
) -> LightCapabilities {
    let default_caps = LightCapabilities::defaults_for(LightType::ExtendedColor);
    let (Some(state), Some(hub_key)) = (state, hub_key) else {
        return default_caps;
    };
    if native_ids.is_empty() {
        return default_caps;
    }

    let db = builtin_db();
    let state = match state.lock() {
        Ok(state) => state,
        Err(_) => return default_caps,
    };

    let device_caps: Vec<LightCapabilities> = native_ids
        .iter()
        .map(|native_id| {
            state
                .canonical_registry
                .find_by_native_id(hub_key, native_id)
                .and_then(|device| {
                    let manufacturer = device.manufacturer.as_deref()?;
                    let model = device.model.as_deref()?;
                    db.lookup(manufacturer, model)
                })
                .map(|entry| entry.capabilities())
                .unwrap_or_else(|| default_caps.clone())
        })
        .collect();

    LightCapabilities::common_for(device_caps.iter()).unwrap_or(default_caps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhythm_core::runtime::hub_registry::DeviceType;

    use crate::canonical::identity::{DiscoveredIdentity, HubKey};
    use crate::hub::HubType;
    use crate::state::AppState;

    #[test]
    fn resolve_room_capabilities_uses_canonical_device_data() {
        let hub_key = HubKey::new(HubType::new(HubType::HUE), "bridge.local");
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let registry = Arc::new(Mutex::new(HubDeviceRegistry::new()));

        registry.lock().unwrap().upsert_room(
            "room1",
            "Living Room",
            "gl-room1",
            &["hue-light-1".to_string(), "hue-light-2".to_string()],
        );

        let identities = [
            DiscoveredIdentity {
                native_id: "hue-light-1".to_string(),
                room_id: "room1".to_string(),
                room_name: "Living Room".to_string(),
                name: "Color Lamp".to_string(),
                device_type: DeviceType::Light,
                hardware_ids: vec![],
                manufacturer: Some("Signify Netherlands B.V.".to_string()),
                model: Some("LCT016".to_string()),
            },
            DiscoveredIdentity {
                native_id: "hue-light-2".to_string(),
                room_id: "room1".to_string(),
                room_name: "Living Room".to_string(),
                name: "White Ambiance Lamp".to_string(),
                device_type: DeviceType::Light,
                hardware_ids: vec![],
                manufacturer: Some("Signify Netherlands B.V.".to_string()),
                model: Some("LTW015".to_string()),
            },
        ];

        {
            let mut state = state.lock().unwrap();
            for identity in identities {
                let _ = state.canonical_registry.resolve(&identity, &hub_key, 1);
            }
        }

        let caps = resolve_room_capabilities(&registry, Some(&state), Some(&hub_key), "room1");

        assert_eq!(caps.light_type, LightType::ColorTemperature);
        assert_eq!(caps.min_kelvin, Some(2200));
        assert_eq!(caps.max_kelvin, Some(6500));
        assert!(!caps.supports_xy_color());
    }

    #[test]
    fn format_room_label_prefers_room_name() {
        let registry = Arc::new(Mutex::new(HubDeviceRegistry::new()));
        registry
            .lock()
            .unwrap()
            .upsert_room("room1", "Living Room", "gl-room1", &[]);

        assert_eq!(format_room_label(&registry, "room1"), "Living Room (room1)");
        assert_eq!(format_room_label(&registry, "unknown"), "unknown");
    }

    #[test]
    fn adapt_lighting_command_recomputes_xy_from_clamped_kelvin() {
        let caps = LightCapabilities {
            min_kelvin: Some(2000),
            max_kelvin: Some(7000),
            ..LightCapabilities::defaults_for(LightType::ExtendedColor)
        };
        let command = LightingCommand::new(77, 1827);

        let adapted = adapt_lighting_command(&caps, &command, ColorPreference::PreferXy);
        let expected_xy = kelvin_to_xy(2000);

        assert_eq!(adapted.brightness, Some(77));
        assert_eq!(adapted.kelvin, None);
        assert_eq!(adapted.xy, Some((expected_xy.x, expected_xy.y)));
    }
}
