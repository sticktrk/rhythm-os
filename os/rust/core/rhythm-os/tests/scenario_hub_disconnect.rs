//! Scenario 3: Hub disconnect → degraded state → reconnect → recovery.
//!
//! Tests that room state survives hub disconnect/reconnect cycles, and that
//! the system degrades gracefully when one hub in a multi-hub setup goes offline.
//!
//! ## API journey
//!
//! 1. Two hubs connected (Hue + HA), Kitchen bound to both
//! 2. Kitchen is ON with rhythm_enabled, hard_off=false
//! 3. Hue bridge goes offline → hub removed from active hubs
//! 4. Room state (rhythm_enabled, offsets) preserved in engine
//! 5. Hub reconnects → re-sync → state restored, Kitchen fully operational

mod harness;

use harness::{rooms_with_lights, TestHarness};

/// Helper: create a harness with two hubs, Kitchen bound to both.
fn setup_bound_kitchen() -> (TestHarness, rhythm_os::canonical::identity::HubKey) {
    let (rooms, devices) =
        rooms_with_lights(&[("hue-kitchen", "Kitchen"), ("hue-bedroom", "Bedroom")]);
    let mut harness = TestHarness::new().with_discovery(rooms, devices);
    harness.sync();

    let ha_key = harness.add_hub("ha", "192.168.1.200");
    let (ha_rooms, ha_devices) = rooms_with_lights(&[("ha-kitchen", "Kitchen")]);
    harness.set_hub_discovery(&ha_key, ha_rooms, ha_devices);
    harness.sync_hub(&ha_key);

    // Resolve Kitchen binding
    let (entry_id, _, _) = harness.triage_room_binding(0).unwrap();
    harness.triage_bind(&entry_id).unwrap();

    assert_eq!(harness.hub_target_count("hue-kitchen"), 2);
    (harness, ha_key)
}

// ============================================================================
// Scenario 3a: Room state preserved through disconnect
// ============================================================================

/// Engine room state (rhythm_enabled, hard_off, offsets) should survive
/// when the hub that owns the room data disappears and reappears.
#[test]
fn room_state_survives_hub_resync() {
    let (harness, _ha_key) = setup_bound_kitchen();
    harness.set_settings(Some(false));

    // -- Setup: modify room state --
    harness.action("hue-kitchen", "on").unwrap();
    harness.action("hue-kitchen", "lights_off").unwrap();

    let snap_before = harness.snapshot("hue-kitchen").unwrap();
    assert!(snap_before.rhythm_enabled);
    assert!(!snap_before.soft_off);
    assert!(snap_before.hard_off);
    assert!(!harness.lights_on("hue-kitchen"));

    // -- Action: re-sync primary hub (simulates disconnect → reconnect) --
    harness.sync();

    // -- Assert: state preserved --
    let snap_after = harness.snapshot("hue-kitchen").unwrap();
    assert!(snap_after.rhythm_enabled, "rhythm_enabled preserved");
    assert!(!snap_after.soft_off, "legacy soft_off collapsed");
    assert!(snap_after.hard_off, "hard_off preserved");
    assert!(!harness.lights_on("hue-kitchen"), "lights_off preserved");
}

// ============================================================================
// Scenario 3b: Re-sync preserves cross-hub topology
// ============================================================================

/// Re-syncing the primary hub should not affect the second hub's targets.
/// The Kitchen room should still have 2 hub targets after re-sync.
#[test]
fn resync_primary_preserves_cross_hub_targets() {
    let (harness, _ha_key) = setup_bound_kitchen();

    // Verify initial state
    assert_eq!(
        harness.hub_target_count("hue-kitchen"),
        2,
        "Kitchen has 2 targets initially"
    );

    // -- Action: re-sync primary (Hue) hub --
    harness.sync();

    // -- Assert: both hub targets still present --
    assert_eq!(
        harness.hub_target_count("hue-kitchen"),
        2,
        "Kitchen should still have 2 hub targets after Hue re-sync"
    );
}

// ============================================================================
// Scenario 3c: Re-sync secondary hub preserves state
// ============================================================================

/// Re-syncing the secondary hub should preserve room state and topology.
#[test]
fn resync_secondary_preserves_state() {
    let (harness, ha_key) = setup_bound_kitchen();

    // Modify state
    harness.action("hue-kitchen", "on").unwrap();

    // -- Action: re-sync HA hub --
    harness.sync_hub(&ha_key);

    // -- Assert: state preserved --
    assert!(harness.lights_on("hue-kitchen"), "lights_on preserved");
    assert_eq!(
        harness.hub_target_count("hue-kitchen"),
        2,
        "Kitchen still has 2 hub targets"
    );
    assert_eq!(harness.topology_room_count(), 2, "room count unchanged");
}

// ============================================================================
// Scenario 3d: Single-hub rooms unaffected by other hub's lifecycle
// ============================================================================

/// When one hub re-syncs, single-hub rooms on the OTHER hub should be
/// completely unaffected.
#[test]
fn single_hub_rooms_unaffected_by_other_hub_resync() {
    let (harness, ha_key) = setup_bound_kitchen();

    // Turn on Bedroom (Hue-only)
    harness.action("hue-bedroom", "on").unwrap();
    let snap_before = harness.snapshot("hue-bedroom").unwrap();

    // -- Action: re-sync HA hub --
    harness.sync_hub(&ha_key);

    // -- Assert: Bedroom state unchanged --
    assert!(
        harness.lights_on("hue-bedroom"),
        "Bedroom lights_on unchanged"
    );
    let snap_after = harness.snapshot("hue-bedroom").unwrap();
    assert_eq!(
        snap_after.rhythm_enabled, snap_before.rhythm_enabled,
        "Bedroom rhythm_enabled unchanged"
    );
}

// ============================================================================
// Scenario 3e: Multiple re-sync cycles maintain consistency
// ============================================================================

/// Room state should remain consistent through multiple re-sync cycles
/// of both hubs (simulates repeated network blips).
#[test]
fn multiple_resync_cycles_maintain_consistency() {
    let (harness, ha_key) = setup_bound_kitchen();

    // Set up initial state
    harness.action("hue-kitchen", "on").unwrap();
    harness.action("hue-bedroom", "on").unwrap();

    // -- Action: 3 re-sync cycles alternating hubs --
    for _ in 0..3 {
        harness.sync(); // Hue re-syncs
        harness.sync_hub(&ha_key); // HA re-syncs
    }

    // -- Assert: everything still works --
    assert!(harness.lights_on("hue-kitchen"), "Kitchen still on");
    assert!(harness.lights_on("hue-bedroom"), "Bedroom still on");
    assert_eq!(
        harness.hub_target_count("hue-kitchen"),
        2,
        "Kitchen still has 2 hub targets"
    );
    assert_eq!(harness.topology_room_count(), 2, "room count stable");

    // Actions still work
    harness.action("hue-kitchen", "lights_off").unwrap();
    assert!(
        !harness.lights_on("hue-kitchen"),
        "Kitchen turned off after re-syncs"
    );
}
