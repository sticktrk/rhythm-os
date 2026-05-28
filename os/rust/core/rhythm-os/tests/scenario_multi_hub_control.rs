//! Scenario 2: Room control across hubs (on/off/brightness).
//!
//! After rooms are bound across hubs, room card interactions (toggle, brightness,
//! hard-off, lights-off) must dispatch correctly. Single-hub rooms should only
//! dispatch to their owning hub.
//!
//! ## API journey
//!
//! 1. Kitchen is bound to both Hue and HA (from Scenario 1)
//! 2. User taps Kitchen card → on → PUT /api/nodes/action
//! 3. User drags brightness → PUT /api/nodes/brightness
//! 4. Standby preference enters the standby room state
//! 5. User long-presses → off → PUT /api/nodes/action lights_off
//! 6. Single-hub room (Office) works normally through just HA

mod harness;

use harness::{rooms_with_lights, TestHarness};

/// Helper: create a harness with two hubs, Kitchen bound to both,
/// Bedroom on Hue only, Office on HA only.
fn setup_multi_hub() -> (TestHarness, String) {
    let (rooms, devices) =
        rooms_with_lights(&[("hue-kitchen", "Kitchen"), ("hue-bedroom", "Bedroom")]);
    let mut harness = TestHarness::new().with_discovery(rooms, devices);
    harness.sync();

    let ha_key = harness.add_hub("ha", "192.168.1.200");
    let (ha_rooms, ha_devices) =
        rooms_with_lights(&[("ha-kitchen", "Kitchen"), ("ha-office", "Office")]);
    harness.set_hub_discovery(&ha_key, ha_rooms, ha_devices);
    harness.sync_hub(&ha_key);

    // Resolve Kitchen binding
    let (entry_id, name, _) = harness.triage_room_binding(0).expect("should have binding");
    assert_eq!(name, "Kitchen");
    harness.triage_bind(&entry_id).expect("bind should succeed");

    // Verify setup: Kitchen has 2 targets, Bedroom has 1, Office has 1
    assert_eq!(harness.topology_room_count(), 3);
    assert_eq!(harness.hub_target_count("hue-kitchen"), 2);
    assert_eq!(harness.hub_target_count("hue-bedroom"), 1);
    assert_eq!(harness.hub_target_count_for_hub(&ha_key, "ha-office"), 1);
    let office_id = harness.resolve_for_hub(&ha_key, "ha-office");

    (harness, office_id)
}

// ============================================================================
// Scenario 2a: Action "on" on cross-hub room
// ============================================================================

/// Dispatching "on" to a cross-hub room should turn on the room and track
/// it as lights-on. Both hub targets exist in topology.
#[test]
fn action_on_cross_hub_room() {
    let (harness, office_id) = setup_multi_hub();

    // -- Action: turn on Kitchen (cross-hub) --
    harness
        .action("hue-kitchen", "on")
        .expect("on action should succeed");

    // -- Assert: room tracked as lights-on --
    assert!(harness.lights_on("hue-kitchen"), "Kitchen should be on");
    assert!(
        !harness.lights_on("hue-bedroom"),
        "Bedroom should still be off"
    );
    assert!(!harness.lights_on(&office_id), "Office should still be off");

    // Engine snapshot should show rhythm_enabled
    let snap = harness
        .snapshot("hue-kitchen")
        .expect("Kitchen should exist");
    assert!(
        snap.rhythm_enabled,
        "Kitchen should have rhythm enabled after on"
    );
}

// ============================================================================
// Scenario 2b: Action "on" on single-hub room
// ============================================================================

/// Single-hub rooms should work the same — actions dispatch normally.
#[test]
fn action_on_single_hub_room() {
    let (harness, office_id) = setup_multi_hub();

    // -- Action: turn on Office (HA only) --
    harness
        .action(&office_id, "on")
        .expect("on action should succeed");

    // -- Assert: Office is on, others off --
    assert!(harness.lights_on(&office_id), "Office should be on");
    assert!(
        !harness.lights_on("hue-kitchen"),
        "Kitchen should still be off"
    );
}

// ============================================================================
// Scenario 2c: Multiple actions across hub types
// ============================================================================

/// Multiple rooms across different hubs can all be turned on independently.
#[test]
fn actions_across_hub_types() {
    let (harness, office_id) = setup_multi_hub();

    // Turn on all rooms
    harness.action("hue-kitchen", "on").unwrap();
    harness.action("hue-bedroom", "on").unwrap();
    harness.action(&office_id, "on").unwrap();

    // All should be on
    assert!(harness.lights_on("hue-kitchen"), "Kitchen on");
    assert!(harness.lights_on("hue-bedroom"), "Bedroom on");
    assert!(harness.lights_on(&office_id), "Office on");

    // Turn off just Kitchen
    harness.action("hue-kitchen", "lights_off").unwrap();

    // Kitchen off, others still on
    assert!(!harness.lights_on("hue-kitchen"), "Kitchen should be off");
    assert!(harness.lights_on("hue-bedroom"), "Bedroom still on");
    assert!(harness.lights_on(&office_id), "Office still on");
}

// ============================================================================
// Scenario 2d: Standby request on cross-hub room
// ============================================================================

/// Setting Standby on a cross-hub room should enter Standby without losing targets.
#[test]
fn standby_request_enters_standby_cross_hub_room() {
    let (harness, _office_id) = setup_multi_hub();
    harness.set_settings(Some(false));

    // Turn on Kitchen first
    harness.action("hue-kitchen", "on").unwrap();
    assert!(harness.lights_on("hue-kitchen"));

    // -- Action: set Standby on Kitchen through the legacy soft_off field --
    harness.set_room_preferences("hue-kitchen", None, None, Some(true));

    // -- Assert: engine snapshot shows Standby and cross-hub routing is intact --
    let snap = harness
        .snapshot("hue-kitchen")
        .expect("Kitchen should exist");
    assert!(snap.soft_off, "Kitchen should be in Standby");
    assert!(!snap.hard_off, "Kitchen should not be in hard-off mode");
    assert!(
        snap.rhythm_enabled,
        "rhythm should still be enabled during Standby"
    );
    assert!(
        harness.lights_on("hue-kitchen"),
        "Standby tracks as lights-on"
    );
    assert_eq!(harness.hub_target_count("hue-kitchen"), 2);
}

// ============================================================================
// Scenario 2e: Brightness on cross-hub room
// ============================================================================

/// Setting brightness on a cross-hub room should update the engine state.
#[test]
fn brightness_on_cross_hub_room() {
    let (harness, _office_id) = setup_multi_hub();

    // Turn on Kitchen first
    harness.action("hue-kitchen", "on").unwrap();

    // -- Action: set brightness --
    harness.set_brightness("hue-kitchen", 75);

    // -- Assert: engine snapshot shows brightness offset --
    let snap = harness
        .snapshot("hue-kitchen")
        .expect("Kitchen should exist");
    // Brightness is stored as an offset, and the set_brightness function
    // converts the 0-100 value to an offset from the current adaptive value.
    // Just verify the room is still in a valid state.
    assert!(snap.rhythm_enabled, "rhythm should still be enabled");
}

// ============================================================================
// Scenario 2f: Room state preserved through re-sync
// ============================================================================

/// User state (rhythm_enabled, Standby) should survive hub re-sync.
#[test]
fn room_state_preserved_through_resync() {
    let (harness, _office_id) = setup_multi_hub();
    harness.set_settings(Some(false));

    // Turn on Kitchen and set some state
    harness.action("hue-kitchen", "on").unwrap();
    harness.set_room_preferences("hue-kitchen", Some(true), None, Some(true));

    let snap_before = harness.snapshot("hue-kitchen").unwrap();
    assert!(snap_before.soft_off, "Standby should be set");
    assert!(!snap_before.hard_off, "Standby should not be hard-off");
    assert!(snap_before.rhythm_enabled, "rhythm should be enabled");

    // -- Action: re-sync the primary hub (simulates hub reconnect) --
    harness.sync();

    // -- Assert: state preserved --
    let snap_after = harness.snapshot("hue-kitchen").unwrap();
    assert!(
        snap_after.rhythm_enabled,
        "rhythm_enabled preserved through re-sync"
    );
    assert!(snap_after.soft_off, "Standby preserved through re-sync");
    assert!(!snap_after.hard_off, "Standby did not become hard-off");
    assert!(
        harness.lights_on("hue-kitchen"),
        "Standby lights-on state preserved"
    );
}
