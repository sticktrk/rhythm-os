//! Hue SSE event translation.
//!
//! Converts raw Hue SSE events into hub-agnostic `HubEvent`s.
//! Button events use the shared `resolve_button_event` pattern from
//! rhythm-os; motion events are resolved via the device registry.

use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use log::info;

use rhythm_os::hub::HubEvent;
use rhythm_os::registry::HubDeviceRegistry;

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
    use crate::sse::HueSseEvent;
    use rhythm_os::button_resolve::{resolve_button_event, RawButtonEvent};
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

            // Adapt the old-style Fn(&str) callback to the new Fn(&RawButtonEvent)
            let adapter = on_unknown_button.map(|cb| {
                move |evt: &RawButtonEvent| {
                    cb(evt.button_id);
                }
            });

            resolve_button_event(
                registry,
                &raw,
                adapter.as_ref().map(|f| f as &dyn Fn(&RawButtonEvent)),
                &map_hue_button_str,
            )
        }

        HueSseEvent::MotionEvent {
            motion_id,
            motion_detected,
        } => {
            if let Some(cb) = on_activity {
                cb();
            }

            // First lookup — motion sensors are now devices with DeviceType::Motion
            let room_id = {
                let reg = match registry.lock() {
                    Ok(r) => r,
                    Err(_) => return Vec::new(),
                };
                reg.get_room_for_motion_sensor(&motion_id)
            };

            // If not found, try on-demand discovery then re-check
            let room_id = match room_id {
                Some(id) => id,
                None => {
                    if let Some(cb) = on_unknown_motion {
                        cb(&motion_id);
                    }
                    let reg = match registry.lock() {
                        Ok(r) => r,
                        Err(_) => return Vec::new(),
                    };
                    match reg.get_room_for_motion_sensor(&motion_id) {
                        Some(id) => id,
                        None => {
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

/// Spawn a translator thread that converts raw SSE events to hub-agnostic `HubEvent`s.
///
/// Takes a `Receiver<HueSseEvent>` (from the platform's SSE reader) and spawns
/// a thread that translates each event via `translate_sse_event`. Used by both
/// embedded and desktop lifecycle modules.
#[allow(clippy::type_complexity)]
pub fn start_event_translator(
    sse_rx: Receiver<crate::sse::HueSseEvent>,
    registry: Arc<Mutex<HubDeviceRegistry>>,
    shutdown: Arc<AtomicBool>,
    on_activity: Option<Arc<dyn Fn() + Send + Sync>>,
    on_unknown_button: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    on_unknown_motion: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    stack_size: Option<usize>,
) -> Receiver<HubEvent> {
    rhythm_os::lifecycle::start_event_translator(
        sse_rx,
        move |event| {
            let activity_ref: Option<&dyn Fn()> =
                on_activity.as_ref().map(|f| f.as_ref() as &dyn Fn());
            let unknown_ref: Option<&dyn Fn(&str)> = on_unknown_button
                .as_ref()
                .map(|f| f.as_ref() as &dyn Fn(&str));
            let motion_ref: Option<&dyn Fn(&str)> = on_unknown_motion
                .as_ref()
                .map(|f| f.as_ref() as &dyn Fn(&str));
            translate_sse_event(
                &registry,
                event.clone(),
                activity_ref,
                unknown_ref,
                motion_ref,
            )
        },
        shutdown,
        "hue-evt",
        stack_size,
        None, // on_activity is handled inside the translate closure
    )
}
