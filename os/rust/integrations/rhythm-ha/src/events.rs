//! Event translation (HA events -> HubEvent).
//!
//! Converts Home Assistant WebSocket events into hub-agnostic `HubEvent`s
//! that the main event loop can process.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use log::info;
use rhythm_core::runtime::events::ZhaEventArgs;
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_core::ButtonAction;
use rhythm_core::DeviceRegistry;
use rhythm_os::button_resolve::{resolve_button_event, RawButtonEvent};
use rhythm_os::hub::HubEvent;
use rhythm_os::hue_buttons::map_hue_button_str;
use serde_json::Value;

use crate::registry::HaDeviceRegistry;

/// Translate a `rhythm_service_event` from the custom integration.
///
/// Event data format:
/// ```json
/// { "service": "step_up", "area_id": "living_room" }
/// ```
pub fn translate_service_event(event_data: &Value, _registry: &HaDeviceRegistry) -> Vec<HubEvent> {
    let service = match event_data.get("service").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return Vec::new(),
    };

    let area_ids = extract_area_ids(event_data);
    if area_ids.is_empty() {
        info!(target: "evt", "HA service event '{}' with no area_id, ignoring", service);
        return Vec::new();
    }

    let action = match ButtonAction::from_service_name(service) {
        Some(a) => a,
        None => {
            info!(target: "evt", "HA service event '{}': unknown service, ignoring", service);
            return Vec::new();
        }
    };

    area_ids
        .into_iter()
        .map(|area_id| {
            info!(target: "evt", "HA service: {} -> {:?} for area {}", service, action, area_id);
            HubEvent::Button {
                hub_key: None,
                room_id: area_id,
                action,
                device_id: None,
            }
        })
        .collect()
}

/// Translate a ZHA event from Home Assistant.
///
/// Uses on-demand discovery: if the device is unknown, looks up the HA
/// `device_id` in the shared cache to find its area, then auto-registers it.
///
/// Event data format:
/// ```json
/// {
///     "device_ieee": "00:11:22:33:44:55",
///     "device_id": "ha-device-uuid",
///     "command": "on_press",
///     "args": { "step_mode": 0 }
/// }
/// ```
pub fn translate_zha_event(
    event_data: &Value,
    registry: &Arc<Mutex<HaDeviceRegistry>>,
    device_area_cache: &Arc<Mutex<HashMap<String, String>>>,
) -> Vec<HubEvent> {
    let device_ieee = match event_data.get("device_ieee").and_then(|v| v.as_str()) {
        Some(d) => d,
        None => return Vec::new(),
    };

    let command = match event_data.get("command").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return Vec::new(),
    };

    // Look up room for this device
    let room_id = {
        let reg = match registry.lock() {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        reg.get_room_for_device(device_ieee)
    };

    let room_id = match room_id {
        Some(r) => r,
        None => {
            // On-demand discovery: look up HA device_id in the area cache
            let ha_device_id = match event_data.get("device_id").and_then(|v| v.as_str()) {
                Some(id) => id,
                None => {
                    info!(target: "evt", "ZHA event from unknown device {} (no device_id), ignoring", device_ieee);
                    return Vec::new();
                }
            };

            let area_id = match device_area_cache
                .lock()
                .ok()
                .and_then(|c| c.get(ha_device_id).cloned())
            {
                Some(a) => a,
                None => {
                    info!(target: "evt", "ZHA event: device {} (ha_id={}) not in area cache, cannot auto-register", device_ieee, ha_device_id);
                    return Vec::new();
                }
            };

            // Auto-register the ZHA device
            if let Ok(mut reg) = registry.lock() {
                reg.upsert_device(device_ieee, &area_id, &[], DeviceType::Button);
                info!(target: "evt", "On-demand registered ZHA device {} (ha_id={}) for area {}", device_ieee, ha_device_id, area_id);
            }

            // Re-lookup after registration
            match registry
                .lock()
                .ok()
                .and_then(|r| r.get_room_for_device(device_ieee))
            {
                Some(r) => r,
                None => return Vec::new(),
            }
        }
    };

    // Parse args if present
    let args: Option<ZhaEventArgs> = event_data
        .get("args")
        .and_then(|v| serde_json::from_value(v.clone()).ok());

    let action = match ButtonAction::from_zha_event(command, args.as_ref()) {
        Some(a) => a,
        None => {
            info!(target: "evt", "ZHA command '{}' from device {}: no action mapped", command, device_ieee);
            return Vec::new();
        }
    };

    info!(target: "evt",
        "ZHA event: device={} command={} -> {:?} for room {}",
        device_ieee, command, action, room_id
    );

    vec![HubEvent::Button {
        hub_key: None,
        room_id,
        action,
        device_id: Some(device_ieee.to_string()),
    }]
}

/// Extract area_id(s) from event data.
///
/// Supports both single string and array of strings.
fn extract_area_ids(data: &Value) -> Vec<String> {
    match data.get("area_id") {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

/// Translate a `hue_event` from HA's native Hue integration.
///
/// Uses the shared `resolve_button_event` pattern for lookup → discover → map → emit.
/// When the button is unknown and a `device_area_cache` is available, performs
/// on-demand registration: looks up the HA device_id in the cache to find the
/// area, then registers the button in the registry.
///
/// Event data format (HA fires this when Hue buttons are pressed):
/// ```json
/// {
///     "id": "hue-button-resource-uuid",
///     "device_id": "ha-device-id",
///     "unique_id": "00:17:88:...-button",
///     "type": "initial_press",
///     "subtype": 1
/// }
/// ```
pub fn translate_hue_event(
    event_data: &Value,
    registry: &Arc<Mutex<HaDeviceRegistry>>,
    device_area_cache: &Arc<Mutex<HashMap<String, String>>>,
) -> Vec<HubEvent> {
    let button_id = match event_data.get("id").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => return Vec::new(),
    };

    let event_type = match event_data.get("type").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return Vec::new(),
    };

    // subtype is the button number (1-4 on a Hue dimmer switch)
    let subtype = event_data
        .get("subtype")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u8;
    let ha_device_id = event_data.get("device_id").and_then(|v| v.as_str());

    // HA hue_event uses the same `id` for all buttons on a device,
    // differentiated only by `subtype`. Synthesize a unique button_id
    // so each physical button gets its own registry entry.
    let button_id_with_subtype = format!("{}:{}", button_id, subtype);

    let raw = RawButtonEvent {
        button_id: &button_id_with_subtype,
        event_type,
        fallback_control_id: Some(subtype),
        device_hint: ha_device_id,
    };

    // Build on-demand discovery callback using the device_area_cache
    let cache = device_area_cache.clone();
    let reg = registry.clone();
    let on_unknown = |evt: &RawButtonEvent| {
        let device_hint = match evt.device_hint {
            Some(d) => d,
            None => return,
        };
        let area_id = match cache.lock().ok().and_then(|c| c.get(device_hint).cloned()) {
            Some(a) => a,
            None => {
                info!(target: "evt", "Hue event: device {} not in area cache, cannot auto-register", device_hint);
                return;
            }
        };
        let button_id = evt.button_id.to_string();
        let control_id = evt.fallback_control_id.unwrap_or(1);
        if let Ok(mut r) = reg.lock() {
            r.upsert_device(
                device_hint,
                &area_id,
                &[(button_id, control_id)],
                DeviceType::Button,
            );
            info!(target: "evt", "On-demand registered button {} (device={}, area={}, control={})",
                evt.button_id, device_hint, area_id, control_id);
        }
    };

    resolve_button_event(registry, &raw, Some(&on_unknown), &map_hue_button_str)
}

/// Translate a `state_changed` event for motion sensors.
///
/// Filters for `binary_sensor.*` entities. If the entity is already registered
/// as a motion sensor, translates directly. If not, performs on-demand discovery:
/// checks `device_class` from the event attributes, looks up the entity's area
/// in the shared cache, and auto-registers it — mirroring how `translate_hue_event`
/// handles unknown buttons.
///
/// Event data format:
/// ```json
/// {
///     "entity_id": "binary_sensor.living_room_motion",
///     "new_state": { "state": "on", "attributes": { "device_class": "motion" } },
///     "old_state": { "state": "off", ... }
/// }
/// ```
fn translate_state_changed(
    event_data: &Value,
    registry: &Arc<Mutex<HaDeviceRegistry>>,
    cache: &Arc<Mutex<HashMap<String, String>>>,
) -> Vec<HubEvent> {
    let entity_id = match event_data.get("entity_id").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => return Vec::new(),
    };

    // Route event.* entities to button handler
    if entity_id.starts_with("event.") {
        return translate_event_entity(entity_id, event_data, registry, cache);
    }

    // Fast filter: only process binary_sensor entities
    if !entity_id.starts_with("binary_sensor.") {
        return Vec::new();
    }

    // Check if this entity is a registered motion sensor
    let room_id = {
        let reg = match registry.lock() {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        reg.get_room_for_motion_sensor(entity_id)
    };

    let room_id = match room_id {
        Some(r) => r,
        None => {
            // On-demand discovery: check if this is a motion/occupancy sensor
            let device_class = event_data
                .get("new_state")
                .and_then(|s| s.get("attributes"))
                .and_then(|a| a.get("device_class"))
                .and_then(|v| v.as_str());

            if !matches!(device_class, Some("motion") | Some("occupancy")) {
                return Vec::new();
            }

            let area_id = match cache.lock().ok().and_then(|c| c.get(entity_id).cloned()) {
                Some(a) => a,
                None => {
                    info!(target: "evt",
                        "Motion sensor {} (class={:?}) not in entity cache, cannot auto-register",
                        entity_id, device_class);
                    return Vec::new();
                }
            };

            // Auto-register as Motion device
            if let Ok(mut reg) = registry.lock() {
                reg.upsert_device(entity_id, &area_id, &[], DeviceType::Motion);
                info!(target: "evt",
                    "On-demand registered motion sensor {} for room {}",
                    entity_id, area_id);
            }

            area_id
        }
    };

    // Extract new state
    let new_state = match event_data
        .get("new_state")
        .and_then(|s| s.get("state"))
        .and_then(|v| v.as_str())
    {
        Some(s) => s,
        None => return Vec::new(),
    };

    let detected = new_state == "on";

    info!(target: "evt",
        "Motion: {} -> detected={} for room {}",
        entity_id, detected, room_id
    );

    vec![HubEvent::Motion {
        hub_key: None,
        room_id,
        sensor_id: entity_id.to_string(),
        detected,
    }]
}

/// Translate a `state_changed` event for an `event.*` entity (HA button event).
///
/// HA 2023.8+ introduced `event.*` entities that standardize button presses
/// across all integrations (Zigbee2MQTT, deCONZ, Matter, native Hue, etc.).
/// Each physical button on a remote gets its own entity.
///
/// Uses the shared `resolve_button_event` pattern for lookup → discover → map → emit.
///
/// Event data format:
/// ```json
/// {
///     "entity_id": "event.living_room_dimmer_button_1",
///     "new_state": {
///         "state": "2024-01-15T10:30:00.000+00:00",
///         "attributes": {
///             "event_type": "initial_press",
///             "device_class": "button"
///         }
///     }
/// }
/// ```
fn translate_event_entity(
    entity_id: &str,
    event_data: &Value,
    registry: &Arc<Mutex<HaDeviceRegistry>>,
    cache: &Arc<Mutex<HashMap<String, String>>>,
) -> Vec<HubEvent> {
    // Extract event_type from new_state.attributes.event_type
    let event_type = match event_data
        .get("new_state")
        .and_then(|s| s.get("attributes"))
        .and_then(|a| a.get("event_type"))
        .and_then(|v| v.as_str())
    {
        Some(t) => t,
        None => return Vec::new(),
    };

    let control_id = parse_button_number(entity_id);

    let raw = RawButtonEvent {
        button_id: entity_id,
        event_type,
        fallback_control_id: Some(control_id),
        device_hint: None, // entity_id is looked up directly in the cache
    };

    // Build on-demand discovery callback
    let cache_clone = cache.clone();
    let reg_clone = registry.clone();
    let entity_owned = entity_id.to_string();
    let on_unknown = move |evt: &RawButtonEvent| {
        let area_id = match cache_clone
            .lock()
            .ok()
            .and_then(|c| c.get(&entity_owned).cloned())
        {
            Some(a) => a,
            None => {
                info!(target: "evt", "Event entity {} not in area cache, cannot auto-register", entity_owned);
                return;
            }
        };
        let button_id = evt.button_id.to_string();
        let ctl = evt.fallback_control_id.unwrap_or(1);
        if let Ok(mut r) = reg_clone.lock() {
            r.upsert_device(
                &entity_owned,
                &area_id,
                &[(button_id, ctl)],
                DeviceType::Button,
            );
            info!(target: "evt", "On-demand registered event entity {} for area {} (control={})",
                entity_owned, area_id, ctl);
        }
    };

    resolve_button_event(registry, &raw, Some(&on_unknown), &map_event_entity_action)
}

/// Map an HA event entity action to a `ButtonAction`.
///
/// Tries Hue-style mapping first (handles `initial_press`, `repeat`, etc.),
/// then falls back to generic HA event types (`pressed`, `long_pressed`, etc.).
fn map_event_entity_action(control_id: u8, event_type: &str) -> Option<ButtonAction> {
    // Try Hue mapping first (handles initial_press, repeat, long_press, etc.)
    if let Some(action) = map_hue_button_str(control_id, event_type) {
        return Some(action);
    }
    // Generic fallback for non-Hue event entities (Zigbee2MQTT, Matter, etc.)
    match event_type {
        "pressed" => match control_id {
            1 => Some(ButtonAction::Reset),
            2 => Some(ButtonAction::UpPress),
            3 => Some(ButtonAction::DownPress),
            4 => Some(ButtonAction::OffPress),
            _ => Some(ButtonAction::Toggle),
        },
        "double_pressed" => Some(ButtonAction::OnPress),
        "long_pressed" => match control_id {
            4 => Some(ButtonAction::LightsOff),
            _ => None,
        },
        _ => None,
    }
}

/// Extract button number from an event entity_id.
///
/// Hue dimmers: `event.living_room_dimmer_button_1` → 1
/// Numbered: `event.some_device_3` → 3
/// Single buttons: `event.living_room_switch` → 1 (default)
fn parse_button_number(entity_id: &str) -> u8 {
    let name = entity_id.strip_prefix("event.").unwrap_or(entity_id);
    if let Some(last_underscore) = name.rfind('_') {
        if let Ok(n) = name[last_underscore + 1..].parse::<u8>() {
            if (1..=8).contains(&n) {
                return n;
            }
        }
    }
    1 // default to button 1
}

/// Translate a raw HA WebSocket event into hub-agnostic events.
///
/// Dispatches based on event type:
/// - `rhythm_service_event` -> service event translation
/// - `zha_event` -> ZHA device event translation
/// - `hue_event` -> HA native Hue integration button events (with on-demand discovery)
/// - `state_changed` -> motion sensor state changes
pub fn translate_ws_event(
    event_type: &str,
    event_data: &Value,
    registry: &Arc<Mutex<HaDeviceRegistry>>,
    device_area_cache: &Arc<Mutex<HashMap<String, String>>>,
) -> Vec<HubEvent> {
    match event_type {
        "hue_event" => translate_hue_event(event_data, registry, device_area_cache),
        "zha_event" => translate_zha_event(event_data, registry, device_area_cache),
        "state_changed" => translate_state_changed(event_data, registry, device_area_cache),
        _ => {
            let reg = match registry.lock() {
                Ok(r) => r,
                Err(_) => return Vec::new(),
            };
            match event_type {
                "rhythm_service_event" => translate_service_event(event_data, &reg),
                _ => Vec::new(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhythm_core::room::RoomSource;
    use serde_json::json;

    use rhythm_core::runtime::hub_registry::DeviceType;

    #[allow(clippy::type_complexity)]
    fn make_registry_and_cache() -> (
        Arc<Mutex<HaDeviceRegistry>>,
        Arc<Mutex<HashMap<String, String>>>,
    ) {
        let mut reg = HaDeviceRegistry::with_options(RoomSource::HomeAssistant, true);
        reg.upsert_room("living_room", "Living Room", "living_room", &[]);
        let registry = Arc::new(Mutex::new(reg));
        let cache = Arc::new(Mutex::new(HashMap::new()));
        (registry, cache)
    }

    #[test]
    fn test_hue_event_on_demand_registration() {
        let (registry, cache) = make_registry_and_cache();

        // Populate cache: HA device → area
        cache
            .lock()
            .unwrap()
            .insert("ha-dev-1".to_string(), "living_room".to_string());

        let event_data = json!({
            "id": "hue-btn-uuid",
            "device_id": "ha-dev-1",
            "type": "initial_press",
            "subtype": 1
        });

        let results = translate_hue_event(&event_data, &registry, &cache);

        assert_eq!(results.len(), 1);
        match &results[0] {
            HubEvent::Button {
                room_id,
                action,
                device_id,
                ..
            } => {
                assert_eq!(room_id, "living_room");
                assert_eq!(*action, ButtonAction::Reset); // button 1 initial_press
                assert_eq!(device_id.as_deref(), Some("ha-dev-1"));
            }
            _ => panic!("Expected Button event"),
        }

        // Verify button was registered with synthesized id (id:subtype)
        let reg = registry.lock().unwrap();
        assert!(reg.get_device_for_button("hue-btn-uuid:1").is_some());
    }

    #[test]
    fn test_hue_event_each_button_gets_own_control_id() {
        let (registry, cache) = make_registry_and_cache();
        cache
            .lock()
            .unwrap()
            .insert("ha-dev-1".to_string(), "living_room".to_string());

        // Press button 1 (on)
        let btn1 =
            json!({"id": "dimmer", "device_id": "ha-dev-1", "type": "initial_press", "subtype": 1});
        let r1 = translate_hue_event(&btn1, &registry, &cache);
        assert_eq!(r1.len(), 1);
        assert!(matches!(
            &r1[0],
            HubEvent::Button {
                action: ButtonAction::Reset,
                ..
            }
        ));

        // Press button 2 (dim up)
        let btn2 =
            json!({"id": "dimmer", "device_id": "ha-dev-1", "type": "initial_press", "subtype": 2});
        let r2 = translate_hue_event(&btn2, &registry, &cache);
        assert_eq!(r2.len(), 1);
        assert!(matches!(
            &r2[0],
            HubEvent::Button {
                action: ButtonAction::UpPress,
                ..
            }
        ));

        // Press button 3 (dim down)
        let btn3 =
            json!({"id": "dimmer", "device_id": "ha-dev-1", "type": "initial_press", "subtype": 3});
        let r3 = translate_hue_event(&btn3, &registry, &cache);
        assert_eq!(r3.len(), 1);
        assert!(matches!(
            &r3[0],
            HubEvent::Button {
                action: ButtonAction::DownPress,
                ..
            }
        ));

        // Press button 4 (off)
        let btn4 =
            json!({"id": "dimmer", "device_id": "ha-dev-1", "type": "initial_press", "subtype": 4});
        let r4 = translate_hue_event(&btn4, &registry, &cache);
        assert_eq!(r4.len(), 1);
        assert!(matches!(
            &r4[0],
            HubEvent::Button {
                action: ButtonAction::OffPress,
                ..
            }
        ));

        // Verify 4 distinct button entries in registry
        let reg = registry.lock().unwrap();
        assert!(reg.get_device_for_button("dimmer:1").is_some());
        assert!(reg.get_device_for_button("dimmer:2").is_some());
        assert!(reg.get_device_for_button("dimmer:3").is_some());
        assert!(reg.get_device_for_button("dimmer:4").is_some());
    }

    #[test]
    fn test_hue_event_known_button() {
        let (registry, cache) = make_registry_and_cache();

        // Pre-register with synthesized button_id (id:subtype)
        registry.lock().unwrap().upsert_device(
            "ha-dev-1",
            "living_room",
            &[("hue-btn-uuid:4".to_string(), 4)],
            DeviceType::Button,
        );

        let event_data = json!({
            "id": "hue-btn-uuid",
            "device_id": "ha-dev-1",
            "type": "initial_press",
            "subtype": 4
        });

        let results = translate_hue_event(&event_data, &registry, &cache);

        assert_eq!(results.len(), 1);
        match &results[0] {
            HubEvent::Button {
                room_id, action, ..
            } => {
                assert_eq!(room_id, "living_room");
                assert_eq!(*action, ButtonAction::OffPress);
            }
            _ => panic!("Expected Button event"),
        }
    }

    #[test]
    fn test_hue_event_unknown_button_no_cache() {
        let (registry, cache) = make_registry_and_cache();
        // Cache is empty — device can't be resolved

        let event_data = json!({
            "id": "hue-btn-uuid",
            "device_id": "ha-dev-1",
            "type": "initial_press",
            "subtype": 1
        });

        let results = translate_hue_event(&event_data, &registry, &cache);
        assert!(results.is_empty());
    }

    #[test]
    fn test_motion_sensor_on_demand_registration() {
        let (registry, cache) = make_registry_and_cache();

        // Populate cache: entity_id → area_id (from area discovery)
        cache.lock().unwrap().insert(
            "binary_sensor.living_room_motion".to_string(),
            "living_room".to_string(),
        );

        let event_data = json!({
            "entity_id": "binary_sensor.living_room_motion",
            "new_state": {
                "state": "on",
                "attributes": { "device_class": "motion" }
            },
            "old_state": { "state": "off" }
        });

        let results = translate_state_changed(&event_data, &registry, &cache);

        assert_eq!(results.len(), 1);
        match &results[0] {
            HubEvent::Motion {
                room_id,
                sensor_id,
                detected,
                ..
            } => {
                assert_eq!(room_id, "living_room");
                assert_eq!(sensor_id, "binary_sensor.living_room_motion");
                assert!(*detected);
            }
            _ => panic!("Expected Motion event"),
        }

        // Verify sensor was auto-registered
        let reg = registry.lock().unwrap();
        assert_eq!(
            reg.get_room_for_motion_sensor("binary_sensor.living_room_motion"),
            Some("living_room".to_string()),
        );
    }

    #[test]
    fn test_motion_sensor_known_skips_discovery() {
        let (registry, cache) = make_registry_and_cache();

        // Pre-register the sensor as a Motion device
        registry.lock().unwrap().upsert_device(
            "binary_sensor.kitchen_motion",
            "kitchen",
            &[],
            DeviceType::Motion,
        );

        // Cache is empty — shouldn't matter since sensor is already registered
        let event_data = json!({
            "entity_id": "binary_sensor.kitchen_motion",
            "new_state": { "state": "off" },
            "old_state": { "state": "on" }
        });

        let results = translate_state_changed(&event_data, &registry, &cache);
        assert_eq!(results.len(), 1);
        match &results[0] {
            HubEvent::Motion {
                room_id, detected, ..
            } => {
                assert_eq!(room_id, "kitchen");
                assert!(!*detected);
            }
            _ => panic!("Expected Motion event"),
        }
    }

    #[test]
    fn test_motion_sensor_non_motion_binary_sensor_ignored() {
        let (registry, cache) = make_registry_and_cache();

        // Cache has the entity, but it's a door sensor (not motion)
        cache.lock().unwrap().insert(
            "binary_sensor.front_door".to_string(),
            "living_room".to_string(),
        );

        let event_data = json!({
            "entity_id": "binary_sensor.front_door",
            "new_state": {
                "state": "on",
                "attributes": { "device_class": "door" }
            },
            "old_state": { "state": "off" }
        });

        let results = translate_state_changed(&event_data, &registry, &cache);
        assert!(results.is_empty());
    }

    #[test]
    fn test_motion_sensor_occupancy_class_works() {
        let (registry, cache) = make_registry_and_cache();

        cache.lock().unwrap().insert(
            "binary_sensor.office_occupancy".to_string(),
            "living_room".to_string(),
        );

        let event_data = json!({
            "entity_id": "binary_sensor.office_occupancy",
            "new_state": {
                "state": "on",
                "attributes": { "device_class": "occupancy" }
            },
            "old_state": { "state": "off" }
        });

        let results = translate_state_changed(&event_data, &registry, &cache);
        assert_eq!(results.len(), 1);
        assert!(matches!(
            &results[0],
            HubEvent::Motion { detected: true, .. }
        ));
    }

    #[test]
    fn test_motion_sensor_not_in_cache_ignored() {
        let (registry, cache) = make_registry_and_cache();
        // Cache is empty — sensor can't be resolved

        let event_data = json!({
            "entity_id": "binary_sensor.unknown_motion",
            "new_state": {
                "state": "on",
                "attributes": { "device_class": "motion" }
            },
            "old_state": { "state": "off" }
        });

        let results = translate_state_changed(&event_data, &registry, &cache);
        assert!(results.is_empty());
    }

    // -----------------------------------------------------------------------
    // ZHA on-demand discovery tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_zha_event_on_demand_registration() {
        let (registry, cache) = make_registry_and_cache();

        // Populate cache: HA device_id → area
        cache
            .lock()
            .unwrap()
            .insert("ha-dev-zha-1".to_string(), "living_room".to_string());

        let event_data = json!({
            "device_ieee": "00:17:88:01:aa:bb:cc:dd",
            "device_id": "ha-dev-zha-1",
            "command": "on_press",
            "args": {}
        });

        let results = translate_zha_event(&event_data, &registry, &cache);

        assert_eq!(results.len(), 1);
        match &results[0] {
            HubEvent::Button {
                room_id,
                action,
                device_id,
                ..
            } => {
                assert_eq!(room_id, "living_room");
                assert_eq!(*action, ButtonAction::OnPress);
                assert_eq!(device_id.as_deref(), Some("00:17:88:01:aa:bb:cc:dd"));
            }
            _ => panic!("Expected Button event"),
        }
    }

    #[test]
    fn test_zha_event_unknown_device_no_cache() {
        let (registry, cache) = make_registry_and_cache();
        // Cache is empty

        let event_data = json!({
            "device_ieee": "00:17:88:01:aa:bb:cc:dd",
            "device_id": "ha-dev-zha-1",
            "command": "on_press"
        });

        let results = translate_zha_event(&event_data, &registry, &cache);
        assert!(results.is_empty());
    }

    #[test]
    fn test_zha_event_known_device() {
        let (registry, cache) = make_registry_and_cache();

        // Pre-register the device
        registry.lock().unwrap().upsert_device(
            "00:17:88:01:aa:bb:cc:dd",
            "living_room",
            &[],
            DeviceType::Button,
        );

        let event_data = json!({
            "device_ieee": "00:17:88:01:aa:bb:cc:dd",
            "device_id": "ha-dev-zha-1",
            "command": "off_press"
        });

        let results = translate_zha_event(&event_data, &registry, &cache);

        assert_eq!(results.len(), 1);
        match &results[0] {
            HubEvent::Button {
                room_id, action, ..
            } => {
                assert_eq!(room_id, "living_room");
                assert_eq!(*action, ButtonAction::Reset); // off_press → Reset
            }
            _ => panic!("Expected Button event"),
        }
    }

    // -----------------------------------------------------------------------
    // Event entity tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_event_entity_hue_dimmer_button_1() {
        let (registry, cache) = make_registry_and_cache();

        cache.lock().unwrap().insert(
            "event.living_room_dimmer_button_1".to_string(),
            "living_room".to_string(),
        );

        let event_data = json!({
            "entity_id": "event.living_room_dimmer_button_1",
            "new_state": {
                "state": "2024-01-15T10:30:00.000+00:00",
                "attributes": {
                    "event_type": "initial_press",
                    "device_class": "button"
                }
            }
        });

        let results = translate_state_changed(&event_data, &registry, &cache);

        assert_eq!(results.len(), 1);
        match &results[0] {
            HubEvent::Button {
                room_id, action, ..
            } => {
                assert_eq!(room_id, "living_room");
                assert_eq!(*action, ButtonAction::Reset); // button 1 initial_press
            }
            _ => panic!("Expected Button event"),
        }
    }

    #[test]
    fn test_event_entity_all_hue_dimmer_buttons() {
        let (registry, cache) = make_registry_and_cache();

        for i in 1..=4 {
            cache.lock().unwrap().insert(
                format!("event.dimmer_button_{}", i),
                "living_room".to_string(),
            );
        }

        // Button 1 → Reset
        let r1 = translate_state_changed(
            &json!({"entity_id": "event.dimmer_button_1", "new_state": {"state": "t", "attributes": {"event_type": "initial_press"}}}),
            &registry,
            &cache,
        );
        assert_eq!(r1.len(), 1);
        assert!(matches!(
            &r1[0],
            HubEvent::Button {
                action: ButtonAction::Reset,
                ..
            }
        ));

        // Button 2 → UpPress
        let r2 = translate_state_changed(
            &json!({"entity_id": "event.dimmer_button_2", "new_state": {"state": "t", "attributes": {"event_type": "initial_press"}}}),
            &registry,
            &cache,
        );
        assert_eq!(r2.len(), 1);
        assert!(matches!(
            &r2[0],
            HubEvent::Button {
                action: ButtonAction::UpPress,
                ..
            }
        ));

        // Button 3 → DownPress
        let r3 = translate_state_changed(
            &json!({"entity_id": "event.dimmer_button_3", "new_state": {"state": "t", "attributes": {"event_type": "initial_press"}}}),
            &registry,
            &cache,
        );
        assert_eq!(r3.len(), 1);
        assert!(matches!(
            &r3[0],
            HubEvent::Button {
                action: ButtonAction::DownPress,
                ..
            }
        ));

        // Button 4 → OffPress
        let r4 = translate_state_changed(
            &json!({"entity_id": "event.dimmer_button_4", "new_state": {"state": "t", "attributes": {"event_type": "initial_press"}}}),
            &registry,
            &cache,
        );
        assert_eq!(r4.len(), 1);
        assert!(matches!(
            &r4[0],
            HubEvent::Button {
                action: ButtonAction::OffPress,
                ..
            }
        ));
    }

    #[test]
    fn test_event_entity_on_demand_registration() {
        let (registry, cache) = make_registry_and_cache();

        cache.lock().unwrap().insert(
            "event.dimmer_button_2".to_string(),
            "living_room".to_string(),
        );

        let event_data = json!({
            "entity_id": "event.dimmer_button_2",
            "new_state": {
                "state": "t",
                "attributes": { "event_type": "repeat" }
            }
        });

        let results = translate_state_changed(&event_data, &registry, &cache);

        assert_eq!(results.len(), 1);
        match &results[0] {
            HubEvent::Button {
                room_id, action, ..
            } => {
                assert_eq!(room_id, "living_room");
                assert_eq!(*action, ButtonAction::UpHold); // button 2 repeat
            }
            _ => panic!("Expected Button event"),
        }

        // Verify button was auto-registered
        let reg = registry.lock().unwrap();
        assert!(reg.get_device_for_button("event.dimmer_button_2").is_some());
    }

    #[test]
    fn test_event_entity_not_in_cache_ignored() {
        let (registry, cache) = make_registry_and_cache();
        // Cache is empty

        let event_data = json!({
            "entity_id": "event.unknown_button_1",
            "new_state": {
                "state": "t",
                "attributes": { "event_type": "initial_press" }
            }
        });

        let results = translate_state_changed(&event_data, &registry, &cache);
        assert!(results.is_empty());
    }

    #[test]
    fn test_event_entity_no_event_type_ignored() {
        let (registry, cache) = make_registry_and_cache();

        cache.lock().unwrap().insert(
            "event.dimmer_button_1".to_string(),
            "living_room".to_string(),
        );

        let event_data = json!({
            "entity_id": "event.dimmer_button_1",
            "new_state": {
                "state": "t",
                "attributes": { "device_class": "button" }
            }
        });

        let results = translate_state_changed(&event_data, &registry, &cache);
        assert!(results.is_empty());
    }

    #[test]
    fn test_event_entity_generic_pressed() {
        let (registry, cache) = make_registry_and_cache();

        cache
            .lock()
            .unwrap()
            .insert("event.ikea_button_1".to_string(), "living_room".to_string());

        let event_data = json!({
            "entity_id": "event.ikea_button_1",
            "new_state": {
                "state": "t",
                "attributes": { "event_type": "pressed" }
            }
        });

        let results = translate_state_changed(&event_data, &registry, &cache);
        assert_eq!(results.len(), 1);
        match &results[0] {
            HubEvent::Button { action, .. } => {
                assert_eq!(*action, ButtonAction::Reset); // button 1 pressed → Reset
            }
            _ => panic!("Expected Button event"),
        }
    }

    // -----------------------------------------------------------------------
    // parse_button_number tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_button_number() {
        assert_eq!(parse_button_number("event.dimmer_button_1"), 1);
        assert_eq!(parse_button_number("event.dimmer_button_4"), 4);
        assert_eq!(parse_button_number("event.some_device_3"), 3);
        assert_eq!(parse_button_number("event.single_switch"), 1); // no number → default
        assert_eq!(parse_button_number("event.button_0"), 1); // 0 out of range → default
    }

    // -----------------------------------------------------------------------
    // map_event_entity_action tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_map_event_entity_action_hue_style() {
        // Hue-style events should map via map_hue_button_str
        assert_eq!(
            map_event_entity_action(1, "initial_press"),
            Some(ButtonAction::Reset)
        );
        assert_eq!(
            map_event_entity_action(2, "repeat"),
            Some(ButtonAction::UpHold)
        );
        assert_eq!(
            map_event_entity_action(4, "long_press"),
            Some(ButtonAction::LightsOff)
        );
    }

    #[test]
    fn test_map_event_entity_action_generic() {
        assert_eq!(
            map_event_entity_action(1, "pressed"),
            Some(ButtonAction::Reset)
        );
        assert_eq!(
            map_event_entity_action(2, "pressed"),
            Some(ButtonAction::UpPress)
        );
        assert_eq!(
            map_event_entity_action(3, "pressed"),
            Some(ButtonAction::DownPress)
        );
        assert_eq!(
            map_event_entity_action(4, "pressed"),
            Some(ButtonAction::OffPress)
        );
        assert_eq!(
            map_event_entity_action(5, "pressed"),
            Some(ButtonAction::Toggle)
        );
        assert_eq!(
            map_event_entity_action(1, "double_pressed"),
            Some(ButtonAction::OnPress)
        );
        assert_eq!(
            map_event_entity_action(4, "long_pressed"),
            Some(ButtonAction::LightsOff)
        );
        assert_eq!(map_event_entity_action(1, "long_pressed"), None);
        assert_eq!(map_event_entity_action(1, "unknown_type"), None);
    }
}
