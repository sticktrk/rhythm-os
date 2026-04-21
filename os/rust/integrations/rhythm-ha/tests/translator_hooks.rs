use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_core::ButtonAction;
use rhythm_ha::ha_lifecycle::{start_event_translator, HaWsEvent};
use rhythm_ha::registry::HaDeviceRegistry;
use rhythm_os::button_resolve::RawButtonEvent;
use rhythm_os::hub::HubEvent;

fn make_registry() -> Arc<Mutex<HaDeviceRegistry>> {
    let mut reg = HaDeviceRegistry::with_options(true);
    reg.upsert_room("living_room", "Living Room", "living_room", &[]);
    Arc::new(Mutex::new(reg))
}

#[test]
fn translator_uses_raw_button_hook_for_unknown_hue_event() {
    let registry = make_registry();
    let (ws_tx, ws_rx) = std::sync::mpsc::channel();
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
                evt.device_hint
                    .expect("HA hue_event should include device_hint"),
                "living_room",
                &[(
                    evt.button_id.to_string(),
                    evt.fallback_control_id.unwrap_or(1),
                )],
                DeviceType::Button,
            );
        });

    let out_rx = start_event_translator(
        ws_rx,
        registry,
        Arc::new(AtomicBool::new(false)),
        None,
        Some(on_unknown_button),
        None,
    );

    ws_tx
        .send(HaWsEvent::ServiceEvent {
            event_type: "hue_event".to_string(),
            data: serde_json::json!({
                "id": "ha-hue-button",
                "device_id": "ha-device-1",
                "type": "initial_press",
                "subtype": 2
            }),
        })
        .unwrap();

    let hook = hook_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(hook.0, "ha-hue-button:2");
    assert_eq!(hook.1, Some(2));
    assert_eq!(hook.2.as_deref(), Some("ha-device-1"));

    let event = out_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    match event {
        HubEvent::Button {
            room_id,
            action,
            device_id,
            ..
        } => {
            assert_eq!(room_id, "living_room");
            assert_eq!(action, ButtonAction::UpPress);
            assert_eq!(device_id.as_deref(), Some("ha-device-1"));
        }
        other => panic!("expected button event, got {:?}", other),
    }
}

#[test]
fn translator_uses_motion_hook_for_unknown_state_changed_motion() {
    let registry = make_registry();
    let (ws_tx, ws_rx) = std::sync::mpsc::channel();
    let hook_registry = registry.clone();

    let on_unknown_motion: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |sensor_id: &str| {
        hook_registry.lock().unwrap().upsert_device(
            sensor_id,
            "living_room",
            &[],
            DeviceType::Motion,
        );
    });

    let out_rx = start_event_translator(
        ws_rx,
        registry,
        Arc::new(AtomicBool::new(false)),
        None,
        None,
        Some(on_unknown_motion),
    );

    ws_tx
        .send(HaWsEvent::ServiceEvent {
            event_type: "state_changed".to_string(),
            data: serde_json::json!({
                "entity_id": "binary_sensor.living_room_motion",
                "new_state": {
                    "state": "on",
                    "attributes": { "device_class": "motion" }
                },
                "old_state": { "state": "off" }
            }),
        })
        .unwrap();

    let event = out_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    match event {
        HubEvent::Motion {
            room_id,
            sensor_id,
            detected,
            ..
        } => {
            assert_eq!(room_id, "living_room");
            assert_eq!(sensor_id, "binary_sensor.living_room_motion");
            assert!(detected);
        }
        other => panic!("expected motion event, got {:?}", other),
    }
}
