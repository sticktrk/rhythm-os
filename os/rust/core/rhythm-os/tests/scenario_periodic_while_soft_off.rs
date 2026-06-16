//! A periodic tick after `OffPress` must leave the hard-off room alone,
//! not bounce it back to adaptive brightness.
//!
//! This protects the morning-bug path: periodic runs every 60s, so any
//! regression that treats hard-off as adaptive would undo an off press.

mod harness;

use harness::*;

#[test]
fn periodic_tick_after_off_press_keeps_room_hard_off() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    h.action("kitchen", "on").unwrap();
    h.action("kitchen", "off").unwrap();
    let resolved = h.resolve("kitchen");
    spy.reset();

    let runtime = h.state.lock().unwrap().hub_runtime().unwrap();
    runtime.periodic_tick_room(&resolved, 14.0).unwrap();

    assert_eq!(
        spy.turn_off_calls().len(),
        0,
        "periodic must not repeat hard-off for an already off room"
    );
    assert!(
        spy.turn_on_calls().is_empty(),
        "periodic must not relight a hard-off room"
    );
    let snap = h.snapshot("kitchen").unwrap();
    assert!(!snap.soft_off);
    assert!(snap.hard_off);
}

#[test]
fn multiple_periodic_ticks_after_off_press_stay_hard_off() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    h.action("kitchen", "on").unwrap();
    h.action("kitchen", "off").unwrap();
    let resolved = h.resolve("kitchen");
    spy.reset();

    let runtime = h.state.lock().unwrap().hub_runtime().unwrap();
    for hour in [14.0, 14.05, 14.1, 14.2, 14.5] {
        runtime.periodic_tick_room(&resolved, hour).unwrap();
    }

    assert_eq!(spy.turn_off_calls().len(), 0);
    assert!(spy.turn_on_calls().is_empty());
    let snap = h.snapshot("kitchen").unwrap();
    assert!(!snap.soft_off);
    assert!(snap.hard_off);
}
