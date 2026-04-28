//! A periodic tick after `OffPress` must keep the room at 1%, not bounce it
//! back to adaptive brightness.
//!
//! This is the highest-value coverage gap from the morning-bug investigation:
//! periodic runs every 60s, so any silent regression that ignores `soft_off`
//! during periodic would undo every successful soft-off after one tick.

mod harness;

use harness::*;

#[test]
fn periodic_tick_after_off_press_keeps_room_at_one_percent() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.action("kitchen", "on").unwrap();
    h.action("kitchen", "off").unwrap();
    let resolved = h.resolve("kitchen");
    spy.reset();

    let runtime = h.state.lock().unwrap().hub_runtime().unwrap();
    runtime.periodic_tick_room(&resolved, 14.0).unwrap();

    assert_eq!(
        spy.turn_off_calls().len(),
        0,
        "periodic must not hard-off a soft-off room"
    );
    for (_, cmd) in spy.turn_on_calls().iter() {
        assert_eq!(
            cmd.brightness, 1,
            "periodic must preserve 1% while soft-off, got {}",
            cmd.brightness
        );
    }
}

#[test]
fn multiple_periodic_ticks_after_off_press_stay_at_one_percent() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.action("kitchen", "on").unwrap();
    h.action("kitchen", "off").unwrap();
    let resolved = h.resolve("kitchen");
    spy.reset();

    let runtime = h.state.lock().unwrap().hub_runtime().unwrap();
    for hour in [14.0, 14.05, 14.1, 14.2, 14.5] {
        runtime.periodic_tick_room(&resolved, hour).unwrap();
    }

    assert_eq!(spy.turn_off_calls().len(), 0);
    for (_, cmd) in spy.turn_on_calls().iter() {
        assert_eq!(cmd.brightness, 1, "every tick must stay at 1%");
    }
}
