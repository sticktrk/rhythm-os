//! Hue SSE event translation.
//!
//! Converts raw Hue SSE events into hub-agnostic `HubEvent`s.
//! Button events use the shared `resolve_button_event` pattern from
//! rhythm-os; motion events are resolved via the device registry.

use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use log::info;

use rhythm_os::button_resolve::RawButtonEvent;
use rhythm_os::hub::HubEvent;
use rhythm_os::registry::HubDeviceRegistry;

enum MotionLookup {
    Room(String),
    KnownRoomless,
    Unknown,
    LockFailed,
}

fn lookup_motion_room(registry: &Arc<Mutex<HubDeviceRegistry>>, motion_id: &str) -> MotionLookup {
    let reg = match registry.lock() {
        Ok(r) => r,
        Err(_) => return MotionLookup::LockFailed,
    };

    if let Some(room_id) = reg.get_room_for_motion_sensor(motion_id) {
        return MotionLookup::Room(room_id);
    }

    if reg.has_device(motion_id) {
        MotionLookup::KnownRoomless
    } else {
        MotionLookup::Unknown
    }
}

fn translate_sse_event_with_hooks(
    registry: &Arc<Mutex<HubDeviceRegistry>>,
    event: crate::sse::HueSseEvent,
    on_activity: Option<&dyn Fn()>,
    on_unknown_button: Option<&dyn Fn(&RawButtonEvent)>,
    on_unknown_motion: Option<&dyn Fn(&str)>,
) -> Vec<HubEvent> {
    use crate::sse::HueSseEvent;
    use rhythm_os::button_resolve::resolve_button_event;
    use rhythm_os::hue_buttons::map_hue_button_str;

    match event {
        HueSseEvent::Connected => {
            vec![HubEvent::Connected { hub_key: None }]
        }

        HueSseEvent::ButtonEvent {
            button_id,
            event_type,
        } => {
            if let Some(cb) = on_activity {
                cb();
            }

            let raw = RawButtonEvent {
                button_id: &button_id,
                event_type: &event_type,
                fallback_control_id: None,
                device_hint: None,
            };

            resolve_button_event(registry, &raw, on_unknown_button, &map_hue_button_str)
        }

        HueSseEvent::MotionEvent {
            motion_id,
            motion_detected,
        } => {
            if let Some(cb) = on_activity {
                cb();
            }

            // If not found, try on-demand discovery then re-check
            let room_id = match lookup_motion_room(registry, &motion_id) {
                MotionLookup::Room(id) => id,
                MotionLookup::KnownRoomless => {
                    info!(target: "evt", "SSE: Motion sensor {} is known without a registry room; forwarding for topology routing", motion_id);
                    String::new()
                }
                MotionLookup::LockFailed => return Vec::new(),
                MotionLookup::Unknown => {
                    if let Some(cb) = on_unknown_motion {
                        cb(&motion_id);
                    }
                    match lookup_motion_room(registry, &motion_id) {
                        MotionLookup::Room(id) => id,
                        MotionLookup::KnownRoomless => {
                            info!(target: "evt", "SSE: Motion sensor {} was discovered without a registry room; forwarding for topology routing", motion_id);
                            String::new()
                        }
                        MotionLookup::LockFailed => return Vec::new(),
                        MotionLookup::Unknown => {
                            info!(target: "evt", "SSE: Unknown motion sensor {}, ignoring", motion_id);
                            return Vec::new();
                        }
                    }
                }
            };

            info!(target: "evt", "SSE: Motion sensor {} -> detected={} for room {}", motion_id, motion_detected, room_id);

            vec![HubEvent::Motion {
                hub_key: None,
                room_id,
                sensor_id: motion_id.clone(),
                detected: motion_detected,
            }]
        }

        HueSseEvent::Heartbeat => {
            vec![HubEvent::Heartbeat { hub_key: None }]
        }

        HueSseEvent::Disconnected(reason) => {
            vec![HubEvent::Disconnected {
                hub_key: None,
                reason,
            }]
        }
    }
}

/// Translate a raw Hue SSE event into zero or more hub-agnostic events.
///
/// For button events: uses the shared `resolve_button_event` pattern —
/// registry lookup → discovery if unknown → map to `ButtonAction` → emit.
///
/// If `on_unknown_device` is provided, it is called when a button_id or
/// motion_id is not found in the registry. The callback receives the
/// resource_id and resource_type ("button" or "motion") so it can call
/// `discover_device()`. After the callback returns, the registry is re-checked.
pub fn translate_sse_event(
    registry: &Arc<Mutex<HubDeviceRegistry>>,
    event: crate::sse::HueSseEvent,
    on_activity: Option<&dyn Fn()>,
    on_unknown_button: Option<&dyn Fn(&str)>,
    on_unknown_motion: Option<&dyn Fn(&str)>,
) -> Vec<HubEvent> {
    let adapter = on_unknown_button.map(|cb| {
        move |evt: &RawButtonEvent| {
            cb(evt.button_id);
        }
    });

    translate_sse_event_with_hooks(
        registry,
        event,
        on_activity,
        adapter.as_ref().map(|f| f as &dyn Fn(&RawButtonEvent)),
        on_unknown_motion,
    )
}

/// Spawn a translator thread that converts raw SSE events to hub-agnostic `HubEvent`s.
///
/// Takes a `Receiver<HueSseEvent>` (from the platform's SSE reader) and spawns
/// a thread that translates each event via `translate_sse_event`. Used by both
/// lifecycle wrappers and tests.
#[allow(clippy::type_complexity)]
pub fn start_event_translator(
    sse_rx: Receiver<crate::sse::HueSseEvent>,
    registry: Arc<Mutex<HubDeviceRegistry>>,
    shutdown: Arc<AtomicBool>,
    on_activity: Option<Arc<dyn Fn() + Send + Sync>>,
    on_unknown_button: Option<Arc<dyn Fn(&RawButtonEvent) + Send + Sync>>,
    on_unknown_motion: Option<Arc<dyn Fn(&str) + Send + Sync>>,
) -> Receiver<HubEvent> {
    rhythm_os::lifecycle::start_event_translator(
        sse_rx,
        move |event| {
            let activity_ref: Option<&dyn Fn()> =
                on_activity.as_ref().map(|f| f.as_ref() as &dyn Fn());
            let unknown_ref: Option<&dyn Fn(&RawButtonEvent)> = on_unknown_button
                .as_ref()
                .map(|f| f.as_ref() as &dyn Fn(&RawButtonEvent));
            let motion_ref: Option<&dyn Fn(&str)> = on_unknown_motion
                .as_ref()
                .map(|f| f.as_ref() as &dyn Fn(&str));
            translate_sse_event_with_hooks(
                &registry,
                event.clone(),
                activity_ref,
                unknown_ref,
                motion_ref,
            )
        },
        shutdown,
        "hue-evt",
        None, // on_activity is handled inside the translate closure
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rhythm_core::runtime::hub_registry::DeviceType;

    #[test]
    fn known_roomless_motion_forwards_to_topology_without_unknown_discovery() {
        let registry = Arc::new(Mutex::new(HubDeviceRegistry::new()));
        registry
            .lock()
            .unwrap()
            .upsert_device("motion-svc-1", None, &[], DeviceType::Motion);

        let unknown_calls = AtomicUsize::new(0);
        let on_unknown = |_: &str| {
            unknown_calls.fetch_add(1, Ordering::SeqCst);
        };

        let events = translate_sse_event(
            &registry,
            crate::sse::HueSseEvent::MotionEvent {
                motion_id: "motion-svc-1".to_string(),
                motion_detected: true,
            },
            None,
            None,
            Some(&on_unknown),
        );

        assert_eq!(events.len(), 1);
        match &events[0] {
            HubEvent::Motion {
                room_id,
                sensor_id,
                detected,
                ..
            } => {
                assert!(room_id.is_empty());
                assert_eq!(sensor_id, "motion-svc-1");
                assert!(*detected);
            }
            other => panic!("expected motion event, got {:?}", other),
        }
        assert_eq!(unknown_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn unknown_motion_still_uses_discovery_hook_then_routes() {
        let registry = Arc::new(Mutex::new(HubDeviceRegistry::new()));
        let unknown_calls = AtomicUsize::new(0);
        let registry_for_hook = registry.clone();
        let on_unknown = |motion_id: &str| {
            unknown_calls.fetch_add(1, Ordering::SeqCst);
            registry_for_hook.lock().unwrap().upsert_device(
                motion_id,
                Some("room-1"),
                &[],
                DeviceType::Motion,
            );
        };

        let events = translate_sse_event(
            &registry,
            crate::sse::HueSseEvent::MotionEvent {
                motion_id: "motion-svc-1".to_string(),
                motion_detected: true,
            },
            None,
            None,
            Some(&on_unknown),
        );

        assert_eq!(unknown_calls.load(Ordering::SeqCst), 1);
        assert_eq!(events.len(), 1);
        match &events[0] {
            HubEvent::Motion {
                room_id,
                sensor_id,
                detected,
                ..
            } => {
                assert_eq!(room_id, "room-1");
                assert_eq!(sensor_id, "motion-svc-1");
                assert!(*detected);
            }
            other => panic!("expected motion event, got {:?}", other),
        }
    }
}
