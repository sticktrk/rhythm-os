//! Scenario: a slow hub controller (e.g. a Hue bridge that hangs for 200ms
//! per call) must not starve other rooms attached to the same engine.
//!
//! These scenarios drive `handle_hub_event` directly so they exercise the
//! real button-ingress path that spawns controller work off the event loop.
//! If the controller blocks, later button ingress must still return quickly,
//! subsequent presses must still be applied, and room state must remain sane.

mod harness;

use std::time::{Duration, Instant};

use harness::{rooms_with_lights, TestHarness};
use rhythm_core::ButtonAction;
use rhythm_os::event_loop::{handle_hub_event, MotionTimerState};
use rhythm_os::hub::HubEvent;

fn dispatch_button_event(
    harness: &TestHarness,
    motion: &mut MotionTimerState,
    room_id: &str,
    action: ButtonAction,
    device_id: &str,
) -> Duration {
    let started = Instant::now();
    handle_hub_event(
        &harness.state,
        HubEvent::Button {
            hub_key: Some(harness.hub_key.clone()),
            room_id: room_id.to_string(),
            action,
            device_id: Some(device_id.to_string()),
        },
        motion,
    );
    started.elapsed()
}

fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool, failure: &str) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(predicate(), "{failure}");
}

#[test]
fn slow_controller_does_not_drop_button_events_for_other_rooms() {
    let (rooms, devices) =
        rooms_with_lights(&[("mock-kitchen", "Kitchen"), ("mock-bedroom", "Bedroom")]);

    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();

    // Slow the kitchen room's turn_on to 50ms — fast enough to keep the
    // test snappy but obvious if calls are serialised. Bedroom is fast.
    let kitchen_node = harness.resolve("mock-kitchen");
    let bedroom_node = harness.resolve("mock-bedroom");
    spy.set_turn_on_delay_for_room(&kitchen_node, Duration::from_millis(50));

    let mut motion = MotionTimerState::new();
    let kitchen_ingress = dispatch_button_event(
        &harness,
        &mut motion,
        "mock-kitchen",
        ButtonAction::OnPress,
        "button-kitchen",
    );
    let bedroom_ingress = dispatch_button_event(
        &harness,
        &mut motion,
        "mock-bedroom",
        ButtonAction::OnPress,
        "button-bedroom",
    );

    assert!(
        kitchen_ingress < Duration::from_millis(100),
        "slow kitchen controller must not block first button ingress, got {:?}",
        kitchen_ingress
    );
    assert!(
        bedroom_ingress < Duration::from_millis(100),
        "slow kitchen controller must not block subsequent bedroom ingress, got {:?}",
        bedroom_ingress
    );

    wait_until(
        Duration::from_secs(2),
        || {
            let calls = spy.turn_on_calls();
            let kitchen_on = calls.iter().any(|(room, _)| room == &kitchen_node);
            let bedroom_on = calls.iter().any(|(room, _)| room == &bedroom_node);
            kitchen_on
                && bedroom_on
                && harness.lights_on("mock-kitchen")
                && harness.lights_on("mock-bedroom")
        },
        "both button events should eventually reach the controller and update engine state",
    );
}

#[test]
fn slow_lights_off_does_not_lose_subsequent_on_press() {
    // Hard-off (`lights_off`) is the only action that drives `turn_off` in
    // the engine — `off` is a soft-off implemented as a low-brightness
    // turn_on. Drive a slow turn_off then an on press and verify both
    // reach the controller and the room ends up on.
    let (rooms, devices) = rooms_with_lights(&[("mock-kitchen", "Kitchen")]);

    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();

    let mut motion = MotionTimerState::new();
    spy.set_turn_off_delay(Duration::from_millis(300));

    harness.action("mock-kitchen", "on").unwrap();
    spy.reset();

    let lights_off_ingress = dispatch_button_event(
        &harness,
        &mut motion,
        "mock-kitchen",
        ButtonAction::LightsOff,
        "button-kitchen",
    );
    assert!(
        lights_off_ingress < Duration::from_millis(100),
        "slow lights_off must not block ingress, got {:?}",
        lights_off_ingress
    );

    wait_until(
        Duration::from_secs(2),
        || spy.turn_off_count() == 1 && !harness.lights_on("mock-kitchen"),
        "slow lights_off should still complete through the ingress path",
    );

    let on_ingress = dispatch_button_event(
        &harness,
        &mut motion,
        "mock-kitchen",
        ButtonAction::OnPress,
        "button-kitchen",
    );
    assert!(
        on_ingress < Duration::from_millis(100),
        "follow-up on press must not be blocked after a slow lights_off, got {:?}",
        on_ingress
    );

    wait_until(
        Duration::from_secs(2),
        || {
            spy.turn_off_count() == 1
                && spy.turn_on_count() >= 1
                && harness.lights_on("mock-kitchen")
        },
        "slow lights_off should not lose the subsequent on press",
    );
}

#[test]
fn slow_controller_preserves_room_state_across_actions() {
    // Engine state mutations (e.g. preferences) should remain consistent
    // even when the controller is artificially slow — the engine doesn't
    // require a controller ack before updating its own state.
    let (rooms, devices) = rooms_with_lights(&[("mock-kitchen", "Kitchen")]);

    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();

    spy.set_turn_on_delay(Duration::from_millis(60));

    harness.set_room_preferences("mock-kitchen", Some(true), None, Some(false));
    let mut motion = MotionTimerState::new();
    let ingress = dispatch_button_event(
        &harness,
        &mut motion,
        "mock-kitchen",
        ButtonAction::OnPress,
        "button-kitchen",
    );
    assert!(
        ingress < Duration::from_millis(100),
        "slow turn_on must not block button ingress, got {:?}",
        ingress
    );

    wait_until(
        Duration::from_secs(2),
        || spy.turn_on_count() >= 1 && harness.lights_on("mock-kitchen"),
        "slow turn_on should still reach the controller and update room state",
    );

    let snap = harness.snapshot("mock-kitchen").unwrap();
    assert!(
        snap.rhythm_enabled,
        "rhythm_enabled preference must persist through a slow turn_on"
    );
    assert!(harness.lights_on("mock-kitchen"));
}
