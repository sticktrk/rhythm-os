//! Disabling rhythm must not break the bottom button.
//!
//! `rhythm_off` excludes a room from periodic ticks, but a manual press should
//! still operate normally. Easy to break if anyone gates dispatch on
//! `rhythm_enabled`.

mod harness;

use harness::*;

#[test]
fn off_press_after_rhythm_off_still_dims_to_one_percent() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    h.action("kitchen", "on").unwrap();
    h.action("kitchen", "rhythm_off").unwrap();
    spy.reset();

    h.action("kitchen", "off").unwrap();

    let calls = spy.turn_on_calls();
    assert_eq!(spy.turn_off_calls().len(), 0);
    assert_eq!(
        calls.len(),
        1,
        "off press must still dispatch with rhythm disabled"
    );
    assert_eq!(calls[0].1.brightness, 1);
}

#[test]
fn on_press_after_rhythm_off_still_turns_on_adaptive() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.action("kitchen", "rhythm_off").unwrap();
    spy.reset();

    h.action("kitchen", "on").unwrap();

    let calls = spy.turn_on_calls();
    assert!(
        !calls.is_empty(),
        "on press must dispatch even with rhythm off"
    );
    let cmd = &calls.last().unwrap().1;
    assert!(
        cmd.brightness > 1,
        "manual on must lift above 1% even with rhythm off, got {}",
        cmd.brightness
    );
}
