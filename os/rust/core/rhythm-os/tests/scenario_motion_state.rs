//! Scenario: Motion sensor state interactions — "Room Grid → Motion Indicator".
//!
//! Tests motion sensor state interactions visible in the room grid UI. Motion
//! sensors are a key feature shown per-room, and their interaction with manual
//! actions and soft-off needs scenario coverage.
//!
//! ## API journey
//!
//! 1. Room grid shows motion icon on rooms with motion sensors
//! 2. Manual actions override motion state
//! 3. Soft-off works while motion state is active

mod harness;

use harness::{motion_sensor, room, TestHarness};

// ============================================================================
// Scenario: Manual on action works on a room with motion
// ============================================================================

/// A user tapping "on" on a motion room should still turn on the lights
/// normally, even though motion is active.
#[test]
fn manual_on_works_on_motion_room() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(
        vec![room("hallway", "Hallway")],
        vec![motion_sensor("motion_01", "hallway")],
    );
    harness.sync();

    // Set up motion state
    harness.set_motion_active("hallway");

    // -- Action: manual on --
    harness.action("hallway", "on").unwrap();

    // -- Assert: lights turned on --
    assert!(
        spy.turn_on_count() > 0,
        "on action should produce turn_on call"
    );
    assert!(harness.lights_on("hallway"), "hallway should be on");
    let snap = harness.snapshot("hallway").unwrap();
    assert!(snap.rhythm_enabled, "rhythm should be enabled after on");
}

// ============================================================================
// Scenario: lights_off does not clear motion snapshot
// ============================================================================

/// Individual lights_off action should not clear the motion snapshot from
/// AppState.
#[test]
fn lights_off_does_not_clear_motion_snapshot() {
    let harness = TestHarness::new().with_discovery(
        vec![room("hallway", "Hallway")],
        vec![motion_sensor("motion_01", "hallway")],
    );
    harness.sync();

    // Set up: lights on + motion active
    harness.set_lights_on("hallway", true);
    harness.set_motion_active("hallway");

    // -- Action: lights_off --
    harness.action("hallway", "lights_off").unwrap();

    // -- Assert: lights off, but motion snapshot remains --
    assert!(!harness.lights_on("hallway"), "hallway should be off");
    let resolved = harness.resolve("hallway");
    let has_motion = harness
        .state
        .lock()
        .unwrap()
        .motion_snapshots
        .contains_key(&resolved);
    assert!(has_motion, "motion snapshot should still be present");
}

// ============================================================================
// Scenario: soft_off on a motion room
// ============================================================================

/// Setting soft_off preference on a motion room should work independently
/// of motion state.
#[test]
fn soft_off_on_motion_room() {
    let harness = TestHarness::new().with_discovery(
        vec![room("hallway", "Hallway")],
        vec![motion_sensor("motion_01", "hallway")],
    );
    harness.sync();
    harness.set_settings(Some(false));

    // Lights on + motion active
    harness.action("hallway", "on").unwrap();
    harness.set_motion_active("hallway");

    // -- Action: set soft_off preference --
    harness.set_room_preferences("hallway", None, None, Some(true));

    // -- Assert: soft_off set, motion snapshot still present --
    let snap = harness.snapshot("hallway").unwrap();
    assert!(snap.soft_off, "soft_off should be set");
    let resolved = harness.resolve("hallway");
    let has_motion = harness
        .state
        .lock()
        .unwrap()
        .motion_snapshots
        .contains_key(&resolved);
    assert!(has_motion, "motion snapshot should still be present");
}
