//! Scenario 5: Fix My Lights across multiple hubs.
//!
//! "Fix My Lights" is the panic button — it must reset ALL on-rooms across
//! ALL connected hubs to current adaptive values. Motion-owned rooms turn off.
//!
//! ## API journey
//!
//! 1. Two hubs connected. Rooms: Kitchen (both), Living Room (both),
//!    Bedroom (Hue), Office (HA)
//! 2. Some rooms on, some off, some with motion
//! 3. User taps "Fix My Lights" → POST /api/rooms/fix
//! 4. All on-rooms across both hubs reset, motion rooms turn off
//! 5. AllRoomsScreen shows all rooms at default adaptive values

mod harness;

use harness::{motion_sensor, rooms_with_lights, TestHarness};

/// Helper: create a fully bound multi-hub setup.
///
/// Returns harness with:
/// - Kitchen: bound to Hue + HA (2 targets)
/// - Living Room: bound to Hue + HA (2 targets)
/// - Bedroom: Hue only (1 target)
/// - Office: HA only (1 target)
fn setup_four_rooms() -> TestHarness {
    let (rooms, devices) = rooms_with_lights(&[
        ("hue-kitchen", "Kitchen"),
        ("hue-living", "Living Room"),
        ("hue-bedroom", "Bedroom"),
    ]);
    let mut harness = TestHarness::new().with_discovery(rooms, devices);
    harness.sync();

    let ha_key = harness.add_hub("ha", "192.168.1.200");
    let (ha_rooms, ha_devices) = rooms_with_lights(&[
        ("ha-kitchen", "Kitchen"),
        ("ha-living", "Living Room"),
        ("ha-office", "Office"),
    ]);
    harness.set_hub_discovery(&ha_key, ha_rooms, ha_devices);
    harness.sync_hub(&ha_key);

    // Resolve all room bindings
    while harness.triage_pending_room_count() > 0 {
        let (entry_id, _, _) = harness.triage_room_binding(0).unwrap();
        harness.triage_bind(&entry_id).unwrap();
    }

    assert_eq!(harness.topology_room_count(), 4);
    harness
}

// ============================================================================
// Scenario 5a: Fix resets all on-rooms across both hubs
// ============================================================================

/// Fix My Lights should reset all on-rooms across all hubs.
#[test]
fn fix_resets_all_on_rooms_across_hubs() {
    let harness = setup_four_rooms();

    // Turn on cross-hub rooms and single-hub rooms
    harness.action("hue-kitchen", "on").unwrap();
    harness.action("hue-living", "on").unwrap();
    harness.action("ha-office", "on").unwrap();
    // Bedroom stays off

    assert!(harness.lights_on("hue-kitchen"));
    assert!(harness.lights_on("hue-living"));
    assert!(harness.lights_on("ha-office"));
    assert!(!harness.lights_on("hue-bedroom"));

    // -- Action: Fix My Lights --
    let result = harness.fix_my_lights();

    // -- Assert: 3 rooms reset, bedroom untouched --
    assert_eq!(result["rooms_reset"], 3, "all 3 on-rooms should be reset");

    let kitchen_id = harness.resolve("hue-kitchen");
    let living_id = harness.resolve("hue-living");
    let office_id = harness.resolve("ha-office");
    let bedroom_id = harness.resolve("hue-bedroom");

    let room_ids: Vec<&str> = result["rooms"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(
        room_ids.contains(&kitchen_id.as_str()),
        "Kitchen should be reset"
    );
    assert!(
        room_ids.contains(&living_id.as_str()),
        "Living Room should be reset"
    );
    assert!(
        room_ids.contains(&office_id.as_str()),
        "Office should be reset"
    );
    assert!(
        !room_ids.contains(&bedroom_id.as_str()),
        "Bedroom should NOT be reset (was off)"
    );
}

// ============================================================================
// Scenario 5b: OFF rooms not touched by fix
// ============================================================================

/// Rooms that are already off should not appear in the fix response.
#[test]
fn fix_skips_off_rooms() {
    let harness = setup_four_rooms();

    // Only turn on Kitchen
    harness.action("hue-kitchen", "on").unwrap();

    // -- Action: Fix My Lights --
    let result = harness.fix_my_lights();

    // -- Assert: only Kitchen reset --
    assert_eq!(result["rooms_reset"], 1, "only Kitchen should be reset");
    assert_eq!(result["motion_cleared"], 0, "no motion rooms");

    // Other rooms untouched
    assert!(!harness.lights_on("hue-living"), "Living Room stays off");
    assert!(!harness.lights_on("hue-bedroom"), "Bedroom stays off");
    assert!(!harness.lights_on("ha-office"), "Office stays off");
}

// ============================================================================
// Scenario 5c: Soft-off rooms reset by fix
// ============================================================================

/// Rooms in soft-off mode are intentionally EXCLUDED from Fix My Lights.
/// Fix only acts on rooms where lights_on=true AND soft_off=false.
/// Soft-off is a user-chosen idle state, not something to "fix".
#[test]
fn fix_skips_soft_off_rooms() {
    let harness = setup_four_rooms();

    // Turn on Kitchen and Living Room
    harness.action("hue-kitchen", "on").unwrap();
    harness.action("hue-living", "on").unwrap();

    // Put Kitchen into soft-off
    harness.set_room_preferences("hue-kitchen", None, None, Some(true));
    let snap = harness.snapshot("hue-kitchen").unwrap();
    assert!(snap.soft_off, "Kitchen should be in soft-off mode");

    // -- Action: Fix My Lights --
    let result = harness.fix_my_lights();

    // -- Assert: only Living Room reset (Kitchen is soft-off, excluded) --
    assert_eq!(
        result["rooms_reset"], 1,
        "only non-soft-off room should be reset"
    );

    let living_id = harness.resolve("hue-living");
    let room_ids: Vec<&str> = result["rooms"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(
        room_ids.contains(&living_id.as_str()),
        "Living Room should be reset"
    );

    // Kitchen should still be in soft-off
    let snap = harness.snapshot("hue-kitchen").unwrap();
    assert!(snap.soft_off, "Kitchen should still be in soft-off");
}

// ============================================================================
// Scenario 5d: Motion rooms turned off, not reset
// ============================================================================

/// Rooms with motion sensors should be turned off (OffPress) by Fix My Lights,
/// not reset to adaptive values.
#[test]
fn fix_turns_off_motion_rooms_across_hubs() {
    // Special setup: Bedroom has a motion sensor
    let (rooms, mut devices) =
        rooms_with_lights(&[("hue-kitchen", "Kitchen"), ("hue-bedroom", "Bedroom")]);
    devices.push(motion_sensor("motion-bedroom", "hue-bedroom"));

    let mut harness = TestHarness::new().with_discovery(rooms, devices);
    harness.sync();

    let ha_key = harness.add_hub("ha", "192.168.1.200");
    let (ha_rooms, ha_devices) = rooms_with_lights(&[("ha-kitchen", "Kitchen")]);
    harness.set_hub_discovery(&ha_key, ha_rooms, ha_devices);
    harness.sync_hub(&ha_key);

    // Bind Kitchen
    while harness.triage_pending_room_count() > 0 {
        let (entry_id, _, _) = harness.triage_room_binding(0).unwrap();
        harness.triage_bind(&entry_id).unwrap();
    }

    // Turn on both rooms
    harness.set_lights_on("hue-kitchen", true);
    harness.set_lights_on("hue-bedroom", true);
    harness.set_motion_active("hue-bedroom");

    // -- Action: Fix My Lights --
    let result = harness.fix_my_lights();

    // -- Assert: Kitchen reset, Bedroom motion-cleared --
    assert_eq!(
        result["rooms_reset"], 1,
        "Kitchen should be reset (normal room)"
    );
    assert_eq!(
        result["motion_cleared"], 1,
        "Bedroom should be motion-cleared"
    );

    let bedroom_id = harness.resolve("hue-bedroom");
    let motion_rooms = result["motion_rooms"].as_array().unwrap();
    assert_eq!(
        motion_rooms[0].as_str().unwrap(),
        bedroom_id,
        "Bedroom should appear in motion_rooms"
    );

    // Bedroom should now be off
    assert!(
        !harness.lights_on("hue-bedroom"),
        "Bedroom should be off after fix"
    );
}

// ============================================================================
// Scenario 5e: Fix followed by actions still works
// ============================================================================

/// After Fix My Lights, rooms should still be controllable via normal actions.
#[test]
fn actions_work_after_fix() {
    let harness = setup_four_rooms();

    // Turn on 2 rooms
    harness.action("hue-kitchen", "on").unwrap();
    harness.action("hue-living", "on").unwrap();

    let result = harness.fix_my_lights();
    assert_eq!(result["rooms_reset"], 2, "2 on-rooms should be reset");

    // -- Action: turn off one room, then turn on another after fix --
    harness.action("hue-kitchen", "lights_off").unwrap();
    harness.action("ha-office", "on").unwrap();

    // -- Assert: actions work normally --
    assert!(!harness.lights_on("hue-kitchen"), "Kitchen should be off");
    assert!(harness.lights_on("ha-office"), "Office should be on");

    // Actions on cross-hub rooms still work after fix
    harness.action("hue-kitchen", "on").unwrap();
    assert!(
        harness.lights_on("hue-kitchen"),
        "Kitchen should be back on"
    );
}

// ============================================================================
// Scenario 5f: Fix with no rooms on does nothing
// ============================================================================

/// Fix My Lights with all rooms off should be a no-op.
#[test]
fn fix_with_no_rooms_on_is_noop() {
    let harness = setup_four_rooms();

    // All rooms off (default state)

    // -- Action: Fix My Lights --
    let result = harness.fix_my_lights();

    // -- Assert: nothing happened --
    assert_eq!(result["rooms_reset"], 0);
    assert_eq!(result["motion_cleared"], 0);
    assert!(result["rooms"].as_array().unwrap().is_empty());
}

// ============================================================================
// Scenario 5g: Fix after re-sync still works
// ============================================================================

/// Fix My Lights should work correctly even after hub re-sync cycles.
#[test]
fn fix_works_after_resync_cycles() {
    let harness = setup_four_rooms();

    // Turn on rooms
    harness.action("hue-kitchen", "on").unwrap();
    harness.action("hue-bedroom", "on").unwrap();

    // Re-sync primary hub
    harness.sync();

    // -- Action: Fix My Lights --
    let result = harness.fix_my_lights();

    // -- Assert: both on-rooms still fixable --
    assert_eq!(
        result["rooms_reset"], 2,
        "both on-rooms should be reset after re-sync"
    );
}
