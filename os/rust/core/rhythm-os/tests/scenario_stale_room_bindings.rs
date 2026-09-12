//! Room review proposals must follow current topology, including after restart.

mod harness;

use harness::{light, room, rooms_with_lights, TestHarness};
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::commands;
use rhythm_os::storage::{load_persisted_state, FileStorage, Storage};
use std::sync::Arc;

fn pending_binding() -> (TestHarness, HubKey, String, String) {
    let mut harness = TestHarness::new().with_discovery(
        vec![room("primary-kitchen", "Kitchen")],
        vec![light("primary-lamp", "primary-kitchen")],
    );
    harness.sync();
    let secondary = harness.add_hub("ha", "192.0.2.2");
    harness.set_hub_discovery(
        &secondary,
        vec![room("secondary-kitchen", "Kitchen")],
        vec![light("secondary-lamp", "secondary-kitchen")],
    );
    harness.sync_hub(&secondary);
    let (entry_id, _, target_id) = harness.triage_room_binding(0).unwrap();
    (harness, secondary, entry_id, target_id)
}

fn assert_no_room_proposals(harness: &TestHarness) {
    assert_eq!(harness.triage_pending_room_count(), 0);
    let queue: serde_json::Value =
        serde_json::from_str(&commands::build_triage_queue(&harness.state).unwrap()).unwrap();
    assert!(queue
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["kind"] != "room_binding"));
    let counts: serde_json::Value =
        serde_json::from_str(&commands::build_triage_count(&harness.state).unwrap()).unwrap();
    assert_eq!(counts["rooms"], 0);
}

#[test]
fn source_room_disappearing_retires_its_pending_merge() {
    let (harness, secondary, old_entry_id, _) = pending_binding();
    harness.set_hub_discovery(&secondary, vec![], vec![]);
    harness.sync_hub(&secondary);
    assert_no_room_proposals(&harness);

    // Expiry must not become a permanent user decision: rediscovery can
    // propose a fresh merge, which still requires approval.
    harness.set_hub_discovery(
        &secondary,
        vec![room("secondary-kitchen", "Kitchen")],
        vec![light("secondary-lamp", "secondary-kitchen")],
    );
    harness.sync_hub(&secondary);
    assert_eq!(harness.triage_pending_room_count(), 1);
    let (entry_id, _, _) = harness.triage_room_binding(0).unwrap();
    assert_ne!(entry_id, old_entry_id);
    assert!(harness.triage_bind(&old_entry_id).is_err());
    harness.triage_bind(&entry_id).unwrap();
    assert_no_room_proposals(&harness);
}

#[test]
fn deleting_the_only_target_retires_its_pending_merge() {
    let (harness, _, _, target_id) = pending_binding();
    commands::do_topology_delete_room(&harness.state, &target_id).unwrap();
    assert_no_room_proposals(&harness);
}

#[test]
fn manual_merge_retires_the_now_redundant_proposal() {
    let (harness, secondary, _, target_id) = pending_binding();
    let source_id = harness.resolve_for_hub(&secondary, "secondary-kitchen");
    commands::do_topology_merge_rooms(&harness.state, &target_id, &source_id).unwrap();
    assert_no_room_proposals(&harness);
    assert_eq!(harness.topology_room_count(), 1);
}

#[test]
fn deleting_one_candidate_keeps_a_live_candidate_available_for_approval() {
    let (harness, secondary, entry_id, target_id) = pending_binding();
    let other_target = {
        let mut state = harness.state.lock().unwrap();
        let other = state.topology.create_room("New Kitchen");
        let proposal = state
            .canonical_registry
            .triage_mut()
            .get_mut(&entry_id)
            .unwrap()
            .room_binding
            .as_mut()
            .unwrap();
        proposal.candidate_rooms = vec![
            (target_id.clone(), "Old Kitchen".into()),
            ("deleted-candidate".into(), "Old Kitchen".into()),
            (other.clone(), "Old Kitchen".into()),
        ];
        other
    };
    commands::do_topology_delete_room(&harness.state, &target_id).unwrap();
    let queue: serde_json::Value =
        serde_json::from_str(&commands::build_triage_queue(&harness.state).unwrap()).unwrap();
    let proposal = &queue
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == entry_id)
        .unwrap()["room_binding"];
    assert_eq!(proposal["target_rhythm_room_id"], other_target);
    assert_eq!(
        proposal["candidate_rooms"],
        serde_json::json!([[other_target, "New Kitchen"]])
    );
    assert_ne!(
        harness.resolve_for_hub(&secondary, "secondary-kitchen"),
        other_target
    );
    harness.triage_bind(&entry_id).unwrap();
    assert_eq!(
        harness.resolve_for_hub(&secondary, "secondary-kitchen"),
        other_target
    );
}

#[test]
fn hub_disconnect_preserves_a_valid_legacy_proposal_without_candidate_list() {
    let (harness, _, entry_id, target_id) = pending_binding();
    {
        let mut state = harness.state.lock().unwrap();
        state
            .canonical_registry
            .triage_mut()
            .get_mut(&entry_id)
            .unwrap()
            .room_binding
            .as_mut()
            .unwrap()
            .candidate_rooms
            .clear();
        state.hubs.clear();
    }
    commands::do_topology_rename_room(&harness.state, &target_id, "Kitchen").unwrap();
    assert_eq!(
        harness.triage_room_binding(0).unwrap(),
        (entry_id, "Kitchen".into(), target_id)
    );
}

#[test]
fn topology_reconciliation_preserves_resolved_room_review_decisions() {
    use rhythm_os::canonical::triage::TriageStatus;
    for status in [
        TriageStatus::KeptSeparate,
        TriageStatus::NewDevice,
        TriageStatus::Confirmed,
        TriageStatus::Dismissed,
    ] {
        let (harness, _, entry_id, _) = pending_binding();
        let before = {
            let mut state = harness.state.lock().unwrap();
            state
                .canonical_registry
                .triage_mut()
                .resolve(&entry_id, status, "api", 1000);
            let before = serde_json::to_value(state.canonical_registry.triage()).unwrap();
            state.topology = Default::default();
            before
        };
        commands::do_topology_create_room(&harness.state, "Unrelated room").unwrap();
        assert_eq!(
            serde_json::to_value(harness.state.lock().unwrap().canonical_registry.triage())
                .unwrap(),
            before
        );
    }
}

#[test]
fn backup_restore_retires_legacy_missing_source_and_target_proposals() {
    for missing_source in [true, false] {
        let (harness, secondary, _, target_id) = pending_binding();
        let mut bundle = commands::build_backup_bundle_dto(&harness.state, false).unwrap();
        let removed_id = if missing_source {
            harness.resolve_for_hub(&secondary, "secondary-kitchen")
        } else {
            target_id
        };
        // Model an older release's backup: topology changed but review did not.
        bundle.installation.topology.remove_room(&removed_id);
        let restored = TestHarness::new();
        commands::do_backup_restore(&restored.state, bundle).unwrap();
        assert_no_room_proposals(&restored);
    }
}

#[test]
fn startup_retires_and_persists_legacy_stale_proposals_without_hub_connectivity() {
    let (harness, secondary, _, _) = pending_binding();
    let source_id = harness.resolve_for_hub(&secondary, "secondary-kitchen");
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "rhythm-room-review-{}-{unique}",
        std::process::id()
    ));
    let storage = Arc::new(FileStorage::new(dir.to_str().unwrap()).unwrap());
    {
        let mut state = harness.state.lock().unwrap();
        state.topology.remove_room(&source_id);
        state.storage = Some(storage.clone());
        commands::save_authority_state(&state).unwrap();
    }
    let mut restarted = rhythm_os::state::AppState {
        storage: Some(storage.clone()),
        ..Default::default()
    };
    load_persisted_state(&mut restarted);
    assert_eq!(
        restarted.canonical_registry.triage().pending_room_count(),
        0
    );
    let saved = storage.load_authority_state().unwrap().unwrap();
    let registry: rhythm_os::canonical::registry::CanonicalRegistry =
        serde_json::from_value(saved.canonical_registry).unwrap();
    assert_eq!(registry.triage().pending_room_count(), 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn resync_with_no_rooms_prunes_source_binding_but_preserves_rhythm_room() {
    let (rooms, devices) = rooms_with_lights(&[("office", "Office")]);
    let harness = TestHarness::new().with_discovery(rooms, devices);
    harness.sync();

    let topology_room_id = harness.resolve("office");
    {
        let state = harness.state.lock().unwrap();
        let room = state
            .topology
            .get(&topology_room_id)
            .expect("topology room should exist after initial sync");
        assert_eq!(room.hub_room_bindings.len(), 1);
    }

    harness.set_hub_discovery(&harness.hub_key, vec![], vec![]);
    let report = harness.sync();

    assert_eq!(
        report.rooms_removed, 1,
        "empty rediscovery should prune the stale source room"
    );

    let registry = {
        let state = harness.state.lock().unwrap();
        let room = state
            .topology
            .get(&topology_room_id)
            .expect("Rhythm room should survive binding removal");
        assert!(
            room.hub_room_bindings.is_empty(),
            "source bindings should be removed"
        );
        assert!(
            state
                .hub_runtime()
                .expect("runtime should still exist")
                .engine_room_snapshot(&topology_room_id)
                .is_some(),
            "runtime room should remain because only bindings were pruned"
        );
        state
            .hubs
            .get(&harness.hub_key)
            .and_then(|hub| hub.registry.clone())
            .expect("hub registry should exist")
    };

    let registry_rooms = registry.lock().unwrap().rooms();
    assert!(
        registry_rooms.is_empty(),
        "native source room should be removed from the hub registry"
    );
}
