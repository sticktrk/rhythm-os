#![cfg(feature = "test-support")]

#[path = "common/mod.rs"]
mod harness;

use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::{ExternalLightHubIntegration, HubType};
use rhythm_os::pairing::{PairingStatus, UnpairingRequest};
use rhythm_os::{commands, handlers};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn pair(rig: &harness::TestRig) -> String {
    let session = rhythm_matter::desktop_lifecycle::INTEGRATION
        .start_pairing(
            &rig.state,
            &serde_json::json!({
                "setup_payload": "3497-011-2332",
                "network": "wifi",
                "rendezvous": "on_network"
            }),
        )
        .unwrap();
    assert_eq!(session.status, PairingStatus::Complete);
    let native_id = session.device.unwrap().device_id;
    rig.state
        .lock()
        .unwrap()
        .canonical_registry
        .find_by_native_id(&HubKey::new(HubType::new("matter"), "local"), &native_id)
        .unwrap()
        .id
        .clone()
}

fn archived_room_light() -> (harness::TestRig, String, String) {
    let rig = harness::connect_rig(None);
    harness::store_commissioning_wifi(&rig.state, "SyntheticNet", "synthetic-secret");
    let device_id = pair(&rig);
    let room: serde_json::Value =
        serde_json::from_str(&commands::do_topology_create_room(&rig.state, "Patio").unwrap())
            .unwrap();
    let room_id = room["id"].as_str().unwrap().to_string();
    commands::do_canonical_assign_room(&rig.state, &device_id, Some(&room_id)).unwrap();
    let response = handlers::handle_unpair_device(
        &rig.state,
        &UnpairingRequest {
            hub_type: "matter".into(),
            params: serde_json::json!({"device_id": device_id, "archive": true}),
        },
    );
    assert_eq!(response.status, 200);
    {
        let state = rig.state.lock().unwrap();
        assert!(state
            .canonical_registry
            .get(&device_id)
            .unwrap()
            .is_removed());
        assert!(state.topology.get_device_node(&device_id).is_none());
    }
    (rig, device_id, room_id)
}

fn assert_room_light(rig: &harness::TestRig, device_id: &str, room_id: &str) {
    let state = rig.state.lock().unwrap();
    assert_eq!(state.canonical_registry.devices().count(), 1);
    assert_eq!(
        state
            .canonical_registry
            .get(device_id)
            .unwrap()
            .room_id
            .as_deref(),
        Some(room_id)
    );
    assert_eq!(
        state.topology.device_parent_room_id(device_id),
        Some(room_id)
    );
    assert_eq!(
        state
            .topology
            .get(room_id)
            .unwrap()
            .devices
            .iter()
            .filter(|entry| entry.device_id == device_id)
            .count(),
        1
    );
    assert_eq!(
        state.canonical_registry.triage().pending_unassigned_count(),
        0
    );
    let runtime = state.hub_runtime().unwrap();
    assert_eq!(
        runtime
            .engine_node_snapshot(device_id)
            .unwrap()
            .parent_id
            .as_deref(),
        Some(room_id)
    );
    drop(state);
    let snapshot: serde_json::Value =
        serde_json::from_str(&commands::build_state_snapshot(&rig.state).unwrap()).unwrap();
    let node = snapshot["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == device_id)
        .unwrap();
    assert_eq!(node["parent_id"], room_id);
}

#[test]
fn restored_light_rejoins_topology_runtime_and_snapshot_without_duplicates() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (rig, device_id, room_id) = archived_room_light();
    assert_eq!(pair(&rig), device_id);
    assert_room_light(&rig, &device_id, &room_id);
    assert_eq!(rig.transport.commission_requests().len(), 2);
    assert_eq!(
        rig.transport.commission_requests()[0].node_id,
        rig.transport.commission_requests()[1].node_id
    );

    // A normal repeat-pair must retain the repaired placement without another node.
    assert_eq!(pair(&rig), device_id);
    assert_room_light(&rig, &device_id, &room_id);
    assert_eq!(rig.transport.commission_requests().len(), 2);

    let restarted = harness::reconnect_rig_with_transport(rig.data_dir.clone(), |transport| {
        transport.add_device(100, "Vendor", "Lamp");
    });
    rhythm_os::room_sync::sync_all_hubs(&restarted.state).unwrap();
    assert_room_light(&restarted, &device_id, &room_id);
}

#[test]
fn restoring_after_room_deletion_requires_standalone_review() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (rig, device_id, room_id) = archived_room_light();
    commands::do_topology_delete_room(&rig.state, &room_id).unwrap();
    assert_eq!(pair(&rig), device_id);
    let state = rig.state.lock().unwrap();
    assert!(state.topology.get(&room_id).is_none());
    assert_eq!(
        state.canonical_registry.get(&device_id).unwrap().room_id,
        None
    );
    assert_eq!(
        state
            .topology
            .get_device_node(&device_id)
            .unwrap()
            .parent_id,
        None
    );
    assert_eq!(
        state.canonical_registry.triage().pending_unassigned_count(),
        1
    );
}

#[test]
fn permanently_deleted_light_pairs_as_new_without_the_old_room() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (rig, device_id, room_id) = archived_room_light();
    rig.state.lock().unwrap().purge_pairing_recovery_fn =
        Some(std::sync::Arc::new(|state, _, native_id| {
            rhythm_matter::desktop_lifecycle::INTEGRATION.purge_pairing_recovery(state, native_id)
        }));
    assert_eq!(
        handlers::handle_delete_removed_device(&rig.state, &device_id, None).status,
        204
    );
    let new_id = pair(&rig);
    assert_ne!(new_id, device_id);
    let state = rig.state.lock().unwrap();
    assert!(state.canonical_registry.get(&device_id).is_none());
    assert!(state.topology.get_device_node(&device_id).is_none());
    assert_eq!(state.canonical_registry.get(&new_id).unwrap().room_id, None);
    assert_eq!(
        state.topology.get_device_node(&new_id).unwrap().parent_id,
        None
    );
    assert!(state.topology.get(&room_id).unwrap().devices.is_empty());
    assert_eq!(
        state.canonical_registry.triage().pending_unassigned_count(),
        1
    );
}
