//! Integration flow tests for rhythm-hue.
//!
//! Tests the full pipeline: SSE event → translate → HubEvent → engine → HueLightController → SpyHueTransport.
//! Verifies Hue-specific formatting (grouped_light_id, username, kelvin, fade_ms).

use std::sync::{Arc, Mutex};

use rhythm_core::runtime::handle::RuntimeHandle;
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_core::runtime::orchestrator::RhythmRuntime;
use rhythm_core::runtime::registry::SimpleDeviceRegistry;
use rhythm_core::runtime::scheduler::NoOpScheduler;
use rhythm_core::runtime::time::MockTimeProvider;
use rhythm_core::runtime::RuntimeConfig;
use rhythm_core::InputEvent;

use rhythm_hue::controller::HueLightController;
use rhythm_hue::hue_lifecycle::translate_sse_event;
use rhythm_hue::sse::HueSseEvent;
use rhythm_hue::test_support::{HueTransportCall, SpyHueTransport};

use rhythm_os::hub::HubEvent;
use rhythm_os::registry::HubDeviceRegistry;

/// Create a testable Hue pipeline: spy transport + controller + runtime + registry.
fn make_hue_pipeline() -> (
    Arc<dyn RuntimeHandle>,
    Arc<Mutex<HubDeviceRegistry>>,
    Arc<SpyHueTransport>,
) {
    let spy = Arc::new(SpyHueTransport::new());
    let registry = Arc::new(Mutex::new(HubDeviceRegistry::new()));

    // Room with grouped_light_id
    registry
        .lock()
        .unwrap()
        .upsert_room("room1", "Living Room", "gl-room1", &[]);

    // Button device: btn-1 has control_id=1
    registry.lock().unwrap().upsert_device(
        "switch-1",
        "room1",
        &[("btn-1".to_string(), 1)],
        DeviceType::Button,
    );

    let controller = HueLightController::new(spy.clone(), "testuser".to_string(), registry.clone());

    let runtime = RhythmRuntime::new(
        controller,
        MockTimeProvider::new(14.0, 172, 2026),
        NoOpScheduler::new(),
        SimpleDeviceRegistry::new(),
        RuntimeConfig::default(),
    );
    runtime.add_room("room1", "Living Room");

    (Arc::new(runtime), registry, spy)
}

// ============================================================================
// Tests
// ============================================================================

#[test]
fn sse_button_press_produces_correct_transport_call() {
    let (runtime, registry, spy) = make_hue_pipeline();

    // Simulate SSE button event → translate → HubEvent
    let sse_event = HueSseEvent::ButtonEvent {
        button_id: "btn-1".to_string(),
        event_type: "initial_press".to_string(),
    };

    let hub_events = translate_sse_event(&registry, sse_event, None, None, None);

    assert_eq!(
        hub_events.len(),
        1,
        "Expected one HubEvent from button press"
    );

    match &hub_events[0] {
        HubEvent::Button {
            room_id, action, ..
        } => {
            assert_eq!(room_id, "room1");
            let input = InputEvent::new(room_id, *action);
            runtime.handle_event(&input).unwrap();
        }
        other => panic!("Expected Button event, got {:?}", other),
    }

    // Verify the spy transport received the correct call
    let calls = spy.set_grouped_light_calls();
    assert_eq!(calls.len(), 1, "Expected one transport call");

    match &calls[0] {
        HueTransportCall::SetGroupedLight {
            grouped_light_id,
            on,
            brightness,
            kelvin,
            xy: _,
            fade_ms,
        } => {
            assert_eq!(grouped_light_id, "gl-room1");
            assert!(*on);
            assert!(
                brightness.unwrap() >= 1 && brightness.unwrap() <= 100,
                "brightness {:?} out of range",
                brightness
            );
            assert!(
                kelvin.unwrap() >= 2000 && kelvin.unwrap() <= 6500,
                "kelvin {:?} out of range",
                kelvin
            );
            assert_eq!(*fade_ms, Some(500));
        }
        other => panic!("Expected SetGroupedLight, got {:?}", other),
    }
}

#[test]
fn sse_button_off_press_sends_soft_off() {
    let (runtime, registry, spy) = make_hue_pipeline();

    // First turn on
    let input = InputEvent::new("room1", rhythm_core::ButtonAction::OnPress);
    runtime.handle_event(&input).unwrap();
    spy.reset();

    // Add button 4 (control_id=4 → OffPress on initial_press)
    registry.lock().unwrap().upsert_device(
        "switch-1",
        "room1",
        &[("btn-1".to_string(), 1), ("btn-4".to_string(), 4)],
        DeviceType::Button,
    );

    let sse_event = HueSseEvent::ButtonEvent {
        button_id: "btn-4".to_string(),
        event_type: "initial_press".to_string(),
    };

    let hub_events = translate_sse_event(&registry, sse_event, None, None, None);

    if let Some(HubEvent::Button {
        room_id, action, ..
    }) = hub_events.first()
    {
        let input = InputEvent::new(room_id, *action);
        runtime.handle_event(&input).unwrap();
    }

    let calls = spy.set_grouped_light_calls();
    assert!(!calls.is_empty(), "Expected at least one transport call");
    match &calls[0] {
        HueTransportCall::SetGroupedLight {
            on,
            grouped_light_id,
            brightness,
            ..
        } => {
            assert!(
                *on,
                "Button 4 initial_press should soft-off (on at min brightness)"
            );
            assert_eq!(*brightness, Some(1), "soft-off brightness should be 1%");
            assert_eq!(grouped_light_id, "gl-room1");
        }
        other => panic!("Expected SetGroupedLight, got {:?}", other),
    }
}

#[test]
fn direct_engine_action_uses_correct_grouped_light() {
    let (runtime, _, spy) = make_hue_pipeline();

    let input = InputEvent::new("room1", rhythm_core::ButtonAction::Reset);
    runtime.handle_event(&input).unwrap();

    let calls = spy.set_grouped_light_calls();
    assert_eq!(calls.len(), 1);
    match &calls[0] {
        HueTransportCall::SetGroupedLight {
            grouped_light_id,
            on,
            ..
        } => {
            assert_eq!(grouped_light_id, "gl-room1");
            assert!(*on);
        }
        _ => unreachable!(),
    }
}

#[test]
fn unknown_button_produces_unroutable_event() {
    let (_runtime, registry, _spy) = make_hue_pipeline();

    let sse_event = HueSseEvent::ButtonEvent {
        button_id: "unknown-btn".to_string(),
        event_type: "initial_press".to_string(),
    };

    let hub_events = translate_sse_event(&registry, sse_event, None, None, None);

    assert_eq!(
        hub_events.len(),
        1,
        "Unknown button should produce one UnroutableButton event"
    );
    match &hub_events[0] {
        HubEvent::UnroutableButton {
            device_id,
            button_id,
            ..
        } => {
            assert_eq!(button_id, "unknown-btn");
            assert!(
                device_id.is_none(),
                "Unknown button should have no device_id"
            );
        }
        other => panic!("Expected UnroutableButton, got {:?}", other),
    }
}

#[test]
fn heartbeat_produces_no_transport_call() {
    let (_runtime, registry, spy) = make_hue_pipeline();

    let sse_event = HueSseEvent::Heartbeat;
    let hub_events = translate_sse_event(&registry, sse_event, None, None, None);

    for event in &hub_events {
        match event {
            HubEvent::Heartbeat { .. } => {}
            other => panic!("Unexpected event from heartbeat: {:?}", other),
        }
    }

    assert_eq!(spy.set_grouped_light_count(), 0);
}
