//! Top-button (on / reset) flow tests for the Hue dimmer.
//!
//! Hue dimmer button 1 (top) maps:
//! - short press → `Reset` (clears offsets, enables rhythm, turns on)
//! - long press → `OnPress` (plain on, preserves offsets)
//!
//! These cover daily flows that exercise transitions out of soft-off and
//! hard-off back into adaptive lighting.

mod harness;

use harness::*;

#[test]
fn top_short_press_from_hard_off_turns_on_adaptive() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.action("kitchen", "on").unwrap();
    h.action("kitchen", "lights_off").unwrap();
    spy.reset();

    h.action("kitchen", "on").unwrap();

    let calls = spy.turn_on_calls();
    assert_eq!(calls.len(), 1, "on after hard-off should dispatch turn_on");
    let cmd = &calls[0].1;
    assert!(cmd.brightness > 1, "should resume adaptive, not stay at 1%");
    assert!(cmd.brightness <= 100);
    assert_eq!(spy.turn_off_calls().len(), 0);
}

#[test]
fn top_short_press_from_soft_off_resumes_adaptive() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    h.action("kitchen", "on").unwrap();
    h.action("kitchen", "off").unwrap();
    spy.reset();

    h.action("kitchen", "on").unwrap();

    let calls = spy.turn_on_calls();
    assert_eq!(calls.len(), 1, "on after soft-off should dispatch turn_on");
    let cmd = &calls[0].1;
    assert!(
        cmd.brightness > 1,
        "soft-off→on must lift brightness above 1%, got {}",
        cmd.brightness
    );
}

#[test]
fn reset_after_dim_up_clears_offset() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.action("kitchen", "on").unwrap();
    let baseline = spy.turn_on_calls().last().unwrap().1.brightness;

    h.action("kitchen", "dim_up").unwrap();
    let dimmed = spy.turn_on_calls().last().unwrap().1.brightness;
    assert!(
        dimmed >= baseline,
        "dim_up should not lower brightness ({} vs {})",
        dimmed,
        baseline
    );
    spy.reset();

    h.action("kitchen", "reset").unwrap();

    let after_reset = spy.turn_on_calls().last().unwrap().1.brightness;
    assert_eq!(
        after_reset, baseline,
        "reset must restore baseline adaptive brightness"
    );
}
