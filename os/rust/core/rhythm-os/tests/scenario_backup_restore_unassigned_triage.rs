//! Scenario regression: backup/restore must preserve pending unassigned-device review.

mod harness;

use harness::{rooms_with_lights, TestHarness};
use rhythm_os::canonical::triage::TriageKind;
use rhythm_os::commands;
use rhythm_os::topology::DevicePlacement;

#[test]
fn backup_restore_after_room_delete_preserves_unassigned_triage() {
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
    let restored = TestHarness::new();
    commands::do_backup_restore(&restored.state, bundle).unwrap();

    let state = restored.state.lock().unwrap();
    let node = state
        .topology
        .get_device_node(&canonical_id)
        .expect("restored standalone node should exist");
    let pending = state
        .canonical_registry
        .triage()
        .pending_by_kind(TriageKind::UnassignedDevice);

    assert_eq!(state.topology.room_count(), 0);
    assert_eq!(node.parent_id, None);
    assert_eq!(node.placement, DevicePlacement::Standalone);
    assert_eq!(
        state.canonical_registry.triage().pending_unassigned_count(),
        1
    );
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].canonical_id.as_deref(),
        Some(canonical_id.as_str())
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
    assert!(
        state
            .hub_runtime()
            .and_then(|runtime| runtime.engine_node_snapshot(&canonical_id))
            .is_none(),
        "restored pending device should stay outside the automatic runtime"
    );
}
