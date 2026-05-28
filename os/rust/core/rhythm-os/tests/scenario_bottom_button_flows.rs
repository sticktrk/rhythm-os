//! Bottom-button (off) flow tests for the Hue dimmer.
//!
//! Hue dimmer button 4 (bottom) maps:
//! - short press → `OffPress` → hard turn_off (hard_off=true)
//! - long press → `LightsOff` → hard turn_off (hard_off=true)
//!
//! These scenarios cover daily-use combinations the basic
//! `scenario_integration_flow` tests don't reach: repeated presses, presses when
//! the cached power state diverges from intent, and recovery after hard off.

mod harness;

use harness::*;

#[test]
fn bottom_short_press_from_on_sends_hard_off() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    h.action("kitchen", "on").unwrap();
    spy.reset();

    h.action("kitchen", "off").unwrap();

    assert_eq!(spy.turn_off_calls().len(), 1, "off must hard-off");
    assert_eq!(spy.turn_on_count(), 0, "off must not dim");
}

#[test]
fn bottom_short_press_twice_repeats_hard_off() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    h.action("kitchen", "on").unwrap();
    spy.reset();

    h.action("kitchen", "off").unwrap();
    h.action("kitchen", "off").unwrap();

    assert_eq!(
        spy.turn_off_calls().len(),
        2,
        "each off press should hard-off"
    );
    assert_eq!(spy.turn_on_count(), 0);
}

#[test]
fn bottom_short_press_when_externally_off_still_hard_offs() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    // Cache says lights are off (e.g. someone flipped the switch externally),
    // but the user still presses the bottom button.
    h.set_lights_on("kitchen", false);
    spy.reset();

    h.action("kitchen", "off").unwrap();

    assert_eq!(
        spy.turn_off_calls().len(),
        1,
        "off press must dispatch even if cache says off"
    );
    assert_eq!(spy.turn_on_count(), 0);
}

#[test]
fn bottom_long_press_sends_hard_off() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    h.action("kitchen", "on").unwrap();
    spy.reset();

    h.action("kitchen", "lights_off").unwrap();

    assert_eq!(spy.turn_off_calls().len(), 1, "lights_off must hard-off");
    assert_eq!(spy.turn_on_count(), 0, "lights_off must not dim");
}

#[test]
fn bottom_long_then_short_press_stays_hard_off() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    h.action("kitchen", "on").unwrap();
    h.action("kitchen", "lights_off").unwrap();
    spy.reset();

    h.action("kitchen", "off").unwrap();

    assert_eq!(
        spy.turn_off_calls().len(),
        1,
        "short press after hard-off should hard-off again"
    );
    assert_eq!(spy.turn_on_count(), 0);
}
