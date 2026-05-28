//! Scenario 4: Server state sync — action responses and polling accuracy.
//!
//! Tests that action dispatch returns correct immediate state, multiple rapid
//! actions are processed in order, and polling endpoints return consistent
//! snapshots. These are the server-side guarantees that make the client's
//! optimistic UI + 3-second lock system work correctly.
//!
//! ## API journey
//!
//! 1. User taps room card → PUT /api/nodes/action returns immediate state
//! 2. GET /api/nodes/state (15s poll) returns current state reflecting last action
//! 3. Multiple rapid taps → server processes in order, final state correct

mod harness;

use harness::{room, rooms_with_lights, TestHarness};

// ============================================================================
// Scenario 4a: Action response includes immediate state
// ============================================================================

/// The "on" action should return a JSON response that includes the room's
/// rhythm state, so the client can immediately update its cache.
#[test]
fn action_on_returns_rhythm_state() {
    let harness = TestHarness::new().with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    // -- Action: turn on --
    let response = harness
        .action("kitchen", "on")
        .expect("on action should succeed");
    let json: serde_json::Value =
        serde_json::from_str(&response).expect("response should be valid JSON");

    // -- Assert: response includes rhythm state --
    assert!(
        json.get("rhythm_enabled").is_some(),
        "response should include rhythm_enabled"
    );
    assert_eq!(
        json["rhythm_enabled"], true,
        "rhythm should be enabled after on"
    );
}

// ============================================================================
// Scenario 4b: Action "lights_off" returns correct state
// ============================================================================

/// After turning off, the response should reflect the off state.
#[test]
fn action_lights_off_returns_state() {
    let harness = TestHarness::new().with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    // Turn on first, then off
    harness.action("kitchen", "on").unwrap();
    assert!(harness.lights_on("kitchen"));

    // -- Action: lights off --
    let _response = harness
        .action("kitchen", "lights_off")
        .expect("lights_off should succeed");

    // -- Assert: lights tracked as off --
    assert!(
        !harness.lights_on("kitchen"),
        "kitchen should be off after lights_off"
    );
}

// ============================================================================
// Scenario 4c: Multiple rapid actions processed in order
// ============================================================================

/// Rapid toggle sequences should be processed in order with the final state
/// reflecting the last action.
#[test]
fn rapid_actions_final_state_correct() {
    let harness = TestHarness::new().with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    // -- Action: rapid on/off/on/off/on sequence --
    harness.action("kitchen", "on").unwrap();
    harness.action("kitchen", "lights_off").unwrap();
    harness.action("kitchen", "on").unwrap();
    harness.action("kitchen", "lights_off").unwrap();
    harness.action("kitchen", "on").unwrap();

    // -- Assert: final state is on --
    assert!(harness.lights_on("kitchen"), "final state should be on");
    let snap = harness.snapshot("kitchen").expect("kitchen should exist");
    assert!(snap.rhythm_enabled, "rhythm should be enabled");
}

// ============================================================================
// Scenario 4d: Rhythm on/off toggles correctly
// ============================================================================

/// rhythm_on and rhythm_off actions should toggle rhythm_enabled.
/// rhythm_off marks lights_on=false (rhythm is no longer managing lights),
/// rhythm_on marks lights_on=true (rhythm takes over and turns on).
#[test]
fn rhythm_toggle_changes_rhythm_enabled() {
    let harness = TestHarness::new().with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    // Turn on first
    harness.action("kitchen", "on").unwrap();
    assert!(harness.lights_on("kitchen"));

    // -- Action: disable rhythm --
    harness.action("kitchen", "rhythm_off").unwrap();

    // -- Assert: rhythm disabled --
    let snap = harness.snapshot("kitchen").unwrap();
    assert!(!snap.rhythm_enabled, "rhythm should be disabled");

    // -- Action: re-enable rhythm --
    harness.action("kitchen", "rhythm_on").unwrap();

    // -- Assert: rhythm re-enabled --
    let snap = harness.snapshot("kitchen").unwrap();
    assert!(snap.rhythm_enabled, "rhythm should be re-enabled");
}

// ============================================================================
// Scenario 4e: Preferences update reflected in snapshots
// ============================================================================

/// Room preferences (Standby request, disabled) should be immediately reflected
/// in engine snapshots.
#[test]
fn preferences_reflected_in_snapshots() {
    let harness = TestHarness::new().with_discovery(
        vec![room("kitchen", "Kitchen"), room("bedroom", "Bedroom")],
        vec![],
    );
    harness.sync();
    harness.set_settings(Some(false));

    // -- Action: send a Standby request on kitchen through the legacy soft_off field --
    harness.action("kitchen", "on").unwrap();
    harness.set_room_preferences("kitchen", None, None, Some(true));

    // -- Assert: snapshot reflects Standby state --
    let snap = harness.snapshot("kitchen").unwrap();
    assert!(snap.soft_off, "Standby should be retained");
    assert!(!snap.hard_off, "Standby should not map to hard_off");

    // -- Action: disable bedroom --
    harness.set_room_preferences("bedroom", None, Some(true), None);

    // -- Assert: bedroom disabled --
    let snap = harness.snapshot("bedroom").unwrap();
    assert!(snap.disabled, "bedroom should be disabled");
}

// ============================================================================
// Scenario 4f: Action on cross-hub room in multi-hub setup
// ============================================================================

/// Cross-hub rooms should accept all standard actions without errors.
#[test]
fn all_actions_work_on_cross_hub_room() {
    let (rooms, devices) = rooms_with_lights(&[("hue-kitchen", "Kitchen")]);
    let mut harness = TestHarness::new().with_discovery(rooms, devices);
    harness.sync();

    let ha_key = harness.add_hub("ha", "192.168.1.200");
    let (ha_rooms, ha_devices) = rooms_with_lights(&[("ha-kitchen", "Kitchen")]);
    harness.set_hub_discovery(&ha_key, ha_rooms, ha_devices);
    harness.sync_hub(&ha_key);

    // Bind Kitchen
    let (entry_id, _, _) = harness.triage_room_binding(0).unwrap();
    harness.triage_bind(&entry_id).unwrap();

    // -- Action: cycle through standard actions --
    harness.action("hue-kitchen", "on").expect("on");
    harness.action("hue-kitchen", "step_up").expect("step_up");
    harness
        .action("hue-kitchen", "step_down")
        .expect("step_down");
    harness.action("hue-kitchen", "dim_up").expect("dim_up");
    harness.action("hue-kitchen", "dim_down").expect("dim_down");
    harness.action("hue-kitchen", "reset").expect("reset");
    harness
        .action("hue-kitchen", "rhythm_off")
        .expect("rhythm_off");
    harness
        .action("hue-kitchen", "rhythm_on")
        .expect("rhythm_on");
    harness
        .action("hue-kitchen", "lights_off")
        .expect("lights_off");

    // -- Assert: room still in consistent state --
    assert!(
        !harness.lights_on("hue-kitchen"),
        "should be off after lights_off"
    );
    let snap = harness.snapshot("hue-kitchen").expect("should exist");
    assert!(
        snap.rhythm_enabled,
        "rhythm should be on (rhythm_on was last rhythm action)"
    );
}
