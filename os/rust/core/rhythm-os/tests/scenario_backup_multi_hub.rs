//! Scenario regression: multi-hub backup/restore preserves merged topology and canonical state.

mod harness;

use harness::{light, room, TestHarness};
use rhythm_os::canonical::triage::TriageKind;
use rhythm_os::commands;

#[test]
fn backup_restore_preserves_approved_multi_hub_room_binding() {
    let harness = TestHarness::new().with_discovery(
        vec![room("mock-kitchen", "Kitchen")],
        vec![light("lamp-1", "mock-kitchen")],
    );
    harness.sync();

    let mut harness = harness;
    let ha_key = harness.add_hub("ha", "192.168.1.200");
    harness.set_hub_discovery(
        &ha_key,
        vec![room("ha-kitchen", "Kitchen")],
        vec![light("lamp-1", "ha-kitchen")],
    );
    harness.sync_hub(&ha_key);

    let (entry_id, _, _) = harness
        .triage_room_binding(0)
        .expect("room binding triage should exist");
    harness.triage_bind(&entry_id).expect("bind should succeed");

    let merged_room_id = harness.resolve("mock-kitchen");
    let bundle = commands::build_backup_bundle_dto(&harness.state, false).unwrap();

    let restored = TestHarness::new();
    commands::do_backup_restore(&restored.state, bundle).unwrap();

    let state = restored.state.lock().unwrap();
    assert_eq!(state.topology.room_count(), 1);
    assert_eq!(
        state.topology.translate_room_id(&ha_key, "ha-kitchen"),
        Some(merged_room_id.as_str()),
        "approved room binding should survive restore"
    );
    assert_eq!(
        state
            .topology
            .get(&merged_room_id)
            .expect("restored merged room should exist")
            .hub_room_bindings
            .len(),
        2,
        "restored merged room should still target both hubs"
    );
    assert_eq!(state.canonical_registry.triage().pending_room_count(), 0);
}

#[test]
fn backup_restore_preserves_multi_hub_canonical_device_merge() {
    let harness = TestHarness::new().with_discovery(
        vec![room("mock-kitchen", "Kitchen")],
        vec![light("lamp-1", "mock-kitchen")],
    );
    harness.sync();

    let mut harness = harness;
    let ha_key = harness.add_hub("ha", "192.168.1.200");
    harness.set_hub_discovery(
        &ha_key,
        vec![room("ha-kitchen", "Kitchen")],
        vec![light("lamp-1", "ha-kitchen")],
    );
    harness.sync_hub(&ha_key);

    let (room_entry_id, _, _) = harness
        .triage_room_binding(0)
        .expect("room binding triage should exist");
    harness
        .triage_bind(&room_entry_id)
        .expect("room bind should succeed");

    let (merge_entry_id, canonical_id) = {
        let state = harness.state.lock().unwrap();
        let entry = state
            .canonical_registry
            .triage()
            .pending_by_kind(TriageKind::DeviceMerge)
            .into_iter()
            .next()
            .expect("device merge triage should exist");
        (
            entry.id.clone(),
            entry.candidate_matches[0].canonical_id.clone(),
        )
    };
    commands::do_triage_merge(&harness.state, &merge_entry_id, &canonical_id).unwrap();

    let merged_room_id = harness.resolve("mock-kitchen");
    let primary_hub_key = harness.hub_key.clone();
    let bundle = commands::build_backup_bundle_dto(&harness.state, false).unwrap();

    let restored = TestHarness::new();
    commands::do_backup_restore(&restored.state, bundle).unwrap();

    let state = restored.state.lock().unwrap();
    assert_eq!(state.topology.room_count(), 1);
    assert_eq!(state.canonical_registry.device_count(), 1);
    assert_eq!(state.canonical_registry.triage().pending_device_count(), 0);

    let primary_device = state
        .canonical_registry
        .find_by_native_id(&primary_hub_key, "lamp-1")
        .expect("primary hub endpoint should survive restore");
    let secondary_device = state
        .canonical_registry
        .find_by_native_id(&ha_key, "lamp-1")
        .expect("secondary hub endpoint should survive restore");

    assert_eq!(primary_device.id, canonical_id);
    assert_eq!(secondary_device.id, canonical_id);
    assert_eq!(primary_device.active_endpoints().count(), 2);
    assert_eq!(
        primary_device.room_id.as_deref(),
        Some(merged_room_id.as_str())
    );
    assert_eq!(
        state
            .topology
            .get_device_node(&canonical_id)
            .expect("restored merged device node should exist")
            .parent_id
            .as_deref(),
        Some(merged_room_id.as_str())
    );
}
