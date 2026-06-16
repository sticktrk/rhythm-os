//! `Stop` after a hold must not produce surprise dispatches.
//!
//! Hue dimmer hold-and-release sequences emit a `Stop` event after the user
//! lets go of buttons 2/3. The engine documents this as ignored — pin it so a
//! future state machine refactor doesn't accidentally turn it into another
//! adaptive turn_on.

mod harness;

use harness::*;

use rhythm_core::runtime::events::{ButtonAction, InputEvent};

#[test]
fn stop_after_step_up_dispatches_nothing() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.action("kitchen", "on").unwrap();
    h.action("kitchen", "step_up").unwrap();
    spy.reset();

    let runtime = h.state.lock().unwrap().hub_runtime().unwrap();
    let resolved = h.resolve("kitchen");
    let event = InputEvent::new(&resolved, ButtonAction::Stop);
    runtime.handle_event(&event).unwrap();

    assert_eq!(spy.turn_on_count(), 0, "Stop must not turn_on");
    assert_eq!(spy.turn_off_calls().len(), 0, "Stop must not turn_off");
}

#[test]
fn stop_after_step_down_dispatches_nothing() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.action("kitchen", "on").unwrap();
    h.action("kitchen", "step_down").unwrap();
    spy.reset();

    let runtime = h.state.lock().unwrap().hub_runtime().unwrap();
    let resolved = h.resolve("kitchen");
    let event = InputEvent::new(&resolved, ButtonAction::Stop);
    runtime.handle_event(&event).unwrap();

    assert_eq!(spy.turn_on_count(), 0);
    assert_eq!(spy.turn_off_calls().len(), 0);
}
