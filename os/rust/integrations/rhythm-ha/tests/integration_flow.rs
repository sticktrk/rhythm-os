//! Integration flow tests for rhythm-ha.
//!
//! Tests the full pipeline: HA service event → translate → HubEvent → engine → HaLightController → SpyHaTransport.
//! Verifies HA-specific formatting (area_id, brightness_pct, color_temp_kelvin, transition).

use std::sync::{Arc, Mutex};

use rhythm_core::runtime::handle::RuntimeHandle;
use rhythm_core::runtime::orchestrator::RhythmRuntime;
use rhythm_core::runtime::registry::SimpleDeviceRegistry;
use rhythm_core::runtime::scheduler::NoOpScheduler;
use rhythm_core::runtime::time::MockTimeProvider;
use rhythm_core::runtime::RuntimeConfig;
use rhythm_core::InputEvent;

use rhythm_ha::controller::HaLightController;
use rhythm_ha::events::translate_service_event;
use rhythm_ha::test_support::SpyHaTransport;

use rhythm_os::hub::HubEvent;
use rhythm_os::registry::HubDeviceRegistry;

/// Create a testable HA pipeline: spy transport + controller + runtime + registry.
fn make_ha_pipeline() -> (
    Arc<dyn RuntimeHandle>,
    HubDeviceRegistry,
    Arc<SpyHaTransport>,
) {
    let spy = Arc::new(SpyHaTransport::new());
    let mut registry = HubDeviceRegistry::with_options(true);

    // Room (for HA, grouped_light_id defaults to room_id)
    registry.upsert_room("living_room", "Living Room", "living_room", &[]);

    let controller_registry = Arc::new(Mutex::new(HubDeviceRegistry::with_options(true)));
    controller_registry.lock().unwrap().upsert_room(
        "living_room",
        "Living Room",
        "living_room",
        &[],
    );

    let controller = HaLightController::new(spy.clone(), controller_registry);

    let runtime = RhythmRuntime::new(
        Arc::new(controller),
        MockTimeProvider::new(14.0, 172, 2026), // 2 PM, June 21
        NoOpScheduler::new(),
        SimpleDeviceRegistry::new(),
        RuntimeConfig::default(),
    );
    runtime.add_room("living_room", "Living Room");

    (Arc::new(runtime), registry, spy)
}

// ============================================================================
// Tests
// ============================================================================

#[test]
fn service_event_rhythm_on_produces_correct_transport_call() {
    let (runtime, registry, spy) = make_ha_pipeline();

    // Simulate HA service event
    let event_data = serde_json::json!({
        "service": "rhythm_on",
        "area_id": "living_room",
    });

    let hub_events = translate_service_event(&event_data, &registry);
    assert_eq!(hub_events.len(), 1);

    match &hub_events[0] {
        HubEvent::Button {
            room_id, action, ..
        } => {
            assert_eq!(room_id, "living_room");
            let input = InputEvent::new(room_id, *action);
            runtime.handle_event(&input).unwrap();
        }
        other => panic!("Expected Button event, got {:?}", other),
    }

    // rhythm_on enables adaptive lighting but doesn't immediately turn on
    // It just sets the rhythm_enabled flag — no transport call expected
    assert_eq!(
        spy.call_count(),
        0,
        "rhythm_on should not produce a transport call"
    );
}

#[test]
fn service_event_reset_sends_turn_on() {
    let (runtime, registry, spy) = make_ha_pipeline();

    let event_data = serde_json::json!({
        "service": "reset",
        "area_id": "living_room",
    });

    let hub_events = translate_service_event(&event_data, &registry);
    assert_eq!(hub_events.len(), 1);

    match &hub_events[0] {
        HubEvent::Button {
            room_id, action, ..
        } => {
            let input = InputEvent::new(room_id, *action);
            runtime.handle_event(&input).unwrap();
        }
        _ => panic!("Expected Button event"),
    }

    let calls = spy.calls_for_service("turn_on");
    assert_eq!(calls.len(), 1, "reset should dispatch turn_on");

    let data = &calls[0].data;
    assert_eq!(
        data["area_id"], "living_room",
        "Should target living_room area"
    );

    let brightness = data["brightness_pct"].as_u64().unwrap();
    assert!(
        (1..=100).contains(&brightness),
        "brightness_pct {} out of range",
        brightness
    );

    let kelvin = data["color_temp_kelvin"].as_u64().unwrap();
    assert!(
        (2000..=6500).contains(&kelvin),
        "color_temp_kelvin {} out of range",
        kelvin
    );

    // Transition should be included (command.transition_ms from curve, default 500 → 0.5s)
    let transition = data["transition"].as_f64().unwrap();
    assert!(
        transition > 0.0,
        "transition should be > 0, got {}",
        transition
    );
}

#[test]
fn service_event_lights_off_sends_turn_off() {
    let (runtime, registry, spy) = make_ha_pipeline();

    // Turn on first
    let input = InputEvent::new("living_room", rhythm_core::ButtonAction::OnPress);
    runtime.handle_event(&input).unwrap();
    spy.reset();

    let event_data = serde_json::json!({
        "service": "lights_off",
        "area_id": "living_room",
    });

    let hub_events = translate_service_event(&event_data, &registry);
    if let Some(HubEvent::Button {
        room_id, action, ..
    }) = hub_events.first()
    {
        let input = InputEvent::new(room_id, *action);
        runtime.handle_event(&input).unwrap();
    }

    let calls = spy.calls_for_service("turn_off");
    assert_eq!(calls.len(), 1, "lights_off should dispatch turn_off");
    assert_eq!(calls[0].data["area_id"], "living_room");
}

#[test]
fn unknown_service_produces_no_event() {
    let (_runtime, registry, _spy) = make_ha_pipeline();

    let event_data = serde_json::json!({
        "service": "nonexistent_service",
        "area_id": "living_room",
    });

    let hub_events = translate_service_event(&event_data, &registry);
    assert!(
        hub_events.is_empty(),
        "Unknown service should produce no events"
    );
}

#[test]
fn missing_area_id_produces_no_event() {
    let (_runtime, registry, _spy) = make_ha_pipeline();

    let event_data = serde_json::json!({
        "service": "reset",
    });

    let hub_events = translate_service_event(&event_data, &registry);
    assert!(
        hub_events.is_empty(),
        "Missing area_id should produce no events"
    );
}

#[test]
fn step_up_sends_turn_on_with_higher_brightness() {
    let (runtime, registry, spy) = make_ha_pipeline();

    // Turn on to get baseline
    let input = InputEvent::new("living_room", rhythm_core::ButtonAction::OnPress);
    runtime.handle_event(&input).unwrap();
    let baseline = spy.calls_for_service("turn_on")[0].data["brightness_pct"]
        .as_u64()
        .unwrap();
    spy.reset();

    // Step up via service event
    let event_data = serde_json::json!({
        "service": "step_up",
        "area_id": "living_room",
    });

    let hub_events = translate_service_event(&event_data, &registry);
    if let Some(HubEvent::Button {
        room_id, action, ..
    }) = hub_events.first()
    {
        let input = InputEvent::new(room_id, *action);
        runtime.handle_event(&input).unwrap();
    }

    let calls = spy.calls_for_service("turn_on");
    assert!(!calls.is_empty(), "step_up should dispatch turn_on");
    let stepped = calls.last().unwrap().data["brightness_pct"]
        .as_u64()
        .unwrap();
    assert!(
        stepped >= baseline,
        "step_up brightness {} should be >= baseline {}",
        stepped,
        baseline
    );
}
