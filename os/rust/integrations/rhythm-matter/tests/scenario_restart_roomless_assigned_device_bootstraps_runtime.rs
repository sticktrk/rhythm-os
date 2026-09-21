#![cfg(feature = "test-support")]

#[path = "common/mod.rs"]
mod harness;

#[test]
fn scenario_restart_roomless_assigned_device_bootstraps_runtime() {
    exercise_restart(false);
}

#[test]
fn scenario_restart_repairs_previously_restored_light_missing_from_topology() {
    exercise_restart(true);
}

fn exercise_restart(missing_topology: bool) {
    let initial = harness::connect_rig_with_transport(None, |transport| {
        transport.add_device(200, "Vendor", "Lamp");
    });

    rhythm_os::room_sync::sync_all_hubs(&initial.state).unwrap();

    let (canonical_id, room_id) = {
        let hub_key = rhythm_os::canonical::identity::HubKey::new(
            rhythm_os::hub::HubType::new("matter"),
            "local",
        );
        let mut state = initial.state.lock().unwrap();
        let canonical_id = state
            .canonical_registry
            .find_by_native_id(&hub_key, "matter-200")
            .expect("synced Matter device should be in canonical registry")
            .id
            .clone();
        let room_id = state.topology.create_room("Desk");
        state
            .canonical_registry
            .assign_room(&canonical_id, Some(&room_id));
        if missing_topology {
            // Older pairing reactivated the canonical record without
            // recreating the topology node removed by archive.
            state.topology.remove_device_everywhere(&canonical_id);
        } else {
            assert!(state
                .topology
                .attach_device_user_override(&room_id, &canonical_id));
        }
        rhythm_os::commands::save_authority_state(&state).unwrap();
        (canonical_id, room_id)
    };

    let restarted = harness::reconnect_rig_with_transport(initial.data_dir.clone(), |transport| {
        transport.add_device(200, "Vendor", "Lamp");
    });

    let report = rhythm_os::room_sync::sync_all_hubs(&restarted.state).unwrap();
    assert_eq!(report.rooms_added, 0);
    assert_eq!(report.devices_synced, 0);

    let state = restarted.state.lock().unwrap();
    let runtime = state
        .hub_runtime()
        .expect("persisted roomless Matter device should still bootstrap the runtime");
    drop(state);

    let node = runtime
        .engine_node_snapshot(&canonical_id)
        .expect("persisted roomless Matter canonical device should exist in the runtime");
    assert_eq!(node.parent_id.as_deref(), Some(room_id.as_str()));
}
