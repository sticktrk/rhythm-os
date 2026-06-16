//! Scenario regression: backup restore should survive the first live sync without recreating drift.

mod harness;

use harness::{light, room, TestHarness};
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::canonical::triage::TriageKind;
use rhythm_os::commands;

fn restore_and_reattach_multi_hub_state(
    resolve_device_merge: bool,
) -> (TestHarness, HubKey, String, Option<String>) {
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

    let merged_room_id = harness.resolve("mock-kitchen");
    let canonical_id = if resolve_device_merge {
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
        Some(canonical_id)
    } else {
        None
    };

    let bundle = commands::build_backup_bundle_dto(&harness.state, false).unwrap();

    let restored = TestHarness::new();
    commands::do_backup_restore(&restored.state, bundle).unwrap();

    let mut restored = restored;
    let restored_primary_key = restored.add_hub("mock", "192.168.1.100");
    let restored_ha_key = restored.add_hub("ha", "192.168.1.200");
    restored.set_hub_discovery(
        &restored_primary_key,
        vec![room("mock-kitchen", "Kitchen")],
        vec![light("lamp-1", "mock-kitchen")],
    );
    restored.set_hub_discovery(
        &restored_ha_key,
        vec![room("ha-kitchen", "Kitchen")],
        vec![light("lamp-1", "ha-kitchen")],
    );

    (restored, restored_ha_key, merged_room_id, canonical_id)
}

#[test]
fn backup_restore_then_first_sync_keeps_room_binding_approved_without_duplicate_triage() {
    let (restored, ha_key, merged_room_id, _) = restore_and_reattach_multi_hub_state(false);

    restored.sync_all();

    let state = restored.state.lock().unwrap();
    assert_eq!(state.topology.room_count(), 1);
    assert_eq!(
        state.topology.translate_room_id(&ha_key, "ha-kitchen"),
        Some(merged_room_id.as_str()),
        "first sync after restore should reapply the approved room binding silently"
    );
    assert_eq!(
        state
            .topology
            .get(&merged_room_id)
            .expect("merged room should exist")
            .hub_room_bindings
            .len(),
        2
    );
    assert_eq!(state.canonical_registry.triage().pending_room_count(), 0);
    assert_eq!(
        state
            .canonical_registry
            .triage()
            .pending_by_kind(TriageKind::DeviceMerge)
            .len(),
        1,
        "first sync should preserve the existing device merge proposal without duplicating it"
    );
    assert_eq!(
        state.canonical_registry.device_count(),
        2,
        "first sync should not create duplicate canonical devices"
    );

    let primary_device = state
        .canonical_registry
        .find_by_native_id(&restored.hub_key, "lamp-1")
        .expect("primary hub endpoint should survive first sync");
    let secondary_device = state
        .canonical_registry
        .find_by_native_id(&ha_key, "lamp-1")
        .expect("secondary hub endpoint should survive first sync");
    assert_ne!(
        primary_device.id, secondary_device.id,
        "unresolved merge should still have two canonical devices"
    );
    assert_eq!(
        state
            .topology
            .get_device_node(&primary_device.id)
            .expect("primary topology device should exist")
            .parent_id
            .as_deref(),
        Some(merged_room_id.as_str()),
        "primary hub light should stay inside the merged room after restore + first sync"
    );
    assert_eq!(
        state
            .topology
            .get_device_node(&secondary_device.id)
            .expect("secondary topology device should exist")
            .parent_id
            .as_deref(),
        Some(merged_room_id.as_str()),
        "secondary hub light should stay inside the merged room after restore + first sync"
    );
}

#[test]
fn backup_restore_then_first_sync_preserves_merged_device_without_requeueing_triage() {
    let (restored, ha_key, merged_room_id, canonical_id) =
        restore_and_reattach_multi_hub_state(true);
    let canonical_id = canonical_id.expect("merged device id should be captured");

    restored.sync_all();

    let state = restored.state.lock().unwrap();
    assert_eq!(state.topology.room_count(), 1);
    assert_eq!(state.canonical_registry.device_count(), 1);
    assert_eq!(state.canonical_registry.triage().pending_room_count(), 0);
    assert_eq!(state.canonical_registry.triage().pending_device_count(), 0);
    assert_eq!(
        state.topology.translate_room_id(&ha_key, "ha-kitchen"),
        Some(merged_room_id.as_str())
    );

    let primary_device = state
        .canonical_registry
        .find_by_native_id(&restored.hub_key, "lamp-1")
        .expect("primary hub endpoint should survive first sync");
    let secondary_device = state
        .canonical_registry
        .find_by_native_id(&ha_key, "lamp-1")
        .expect("secondary hub endpoint should survive first sync");
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
            .expect("merged topology device node should exist")
            .parent_id
            .as_deref(),
        Some(merged_room_id.as_str())
    );

    let runtime = state
        .hub_runtime()
        .expect("first sync should recreate a runtime");
    assert!(
        runtime.engine_room_snapshot(&merged_room_id).is_some(),
        "merged room should be present in the runtime after sync"
    );
    assert_eq!(
        runtime
            .engine_node_snapshot(&canonical_id)
            .expect("merged runtime node should exist")
            .parent_id
            .as_deref(),
        Some(merged_room_id.as_str())
    );
    assert_eq!(
        runtime.engine_all_node_snapshots().len(),
        2,
        "runtime should contain exactly the merged room and merged device"
    );
}
