//! Dim-up/dim-down cycle tests for the Hue dimmer's middle buttons.
//!
//! Buttons 2 and 3 short-press map to `UpPress`/`DownPress`, which apply a ±10%
//! brightness offset. These tests pin the clamp behavior at the rails and the
//! lift behavior out of soft-off.

mod harness;

use harness::*;

#[test]
fn dim_up_at_max_caps_at_one_hundred() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.action("kitchen", "on").unwrap();

    for _ in 0..15 {
        h.action("kitchen", "dim_up").unwrap();
    }

    let last = spy.turn_on_calls().last().unwrap().1.brightness;
    assert!(last <= 100, "brightness must not exceed 100, got {}", last);
    assert_eq!(last, 100, "repeated dim_up should saturate at 100");
}

#[test]
fn dim_down_at_min_floors_at_one_percent() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.action("kitchen", "on").unwrap();

    for _ in 0..15 {
        h.action("kitchen", "dim_down").unwrap();
    }

    let last = spy.turn_on_calls().last().unwrap().1.brightness;
    assert!(last >= 1, "dim_down must not underflow, got {}", last);
    assert_eq!(
        spy.turn_off_calls().len(),
        0,
        "dim_down must never hard-off"
    );
}

#[test]
fn dim_up_after_soft_off_lifts_off_one_percent() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.action("kitchen", "on").unwrap();
    h.action("kitchen", "off").unwrap();
    spy.reset();

    h.action("kitchen", "dim_up").unwrap();

    let calls = spy.turn_on_calls();
    assert!(!calls.is_empty(), "dim_up should dispatch turn_on");
    let last = calls.last().unwrap().1.brightness;
    assert!(
        last > 1,
        "dim_up after soft-off must lift above 1%, got {}",
        last
    );
}
