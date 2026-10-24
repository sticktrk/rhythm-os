#![cfg(feature = "test-support")]

#[path = "common/mod.rs"]
mod harness;

use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::{ExternalLightHubIntegration, HubType};
use rhythm_os::pairing::{PairingStatus, UnpairingRequest};
use rhythm_os::{commands, handlers};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn pair(rig: &harness::TestRig) -> String {
    pair_with_payload(rig, "3497-011-2332")
}

fn pair_with_payload(rig: &harness::TestRig, setup_payload: &str) -> String {
    let session = rhythm_matter::desktop_lifecycle::INTEGRATION
        .start_pairing(
            &rig.state,
            &serde_json::json!({
                "setup_payload": setup_payload,
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
    commands::do_canonical_rename_device(&rig.state, &device_id, "Old custom bulb name").unwrap();
    commands::do_set_node_color(
        &rig.state,
        &device_id,
        commands::NodeColorUpdate {
            rgb: rhythm_core::Rgb::new(255, 0, 0),
            xy: None,
            brightness: Some(23),
            transition_ms: None,
            scope: commands::NodeColorScope::Mood,
        },
        true,
    )
    .unwrap();
    commands::do_node_preferences_set(
        &rig.state,
        &device_id,
        Some(false),
        Some(true),
        None,
        None,
        None,
        true,
    )
    .unwrap();
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

fn assert_unassigned_light(rig: &harness::TestRig, device_id: &str, old_room_id: &str) {
    let state = rig.state.lock().unwrap();
    let device = state.canonical_registry.get(device_id).unwrap();
    assert!(!device.is_removed());
    assert_eq!(device.room_id, None);
    assert_eq!(
        state.topology.get_device_node(device_id).unwrap().parent_id,
        None
    );
    assert!(state.topology.get(old_room_id).unwrap().devices.is_empty());
    assert_eq!(
        state.canonical_registry.triage().pending_unassigned_count(),
        1
    );
    assert!(
        state
            .hub_runtime()
            .and_then(|runtime| runtime.engine_node_snapshot(device_id))
            .is_none(),
        "pending room selection must stay outside automatic control"
    );
    drop(state);
    let snapshot: serde_json::Value =
        serde_json::from_str(&commands::build_state_snapshot(&rig.state).unwrap()).unwrap();
    assert!(!snapshot["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|node| node["id"] == device_id && !node["parent_id"].is_null()));
}

#[test]
fn saved_key_repair_creates_fresh_device_and_waits_for_room_selection() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (rig, old_id, room_id) = archived_room_light();
    let saved_key = rhythm_matter::setup_recovery::load_setup_payload(
        &rig.state,
        &rig.hub_data.fabric_id,
        "matter-100",
    )
    .unwrap()
    .expect("unpair must retain the setup key for reuse");
    let device_id = pair_with_payload(&rig, &saved_key.setup_payload);
    assert_ne!(device_id, old_id);
    {
        let state = rig.state.lock().unwrap();
        assert!(state.canonical_registry.get(&old_id).is_none());
        assert_ne!(
            state.canonical_registry.get(&device_id).unwrap().name,
            "Old custom bulb name"
        );
        assert!(state.topology.get_device_node(&old_id).is_none());
    }
    assert_unassigned_light(&rig, &device_id, &room_id);
    assert_eq!(rig.transport.commission_requests().len(), 2);
    assert_eq!(
        rig.transport.commission_requests()[0].node_id,
        rig.transport.commission_requests()[1].node_id
    );

    // Repeating setup or restarting before choosing must not resurrect the old room.
    assert_eq!(pair(&rig), device_id);
    assert_unassigned_light(&rig, &device_id, &room_id);
    assert_eq!(rig.transport.commission_requests().len(), 2);

    let restarted = harness::reconnect_rig_with_transport(rig.data_dir.clone(), |transport| {
        transport.add_device(100, "Vendor", "Lamp");
    });
    rhythm_os::room_sync::sync_all_hubs(&restarted.state).unwrap();
    assert_unassigned_light(&restarted, &device_id, &room_id);

    let selected_room: serde_json::Value = serde_json::from_str(
        &commands::do_topology_create_room(&restarted.state, "Office").unwrap(),
    )
    .unwrap();
    let selected_room_id = selected_room["id"].as_str().unwrap();
    commands::do_canonical_assign_room(&restarted.state, &device_id, Some(selected_room_id))
        .unwrap();
    assert_room_light(&restarted, &device_id, selected_room_id);
    let snapshot = restarted
        .state
        .lock()
        .unwrap()
        .hub_runtime()
        .unwrap()
        .engine_node_snapshot(&device_id)
        .unwrap();
    assert!(
        snapshot.profile_settings.mood_scene_id.is_none(),
        "old per-device color must not return"
    );
    assert!(
        !snapshot.disabled,
        "old disabled preference must not return"
    );
    // Repairing a still-active device retains the user's current assignment.
    assert_eq!(pair(&restarted), device_id);
    assert_room_light(&restarted, &device_id, selected_room_id);
}

#[test]
fn restoring_after_room_deletion_requires_standalone_review() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (rig, old_id, room_id) = archived_room_light();
    commands::do_topology_delete_room(&rig.state, &room_id).unwrap();
    let device_id = pair(&rig);
    assert_ne!(device_id, old_id);
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
