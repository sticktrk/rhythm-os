//! Buttons on a disabled room.
//!
//! `disabled` is documented in `primitives.rs::test_rhythm_on_disabled_room` as
//! "disabled doesn't block manual actions" — periodic skips disabled rooms but
//! a physical button press should still dispatch. This file pins that contract
//! so a refactor can't silently flip it (a flip would either ghost the button
//! or expose periodic to disabled rooms).

mod harness;

use harness::*;

#[test]
fn disabled_room_off_press_still_soft_offs() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.action("kitchen", "on").unwrap();
    h.set_room_preferences("kitchen", None, Some(true), None);
    spy.reset();

    h.action("kitchen", "off").unwrap();

    let calls = spy.turn_on_calls();
    assert_eq!(spy.turn_off_calls().len(), 0);
    assert_eq!(
        calls.len(),
        1,
        "manual off press must dispatch on a disabled room"
    );
    assert_eq!(calls[0].1.brightness, 1, "still soft-off at 1%");
}

#[test]
fn disabled_room_on_press_still_turns_on_adaptive() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.set_room_preferences("kitchen", None, Some(true), None);
    spy.reset();

    h.action("kitchen", "on").unwrap();

    let calls = spy.turn_on_calls();
    assert_eq!(
        calls.len(),
        1,
        "manual on press must dispatch on a disabled room"
    );
    assert!(
        calls[0].1.brightness > 1,
        "manual on must lift above 1%, got {}",
        calls[0].1.brightness
    );
}
