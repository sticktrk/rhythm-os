//! Bottom-button (off) flow tests for the Hue dimmer.
//!
//! Hue dimmer button 4 (bottom) maps:
//! - short press → `OffPress` → soft-off (turn_on at 1% brightness, soft_off=true)
//! - long press → `LightsOff` → hard turn_off (hard_off=true)
//!
//! These scenarios cover daily-use combinations the basic
//! `scenario_integration_flow` tests don't reach: repeated presses, presses when
//! the cached power state diverges from intent, and recovery between hard and
//! soft off.

mod harness;

use harness::*;

#[test]
fn bottom_short_press_from_on_dims_to_one_percent() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.action("kitchen", "on").unwrap();
    spy.reset();

    h.action("kitchen", "off").unwrap();

    assert_eq!(spy.turn_off_calls().len(), 0, "soft-off must not hard-off");
    let calls = spy.turn_on_calls();
    assert_eq!(calls.len(), 1, "soft-off should dispatch one turn_on");
    assert_eq!(calls[0].1.brightness, 1, "soft-off brightness must be 1%");
}

#[test]
fn bottom_short_press_twice_stays_at_one_percent() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.action("kitchen", "on").unwrap();
    spy.reset();

    h.action("kitchen", "off").unwrap();
    h.action("kitchen", "off").unwrap();

    assert_eq!(spy.turn_off_calls().len(), 0, "no hard-off across presses");
    let calls = spy.turn_on_calls();
    assert!(calls.len() >= 2, "each off press should dispatch turn_on");
    for (_, cmd) in calls.iter() {
        assert_eq!(cmd.brightness, 1, "every soft-off press must be 1%");
    }
}

#[test]
fn bottom_short_press_when_externally_off_still_soft_offs() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    // Cache says lights are off (e.g. someone flipped the switch externally),
    // but the user still presses the bottom button.
    h.set_lights_on("kitchen", false);
    spy.reset();

    h.action("kitchen", "off").unwrap();

    assert_eq!(spy.turn_off_calls().len(), 0);
    let calls = spy.turn_on_calls();
    assert_eq!(
        calls.len(),
        1,
        "off press must dispatch even if cache says off"
    );
    assert_eq!(calls[0].1.brightness, 1);
}

#[test]
fn bottom_long_press_sends_hard_off() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.action("kitchen", "on").unwrap();
    spy.reset();

    h.action("kitchen", "lights_off").unwrap();

    assert_eq!(spy.turn_off_calls().len(), 1, "lights_off must hard-off");
    assert_eq!(spy.turn_on_count(), 0, "lights_off must not dim");
}

#[test]
fn bottom_long_then_short_press_recovers_to_soft_off() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.action("kitchen", "on").unwrap();
    h.action("kitchen", "lights_off").unwrap();
    spy.reset();

    h.action("kitchen", "off").unwrap();

    let calls = spy.turn_on_calls();
    assert_eq!(calls.len(), 1, "short press after hard-off should soft-off");
    assert_eq!(calls[0].1.brightness, 1, "recovery brightness must be 1%");
    assert_eq!(spy.turn_off_calls().len(), 0);
}
