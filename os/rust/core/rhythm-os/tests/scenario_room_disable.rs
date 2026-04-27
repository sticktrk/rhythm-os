//! Scenario: Room disable toggle — "Settings → Rooms toggle".
//!
//! Tests the disabled room flag which users toggle in Settings to exclude
//! rooms from adaptive lighting. The disable/re-enable lifecycle and its
//! interaction with offsets needs scenario coverage.
//!
//! ## API journey
//!
//! 1. User opens Settings → Rooms
//! 2. Toggles a room's switch to disabled
//! 3. User toggles back to enabled → normal control resumes

mod harness;

use harness::{room, TestHarness};

// ============================================================================
// Scenario: Disable then re-enable restores control
// ============================================================================

/// After re-enabling a disabled room, actions should work normally.
#[test]
fn disable_then_reenable_restores_control() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    // Turn on, then disable
    harness.action("kitchen", "on").unwrap();
    harness.set_room_preferences("kitchen", None, Some(true), None);
    let snap = harness.snapshot("kitchen").unwrap();
    assert!(snap.disabled, "should be disabled");

    // -- Action: re-enable --
    harness.set_room_preferences("kitchen", None, Some(false), None);

    // -- Assert: actions work --
    spy.reset();
    harness.action("kitchen", "reset").unwrap();
    assert!(
        spy.turn_on_count() > 0,
        "reset should produce turn_on after re-enable"
    );
    let snap = harness.snapshot("kitchen").unwrap();
    assert!(!snap.disabled, "should be re-enabled");
}

// ============================================================================
// Scenario: Disabling preserves offsets
// ============================================================================

/// Disabling a room should not zero out its time/brightness offsets.
#[test]
fn disabled_preserves_offsets() {
    let harness = TestHarness::new().with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    // Build up offsets (step_down works at 2PM peak; step_up is boundary no-op)
    harness.action("kitchen", "on").unwrap();
    harness.action("kitchen", "step_down").unwrap();
    harness.action("kitchen", "step_down").unwrap();
    let before = harness.snapshot("kitchen").unwrap();
    assert!(before.time_offset_minutes != 0.0, "should have time offset");

    // -- Action: disable --
    harness.set_room_preferences("kitchen", None, Some(true), None);

    // -- Assert: offsets preserved --
    let after = harness.snapshot("kitchen").unwrap();
    assert!(after.disabled, "should be disabled");
    assert_eq!(
        before.time_offset_minutes, after.time_offset_minutes,
        "time offset should be preserved"
    );
    assert_eq!(
        before.brightness_offset, after.brightness_offset,
        "brightness offset should be preserved"
    );
}

// ============================================================================
// Scenario: Disable during active rhythm
// ============================================================================

/// Disabling a room that has rhythm_enabled should preserve rhythm_enabled
/// (the flag is not cleared — the room is just skipped by periodic updates).
#[test]
fn disable_during_active_rhythm() {
    let harness = TestHarness::new().with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    // Turn on (rhythm_enabled=true)
    harness.action("kitchen", "on").unwrap();
    let snap = harness.snapshot("kitchen").unwrap();
    assert!(snap.rhythm_enabled, "should be rhythm_enabled after on");

    // -- Action: disable --
    harness.set_room_preferences("kitchen", None, Some(true), None);

    // -- Assert: disabled but rhythm_enabled preserved --
    let snap = harness.snapshot("kitchen").unwrap();
    assert!(snap.disabled, "should be disabled");
    assert!(
        snap.rhythm_enabled,
        "rhythm_enabled should be preserved (not cleared)"
    );
}
