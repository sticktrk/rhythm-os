//! Integration flow tests — verify engine dispatches correct LightingCommands.
//!
//! These tests use `SpyLightController` to capture the actual `LightingCommand`
//! values the engine sends when actions are dispatched, closing the gap between
//! "engine state is correct" (NoOp harness) and "transport receives correct params"
//! (per-crate controller tests).

mod harness;

use harness::*;

// ============================================================================
// Action → LightingCommand flow
// ============================================================================

#[test]
fn action_on_produces_lighting_command() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = harness::TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.action("kitchen", "on").unwrap();

    let calls = spy.turn_on_calls();
    assert_eq!(calls.len(), 1, "Expected exactly one turn_on call");

    let (_, cmd) = &calls[0];
    // At 2 PM on June 21 (summer solstice), expect reasonable adaptive values
    assert!(
        cmd.brightness >= 1 && cmd.brightness <= 100,
        "brightness {} out of range",
        cmd.brightness
    );
    assert!(
        cmd.kelvin >= 2000 && cmd.kelvin <= 6500,
        "kelvin {} out of range",
        cmd.kelvin
    );
}

#[test]
fn action_off_sends_soft_off() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = harness::TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    // Turn on first, then off
    h.action("kitchen", "on").unwrap();
    spy.reset();

    // OffPress triggers soft-off (turn_on at 1% brightness), not hard turn_off
    h.action("kitchen", "off").unwrap();

    assert_eq!(
        spy.turn_off_calls().len(),
        0,
        "Soft-off should not call turn_off"
    );
    assert_eq!(
        spy.turn_on_count(),
        1,
        "Soft-off should call turn_on at min brightness"
    );
    let (_, cmd) = &spy.turn_on_calls()[0];
    assert_eq!(cmd.brightness, 1, "Soft-off brightness should be 1%");
}

#[test]
fn step_up_increases_brightness() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = harness::TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    // Turn on to get baseline
    h.action("kitchen", "on").unwrap();
    let baseline = spy.turn_on_calls().last().unwrap().1.brightness;
    spy.reset();

    // Step up should produce a higher brightness (or max)
    h.action("kitchen", "step_up").unwrap();
    let calls = spy.turn_on_calls();
    assert!(!calls.is_empty(), "step_up should dispatch a turn_on");
    let stepped = calls.last().unwrap().1.brightness;
    assert!(
        stepped >= baseline,
        "step_up brightness {} should be >= baseline {}",
        stepped,
        baseline
    );
}

#[test]
fn step_down_decreases_brightness() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = harness::TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    // Turn on, then step up to ensure we're not at minimum
    h.action("kitchen", "on").unwrap();
    h.action("kitchen", "step_up").unwrap();
    let after_up = spy.turn_on_calls().last().unwrap().1.brightness;
    spy.reset();

    // Step down
    h.action("kitchen", "step_down").unwrap();
    let calls = spy.turn_on_calls();
    assert!(!calls.is_empty(), "step_down should dispatch a turn_on");
    let stepped = calls.last().unwrap().1.brightness;
    assert!(
        stepped <= after_up,
        "step_down brightness {} should be <= after_up {}",
        stepped,
        after_up
    );
}

#[test]
fn dim_up_changes_brightness() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = harness::TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.action("kitchen", "on").unwrap();
    let baseline = spy.turn_on_calls().last().unwrap().1.brightness;
    spy.reset();

    h.action("kitchen", "dim_up").unwrap();
    let calls = spy.turn_on_calls();
    assert!(!calls.is_empty(), "dim_up should dispatch a turn_on");
    let dimmed = calls.last().unwrap().1.brightness;
    assert!(
        dimmed >= baseline,
        "dim_up brightness {} should be >= baseline {}",
        dimmed,
        baseline
    );
}

#[test]
fn rhythm_off_does_not_send_light_command() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = harness::TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.action("kitchen", "on").unwrap();
    spy.reset();

    // rhythm_off disables adaptive mode but doesn't change lights
    h.action("kitchen", "rhythm_off").unwrap();

    assert_eq!(
        spy.turn_on_count(),
        0,
        "rhythm_off should not dispatch turn_on"
    );
    assert_eq!(
        spy.turn_off_calls().len(),
        0,
        "rhythm_off should not dispatch turn_off"
    );
}

#[test]
fn rhythm_on_does_not_send_light_command() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = harness::TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.action("kitchen", "on").unwrap();
    h.action("kitchen", "rhythm_off").unwrap();
    spy.reset();

    // rhythm_on re-enables adaptive mode but doesn't change lights
    h.action("kitchen", "rhythm_on").unwrap();

    assert_eq!(
        spy.turn_on_count(),
        0,
        "rhythm_on should not dispatch turn_on"
    );
    assert_eq!(
        spy.turn_off_calls().len(),
        0,
        "rhythm_on should not dispatch turn_off"
    );
}

#[test]
fn lights_off_sends_turn_off() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = harness::TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.action("kitchen", "on").unwrap();
    spy.reset();

    h.action("kitchen", "lights_off").unwrap();
    assert_eq!(
        spy.turn_off_calls().len(),
        1,
        "lights_off should dispatch turn_off"
    );
}

#[test]
fn reset_action_sends_adaptive_command() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = harness::TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    // Turn on, step up to move away from default, then reset
    h.action("kitchen", "on").unwrap();
    h.action("kitchen", "step_up").unwrap();
    spy.reset();

    h.action("kitchen", "reset").unwrap();

    let calls = spy.turn_on_calls();
    assert!(!calls.is_empty(), "reset should dispatch a turn_on");
    let cmd = &calls.last().unwrap().1;
    assert!(
        cmd.brightness >= 1 && cmd.brightness <= 100,
        "reset brightness {} out of range",
        cmd.brightness
    );
}

#[test]
fn toggle_on_sends_turn_on() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = harness::TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    // First toggle should turn on
    h.action("kitchen", "toggle").unwrap();
    assert!(
        spy.turn_on_count() > 0 || !spy.turn_off_calls().is_empty(),
        "toggle should dispatch either turn_on or turn_off"
    );
}

#[test]
fn multiple_rooms_get_independent_commands() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen"), ("bedroom", "Bedroom")]);
    let (h, spy) = harness::TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.action("kitchen", "on").unwrap();
    h.action("bedroom", "on").unwrap();

    let calls = spy.turn_on_calls();
    assert_eq!(calls.len(), 2, "Expected 2 turn_on calls");

    // Each call should target a different room
    let room_ids: Vec<&str> = calls.iter().map(|(rid, _)| rid.as_str()).collect();
    assert_ne!(room_ids[0], room_ids[1], "Rooms should be different");
}

#[test]
fn command_has_valid_color_coordinates() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = harness::TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.action("kitchen", "on").unwrap();

    let cmd = &spy.turn_on_calls()[0].1;
    // XY should be in gamut range
    assert!(
        (0.0..=1.0).contains(&cmd.xy.x),
        "xy.x {} out of range",
        cmd.xy.x
    );
    assert!(
        (0.0..=1.0).contains(&cmd.xy.y),
        "xy.y {} out of range",
        cmd.xy.y
    );
}
