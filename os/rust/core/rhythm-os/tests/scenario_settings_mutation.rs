//! Scenario: Settings mutations — "Settings → Preferences" screen.
//!
//! Tests that changing legacy global settings (power_save, etc.) remains
//! compatible without reviving soft-off behavior. The Settings screen is used
//! by every user and these mutations had zero test coverage.
//!
//! ## API journey
//!
//! 1. User opens Settings → Preferences
//! 2. Toggles Power Save on → hard-off rooms stay off
//! 3. Legacy idle profile settings no longer affect off behavior
//! 4. Changes fade_ms / interval → room state is unaffected

mod harness;

use harness::{room, TestHarness};

// ============================================================================
// Scenario: Enabling power_save leaves hard-off rooms off
// ============================================================================

/// When power_save is toggled ON, hard-off rooms should remain hard-off without
/// resurrecting legacy soft-off behavior.
#[test]
fn power_save_on_leaves_hard_off_rooms_off() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(
        vec![room("kitchen", "Kitchen"), room("bedroom", "Bedroom")],
        vec![],
    );
    harness.sync();

    // Turn both rooms on
    harness.action("kitchen", "on").unwrap();
    harness.action("bedroom", "on").unwrap();

    harness.set_settings(Some(false));

    // Put kitchen into hard-off. Legacy power_save no longer changes the off
    // mode chosen by a button press.
    harness.action("kitchen", "off").unwrap();
    let snap = harness.snapshot("kitchen").unwrap();
    assert!(!snap.soft_off, "kitchen should not enter soft-off state");
    assert!(snap.hard_off, "kitchen should be in hard-off state");

    // -- Action: enable power_save --
    spy.reset();
    harness.set_settings(Some(true));

    // -- Assert: kitchen stayed truly off without a migration dispatch --
    let off_calls = spy.turn_off_calls();
    let kitchen_resolved = harness.resolve("kitchen");
    assert!(
        !off_calls.contains(&kitchen_resolved),
        "power_save ON should not re-dispatch already hard-off room (kitchen)"
    );

    let snap = harness.snapshot("kitchen").unwrap();
    assert!(
        !snap.soft_off && snap.hard_off,
        "kitchen should remain hard_off after power_save ON"
    );
    assert!(
        !harness.lights_on("kitchen"),
        "power_save ON should report hard-off rooms as lights off"
    );

    // Bedroom (was ON) should be unaffected
    let bedroom_resolved = harness.resolve("bedroom");
    assert!(
        !off_calls.contains(&bedroom_resolved),
        "bedroom was not hard-off, should not receive turn_off"
    );
}

// ============================================================================
// Scenario: power_save off does not enable soft-off behavior
// ============================================================================

/// With power_save ON, "off" action truly turns off lights.
/// With power_save OFF, "off" action still truly turns off lights.
#[test]
fn power_save_off_keeps_hard_off_behavior() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    // power_save defaults on — "off" should be a hard off
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
    assert!(snap.hard_off, "power_save ON: off should enter hard_off");
    assert!(
        !snap.soft_off,
        "power_save ON: off should not enter soft_off"
    );
    assert!(
        !harness.lights_on("kitchen"),
        "power_save ON: hard_off should report lights off"
    );

    // Now disable power_save — existing hard-off rooms stay hard-off.
    spy.reset();
    harness.set_settings(Some(false));
    assert!(
        spy.turn_on_count() == 0,
        "power_save OFF: existing hard_off room should not restore idle output"
    );
    assert!(
        !harness.lights_on("kitchen"),
        "power_save OFF: hard_off should remain lights off"
    );

    // New off actions with power_save disabled should still hard-off.
    harness.action("kitchen", "on").unwrap();
    spy.reset();
    harness.action("kitchen", "off").unwrap();
    assert!(
        spy.turn_off_count() > 0,
        "power_save OFF: off should send turn_off"
    );
    assert_eq!(
        spy.turn_on_count(),
        0,
        "power_save OFF: off should not send legacy soft-off turn_on"
    );
    let snap = harness.snapshot("kitchen").unwrap();
    assert!(
        !snap.soft_off,
        "power_save OFF: should not enter soft_off state"
    );
    assert!(snap.hard_off, "power_save OFF: should enter hard_off state");
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
    harness.set_settings(Some(false));

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

/// Enabling power_save when no rooms are legacy soft-off should be a no-op
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
    harness.set_settings(Some(true));

    // -- Assert: no state change --
    let kitchen_after = harness.snapshot("kitchen").unwrap();
    let bedroom_after = harness.snapshot("bedroom").unwrap();
    assert_eq!(kitchen_before.rhythm_enabled, kitchen_after.rhythm_enabled);
    assert_eq!(bedroom_before.rhythm_enabled, bedroom_after.rhythm_enabled);
    assert!(harness.lights_on("kitchen"), "kitchen should still be on");
    assert!(harness.lights_on("bedroom"), "bedroom should still be on");
}

// ============================================================================
// Scenario: off ignores legacy idle profile settings
// ============================================================================

/// Legacy idle profile settings may still be present in stored configs, but
/// OffPress no longer renders those profiles. It should hard-off instead.
#[test]
fn off_ignores_legacy_idle_profile_settings() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();
    harness.set_settings(Some(false));
    harness.set_mode_configs(vec![rhythm_core::ModeConfig {
        mode: rhythm_core::RhythmMode::Day,
        active_profile_id: Some(rhythm_core::RHYTHM_PROFILE_ID.into()),
        idle_profile_id: Some(rhythm_core::DAY_IDLE_PROFILE_ID.into()),
        wake_profile_id: None,
        warning_profile_id: None,
        room_defaults: vec![],
    }]);

    harness.action("kitchen", "on").unwrap();
    spy.reset();
    harness.action("kitchen", "off").unwrap();

    assert_eq!(spy.turn_off_calls().len(), 1, "off should hard-off");
    assert_eq!(
        spy.turn_on_calls().len(),
        0,
        "off should not render legacy idle profile output"
    );
    let snap = harness.snapshot("kitchen").unwrap();
    assert!(!snap.soft_off);
    assert!(snap.hard_off);
}
