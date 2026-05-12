//! Brightness offset persistence across the soft-off cycle.
//!
//! When a user dims up (sets a +offset) then soft-offs and re-presses the top
//! button, what should happen to the offset is a product call. This file pins
//! whatever the current behavior is so a refactor doesn't silently flip it.
//!
//! Reset, by contrast, is documented as offset-clearing.

mod harness;

use harness::*;

#[test]
fn dim_up_offset_survives_off_then_on_cycle() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    h.action("kitchen", "on").unwrap();
    let baseline = spy.turn_on_calls().last().unwrap().1.brightness;

    h.action("kitchen", "dim_up").unwrap();
    let dimmed = spy.turn_on_calls().last().unwrap().1.brightness;
    assert!(dimmed >= baseline, "dim_up must not lower brightness");

    h.action("kitchen", "off").unwrap();
    spy.reset();

    h.action("kitchen", "on").unwrap();
    let after_cycle = spy.turn_on_calls().last().unwrap().1.brightness;

    assert_eq!(
        after_cycle, dimmed,
        "off→on must preserve the dim_up offset (baseline={}, dimmed={}, after={})",
        baseline, dimmed, after_cycle
    );
}

#[test]
fn reset_clears_offset_after_off_cycle() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    h.action("kitchen", "on").unwrap();
    let baseline = spy.turn_on_calls().last().unwrap().1.brightness;

    h.action("kitchen", "dim_up").unwrap();
    h.action("kitchen", "off").unwrap();
    spy.reset();

    h.action("kitchen", "reset").unwrap();
    let after_reset = spy.turn_on_calls().last().unwrap().1.brightness;

    assert_eq!(
        after_reset, baseline,
        "reset must clear offset even when interleaved with off"
    );
}
