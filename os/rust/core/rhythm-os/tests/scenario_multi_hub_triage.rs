//! Scenario 1: Pair 2nd hub → triage → room binding → unified room grid.
//!
//! Tests the core multi-hub value proposition: pairing a second hub discovers
//! overlapping rooms, generates triage entries (room bindings + device merges),
//! and resolving them produces a unified room list.
//!
//! ## API journey
//!
//! 1. User has Hue paired with rooms: Kitchen, Living Room, Bedroom
//! 2. User pairs Home Assistant → server discovers HA rooms: Kitchen, Living Room, Office
//! 3. TriageScreen shows room binding entries for "Kitchen" and "Living Room" (name match)
//! 4. User merges Kitchen + Living Room, keeps Office separate
//! 5. AllRoomsScreen shows 4 rooms: Kitchen (2 hubs), Living Room (2 hubs),
//!    Bedroom (Hue only), Office (HA only)

mod harness;

use harness::{rooms_with_lights, TestHarness};

// ============================================================================
// Scenario 1a: Sync 2nd hub generates room binding triage entries
// ============================================================================

/// When a second hub discovers rooms with the same names as existing rooms,
/// room binding triage entries should be created for user approval.
#[test]
fn second_hub_sync_generates_room_bindings() {
    // -- Setup: first hub with 3 rooms + lights --
    let (rooms, devices) = rooms_with_lights(&[
        ("hue-kitchen", "Kitchen"),
        ("hue-living", "Living Room"),
        ("hue-bedroom", "Bedroom"),
    ]);
    let mut harness = TestHarness::new().with_discovery(rooms, devices);

    let report = harness.sync();
    assert_eq!(report.rooms_added, 3);
    assert_eq!(
        harness.triage_pending_count(),
        0,
        "no triage yet with single hub"
    );

    // -- Action: add 2nd hub and sync --
    let ha_key = harness.add_hub("ha", "192.168.1.200");
    let (ha_rooms, ha_devices) = rooms_with_lights(&[
        ("ha-kitchen", "Kitchen"),    // name matches Hue Kitchen
        ("ha-living", "Living Room"), // name matches Hue Living Room
        ("ha-office", "Office"),      // unique — no match
    ]);
    harness.set_hub_discovery(&ha_key, ha_rooms, ha_devices);

    let report = harness.sync_hub(&ha_key);
    assert_eq!(
        report.rooms_added, 3,
        "HA rooms added as separate rooms initially"
    );

    // -- Assert: room binding triage entries created for name matches --
    assert_eq!(
        harness.triage_pending_room_count(),
        2,
        "Kitchen and Living Room should have binding proposals"
    );

    // Topology should have 6 rooms initially (3 Hue + 3 HA separate)
    assert_eq!(harness.topology_room_count(), 6);

    // Engine should also have 6 rooms
    assert_eq!(harness.all_snapshots().len(), 6);
}

// ============================================================================
// Scenario 1b: Resolve room binding merges topology rooms
// ============================================================================

/// Approving a room binding should merge the HA silo room into the Hue room,
/// removing the duplicate from topology and engine.
#[test]
fn resolve_room_binding_merges_rooms() {
    // -- Setup: two hubs with overlapping room names --
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

    // Verify initial state
    assert_eq!(harness.topology_room_count(), 6);
    assert_eq!(harness.triage_pending_room_count(), 2);

    // -- Action: resolve first room binding --
    let (entry_id, name, _) = harness
        .triage_room_binding(0)
        .expect("should have first binding");
    // One of the two bindings is for Kitchen or Living Room
    assert!(
        name == "Kitchen" || name == "Living Room",
        "first binding should be Kitchen or Living Room, got '{}'",
        name
    );
    harness.triage_bind(&entry_id).expect("bind should succeed");

    // -- Assert: one fewer topology room, one binding resolved --
    assert_eq!(
        harness.topology_room_count(),
        5,
        "merged room removed from topology"
    );
    assert_eq!(
        harness.triage_pending_room_count(),
        1,
        "one binding resolved"
    );

    // The merged room should have 2 hub targets
    let hue_room_id = if name == "Kitchen" {
        "hue-kitchen"
    } else {
        "hue-living"
    };
    assert_eq!(
        harness.hub_target_count(hue_room_id),
        2,
        "merged room should have targets from both hubs"
    );
}

// ============================================================================
// Scenario 1c: Full multi-hub triage resolution
// ============================================================================

/// Complete multi-hub workflow: sync, resolve all triage entries, verify
/// final unified room state.
#[test]
fn full_triage_resolution_produces_unified_rooms() {
    // -- Setup: Hue hub --
    let (rooms, devices) = rooms_with_lights(&[
        ("hue-kitchen", "Kitchen"),
        ("hue-living", "Living Room"),
        ("hue-bedroom", "Bedroom"),
    ]);
    let mut harness = TestHarness::new().with_discovery(rooms, devices);
    harness.sync();

    // -- Setup: HA hub --
    let ha_key = harness.add_hub("ha", "192.168.1.200");
    let (ha_rooms, ha_devices) = rooms_with_lights(&[
        ("ha-kitchen", "Kitchen"),
        ("ha-living", "Living Room"),
        ("ha-office", "Office"),
    ]);
    harness.set_hub_discovery(&ha_key, ha_rooms, ha_devices);
    harness.sync_hub(&ha_key);

    // -- Action: resolve ALL room binding triage entries --
    while harness.triage_pending_room_count() > 0 {
        let (entry_id, _, _) = harness.triage_room_binding(0).unwrap();
        harness.triage_bind(&entry_id).expect("bind should succeed");
    }

    // -- Assert: final state --
    // 4 rooms: Kitchen (2 hubs), Living Room (2 hubs), Bedroom (Hue), Office (HA)
    assert_eq!(
        harness.topology_room_count(),
        4,
        "should have 4 unified rooms"
    );

    // Kitchen and Living Room should have 2 hub targets each
    assert_eq!(
        harness.hub_target_count("hue-kitchen"),
        2,
        "Kitchen should have 2 hub targets"
    );
    assert_eq!(
        harness.hub_target_count("hue-living"),
        2,
        "Living Room should have 2 hub targets"
    );

    // Bedroom and Office should have 1 hub target each
    assert_eq!(
        harness.hub_target_count("hue-bedroom"),
        1,
        "Bedroom should have 1 hub target"
    );
    assert_eq!(
        harness.hub_target_count("ha-office"),
        1,
        "Office should have 1 hub target"
    );

    // Triage room bindings should be empty
    assert_eq!(
        harness.triage_pending_room_count(),
        0,
        "all room bindings resolved"
    );

    // Engine should have 4 rooms (merged rooms removed)
    assert_eq!(
        harness.all_snapshots().len(),
        4,
        "engine should have 4 rooms"
    );
}

// ============================================================================
// Scenario 1d: Re-sync after approved binding re-applies silently
// ============================================================================

/// After approving a room binding, re-syncing the same hub should re-apply
/// the binding automatically without creating a new triage entry.
#[test]
fn re_sync_after_approved_binding_re_applies_silently() {
    // -- Setup: two hubs, resolve Kitchen binding --
    let (rooms, devices) =
        rooms_with_lights(&[("hue-kitchen", "Kitchen"), ("hue-bedroom", "Bedroom")]);
    let mut harness = TestHarness::new().with_discovery(rooms, devices);
    harness.sync();

    let ha_key = harness.add_hub("ha", "192.168.1.200");
    let (ha_rooms, ha_devices) = rooms_with_lights(&[("ha-kitchen", "Kitchen")]);
    harness.set_hub_discovery(&ha_key, ha_rooms, ha_devices);
    harness.sync_hub(&ha_key);

    // Resolve the Kitchen binding
    let (entry_id, name, _) = harness.triage_room_binding(0).expect("should have binding");
    assert_eq!(name, "Kitchen");
    harness.triage_bind(&entry_id).expect("bind should succeed");

    let room_count_after_bind = harness.topology_room_count();
    assert_eq!(room_count_after_bind, 2, "Kitchen merged, Bedroom stays");
    assert_eq!(harness.hub_target_count("hue-kitchen"), 2);
    assert_eq!(harness.triage_pending_room_count(), 0);

    // -- Action: re-sync HA hub (simulates hub reconnect) --
    harness.sync_hub(&ha_key);

    // -- Assert: binding re-applied silently --
    assert_eq!(
        harness.topology_room_count(),
        room_count_after_bind,
        "room count unchanged after re-sync"
    );
    assert_eq!(
        harness.hub_target_count("hue-kitchen"),
        2,
        "Kitchen still has 2 hub targets"
    );
    assert_eq!(
        harness.triage_pending_room_count(),
        0,
        "no new triage entries — binding re-applied"
    );
}

// ============================================================================
// Scenario 1e: Keep separate / dismiss preserves distinct rooms
// ============================================================================

/// When the user dismisses a room binding, the rooms remain separate.
#[test]
fn dismiss_binding_preserves_distinct_rooms() {
    let (rooms, devices) = rooms_with_lights(&[("hue-kitchen", "Kitchen")]);
    let mut harness = TestHarness::new().with_discovery(rooms, devices);
    harness.sync();

    let ha_key = harness.add_hub("ha", "192.168.1.200");
    let (ha_rooms, ha_devices) = rooms_with_lights(&[("ha-kitchen", "Kitchen")]);
    harness.set_hub_discovery(&ha_key, ha_rooms, ha_devices);
    harness.sync_hub(&ha_key);

    assert_eq!(
        harness.topology_room_count(),
        2,
        "two separate Kitchen rooms"
    );
    assert_eq!(harness.triage_pending_room_count(), 1);

    // -- Action: dismiss the binding --
    let (entry_id, _, _) = harness.triage_room_binding(0).unwrap();
    harness
        .triage_dismiss(&entry_id)
        .expect("dismiss should succeed");

    // -- Assert: rooms remain separate --
    assert_eq!(harness.topology_room_count(), 2, "still two Kitchen rooms");
    assert_eq!(
        harness.triage_pending_room_count(),
        0,
        "room binding dismissed"
    );
    assert_eq!(
        harness.hub_target_count("hue-kitchen"),
        1,
        "Hue Kitchen has 1 target"
    );
    assert_eq!(
        harness.hub_target_count("ha-kitchen"),
        1,
        "HA Kitchen has 1 target"
    );
}
