//! Scenario regression: redacted restore can reconnect multi-hub state one hub at a time.

mod harness;

use harness::{light, room, TestHarness};
use rhythm_os::canonical::triage::TriageKind;
use rhythm_os::commands;

fn restore_multi_hub_backup(resolve_device_merge: bool) -> (TestHarness, String, Option<String>) {
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
    assert!(
        restored.state.lock().unwrap().hubs.is_empty(),
        "redacted restore should preserve installation state without connected hubs"
    );

    (restored, merged_room_id, canonical_id)
}

#[test]
fn backup_restore_then_late_missing_hub_reconnect_keeps_room_binding_silent() {
    let (mut restored, merged_room_id, _) = restore_multi_hub_backup(false);

    let primary_key = restored.add_hub("mock", "192.168.1.100");
    restored.set_hub_discovery(
        &primary_key,
        vec![room("mock-kitchen", "Kitchen")],
        vec![light("lamp-1", "mock-kitchen")],
    );
    restored.sync_hub(&primary_key);

    {
        let state = restored.state.lock().unwrap();
        assert_eq!(state.topology.room_count(), 1);
        assert_eq!(
            state
                .topology
                .translate_room_id(&primary_key, "mock-kitchen"),
            Some(merged_room_id.as_str())
        );
        assert_eq!(state.canonical_registry.triage().pending_room_count(), 0);
        assert_eq!(
            state
                .canonical_registry
                .triage()
                .pending_by_kind(TriageKind::DeviceMerge)
                .len(),
            1,
            "primary-hub reconnect should not duplicate the existing merge proposal"
        );
    }

    let ha_key = restored.add_hub("ha", "192.168.1.200");
    restored.set_hub_discovery(
        &ha_key,
        vec![room("ha-kitchen", "Kitchen")],
        vec![light("lamp-1", "ha-kitchen")],
    );
    restored.sync_hub(&ha_key);

    let state = restored.state.lock().unwrap();
    assert_eq!(state.topology.room_count(), 1);
    assert_eq!(
        state.topology.translate_room_id(&ha_key, "ha-kitchen"),
        Some(merged_room_id.as_str()),
        "late hub reconnect should reapply the approved room binding silently"
    );
    assert_eq!(state.canonical_registry.triage().pending_room_count(), 0);
    assert_eq!(
        state
            .canonical_registry
            .triage()
            .pending_by_kind(TriageKind::DeviceMerge)
            .len(),
        1,
        "late hub reconnect should not create a duplicate merge proposal"
    );
    assert_eq!(
        state.canonical_registry.device_count(),
        2,
        "late reconnect should not create duplicate silo canonical devices"
    );
}

#[test]
fn backup_restore_then_late_missing_hub_reconnect_reuses_merged_canonical_device() {
    let (mut restored, merged_room_id, canonical_id) = restore_multi_hub_backup(true);
    let canonical_id = canonical_id.expect("merged device id should be captured");

    let primary_key = restored.add_hub("mock", "192.168.1.100");
    restored.set_hub_discovery(
        &primary_key,
        vec![room("mock-kitchen", "Kitchen")],
        vec![light("lamp-1", "mock-kitchen")],
    );
    restored.sync_hub(&primary_key);

    {
        let state = restored.state.lock().unwrap();
        assert_eq!(state.topology.room_count(), 1);
        assert_eq!(state.canonical_registry.device_count(), 1);
        assert_eq!(state.canonical_registry.triage().pending_room_count(), 0);
        assert_eq!(state.canonical_registry.triage().pending_device_count(), 0);
        assert_eq!(
            state
                .canonical_registry
                .find_by_native_id(&primary_key, "lamp-1")
                .expect("primary endpoint should survive reconnect")
                .id,
            canonical_id
        );
    }

    let ha_key = restored.add_hub("ha", "192.168.1.200");
    restored.set_hub_discovery(
        &ha_key,
        vec![room("ha-kitchen", "Kitchen")],
        vec![light("lamp-1", "ha-kitchen")],
    );
    restored.sync_hub(&ha_key);

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
        .find_by_native_id(&primary_key, "lamp-1")
        .expect("primary endpoint should exist");
    let secondary_device = state
        .canonical_registry
        .find_by_native_id(&ha_key, "lamp-1")
        .expect("late-reconnected endpoint should exist");
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
            .expect("merged device node should exist")
            .parent_id
            .as_deref(),
        Some(merged_room_id.as_str())
    );

    let runtime = state
        .hub_runtime()
        .expect("late reconnect should recreate a runtime");
    assert_eq!(
        runtime
            .engine_node_snapshot(&canonical_id)
            .expect("merged runtime node should exist")
            .parent_id
            .as_deref(),
        Some(merged_room_id.as_str())
    );
}
