//! Scenario regression: backup/restore stays room-centric with periodic nodes.

mod harness;

use std::sync::Arc;

use harness::{light, rooms_with_lights, TestHarness};
use rhythm_core::CompositeController;
use rhythm_os::commands;
use rhythm_os::topology::NodeControlKind;

#[test]
fn backup_restore_uses_public_room_ids_not_internal_light_nodes() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let harness = TestHarness::new().with_discovery(rooms, devices);
    harness.sync();

    let public_room_id = harness.resolve("kitchen");
    {
        let mut state = harness.state.lock().unwrap();
        state.composite_controller = Some(Arc::new(CompositeController::new()));
    }

    let bundle = commands::build_backup_bundle_dto(&harness.state, false).unwrap();
    let mut backup_room_ids: Vec<_> = bundle
        .installation
        .rooms
        .iter()
        .map(|room| room.id.clone())
        .collect();
    backup_room_ids.sort();
    assert!(backup_room_ids.contains(&public_room_id));
    assert!(backup_room_ids
        .iter()
        .all(|id| !id.contains("__rhythm_light_node__")));

    let expected_device_node_ids: Vec<_> = bundle
        .installation
        .topology
        .device_nodes()
        .map(|node| node.id.clone())
        .collect();
    for node_id in expected_device_node_ids {
        assert!(backup_room_ids.contains(&node_id));
    }

    let restored = TestHarness::new();
    commands::do_backup_restore(&restored.state, bundle).unwrap();

    let restored_topology_room_ids: Vec<_> = restored
        .state
        .lock()
        .unwrap()
        .topology
        .rooms()
        .map(|room| room.id.clone())
        .collect();
    assert_eq!(restored_topology_room_ids, vec![public_room_id]);
    assert!(restored_topology_room_ids
        .iter()
        .all(|id| !id.contains("__rhythm_light_node__")));
}

#[test]
fn backup_restore_preserves_standalone_device_nodes() {
    let harness = TestHarness::new().with_discovery(vec![], vec![light("matter-100", "")]);
    harness.sync();

    let triage_id = harness
        .triage_pending_ids()
        .into_iter()
        .next()
        .expect("roomless light should require an explicit standalone decision");
    let result = harness.triage_new(&triage_id).unwrap();
    assert!(result.contains(r#""status":"standalone""#));

    let canonical_id = {
        let state = harness.state.lock().unwrap();
        state
            .canonical_registry
            .find_by_native_id(&harness.hub_key, "matter-100")
            .unwrap()
            .id
            .clone()
    };

    {
        let mut state = harness.state.lock().unwrap();
        state.composite_controller = Some(Arc::new(CompositeController::new()));
    }

    let bundle = commands::build_backup_bundle_dto(&harness.state, false).unwrap();
    assert!(
        bundle
            .installation
            .topology
            .get_device_node(&canonical_id)
            .is_some(),
        "backup topology should include standalone device node"
    );
    assert!(
        bundle.installation.rooms.get(&canonical_id).is_some(),
        "backup runtime state should include standalone device node"
    );

    let restored = TestHarness::new();
    commands::do_backup_restore(&restored.state, bundle).unwrap();

    let state = restored.state.lock().unwrap();
    let node = state
        .topology
        .get_device_node(&canonical_id)
        .expect("restored standalone topology device node should exist");
    assert_eq!(node.parent_id, None);
    assert!(state
        .canonical_registry
        .get(&canonical_id)
        .is_some_and(|device| device.room_id.is_none()));
}

#[test]
fn backup_restore_preserves_user_owned_device_name_across_room_assignment() {
    let harness = TestHarness::new().with_discovery(vec![], vec![light("matter-override", "")]);
    harness.sync();

    let canonical_id = {
        let state = harness.state.lock().unwrap();
        state
            .canonical_registry
            .find_by_native_id(&harness.hub_key, "matter-override")
            .unwrap()
            .id
            .clone()
    };
    commands::do_canonical_rename_device(&harness.state, &canonical_id, "Cozy Corner").unwrap();
    {
        let state = harness.state.lock().unwrap();
        assert!(!state
            .canonical_registry
            .automatic_name_eligible(&canonical_id));
    }

    {
        let mut state = harness.state.lock().unwrap();
        state.composite_controller = Some(Arc::new(CompositeController::new()));
    }
    let bundle = commands::build_backup_bundle_dto(&harness.state, false).unwrap();
    let restored = TestHarness::new();
    commands::do_backup_restore(&restored.state, bundle).unwrap();

    let room = commands::do_topology_create_room(&restored.state, "Office").unwrap();
    let room: serde_json::Value = serde_json::from_str(&room).unwrap();
    let room_id = room["id"].as_str().unwrap();
    commands::do_canonical_assign_room(&restored.state, &canonical_id, Some(room_id)).unwrap();

    let state = restored.state.lock().unwrap();
    let device = state.canonical_registry.get(&canonical_id).unwrap();
    assert_eq!(device.name, "Cozy Corner");
    assert!(!state
        .canonical_registry
        .automatic_name_eligible(&canonical_id));
}

#[test]
fn backup_restore_preserves_topology_control_links() {
    let harness = TestHarness::new().with_discovery(vec![], vec![light("matter-100", "")]);
    harness.sync();

    let canonical_id = {
        let state = harness.state.lock().unwrap();
        state
            .canonical_registry
            .find_by_native_id(&harness.hub_key, "matter-100")
            .unwrap()
            .id
            .clone()
    };

    let room_id = {
        let created = commands::do_topology_create_room(&harness.state, "Office").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&created).unwrap();
        parsed["id"].as_str().unwrap().to_string()
    };

    commands::do_topology_set_control_target(
        &harness.state,
        &canonical_id,
        NodeControlKind::Button,
        Some(&room_id),
    )
    .unwrap();

    let bundle = commands::build_backup_bundle_dto(&harness.state, false).unwrap();
    assert_eq!(
        bundle
            .installation
            .topology
            .explicit_control_target(&canonical_id, &NodeControlKind::Button),
        Some(room_id.as_str())
    );

    let restored = TestHarness::new();
    commands::do_backup_restore(&restored.state, bundle).unwrap();

    let state = restored.state.lock().unwrap();
    assert_eq!(
        state
            .topology
            .explicit_control_target(&canonical_id, &NodeControlKind::Button),
        Some(room_id.as_str())
    );
}
