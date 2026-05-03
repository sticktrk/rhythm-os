//! Motion + button interaction tests.
//!
//! When motion is active, the room is "owned" by motion. A user pressing the
//! bottom button must still soft-off the room without motion bouncing the
//! lights back on, and the top button after motion clears must resume adaptive
//! lighting.

mod harness;

use harness::*;

#[test]
fn off_press_overrides_motion_active_cache() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.action("kitchen", "on").unwrap();
    h.set_motion_active("kitchen");
    h.set_lights_on("kitchen", true);
    spy.reset();

    h.action("kitchen", "off").unwrap();

    assert_eq!(spy.turn_off_calls().len(), 0);
    let calls = spy.turn_on_calls();
    assert_eq!(
        calls.len(),
        1,
        "off press should dispatch one soft-off even when motion is active"
    );
    assert_eq!(
        calls[0].1.brightness, 1,
        "motion must not lift soft-off above 1%"
    );
}

#[test]
fn top_press_after_motion_off_resumes_adaptive() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    // Motion drove lights on, then cleared (motion timeout).
    h.set_motion_active("kitchen");
    h.action("kitchen", "on").unwrap();
    h.set_lights_on("kitchen", false);
    spy.reset();

    h.action("kitchen", "on").unwrap();

    let calls = spy.turn_on_calls();
    assert_eq!(
        calls.len(),
        1,
        "on after motion-off should dispatch turn_on"
    );
    let cmd = &calls[0].1;
    assert!(
        cmd.brightness > 1 && cmd.brightness <= 100,
        "should resume adaptive brightness, got {}",
        cmd.brightness
    );
}
