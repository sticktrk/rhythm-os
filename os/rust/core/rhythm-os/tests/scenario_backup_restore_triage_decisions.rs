//! Scenario regression: backup/restore must preserve room-binding review decisions.

mod harness;

use harness::{light, room, TestHarness};
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::canonical::triage::{TriageKind, TriageStatus};
use rhythm_os::commands;

fn restore_with_room_binding_resolution(
    resolve: impl FnOnce(&TestHarness, &str),
) -> (TestHarness, HubKey, HubKey) {
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
    resolve(&harness, &entry_id);

    let bundle = commands::build_backup_bundle_dto(&harness.state, false).unwrap();

    let restored = TestHarness::new();
    commands::do_backup_restore(&restored.state, bundle).unwrap();

    let mut restored = restored;
    let primary_key = restored.add_hub("mock", "192.168.1.100");
    let restored_ha_key = restored.add_hub("ha", "192.168.1.200");
    restored.set_hub_discovery(
        &primary_key,
        vec![room("mock-kitchen", "Kitchen")],
        vec![light("lamp-1", "mock-kitchen")],
    );
    restored.set_hub_discovery(
        &restored_ha_key,
        vec![room("ha-kitchen", "Kitchen")],
        vec![light("lamp-1", "ha-kitchen")],
    );

    (restored, primary_key, restored_ha_key)
}

#[test]
fn backup_restore_kept_separate_room_binding_stays_resolved_on_first_sync() {
    let (restored, primary_key, ha_key) =
        restore_with_room_binding_resolution(|harness, entry_id| {
            let result = harness.triage_new(entry_id).unwrap();
            assert_eq!(result, r#"{"status":"kept_separate"}"#);
        });

    restored.sync_all();

    let primary_room_id = restored.resolve("mock-kitchen");
    let secondary_room_id = restored.resolve_for_hub(&ha_key, "ha-kitchen");
    let state = restored.state.lock().unwrap();

    assert_eq!(state.topology.room_count(), 2);
    assert_ne!(
        primary_room_id, secondary_room_id,
        "kept-separate decision should preserve distinct topology rooms"
    );
    assert_eq!(
        state
            .topology
            .translate_room_id(&primary_key, "mock-kitchen"),
        Some(primary_room_id.as_str())
    );
    assert_eq!(
        state.topology.translate_room_id(&ha_key, "ha-kitchen"),
        Some(secondary_room_id.as_str())
    );
    assert_eq!(state.canonical_registry.triage().pending_room_count(), 0);
    assert!(
        state.canonical_registry.triage().all().iter().any(|entry| {
            entry.kind == TriageKind::RoomBinding
                && entry.hub_key == ha_key
                && entry.status == TriageStatus::KeptSeparate
        }),
        "kept-separate room binding decision should survive restore"
    );
}

#[test]
fn backup_restore_dismissed_room_binding_stays_separate_on_first_sync() {
    let (restored, primary_key, ha_key) =
        restore_with_room_binding_resolution(|harness, entry_id| {
            harness.triage_dismiss(entry_id).unwrap();
        });

    restored.sync_all();

    let primary_room_id = restored.resolve("mock-kitchen");
    let secondary_room_id = restored.resolve_for_hub(&ha_key, "ha-kitchen");
    let state = restored.state.lock().unwrap();
    let room_binding_entries: Vec<_> = state
        .canonical_registry
        .triage()
        .all()
        .iter()
        .filter(|entry| entry.kind == TriageKind::RoomBinding && entry.hub_key == ha_key)
        .collect();

    assert_eq!(state.topology.room_count(), 2);
    assert_ne!(
        primary_room_id, secondary_room_id,
        "dismiss should not silently merge rooms on a later sync"
    );
    assert_eq!(
        state
            .topology
            .translate_room_id(&primary_key, "mock-kitchen"),
        Some(primary_room_id.as_str())
    );
    assert_eq!(
        state.topology.translate_room_id(&ha_key, "ha-kitchen"),
        Some(secondary_room_id.as_str())
    );
    assert_eq!(
        state.canonical_registry.triage().pending_room_count(),
        0,
        "restore should not silently recreate pending room-binding work"
    );
    assert!(
        room_binding_entries
            .iter()
            .any(|entry| entry.status == TriageStatus::Dismissed),
        "dismissed history should survive restore"
    );
    assert!(
        room_binding_entries
            .iter()
            .all(|entry| entry.status != TriageStatus::Pending),
        "dismissed binding should remain resolved after restore and first sync"
    );
}
