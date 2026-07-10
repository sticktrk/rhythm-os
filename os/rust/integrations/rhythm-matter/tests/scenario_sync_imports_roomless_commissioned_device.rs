#![cfg(feature = "test-support")]

#[path = "common/mod.rs"]
mod harness;

use rhythm_core::{RhythmMode, RoomModeState};
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::canonical::triage::TriageKind;
use rhythm_os::commands;
use rhythm_os::hub::HubType;

#[test]
fn scenario_sync_imports_roomless_commissioned_device() {
    let rig = harness::connect_rig_with_transport(None, |transport| {
        transport.add_device(200, "Vendor", "Lamp");
    });

    let report = rhythm_os::room_sync::sync_all_hubs(&rig.state).unwrap();
    assert_eq!(report.rooms_added, 0);
    assert_eq!(report.devices_synced, 0);

    let hub_key = HubKey::new(HubType::new("matter"), "local");
    let state = rig.state.lock().unwrap();
    let canonical = state
        .canonical_registry
        .find_by_native_id(&hub_key, "matter-200")
        .expect("synced Matter device should be in canonical registry");

    assert_eq!(canonical.name, "Vendor Lamp");
    assert_eq!(canonical.manufacturer.as_deref(), Some("Vendor"));
    assert_eq!(canonical.model.as_deref(), Some("Lamp"));
    assert!(canonical.room_id.is_none());
    let canonical_id = canonical.id.clone();
    let sleep_config = state
        .mode_configs()
        .into_iter()
        .find(|config| config.mode == RhythmMode::Sleep)
        .expect("sleep mode config should exist");
    assert!(
        sleep_config
            .room_defaults
            .iter()
            .any(|default| default.room_id == canonical_id
                && default.state == RoomModeState::HardOff),
        "synced roomless Matter devices should default off in Sleep mode"
    );
    assert_eq!(
        state.canonical_registry.triage().pending_unassigned_count(),
        1
    );
    let triage_id = state
        .canonical_registry
        .triage()
        .pending_by_kind(TriageKind::UnassignedDevice)
        .first()
        .expect("synced roomless Matter device should require triage")
        .id
        .clone();
    assert!(
        state.hub_runtime().is_none(),
        "pending roomless Matter devices should stay outside automatic control"
    );
    drop(state);

    let pending_snapshot: serde_json::Value =
        serde_json::from_str(&commands::build_state_snapshot(&rig.state).unwrap()).unwrap();
    assert!(
        !pending_snapshot["nodes"]
            .as_array()
            .expect("state nodes should be an array")
            .iter()
            .any(|node| node["id"].as_str() == Some(canonical_id.as_str())),
        "pending roomless Matter devices must not appear as controllable state nodes"
    );
    assert_eq!(pending_snapshot["review"]["pending"]["unassigned"], 1);

    commands::try_ensure_runtime(&rig.state)
        .expect("an unrelated runtime bootstrap should still succeed");
    let pending_runtime = rig
        .state
        .lock()
        .unwrap()
        .hub_runtime()
        .expect("explicit runtime bootstrap should install a runtime");
    assert!(
        pending_runtime
            .engine_node_snapshot(&canonical_id)
            .is_none(),
        "runtime bootstrap must preserve pending unassigned-device quarantine"
    );

    let result = commands::do_triage_new_device(&rig.state, &triage_id)
        .expect("explicit standalone confirmation should succeed");
    assert!(result.contains(r#""status":"standalone""#));

    let runtime = rig
        .state
        .lock()
        .unwrap()
        .hub_runtime()
        .expect("standalone confirmation should bootstrap the runtime");
    let node = runtime
        .engine_node_snapshot(&canonical_id)
        .expect("confirmed standalone Matter device should exist in the runtime");
    assert_eq!(node.parent_id, None);

    let caps = rig.hub_data.device_caps.lock().unwrap();
    let stored = caps
        .get("matter-200")
        .expect("synced Matter device should cache capabilities");
    assert!(stored.supports_color_temp());
    assert!(!stored.supports_xy_color());
}
