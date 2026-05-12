//! Quick OffPress → OnPress (and vice versa) sequences.
//!
//! The morning bundle showed bottom (06:02:33), bottom (:42), top (:46), bottom
//! (:50) — all within 17 seconds. Final state must match the last action and
//! the engine must not leave a stuck soft-off flag from an earlier press.

mod harness;

use harness::*;

#[test]
fn off_then_on_ends_active_with_adaptive_brightness() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    h.action("kitchen", "on").unwrap();
    h.action("kitchen", "off").unwrap();
    h.action("kitchen", "on").unwrap();

    let last = spy.turn_on_calls().last().unwrap().1.brightness;
    assert!(last > 1, "final on must lift above 1%, got {}", last);

    let snap = h.snapshot("kitchen").unwrap();
    assert!(!snap.soft_off, "soft_off must clear after on");
    assert!(!snap.hard_off);
}

#[test]
fn on_then_off_ends_soft_off_at_one_percent() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    h.action("kitchen", "off").unwrap();
    h.action("kitchen", "on").unwrap();
    h.action("kitchen", "off").unwrap();

    let last = spy.turn_on_calls().last().unwrap().1.brightness;
    assert_eq!(last, 1);
    assert_eq!(spy.turn_off_calls().len(), 0);

    let snap = h.snapshot("kitchen").unwrap();
    assert!(snap.soft_off);
    assert!(!snap.hard_off);
}

#[test]
fn morning_bundle_replay_off_off_on_off_ends_soft_off() {
    // Mirrors the actual Hue dimmer sequence captured this morning:
    // OffPress, OffPress, Reset (top), OffPress.
    let (rooms, devices) = rooms_with_lights(&[("master", "Master")]);
    let (h, spy) = TestHarness::with_spy_controller_at(6.04, 118);
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    h.action("master", "on").unwrap();
    spy.reset();

    h.action("master", "off").unwrap();
    h.action("master", "off").unwrap();
    h.action("master", "reset").unwrap();
    h.action("master", "off").unwrap();

    let calls = spy.turn_on_calls();
    assert_eq!(spy.turn_off_calls().len(), 0);
    assert_eq!(
        calls.last().unwrap().1.brightness,
        1,
        "last command must be 1% (sequence: off off reset off)"
    );

    let snap = h.snapshot("master").unwrap();
    assert!(snap.soft_off, "must end soft-off");
    assert!(!snap.hard_off);
}
