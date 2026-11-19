use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_os::canonical::identity::{DiscoveredIdentity, HardwareId, HubKey};
use rhythm_os::canonical::registry::ResolveResult;
use rhythm_os::hub::HubType;
use rhythm_os::state::AppState;
use rhythm_os::storage::{load_persisted_state, FileStorage, Storage};
use rhythm_os::topology::{DevicePlacement, HubRoomBinding};
use std::sync::Arc;

fn light(state: &mut AppState, native_id: &str, room_id: Option<&str>) -> String {
    light_on(state, "matter", native_id, room_id)
}

fn light_on(
    state: &mut AppState,
    hub_type: &str,
    native_id: &str,
    room_id: Option<&str>,
) -> String {
    let identity = DiscoveredIdentity {
        native_id: native_id.into(),
        room_id: None,
        room_name: None,
        name: "Synthetic lamp".into(),
        device_type: DeviceType::Light,
        hardware_ids: vec![HardwareId::matter(native_id)],
        manufacturer: None,
        model: None,
    };
    let id = match state.canonical_registry.resolve(
        &identity,
        &HubKey::new(HubType::new(hub_type), "local"),
        1,
    ) {
        ResolveResult::Created { canonical_id } => canonical_id,
        other => panic!("unexpected identity: {other:?}"),
    };
    state.canonical_registry.assign_room(&id, room_id);
    id
}

#[test]
fn restart_repairs_missing_active_light_placement_without_reviving_removed_lights() {
    exercise_restart(false);
}

#[test]
fn legacy_restore_repairs_missing_light_placement() {
    exercise_restart(true);
}

fn exercise_restart(legacy: bool) {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(FileStorage::new(directory.path().to_str().unwrap()).unwrap());
    let mut state = AppState::default();
    state.storage = Some(storage.clone());
    let room = state.topology.create_room("Patio");
    let other_room = state.topology.create_room("Office");
    // Persist the shape left by older commissioning code: active canonical
    // assignment but no topology node or room membership.
    let restored = light(&mut state, "matter-1", Some(&room));
    let archived = light(&mut state, "matter-2", Some(&room));
    state.canonical_registry.soft_remove(&archived, 2);
    let inactive = light(&mut state, "matter-3", Some(&room));
    state
        .canonical_registry
        .get_mut(&inactive)
        .unwrap()
        .endpoints[0]
        .active = false;
    let standalone = light(&mut state, "matter-4", Some(&room));
    state.topology.ensure_standalone_device(&standalone);
    state
        .topology
        .assign_device(&standalone, None, DevicePlacement::UserOverride);
    let moved = light(&mut state, "matter-5", Some(&room));
    state
        .topology
        .attach_device_user_override(&other_room, &moved);
    let deleted_room = light(&mut state, "matter-6", Some("missing-room"));
    let unassigned = light(&mut state, "matter-7", None);
    let deleted = light(&mut state, "matter-8", Some(&room));
    state.canonical_registry.remove_device(&deleted);
    // A hub source room owns placement for the lights it reports. The repair
    // must not turn that membership into a user override, and must leave a
    // light reported elsewhere for the next hub sync.
    let hub_reported = light_on(&mut state, "hue", "hue-1", Some(&room));
    let hub_reported_elsewhere = light_on(&mut state, "hue", "hue-2", Some(&room));
    for (room_id, hub_room_id, native_id) in [
        (&room, "hue-room-1", "hue-1"),
        (&other_room, "hue-room-2", "hue-2"),
    ] {
        state
            .topology
            .get_mut(room_id)
            .unwrap()
            .upsert_hub_room_binding(HubRoomBinding {
                hub_key: HubKey::new(HubType::new("hue"), "local"),
                hub_room_id: hub_room_id.into(),
                control_id: format!("{hub_room_id}-group"),
                light_device_ids: vec![native_id.into()],
            });
    }
    if legacy {
        storage
            .save_canonical_registry(&serde_json::to_value(&state.canonical_registry).unwrap())
            .unwrap();
        storage
            .save_topology(&serde_json::to_value(&state.topology).unwrap())
            .unwrap();
    } else {
        rhythm_os::commands::save_authority_state(&state).unwrap();
    }

    for _ in 0..2 {
        let mut restarted = AppState::default();
        restarted.storage = Some(storage.clone());
        load_persisted_state(&mut restarted);
        assert!(!restarted.authority_state_recovery_required);
        assert_eq!(
            restarted.topology.device_parent_room_id(&restored),
            Some(room.as_str())
        );
        assert_eq!(restarted.topology.get(&room).unwrap().devices.len(), 2);
        assert_eq!(
            restarted
                .topology
                .get_device_node(&restored)
                .unwrap()
                .placement,
            DevicePlacement::UserOverride
        );
        let hub_node = restarted.topology.get_device_node(&hub_reported).unwrap();
        assert_eq!(hub_node.parent_id.as_deref(), Some(room.as_str()));
        assert_eq!(hub_node.placement, DevicePlacement::HubDefault);
        for id in [
            &archived,
            &inactive,
            &deleted_room,
            &unassigned,
            &deleted,
            &hub_reported_elsewhere,
        ] {
            assert!(restarted.topology.get_device_node(id).is_none());
        }
        let node = restarted.topology.get_device_node(&standalone).unwrap();
        assert_eq!(node.parent_id, None);
        assert_eq!(node.placement, DevicePlacement::UserOverride);
        assert_eq!(
            restarted.topology.device_parent_room_id(&moved),
            Some(other_room.as_str())
        );
        let saved = storage.load_authority_state().unwrap().unwrap();
        let topology: rhythm_os::topology::RoomTopologyStore =
            serde_json::from_value(saved.topology).unwrap();
        assert_eq!(
            topology.device_parent_room_id(&restored),
            Some(room.as_str())
        );
        assert!(restarted.canonical_registry.get(&deleted).is_none());
    }
}
