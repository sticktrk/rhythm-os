use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_core::ButtonAction;
use rhythm_hue::hue_lifecycle::start_event_translator;
use rhythm_hue::registry::HueDeviceRegistry;
use rhythm_hue::sse::HueSseEvent;
use rhythm_os::button_resolve::RawButtonEvent;
use rhythm_os::hub::HubEvent;

fn make_registry() -> Arc<Mutex<HueDeviceRegistry>> {
    let mut reg = HueDeviceRegistry::new();
    reg.upsert_room("living_room", "Living Room", "grouped-light-1", &[]);
    Arc::new(Mutex::new(reg))
}

#[test]
fn translator_uses_raw_button_hook_for_unknown_button() {
    let registry = make_registry();
    let (sse_tx, sse_rx) = std::sync::mpsc::channel();
    let (hook_tx, hook_rx) = std::sync::mpsc::channel();
    let hook_registry = registry.clone();

    let on_unknown_button: Arc<dyn Fn(&RawButtonEvent) + Send + Sync> =
        Arc::new(move |evt: &RawButtonEvent| {
            hook_tx
                .send((
                    evt.button_id.to_string(),
                    evt.fallback_control_id,
                    evt.device_hint.map(str::to_string),
                ))
                .unwrap();

            hook_registry.lock().unwrap().upsert_device(
                "hue-device-1",
                "living_room",
                &[(evt.button_id.to_string(), 1)],
                DeviceType::Button,
            );
        });

    let out_rx = start_event_translator(
        sse_rx,
        registry,
        Arc::new(AtomicBool::new(false)),
        None,
        Some(on_unknown_button),
        None,
        None,
    );

    sse_tx
        .send(HueSseEvent::ButtonEvent {
            button_id: "hue-button-1".to_string(),
            event_type: "initial_press".to_string(),
        })
        .unwrap();

    let hook = hook_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(hook.0, "hue-button-1");
    assert_eq!(hook.1, None);
    assert_eq!(hook.2, None);

    let event = out_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    match event {
        HubEvent::Button {
            room_id,
            action,
            device_id,
            ..
        } => {
            assert_eq!(room_id, "living_room");
            assert_eq!(action, ButtonAction::Reset);
            assert_eq!(device_id.as_deref(), Some("hue-device-1"));
        }
        other => panic!("expected button event, got {:?}", other),
    }
}
