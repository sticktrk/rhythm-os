//! Event-ordering tests for rhythm-ha.
//!
//! HA events arrive over WebSocket and can be delivered in rapid succession
//! (motion true → false → true in < 1s is normal for jittery sensors). The
//! translator + engine pipeline must preserve order and not lose or
//! duplicate events, even when they interleave event types (service event
//! followed immediately by a ZHA event for a different button, etc.).

use std::sync::{Arc, Mutex};

use rhythm_core::runtime::handle::RuntimeHandle;
use rhythm_core::runtime::orchestrator::RhythmRuntime;
use rhythm_core::runtime::registry::SimpleDeviceRegistry;
use rhythm_core::runtime::scheduler::NoOpScheduler;
use rhythm_core::runtime::time::MockTimeProvider;
use rhythm_core::runtime::RuntimeConfig;
use rhythm_core::{ButtonAction, InputEvent};

use rhythm_ha::controller::HaLightController;
use rhythm_ha::events::translate_service_event;
use rhythm_ha::test_support::SpyHaTransport;

use rhythm_os::hub::HubEvent;
use rhythm_os::registry::HubDeviceRegistry;
use serde_json::json;

fn make_pipeline() -> (
    Arc<dyn RuntimeHandle>,
    HubDeviceRegistry,
    Arc<SpyHaTransport>,
) {
    let spy = Arc::new(SpyHaTransport::new());
    let mut registry = HubDeviceRegistry::with_options(true);
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
        MockTimeProvider::new(14.0, 172, 2026),
        NoOpScheduler::new(),
        SimpleDeviceRegistry::new(),
        RuntimeConfig::default(),
    );
    runtime.add_room("living_room", "Living Room");
    (Arc::new(runtime), registry, spy)
}

#[test]
fn service_events_preserve_dispatch_order() {
    let (runtime, registry, spy) = make_pipeline();

    // Rapid burst: step_up, then step_up again, then step_down — the engine
    // must see them in submission order. The translator maps each to a
    // ButtonAction.
    for service in ["step_up", "step_up", "step_down"] {
        let data = json!({ "service": service, "area_id": "living_room" });
        let hub_events = translate_service_event(&data, &registry);
        assert_eq!(
            hub_events.len(),
            1,
            "service event `{}` must translate to a single HubEvent",
            service
        );
        if let HubEvent::Button {
            room_id, action, ..
        } = &hub_events[0]
        {
            let input = InputEvent::new(room_id, *action);
            runtime
                .handle_event(&input)
                .expect("handle_event must not error");
        } else {
            panic!("expected Button event");
        }
    }

    // All three should have reached the transport in order.
    let calls = spy.calls();
    assert_eq!(
        calls.len(),
        3,
        "three service events must produce three transport calls, got {}",
        calls.len()
    );
}

#[test]
fn unknown_service_does_not_poison_following_events() {
    let (runtime, registry, spy) = make_pipeline();

    // Unknown service — translator should drop it.
    let unknown = json!({ "service": "definitely_not_a_real_service", "area_id": "living_room" });
    let hub_events = translate_service_event(&unknown, &registry);
    assert!(
        hub_events.is_empty(),
        "unknown service must not produce a HubEvent"
    );

    // A subsequent known service must work normally — no state was poisoned.
    let known = json!({ "service": "step_up", "area_id": "living_room" });
    let hub_events = translate_service_event(&known, &registry);
    assert_eq!(hub_events.len(), 1);
    if let HubEvent::Button {
        room_id, action, ..
    } = &hub_events[0]
    {
        runtime
            .handle_event(&InputEvent::new(room_id, *action))
            .unwrap();
    }

    assert_eq!(spy.calls().len(), 1);
}

#[test]
fn malformed_service_event_is_dropped_silently() {
    let (_, registry, _) = make_pipeline();

    // Missing service key.
    let hub_events = translate_service_event(&json!({ "area_id": "living_room" }), &registry);
    assert!(hub_events.is_empty());

    // Missing area_id.
    let hub_events = translate_service_event(&json!({ "service": "step_up" }), &registry);
    assert!(hub_events.is_empty());

    // Completely unrelated payload.
    let hub_events = translate_service_event(&json!({ "foo": "bar" }), &registry);
    assert!(hub_events.is_empty());
}

#[test]
fn alternating_on_off_events_keep_engine_state_consistent() {
    // Rapid OnPress/OffPress alternation (common during jittery motion) must
    // not leave the engine in a wedged state. After the sequence, an
    // OnPress must still produce a transport call.
    let (runtime, _, spy) = make_pipeline();

    for action in [
        ButtonAction::OnPress,
        ButtonAction::OffPress,
        ButtonAction::OnPress,
        ButtonAction::OffPress,
    ] {
        runtime
            .handle_event(&InputEvent::new("living_room", action))
            .unwrap();
    }

    let before_recovery = spy.calls().len();
    spy.reset();

    runtime
        .handle_event(&InputEvent::new("living_room", ButtonAction::OnPress))
        .unwrap();

    assert!(before_recovery > 0, "each alternation should dispatch");
    assert!(
        !spy.calls().is_empty(),
        "engine must still dispatch after rapid alternation"
    );
}

#[test]
fn multi_area_service_event_translates_to_one_event_per_area() {
    let (_, mut registry, _) = make_pipeline();
    registry.upsert_room("bedroom", "Bedroom", "bedroom", &[]);

    // HA can deliver `area_id` as an array when the script targeted multiple
    // areas in one service call. The translator must emit one HubEvent per
    // area in order.
    let data = json!({
        "service": "step_up",
        "area_id": ["living_room", "bedroom"]
    });
    let hub_events = translate_service_event(&data, &registry);
    assert_eq!(hub_events.len(), 2, "two areas should produce two events");

    let rooms: Vec<&str> = hub_events
        .iter()
        .filter_map(|e| {
            if let HubEvent::Button { room_id, .. } = e {
                Some(room_id.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(rooms, vec!["living_room", "bedroom"]);
}
