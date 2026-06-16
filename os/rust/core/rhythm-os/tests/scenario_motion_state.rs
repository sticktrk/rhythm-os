//! Scenario: Motion sensor state interactions — "Room Grid → Motion Indicator".
//!
//! Tests motion sensor state interactions visible in the room grid UI. Motion
//! sensors are a key feature shown per-room, and their interaction with manual
//! actions and Standby state needs scenario coverage.
//!
//! ## API journey
//!
//! 1. Room grid shows motion icon on rooms with motion sensors
//! 2. Manual actions override motion state
//! 3. Standby requests stay Standby while motion state is active

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
// Scenario: Standby request on a motion room
// ============================================================================

/// Setting Standby on a motion room should enter Standby independently of motion
/// state and queue the motion timer clear.
#[test]
fn standby_request_enters_standby_on_motion_room() {
    let harness = TestHarness::new().with_discovery(
        vec![room("hallway", "Hallway")],
        vec![motion_sensor("motion_01", "hallway")],
    );
    harness.sync();
    harness.set_settings(Some(false));

    // Lights on + motion active
    harness.action("hallway", "on").unwrap();
    harness.set_motion_active("hallway");

    // -- Action: set Standby preference through the legacy soft_off field --
    harness.set_room_preferences("hallway", None, None, Some(true));

    // -- Assert: Standby set, motion snapshot still present until the event loop clears it --
    let snap = harness.snapshot("hallway").unwrap();
    assert!(snap.soft_off, "standby should be set");
    assert!(!snap.hard_off, "standby should not hard-off the room");
    let resolved = harness.resolve("hallway");
    let has_motion = harness
        .state
        .lock()
        .unwrap()
        .motion_snapshots
        .contains_key(&resolved);
    assert!(has_motion, "motion snapshot should still be present");
    assert_eq!(harness.pending_motion_clears(), vec![resolved]);
}
