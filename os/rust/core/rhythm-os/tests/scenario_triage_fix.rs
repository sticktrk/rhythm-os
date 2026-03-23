//! Scenario integration tests: room sync (triage) → Fix My Lights.
//!
//! These tests catch regressions where changes to startup room sync break
//! the Fix My Lights button, or vice versa. Both flows share mutable state
//! (`room_lights_on`, `motion_snapshots`, engine snapshots) and these tests
//! verify that state flows correctly between them.
//!
//! ## Pattern for new scenario tests
//!
//! 1. Create a fresh `TestHarness` with discovered rooms/devices
//! 2. Call `sync()` to populate the engine (triggers runtime creation)
//! 3. Set up state (lights on, motion, etc.)
//! 4. Perform the action under test
//! 5. Assert on observable state (response JSON, engine snapshots, lights_on)
//!
//! Each test is self-contained — no shared state between tests.

mod harness;

use harness::{motion_sensor, room, TestHarness};

// ============================================================================
// Scenario 1: Sync rooms → Fix resets on-rooms
// ============================================================================

/// After syncing rooms from hub and marking some as lights-on,
/// Fix My Lights should reset exactly the on-rooms (not disabled, soft-off, or off ones).
#[test]
fn sync_rooms_then_fix_resets_on_rooms() {
    // -- Setup: discover 3 rooms from hub --
    let harness = TestHarness::new().with_discovery(
        vec![
            room("kitchen", "Kitchen"),
            room("living_room", "Living Room"),
            room("bedroom", "Bedroom"),
        ],
        vec![],
    );

    // -- Action: sync rooms from hub --
    let report = harness.sync();
    assert_eq!(report.rooms_added, 3);
    assert_eq!(harness.all_snapshots().len(), 3);

    // -- Setup: mark 2 rooms as lights-on (simulates event handler state) --
    harness.set_lights_on("kitchen", true);
    harness.set_lights_on("living_room", true);
    // bedroom is off (not in room_lights_on)

    // -- Action: fix my lights --
    let result = harness.fix_my_lights();

    // -- Assert: only the 2 on-rooms were reset --
    assert_eq!(result["rooms_reset"], 2);
    assert_eq!(result["motion_cleared"], 0);

    let room_ids: Vec<&str> = result["rooms"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    // Room IDs in the response are topology UUIDs, resolve hub-native for comparison
    let kitchen_id = harness.resolve("kitchen");
    let living_room_id = harness.resolve("living_room");
    let bedroom_id = harness.resolve("bedroom");
    assert!(
        room_ids.contains(&kitchen_id.as_str()),
        "kitchen should be reset"
    );
    assert!(
        room_ids.contains(&living_room_id.as_str()),
        "living_room should be reset"
    );
    assert!(
        !room_ids.contains(&bedroom_id.as_str()),
        "bedroom should be skipped (lights off)"
    );
}

// ============================================================================
// Scenario 2: Sync rooms + motion sensor → Fix turns off motion rooms
// ============================================================================

/// When a room has a motion sensor registered and its lights are on,
/// Fix My Lights should turn it off (OffPress) instead of resetting it,
/// and populate pending_motion_clear for the event loop.
#[test]
fn sync_rooms_then_fix_turns_off_motion_rooms() {
    // -- Setup: 3 rooms, one with a motion sensor --
    let harness = TestHarness::new().with_discovery(
        vec![
            room("hallway", "Hallway"),
            room("kitchen", "Kitchen"),
            room("office", "Office"),
        ],
        vec![motion_sensor("motion_01", "hallway")],
    );

    let report = harness.sync();
    assert_eq!(report.rooms_added, 3);

    // -- Setup: hallway (motion) and kitchen (normal) have lights on --
    harness.set_lights_on("hallway", true);
    harness.set_lights_on("kitchen", true);
    // office is off

    // -- Action: fix my lights --
    let result = harness.fix_my_lights();

    // -- Assert: kitchen was reset, hallway was turned off as motion room --
    assert_eq!(result["rooms_reset"], 1, "only kitchen should be reset");
    assert_eq!(
        result["motion_cleared"], 1,
        "hallway should be motion-cleared"
    );

    let hallway_id = harness.resolve("hallway");
    let motion_rooms = result["motion_rooms"].as_array().unwrap();
    assert_eq!(motion_rooms[0].as_str().unwrap(), hallway_id);

    // Verify pending_motion_clear was populated for the event loop
    let clears = harness.pending_motion_clears();
    assert!(clears.contains(&hallway_id));

    // Verify lights_on state was updated
    assert!(!harness.lights_on("hallway"), "hallway should now be off");
}

// ============================================================================
// Scenario 3: Fix with no lights-on does nothing
// ============================================================================

/// After a cold start (sync completes but no events have set lights_on yet),
/// Fix My Lights should not affect any rooms. This documents the current
/// behavior: fix only acts on rooms explicitly tracked as lights-on.
#[test]
fn fix_with_no_lights_on_does_nothing() {
    // -- Setup: sync 3 rooms, but don't set any lights on --
    let harness = TestHarness::new().with_discovery(
        vec![
            room("kitchen", "Kitchen"),
            room("living_room", "Living Room"),
            room("bedroom", "Bedroom"),
        ],
        vec![],
    );
    harness.sync();

    // -- Action: fix without any lights-on state --
    let result = harness.fix_my_lights();

    // -- Assert: nothing happened --
    assert_eq!(result["rooms_reset"], 0);
    assert_eq!(result["motion_cleared"], 0);
    assert!(result["rooms"].as_array().unwrap().is_empty());
}

// ============================================================================
// Scenario 4: Action "on" → Fix resets that room
// ============================================================================

/// Dispatching an "on" action to a room should set its lights_on state,
/// making it eligible for Fix My Lights reset.
#[test]
fn action_on_then_fix_resets() {
    // -- Setup: sync rooms --
    let harness = TestHarness::new().with_discovery(
        vec![room("kitchen", "Kitchen"), room("bedroom", "Bedroom")],
        vec![],
    );
    harness.sync();

    // -- Action: turn on kitchen via action dispatch --
    harness.action("kitchen", "on").expect("on action failed");

    // -- Assert: kitchen is now tracked as lights-on --
    assert!(
        harness.lights_on("kitchen"),
        "kitchen should be on after 'on' action"
    );
    assert!(!harness.lights_on("bedroom"), "bedroom should still be off");

    // -- Action: fix my lights --
    let result = harness.fix_my_lights();

    // -- Assert: only kitchen was reset --
    assert_eq!(result["rooms_reset"], 1);
    let kitchen_id = harness.resolve("kitchen");
    let bedroom_id = harness.resolve("bedroom");
    let room_ids: Vec<&str> = result["rooms"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(room_ids.contains(&kitchen_id.as_str()));
    assert!(!room_ids.contains(&bedroom_id.as_str()));
}

// ============================================================================
// Scenario 5: Re-sync preserves room state through fix
// ============================================================================

/// When rooms are re-synced (same rooms rediscovered from hub), user state
/// (rhythm_enabled, offsets, etc.) should be preserved, and Fix My Lights
/// should still work correctly against the preserved state.
#[test]
fn re_sync_preserves_room_state_through_fix() {
    // -- Setup: initial sync --
    let harness = TestHarness::new().with_discovery(
        vec![room("kitchen", "Kitchen"), room("bedroom", "Bedroom")],
        vec![],
    );
    harness.sync();

    // -- Action: turn on kitchen and modify its state --
    harness.action("kitchen", "on").expect("on action failed");

    // Verify kitchen is in the engine
    let snap = harness.snapshot("kitchen");
    assert!(snap.is_some(), "kitchen should exist in engine");

    // -- Action: re-sync (same rooms discovered again) --
    let report = harness.sync();

    // Re-sync should update, not add (rooms already exist)
    assert_eq!(report.rooms_added, 0, "no new rooms on re-sync");

    // -- Assert: room state preserved after re-sync --
    let snap = harness
        .snapshot("kitchen")
        .expect("kitchen should still exist");
    assert!(snap.rhythm_enabled, "rhythm_enabled should be preserved");

    // -- Assert: lights_on state survived re-sync --
    assert!(
        harness.lights_on("kitchen"),
        "lights_on should survive re-sync"
    );

    // -- Action: fix my lights still works --
    let result = harness.fix_my_lights();
    assert_eq!(result["rooms_reset"], 1, "kitchen should still be fixable");
}
