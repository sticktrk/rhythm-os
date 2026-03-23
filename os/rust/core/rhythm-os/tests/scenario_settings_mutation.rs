//! Scenario: Settings mutations — "Settings → Preferences" screen.
//!
//! Tests that changing global settings (power_save, soft_off_brightness)
//! correctly propagates to active rooms via the engine. The Settings screen
//! is used by every user and these mutations had zero test coverage.
//!
//! ## API journey
//!
//! 1. User opens Settings → Preferences
//! 2. Toggles Power Save on → soft-off rooms turn truly off
//! 3. Adjusts Soft Off brightness slider → engine picks up new value
//! 4. Changes fade_ms / interval → room state is unaffected

mod harness;

use harness::{room, TestHarness};

// ============================================================================
// Scenario: Enabling power_save turns off rooms that were in soft-off
// ============================================================================

/// When power_save is toggled ON, any rooms currently in soft-off state
/// should be turned truly off (hard off via the controller).
#[test]
fn power_save_on_turns_off_soft_off_rooms() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(
        vec![room("kitchen", "Kitchen"), room("bedroom", "Bedroom")],
        vec![],
    );
    harness.sync();

    // Turn both rooms on
    harness.action("kitchen", "on").unwrap();
    harness.action("bedroom", "on").unwrap();

    // Put kitchen into soft-off (power_save defaults false, so "off" = soft-off)
    harness.action("kitchen", "off").unwrap();
    let snap = harness.snapshot("kitchen").unwrap();
    assert!(snap.soft_off, "kitchen should be in soft-off state");

    // -- Action: enable power_save --
    spy.reset();
    harness.set_settings(None, None, None, Some(true), None);

    // -- Assert: kitchen was turned truly off --
    let off_calls = spy.turn_off_calls();
    let kitchen_resolved = harness.resolve("kitchen");
    assert!(
        off_calls.contains(&kitchen_resolved),
        "power_save ON should turn off soft-off room (kitchen)"
    );

    // Kitchen's soft_off flag should be cleared by the engine
    let snap = harness.snapshot("kitchen").unwrap();
    assert!(
        !snap.soft_off,
        "soft_off should be cleared after power_save ON"
    );

    // Bedroom (was ON, not soft-off) should be unaffected
    let bedroom_resolved = harness.resolve("bedroom");
    assert!(
        !off_calls.contains(&bedroom_resolved),
        "bedroom was not in soft-off, should not receive turn_off"
    );
}

// ============================================================================
// Scenario: power_save off enables soft-off behavior
// ============================================================================

/// With power_save ON, "off" action truly turns off lights.
/// With power_save OFF, "off" action sends soft-off (dim to low brightness).
#[test]
fn power_save_off_enables_soft_off_behavior() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    // Enable power_save — "off" should be a hard off
    harness.set_settings(None, None, None, Some(true), None);
    harness.action("kitchen", "on").unwrap();
    spy.reset();
    harness.action("kitchen", "off").unwrap();
    assert!(
        spy.turn_off_count() > 0,
        "power_save ON: off should be hard turn_off"
    );
    assert_eq!(
        spy.turn_on_count(),
        0,
        "power_save ON: off should not send turn_on"
    );
    let snap = harness.snapshot("kitchen").unwrap();
    assert!(!snap.soft_off, "power_save ON: should not enter soft_off");

    // Now disable power_save — "off" should be soft-off (turn_on at low brightness)
    harness.set_settings(None, None, None, Some(false), None);
    harness.action("kitchen", "on").unwrap();
    spy.reset();
    harness.action("kitchen", "off").unwrap();
    assert!(
        spy.turn_on_count() > 0,
        "power_save OFF: off should send turn_on (soft-off)"
    );
    let snap = harness.snapshot("kitchen").unwrap();
    assert!(snap.soft_off, "power_save OFF: should enter soft_off state");
}

// ============================================================================
// Scenario: soft_off_brightness change is applied to engine
// ============================================================================

/// Changing soft_off_brightness via settings should affect subsequent
/// soft-off commands dispatched by the engine.
#[test]
fn soft_off_brightness_change_applied() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    // Turn on, then off (enters soft-off since power_save defaults false)
    harness.action("kitchen", "on").unwrap();
    harness.action("kitchen", "off").unwrap();
    let snap = harness.snapshot("kitchen").unwrap();
    assert!(snap.soft_off, "should be in soft-off");

    // -- Action: change soft_off_brightness --
    harness.set_settings(None, None, None, None, Some(25));

    // Turn on again and off to get a fresh soft-off command with new brightness
    harness.action("kitchen", "on").unwrap();
    spy.reset();
    harness.action("kitchen", "off").unwrap();

    // -- Assert: the soft-off turn_on uses the new brightness --
    let on_calls = spy.turn_on_calls();
    assert!(!on_calls.is_empty(), "soft-off should send turn_on");
    let (_, cmd) = &on_calls[0];
    assert_eq!(cmd.brightness, 25, "soft-off brightness should be 25");
}

// ============================================================================
// Scenario: unrelated settings don't affect room state
// ============================================================================

/// Changing fade_ms or update_interval should not alter room snapshots.
#[test]
fn settings_change_preserves_room_state() {
    let harness = TestHarness::new().with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    // Build up some offsets (step_down works at 2PM peak; step_up is boundary no-op)
    harness.action("kitchen", "on").unwrap();
    harness.action("kitchen", "step_down").unwrap();
    let before = harness.snapshot("kitchen").unwrap();
    assert!(
        before.time_offset_minutes != 0.0,
        "step_down should create non-zero time offset"
    );

    // -- Action: change unrelated settings --
    harness.set_settings(Some(800), Some(120), None, None, None);

    // -- Assert: room state unchanged --
    let after = harness.snapshot("kitchen").unwrap();
    assert_eq!(
        before.time_offset_minutes, after.time_offset_minutes,
        "time offset preserved"
    );
    assert_eq!(
        before.brightness_offset, after.brightness_offset,
        "brightness offset preserved"
    );
    assert_eq!(
        before.rhythm_enabled, after.rhythm_enabled,
        "rhythm_enabled preserved"
    );
}

// ============================================================================
// Scenario: power_save toggle with no soft-off rooms is safe
// ============================================================================

/// Enabling power_save when no rooms are in soft-off should be a no-op
/// (no errors, no state changes).
#[test]
fn power_save_toggle_no_soft_off_rooms_safe() {
    let harness = TestHarness::new().with_discovery(
        vec![room("kitchen", "Kitchen"), room("bedroom", "Bedroom")],
        vec![],
    );
    harness.sync();

    // Both rooms on, neither in soft-off
    harness.action("kitchen", "on").unwrap();
    harness.action("bedroom", "on").unwrap();

    let kitchen_before = harness.snapshot("kitchen").unwrap();
    let bedroom_before = harness.snapshot("bedroom").unwrap();

    // -- Action: toggle power_save on --
    harness.set_settings(None, None, None, Some(true), None);

    // -- Assert: no state change --
    let kitchen_after = harness.snapshot("kitchen").unwrap();
    let bedroom_after = harness.snapshot("bedroom").unwrap();
    assert_eq!(kitchen_before.rhythm_enabled, kitchen_after.rhythm_enabled);
    assert_eq!(bedroom_before.rhythm_enabled, bedroom_after.rhythm_enabled);
    assert!(harness.lights_on("kitchen"), "kitchen should still be on");
    assert!(harness.lights_on("bedroom"), "bedroom should still be on");
}
