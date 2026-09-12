//! Room review proposals must follow current topology, including after restart.

mod harness;

use harness::{light, room, rooms_with_lights, TestHarness};
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::canonical::registry::CanonicalRegistry;
use rhythm_os::canonical::triage::TriageStatus;
use rhythm_os::commands;
use rhythm_os::server_event::ServerEvent;
use rhythm_os::storage::{
    load_persisted_state, FileStorage, Storage, StoredAuthorityState, StoredLightProfiles,
    StoredLocation, StoredSettings,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

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
    let (replacement_id, _, _) = harness.triage_room_binding(0).unwrap();
    assert_ne!(replacement_id, entry_id);
    assert!(harness.triage_bind(&entry_id).is_err());
    assert!(harness.triage_bind_to(&entry_id, &other_target).is_err());
    {
        let state = harness.state.lock().unwrap();
        let retired = state.canonical_registry.triage().get(&entry_id).unwrap();
        assert_eq!(retired.status, TriageStatus::Dismissed);
        assert_eq!(
            retired.room_binding.as_ref().unwrap().target_rhythm_room_id,
            target_id
        );
    }
    let queue: serde_json::Value =
        serde_json::from_str(&commands::build_triage_queue(&harness.state).unwrap()).unwrap();
    let proposal = &queue
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == replacement_id)
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
    // Restoring the updated queue must preserve both the retired identity and
    // the fresh proposal, including for clients that omit the explicit target.
    let bundle = commands::build_backup_bundle_dto(&harness.state, false).unwrap();
    let restored = TestHarness::new();
    commands::do_backup_restore(&restored.state, bundle).unwrap();
    assert!(restored.triage_bind(&entry_id).is_err());
    restored.triage_bind(&replacement_id).unwrap();
    harness.triage_bind(&replacement_id).unwrap();
    assert_eq!(
        harness.resolve_for_hub(&secondary, "secondary-kitchen"),
        other_target
    );
}

/// Accept the topology commit, then fail only the subsequent review repair.
/// This isolates the durability boundary from the ordinary transaction rollback.
struct RoomReviewFailStorage {
    entry_id: String,
    fail_repairs: AtomicBool,
    failed_repairs: AtomicUsize,
    saved: Mutex<Option<StoredAuthorityState>>,
}

impl RoomReviewFailStorage {
    fn new(entry_id: String) -> Self {
        Self {
            entry_id,
            fail_repairs: AtomicBool::new(true),
            failed_repairs: AtomicUsize::new(0),
            saved: Mutex::new(None),
        }
    }
}

impl Storage for RoomReviewFailStorage {
    fn load_rooms(&self) -> anyhow::Result<rhythm_core::RoomManager> {
        Ok(Default::default())
    }
    fn save_rooms(&self, _: &rhythm_core::RoomManager) -> anyhow::Result<()> {
        Ok(())
    }
    fn load_light_profiles(&self) -> anyhow::Result<StoredLightProfiles> {
        anyhow::bail!("No stored light profiles")
    }
    fn save_light_profiles(&self, _: &StoredLightProfiles) -> anyhow::Result<()> {
        Ok(())
    }
    fn load_location(&self) -> anyhow::Result<StoredLocation> {
        anyhow::bail!("No stored location")
    }
    fn save_location(&self, _: &StoredLocation) -> anyhow::Result<()> {
        Ok(())
    }
    fn load_settings(&self) -> anyhow::Result<StoredSettings> {
        anyhow::bail!("No stored settings")
    }
    fn save_settings(&self, _: &StoredSettings) -> anyhow::Result<()> {
        Ok(())
    }
    fn load_all_hub_credentials(&self) -> anyhow::Result<Vec<rhythm_os::hub::HubCredentials>> {
        Ok(vec![])
    }
    fn save_all_hub_credentials(&self, _: &[rhythm_os::hub::HubCredentials]) -> anyhow::Result<()> {
        Ok(())
    }
    fn load_hub_registry_for(&self, _: &HubKey) -> anyhow::Result<Option<serde_json::Value>> {
        Ok(None)
    }
    fn save_hub_registry_for(&self, _: &HubKey, _: &serde_json::Value) -> anyhow::Result<()> {
        Ok(())
    }
    fn load_authority_state(&self) -> anyhow::Result<Option<StoredAuthorityState>> {
        Ok(self.saved.lock().unwrap().clone())
    }
    fn save_authority_state(&self, state: &StoredAuthorityState) -> anyhow::Result<()> {
        let registry: CanonicalRegistry = serde_json::from_value(state.canonical_registry.clone())?;
        let repaired = registry
            .triage()
            .get(&self.entry_id)
            .is_some_and(|entry| entry.resolved_by.as_deref() == Some("topology"));
        if repaired && self.fail_repairs.load(Ordering::SeqCst) {
            self.failed_repairs.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("Simulated review repair write failure");
        }
        *self.saved.lock().unwrap() = Some(state.clone());
        Ok(())
    }
}

#[test]
fn completed_topology_changes_update_runtime_and_events_when_review_repair_cannot_persist() {
    for delete in [true, false] {
        let (harness, secondary, entry_id, target_id) = pending_binding();
        let source_id = harness.resolve_for_hub(&secondary, "secondary-kitchen");
        let storage = Arc::new(RoomReviewFailStorage::new(entry_id.clone()));
        let (tx, mut rx) = tokio::sync::broadcast::channel(64);
        {
            let mut state = harness.state.lock().unwrap();
            state.storage = Some(storage.clone());
            state.event_tx = Some(tx);
            commands::save_authority_state(&state).unwrap();
        }

        let removed_id = if delete {
            commands::do_topology_delete_room(&harness.state, &target_id).unwrap();
            &target_id
        } else {
            commands::do_topology_merge_rooms(&harness.state, &target_id, &source_id).unwrap();
            &source_id
        };
        assert_eq!(storage.failed_repairs.load(Ordering::SeqCst), 1);
        {
            let state = harness.state.lock().unwrap();
            assert!(state.topology.get(removed_id).is_none());
            assert!(state
                .hub_runtime()
                .unwrap()
                .engine_room_snapshot(removed_id)
                .is_none());
            assert_eq!(
                state
                    .canonical_registry
                    .triage()
                    .get(&entry_id)
                    .unwrap()
                    .status,
                TriageStatus::Pending
            );
        }
        let saved = storage.load_authority_state().unwrap().unwrap();
        let topology: rhythm_os::topology::RoomTopologyStore =
            serde_json::from_value(saved.topology).unwrap();
        assert!(topology.get(removed_id).is_none());
        let registry: CanonicalRegistry = serde_json::from_value(saved.canonical_registry).unwrap();
        assert_eq!(
            registry.triage().get(&entry_id).unwrap().status,
            TriageStatus::Pending
        );
        let mut nodes_changed = false;
        while let Ok(event) = rx.try_recv() {
            nodes_changed |= matches!(event, ServerEvent::NodesChanged);
        }
        assert!(
            nodes_changed,
            "A committed topology change must notify clients"
        );

        // The next sync retries the repair once persistence recovers.
        storage.fail_repairs.store(false, Ordering::SeqCst);
        harness.sync_hub(&secondary);
        assert_no_room_proposals(&harness);
        let saved = storage.load_authority_state().unwrap().unwrap();
        let registry: CanonicalRegistry = serde_json::from_value(saved.canonical_registry).unwrap();
        assert_eq!(registry.triage().pending_room_count(), 0);
    }
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
