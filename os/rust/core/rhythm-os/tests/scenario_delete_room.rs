//! Scenario regression: deleting a topology room detaches devices cleanly.

mod harness;

use harness::{rooms_with_lights, TestHarness};
use rhythm_os::commands;
use rhythm_os::topology::DevicePlacement;

#[test]
fn delete_room_unassigns_devices_and_queues_triage() {
    let (rooms, devices) = rooms_with_lights(&[("office", "Office")]);
    let harness = TestHarness::new().with_discovery(rooms, devices);
    harness.sync();

    let room_id = harness.resolve("office");
    let canonical_id = {
        let state = harness.state.lock().unwrap();
        state
            .canonical_registry
            .find_by_native_id(&harness.hub_key, "light-office")
            .unwrap()
            .id
            .clone()
    };

    commands::do_topology_delete_room(&harness.state, &room_id).unwrap();

    let state = harness.state.lock().unwrap();
    assert!(state.topology.get(&room_id).is_none());
    assert_eq!(state.topology.room_count(), 0);
    assert_eq!(
        state
            .topology
            .get_device_node(&canonical_id)
            .unwrap()
            .parent_id,
        None
    );
    assert_eq!(
        state
            .topology
            .get_device_node(&canonical_id)
            .unwrap()
            .placement,
        DevicePlacement::Standalone
    );
    assert_eq!(
        state
            .canonical_registry
            .get(&canonical_id)
            .unwrap()
            .room_id
            .as_deref(),
        None
    );
    assert_eq!(
        state.canonical_registry.triage().pending_unassigned_count(),
        1
    );
    assert!(state.pending_motion_clear.contains(&room_id));
    let registry = state
        .hubs
        .get(&harness.hub_key)
        .and_then(|hub| hub.registry.as_ref())
        .expect("hub registry should exist")
        .lock()
        .unwrap();
    assert_eq!(
        registry.devices_for_room("light-office"),
        vec!["light-office".to_string()]
    );
    assert_eq!(
        registry.get_grouped_light_id("light-office"),
        Some("light-office".to_string())
    );
    assert!(state
        .hub_runtime()
        .expect("runtime should exist after sync")
        .engine_room_snapshot(&room_id)
        .is_none());
}

#[test]
fn backup_restore_after_room_delete_preserves_unassigned_device_nodes() {
    let (rooms, devices) = rooms_with_lights(&[("office", "Office")]);
    let harness = TestHarness::new().with_discovery(rooms, devices);
    harness.sync();

    let room_id = harness.resolve("office");
    let canonical_id = {
        let state = harness.state.lock().unwrap();
        state
            .canonical_registry
            .find_by_native_id(&harness.hub_key, "light-office")
            .unwrap()
            .id
            .clone()
    };

    commands::do_topology_delete_room(&harness.state, &room_id).unwrap();

    let bundle = commands::build_backup_bundle_dto(&harness.state, false).unwrap();
    assert_eq!(bundle.installation.topology.room_count(), 0);
    assert!(bundle.installation.rooms.get(&room_id).is_none());
    assert!(
        bundle.installation.rooms.get(&canonical_id).is_some(),
        "standalone device runtime state should be backed up"
    );

    let restored = TestHarness::new();
    commands::do_backup_restore(&restored.state, bundle).unwrap();

    let state = restored.state.lock().unwrap();
    assert_eq!(state.topology.room_count(), 0);
    let node = state
        .topology
        .get_device_node(&canonical_id)
        .expect("restored standalone node should exist");
    assert_eq!(node.parent_id, None);
    assert_eq!(node.placement, DevicePlacement::Standalone);
    assert_eq!(
        state
            .canonical_registry
            .get(&canonical_id)
            .unwrap()
            .room_id
            .as_deref(),
        None
    );
}
