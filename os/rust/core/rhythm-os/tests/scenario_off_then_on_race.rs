//! Quick OffPress → OnPress (and vice versa) sequences.
//!
//! The morning bundle showed bottom (06:02:33), bottom (:42), top (:46), bottom
//! (:50) — all within 17 seconds. Final state must match the last action and
//! the engine must not leave a stuck legacy soft-off flag from an earlier press.

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
fn on_then_off_ends_hard_off() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    h.action("kitchen", "off").unwrap();
    h.action("kitchen", "on").unwrap();
    h.action("kitchen", "off").unwrap();

    assert!(
        matches!(
            spy.calls().last(),
            Some(rhythm_core::spy_controller::SpyCall::TurnOff { .. })
        ),
        "last command must be hard-off"
    );

    let snap = h.snapshot("kitchen").unwrap();
    assert!(!snap.soft_off);
    assert!(snap.hard_off);
}

#[test]
fn morning_bundle_replay_off_off_on_off_ends_hard_off() {
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

    assert!(
        matches!(
            spy.calls().last(),
            Some(rhythm_core::spy_controller::SpyCall::TurnOff { .. })
        ),
        "last command must be hard-off (sequence: off off reset off)"
    );

    let snap = h.snapshot("master").unwrap();
    assert!(!snap.soft_off, "must not end legacy soft-off");
    assert!(snap.hard_off, "must end hard-off");
}
