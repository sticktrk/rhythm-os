//! Scenario: Room disable toggle — "Settings → Rooms toggle".
//!
//! Tests the disabled room flag which users toggle in Settings to exclude
//! rooms from adaptive lighting. The disable/re-enable lifecycle and its
//! interaction with fix/offsets had zero existing coverage.
//!
//! ## API journey
//!
//! 1. User opens Settings → Rooms
//! 2. Toggles a room's switch to disabled
//! 3. Room is excluded from Fix My Lights
//! 4. User toggles back to enabled → normal control resumes

mod harness;

use harness::{room, rooms_with_lights, TestHarness};

// ============================================================================
// Scenario: Disabled room excluded from fix
// ============================================================================

/// Fix My Lights should skip disabled rooms entirely — they are not reset.
#[test]
fn disabled_room_excluded_from_fix() {
    let harness = TestHarness::new().with_discovery(
        vec![room("kitchen", "Kitchen"), room("bedroom", "Bedroom")],
        vec![],
    );
    harness.sync();

    // Both rooms on
    harness.action("kitchen", "on").unwrap();
    harness.action("bedroom", "on").unwrap();

    // Disable kitchen
    harness.set_room_preferences("kitchen", None, Some(true), None);

    // -- Action: Fix My Lights --
    let result = harness.fix_my_lights();

    // -- Assert: only bedroom reset --
    assert_eq!(result["rooms_reset"], 1, "only bedroom should be reset");
    assert!(
        harness.lights_on("kitchen"),
        "kitchen lights should be unchanged (fix skipped it)"
    );
}

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

// ============================================================================
// Scenario: Disabled room not in fix response rooms array
// ============================================================================

/// The fix response's rooms array should only contain rooms that were
/// actually reset — disabled rooms should be absent.
#[test]
fn disabled_room_not_in_fix_response() {
    let (rooms, devices) = rooms_with_lights(&[
        ("kitchen", "Kitchen"),
        ("bedroom", "Bedroom"),
        ("office", "Office"),
    ]);
    let harness = TestHarness::new().with_discovery(rooms, devices);
    harness.sync();

    // All on, disable kitchen
    harness.action("kitchen", "on").unwrap();
    harness.action("bedroom", "on").unwrap();
    harness.action("office", "on").unwrap();
    harness.set_room_preferences("kitchen", None, Some(true), None);

    // -- Action: Fix My Lights --
    let result = harness.fix_my_lights();

    // -- Assert: kitchen not in rooms array --
    assert_eq!(result["rooms_reset"], 2, "should reset bedroom + office");
    let rooms_arr = result["rooms"].as_array().expect("rooms should be array");
    let kitchen_resolved = harness.resolve("kitchen");
    let kitchen_in_response = rooms_arr
        .iter()
        .any(|r| r["room_id"].as_str() == Some(&kitchen_resolved));
    assert!(
        !kitchen_in_response,
        "disabled kitchen should not appear in fix response rooms"
    );
}

// ============================================================================
// Scenario: Partial disable — fix only resets enabled rooms
// ============================================================================

/// With a mix of enabled and disabled rooms, fix should only reset the
/// enabled ones.
#[test]
fn partial_disable_mix() {
    let (rooms, devices) = rooms_with_lights(&[
        ("kitchen", "Kitchen"),
        ("bedroom", "Bedroom"),
        ("office", "Office"),
        ("hallway", "Hallway"),
    ]);
    let harness = TestHarness::new().with_discovery(rooms, devices);
    harness.sync();

    // All on
    for id in &["kitchen", "bedroom", "office", "hallway"] {
        harness.action(id, "on").unwrap();
    }

    // Disable two
    harness.set_room_preferences("kitchen", None, Some(true), None);
    harness.set_room_preferences("office", None, Some(true), None);

    // -- Action: Fix My Lights --
    let result = harness.fix_my_lights();

    // -- Assert: only 2 enabled rooms reset --
    assert_eq!(
        result["rooms_reset"], 2,
        "should reset bedroom + hallway only"
    );
    assert!(harness.lights_on("kitchen"), "disabled kitchen unchanged");
    assert!(harness.lights_on("office"), "disabled office unchanged");
}
