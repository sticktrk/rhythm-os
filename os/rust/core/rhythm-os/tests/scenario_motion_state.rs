//! Scenario: Motion sensor state interactions — "Room Grid → Motion Indicator".
//!
//! Tests motion sensor state interactions visible in the room grid UI. Motion
//! sensors are a key feature shown per-room, and their interaction with
//! fix/soft_off/disable had zero existing coverage.
//!
//! ## API journey
//!
//! 1. Room grid shows motion icon on rooms with motion sensors
//! 2. Fix My Lights turns off motion rooms instead of resetting
//! 3. Manual actions override motion state
//! 4. Disabled + motion rooms are fully excluded from fix

mod harness;

use harness::{motion_sensor, room, TestHarness};

// ============================================================================
// Scenario: Motion rooms are turned off (not reset) by fix
// ============================================================================

/// Fix My Lights should turn off motion rooms via OffPress and reset
/// non-motion on-rooms. The fix response distinguishes the two categories.
#[test]
fn motion_room_excluded_from_fix_reset() {
    let harness = TestHarness::new().with_discovery(
        vec![
            room("hallway", "Hallway"),
            room("kitchen", "Kitchen"),
            room("bedroom", "Bedroom"),
        ],
        vec![motion_sensor("motion_01", "hallway")],
    );
    harness.sync();

    // All rooms on, hallway has active motion
    harness.set_lights_on("hallway", true);
    harness.set_lights_on("kitchen", true);
    harness.set_lights_on("bedroom", true);
    harness.set_motion_active("hallway");

    // -- Action: Fix My Lights --
    let result = harness.fix_my_lights();

    // -- Assert: kitchen+bedroom reset, hallway motion-cleared --
    assert_eq!(
        result["rooms_reset"], 2,
        "kitchen and bedroom should be reset"
    );
    assert_eq!(
        result["motion_cleared"], 1,
        "hallway should be motion-cleared"
    );
    assert!(
        !harness.lights_on("hallway"),
        "hallway should be off after fix"
    );
}

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
/// AppState — only fix_my_lights clears motion state.
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
    assert!(
        has_motion,
        "motion snapshot should still be present (only fix clears it)"
    );
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

// ============================================================================
// Scenario: Disabled motion room excluded from fix entirely
// ============================================================================

/// A room that is both disabled AND has motion should be excluded from
/// fix entirely — not reset AND not motion-cleared.
#[test]
fn disabled_motion_room_excluded_from_fix() {
    let harness = TestHarness::new().with_discovery(
        vec![room("hallway", "Hallway"), room("kitchen", "Kitchen")],
        vec![motion_sensor("motion_01", "hallway")],
    );
    harness.sync();

    // Both on, hallway has motion
    harness.set_lights_on("hallway", true);
    harness.set_lights_on("kitchen", true);
    harness.set_motion_active("hallway");

    // Disable hallway
    harness.set_room_preferences("hallway", None, Some(true), None);

    // -- Action: Fix My Lights --
    let result = harness.fix_my_lights();

    // -- Assert: only kitchen reset, hallway fully excluded --
    assert_eq!(result["rooms_reset"], 1, "only kitchen should be reset");
    assert_eq!(
        result["motion_cleared"], 0,
        "disabled motion room should not be motion-cleared"
    );
    assert!(
        harness.lights_on("hallway"),
        "hallway lights unchanged (fix skipped it)"
    );
}

// ============================================================================
// Scenario: Fix My Lights clears stale offsets on motion rooms
// ============================================================================

/// Motion rooms that accumulated time/brightness offsets (e.g., from the Light
/// Tuning screen's batch offset) should have those offsets cleared by Fix My
/// Lights, not just turned off. Otherwise the stale offset persists and
/// produces wrong brightness on next motion activation.
#[test]
fn fix_clears_offsets_on_motion_rooms() {
    let harness = TestHarness::new().with_discovery(
        vec![room("hallway", "Hallway"), room("kitchen", "Kitchen")],
        vec![motion_sensor("motion_01", "hallway")],
    );
    harness.sync();

    // Apply -120 time offset to both rooms (simulates Light Tuning batch offset)
    harness.set_room_offset("hallway", -120.0);
    harness.set_room_offset("kitchen", -120.0);

    // Both on, hallway has active motion
    harness.set_lights_on("hallway", true);
    harness.set_lights_on("kitchen", true);
    harness.set_motion_active("hallway");

    // Verify offsets are set
    assert_eq!(
        harness.snapshot("hallway").unwrap().time_offset_minutes,
        -120.0
    );
    assert_eq!(
        harness.snapshot("kitchen").unwrap().time_offset_minutes,
        -120.0
    );

    // -- Action: Fix My Lights --
    let result = harness.fix_my_lights();

    // -- Assert: both categories handled --
    assert_eq!(result["rooms_reset"], 1, "kitchen should be reset");
    assert_eq!(
        result["motion_cleared"], 1,
        "hallway should be motion-cleared"
    );

    // -- Assert: offsets cleared on both rooms --
    let hallway = harness.snapshot("hallway").unwrap();
    assert_eq!(
        hallway.time_offset_minutes, 0.0,
        "hallway time offset should be cleared"
    );
    assert_eq!(
        hallway.brightness_offset, 0.0,
        "hallway brightness offset should be cleared"
    );

    let kitchen = harness.snapshot("kitchen").unwrap();
    assert_eq!(
        kitchen.time_offset_minutes, 0.0,
        "kitchen time offset should be cleared (via Reset)"
    );

    // -- Assert: hallway still turned off --
    assert!(
        !harness.lights_on("hallway"),
        "hallway should be off after fix"
    );
}
