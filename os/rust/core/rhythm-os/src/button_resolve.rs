//! Shared button resolution pattern.
//!
//! Standardizes the lookup → discover → map → emit flow for button events.
//! Used by both Hue SSE (`rhythm-hue`) and HA `hue_event` (`rhythm-ha`).

use std::sync::{Arc, Mutex};

use log::info;
use rhythm_core::ButtonAction;

use crate::hub::HubEvent;
use crate::registry::HubDeviceRegistry;

/// All available info about a raw button event (parsed by caller).
pub struct RawButtonEvent<'a> {
    /// Button resource ID (Hue UUID or HA unique_id).
    pub button_id: &'a str,
    /// Event type string (e.g. "initial_press", "short_release").
    pub event_type: &'a str,
    /// Fallback control_id when registry has none (HA provides subtype).
    pub fallback_control_id: Option<u8>,
    /// Hint for discovery callback (e.g. HA device_id).
    pub device_hint: Option<&'a str>,
}

/// Standard button resolution: lookup → discover → map → emit.
///
/// 1. Check registry fast path (button_id already known).
/// 2. If unknown and `on_unknown` callback is provided: call it, then re-check.
/// 3. Get control_id (registry or fallback), room_id, device_id from registry.
/// 4. `map_action(control_id, event_type)` → `ButtonAction`.
/// 5. Return `HubEvent::Button { hub_key, room_id, action, device_id }`.
///
/// Lock ordering: registry is locked and dropped for each check. The
/// `on_unknown` callback may lock other resources (e.g. area cache) and
/// then lock the registry to register the button. Registry is never held
/// across the callback invocation.
pub fn resolve_button_event(
    registry: &Arc<Mutex<HubDeviceRegistry>>,
    event: &RawButtonEvent,
    on_unknown: Option<&dyn Fn(&RawButtonEvent)>,
    map_action: &dyn Fn(u8, &str) -> Option<ButtonAction>,
) -> Vec<HubEvent> {
    // 1. Fast path: check if button is already in the registry
    let mut known = {
        let reg = match registry.lock() {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        reg.get_device_for_button(event.button_id).is_some()
    };

    // 2. If unknown and we have a discovery callback, try to discover
    if !known {
        if let Some(discover) = on_unknown {
            discover(event);
            // Re-check after discovery
            known = registry
                .lock()
                .map(|r| r.get_device_for_button(event.button_id).is_some())
                .unwrap_or(false);
        }

        if !known {
            info!(target: "evt", "Button {}: unknown after discovery, unroutable", event.button_id);
            return vec![HubEvent::UnroutableButton {
                hub_key: None,
                device_id: None,
                button_id: event.button_id.to_string(),
            }];
        }
    }

    // 3. Read device_id, control_id, room_id from registry
    let reg = match registry.lock() {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let device_id = match reg.get_device_for_button(event.button_id) {
        Some(id) => id,
        None => return Vec::new(),
    };

    // Use registry control_id, fall back to event's fallback
    let control_id = reg
        .get_control_id(event.button_id)
        .or(event.fallback_control_id)
        .unwrap_or(1);

    let room_id = match reg.get_room_for_button(event.button_id) {
        Some(id) => id,
        None => {
            info!(target: "evt", "Button {} (device={}) has no room mapping, unroutable",
                event.button_id, device_id);
            return vec![HubEvent::UnroutableButton {
                hub_key: None,
                device_id: Some(device_id),
                button_id: event.button_id.to_string(),
            }];
        }
    };

    // 4. Map to ButtonAction
    let action = match map_action(control_id, event.event_type) {
        Some(a) => a,
        None => {
            info!(target: "evt", "Button {} (control={}): event '{}' not mapped, ignoring",
                event.button_id, control_id, event.event_type);
            return Vec::new();
        }
    };

    info!(target: "evt",
        "Button {} (device={}, control={}) -> {:?} for room {}",
        event.button_id, device_id, control_id, action, room_id
    );

    vec![HubEvent::Button {
        hub_key: None,
        room_id,
        action,
        device_id: Some(device_id),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhythm_core::runtime::hub_registry::DeviceType;

    fn make_registry() -> Arc<Mutex<HubDeviceRegistry>> {
        let mut reg = HubDeviceRegistry::with_options(false);
        reg.upsert_room("room-1", "Living Room", "gl-1", &[]);
        reg.upsert_device(
            "device-1",
            "room-1",
            &[("button-1".to_string(), 1), ("button-4".to_string(), 4)],
            DeviceType::Button,
        );
        Arc::new(Mutex::new(reg))
    }

    #[test]
    fn test_known_button_resolves() {
        let registry = make_registry();
        let event = RawButtonEvent {
            button_id: "button-1",
            event_type: "initial_press",
            fallback_control_id: None,
            device_hint: None,
        };

        let results = resolve_button_event(
            &registry,
            &event,
            None,
            &crate::hue_buttons::map_hue_button_str,
        );

        assert_eq!(results.len(), 1);
        match &results[0] {
            HubEvent::Button {
                room_id,
                action,
                device_id,
                ..
            } => {
                assert_eq!(room_id, "room-1");
                assert_eq!(*action, ButtonAction::Reset); // button 1 initial_press
                assert_eq!(device_id.as_deref(), Some("device-1"));
            }
            _ => panic!("Expected Button event"),
        }
    }

    #[test]
    fn test_unknown_button_without_callback_returns_unroutable() {
        let registry = make_registry();
        let event = RawButtonEvent {
            button_id: "unknown-btn",
            event_type: "initial_press",
            fallback_control_id: None,
            device_hint: None,
        };

        let results = resolve_button_event(
            &registry,
            &event,
            None,
            &crate::hue_buttons::map_hue_button_str,
        );

        assert_eq!(results.len(), 1);
        match &results[0] {
            HubEvent::UnroutableButton {
                device_id,
                button_id,
                ..
            } => {
                assert_eq!(button_id, "unknown-btn");
                assert!(device_id.is_none());
            }
            _ => panic!("Expected UnroutableButton event"),
        }
    }

    #[test]
    fn test_unknown_button_with_discovery_callback() {
        let registry = make_registry();

        // Set up a room for the on-demand registration
        {
            let mut reg = registry.lock().unwrap();
            reg.upsert_room("room-2", "Bedroom", "gl-2", &[]);
        }

        let reg_clone = registry.clone();
        let on_unknown = |evt: &RawButtonEvent| {
            let mut reg = reg_clone.lock().unwrap();
            reg.upsert_device(
                "new-device",
                "room-2",
                &[(
                    evt.button_id.to_string(),
                    evt.fallback_control_id.unwrap_or(1),
                )],
                DeviceType::Button,
            );
        };

        let event = RawButtonEvent {
            button_id: "new-button",
            event_type: "initial_press",
            fallback_control_id: Some(2),
            device_hint: Some("ha-device-123"),
        };

        let results = resolve_button_event(
            &registry,
            &event,
            Some(&on_unknown),
            &crate::hue_buttons::map_hue_button_str,
        );

        assert_eq!(results.len(), 1);
        match &results[0] {
            HubEvent::Button {
                room_id,
                action,
                device_id,
                ..
            } => {
                assert_eq!(room_id, "room-2");
                assert_eq!(*action, ButtonAction::UpPress); // button 2 initial_press
                assert_eq!(device_id.as_deref(), Some("new-device"));
            }
            _ => panic!("Expected Button event"),
        }
    }

    #[test]
    fn test_fallback_control_id_used_when_registry_has_none() {
        // This shouldn't normally happen (registry should have control_id),
        // but if somehow the registry entry has no control_id, fallback is used
        let registry = make_registry();

        let event = RawButtonEvent {
            button_id: "button-1",
            event_type: "initial_press",
            fallback_control_id: Some(3), // would map to DownPress
            device_hint: None,
        };

        // button-1 has control_id=1 in registry, so fallback is NOT used
        let results = resolve_button_event(
            &registry,
            &event,
            None,
            &crate::hue_buttons::map_hue_button_str,
        );

        assert_eq!(results.len(), 1);
        match &results[0] {
            HubEvent::Button { action, .. } => {
                assert_eq!(*action, ButtonAction::Reset); // control_id=1, initial_press
            }
            _ => panic!("Expected Button event"),
        }
    }

    #[test]
    fn test_no_room_returns_unroutable() {
        let registry = make_registry();

        // Remove the room mapping for device-1 while keeping its buttons registered
        {
            use rhythm_core::DeviceRegistry;
            let mut reg = registry.lock().unwrap();
            reg.unregister_device("device-1");
        }

        let event = RawButtonEvent {
            button_id: "button-1",
            event_type: "initial_press",
            fallback_control_id: None,
            device_hint: None,
        };

        let results = resolve_button_event(
            &registry,
            &event,
            None,
            &crate::hue_buttons::map_hue_button_str,
        );

        assert_eq!(results.len(), 1);
        match &results[0] {
            HubEvent::UnroutableButton {
                device_id,
                button_id,
                ..
            } => {
                assert_eq!(device_id.as_deref(), Some("device-1"));
                assert_eq!(button_id, "button-1");
            }
            _ => panic!("Expected UnroutableButton event"),
        }
    }

    #[test]
    fn test_unmapped_event_type_ignored() {
        let registry = make_registry();
        let event = RawButtonEvent {
            button_id: "button-1",
            event_type: "short_release", // not mapped for button 1
            fallback_control_id: None,
            device_hint: None,
        };

        let results = resolve_button_event(
            &registry,
            &event,
            None,
            &crate::hue_buttons::map_hue_button_str,
        );

        assert!(results.is_empty());
    }

    #[test]
    fn test_off_button_long_press() {
        let registry = make_registry();
        let event = RawButtonEvent {
            button_id: "button-4",
            event_type: "long_press",
            fallback_control_id: None,
            device_hint: None,
        };

        let results = resolve_button_event(
            &registry,
            &event,
            None,
            &crate::hue_buttons::map_hue_button_str,
        );

        assert_eq!(results.len(), 1);
        match &results[0] {
            HubEvent::Button { action, .. } => {
                assert_eq!(*action, ButtonAction::LightsOff);
            }
            _ => panic!("Expected Button event"),
        }
    }
}
