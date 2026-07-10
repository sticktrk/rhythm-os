#![cfg(feature = "test-support")]

#[path = "common/mod.rs"]
mod harness;

use rhythm_core::{RhythmMode, RoomModeState};
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::commands;
use rhythm_os::hub::{ExternalLightHubIntegration, HubType};

#[test]
fn scenario_pair_queues_unassigned_device() {
    let rig = harness::connect_rig(None);
    harness::store_commissioning_wifi(&rig.state, "RhythmNet", "secret");

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

    assert_eq!(session.status, rhythm_os::pairing::PairingStatus::Complete);

    let hub_key = HubKey::new(HubType::new("matter"), "local");
    let state = rig.state.lock().unwrap();
    let canonical = state
        .canonical_registry
        .find_by_native_id(&hub_key, "matter-100")
        .expect("paired Matter device should be in canonical registry");
    let canonical_id = canonical.id.clone();
    let sleep_config = state
        .mode_configs()
        .into_iter()
        .find(|config| config.mode == RhythmMode::Sleep)
        .expect("sleep mode config should exist");

    assert!(canonical.room_id.is_none());
    assert!(
        sleep_config
            .room_defaults
            .iter()
            .any(|default| default.room_id == canonical_id
                && default.state == RoomModeState::HardOff),
        "paired roomless Matter devices should default off in Sleep mode"
    );
    assert_eq!(
        canonical
            .endpoint_by_native_id("matter-100")
            .and_then(|endpoint| endpoint.source_room_name.as_deref()),
        None
    );
    assert_eq!(state.canonical_registry.triage().pending_room_count(), 0);
    assert_eq!(
        state.canonical_registry.triage().pending_unassigned_count(),
        1
    );
    let triage_id = state
        .canonical_registry
        .triage()
        .pending_by_kind(rhythm_os::canonical::triage::TriageKind::UnassignedDevice)
        .first()
        .expect("paired roomless Matter device should require triage")
        .id
        .clone();
    let topology_node = state
        .topology
        .get_device_node(&canonical_id)
        .expect("paired roomless Matter device should exist in topology immediately");
    assert_eq!(topology_node.parent_id, None);
    assert!(
        state.hub_runtime().is_none(),
        "pending roomless Matter devices should stay outside automatic control"
    );
    drop(state);

    let result = commands::do_triage_new_device(&rig.state, &triage_id)
        .expect("explicit standalone confirmation should succeed");
    assert!(result.contains(r#""status":"standalone""#));

    let runtime = rig
        .state
        .lock()
        .unwrap()
        .hub_runtime()
        .expect("standalone confirmation should bootstrap the runtime");
    let runtime_node = runtime
        .engine_node_snapshot(&canonical_id)
        .expect("confirmed standalone Matter device should exist in the runtime");
    assert_eq!(runtime_node.parent_id, None);
}
