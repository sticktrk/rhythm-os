//! Sleep-mode button interaction tests.
//!
//! Sleep mode swaps the active light profile to `sleep` and turns lights off
//! (`SleepOn`) or back to rhythm and on (`SleepOff`). Button presses inside
//! sleep mode must still respect their non-powersave semantics — bottom =
//! soft-off, top = adaptive on — without leaking state across the mode toggle.

mod harness;

use harness::*;

#[test]
fn sleep_on_then_bottom_button_dims_to_one_percent() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    h.action("kitchen", "on").unwrap();
    h.action("kitchen", "sleep_on").unwrap();
    spy.reset();

    h.action("kitchen", "off").unwrap();

    assert_eq!(
        spy.turn_off_calls().len(),
        0,
        "soft-off in sleep mode must not hard-off"
    );
    let calls = spy.turn_on_calls();
    assert_eq!(
        calls.len(),
        1,
        "off press in sleep mode should dispatch turn_on"
    );
    assert_eq!(
        calls[0].1.brightness, 1,
        "sleep-mode soft-off brightness must be 1%"
    );
}

#[test]
fn sleep_cycle_preserves_button_behavior() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    h.action("kitchen", "on").unwrap();

    h.action("kitchen", "sleep_on").unwrap();
    h.action("kitchen", "off").unwrap();
    let after_sleep_off = spy.turn_on_calls().last().unwrap().1.brightness;
    assert_eq!(after_sleep_off, 1, "off in sleep should still be 1%");

    h.action("kitchen", "sleep_off").unwrap();
    spy.reset();

    h.action("kitchen", "on").unwrap();
    let calls = spy.turn_on_calls();
    assert!(!calls.is_empty(), "on after sleep_off should dispatch");
    let cmd = &calls.last().unwrap().1;
    assert!(
        cmd.brightness > 1,
        "post-sleep on must resume adaptive (>1%), got {}",
        cmd.brightness
    );
}
