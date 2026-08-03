//! Scenario coverage for capture-first Hue authority without automatic restore.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rhythm_hue::discovery::HueDiscovery;
use rhythm_hue::managed_rooms::{reconcile_managed_rooms, DesiredHueRoom};
use rhythm_hue::ownership::{
    acquire_authoritative_control, finalize_released_control, load_controller_ownership,
    release_authoritative_control, HueBaselineCaptureScope, HueOwnershipPhase,
};
use rhythm_hue::test_support::{HueTransportCall, SpyHueTransport};
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::discovery::HubDiscovery;
use rhythm_os::hub::HubType;
use rhythm_os::storage::FileStorage;

struct TempStorage {
    path: std::path::PathBuf,
    storage: FileStorage,
}

impl TempStorage {
    fn new(name: &str) -> Self {
        static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "rhythm-hue-capture-scenario-{name}-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed),
        ));
        let _ = std::fs::remove_dir_all(&path);
        let storage = FileStorage::new(path.to_str().unwrap()).unwrap();
        Self { path, storage }
    }
}

impl Drop for TempStorage {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn key() -> HubKey {
    HubKey::new(HubType::new(HubType::HUE), "192.0.2.10")
}

#[test]
fn complete_inventory_survives_takeover_and_release_without_restore_writes() {
    let temp = TempStorage::new("complete-inventory");
    let bridge = SpyHueTransport::new();
    bridge.set_resource_response(
        "bridge",
        serde_json::json!({"data": [{"id": "bridge-1"}], "errors": []}),
    );
    bridge.set_resource_response(
        "button",
        serde_json::json!({"data": [{"id": "button-1"}], "errors": []}),
    );
    bridge.set_resource_response(
        "behavior_instance",
        serde_json::json!({"data": [{
            "id": "behavior-1",
            "enabled": true,
            "configuration": {
                "room": {"rid": "room-1", "rtype": "room"},
                "scene": {"rid": "scene-1", "rtype": "scene"}
            }
        }], "errors": []}),
    );
    bridge.set_resource_response(
        "room",
        serde_json::json!({"data": [{"id": "room-1"}], "errors": []}),
    );
    bridge.set_resource_response(
        "scene",
        serde_json::json!({"data": [{"id": "scene-1"}], "errors": []}),
    );
    bridge.set_resource_response(
        "smart_scene",
        serde_json::json!({"data": [{"id": "smart-scene-1"}], "errors": []}),
    );

    let active = acquire_authoritative_control(&temp.storage, &key(), &bridge, "user").unwrap();

    assert_eq!(active.phase, HueOwnershipPhase::Active);
    assert_eq!(
        active.baseline().capture_scope(),
        HueBaselineCaptureScope::FullV2Inventory
    );
    for resource_type in [
        "button",
        "behavior_instance",
        "room",
        "scene",
        "smart_scene",
    ] {
        assert_eq!(
            active.baseline().v2_resource(resource_type).unwrap()["data"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
    assert!(bridge.calls().iter().any(|call| matches!(
        call,
        HueTransportCall::DeleteResource {
            resource_type,
            resource_id,
        } if resource_type == "smart_scene" && resource_id == "smart-scene-1"
    )));

    bridge.reset();
    let pending = release_authoritative_control(&temp.storage, &key(), &bridge, "user")
        .unwrap()
        .unwrap();
    assert_eq!(pending.phase, HueOwnershipPhase::ReleasePending);
    assert!(!bridge.calls().iter().any(|call| matches!(
        call,
        HueTransportCall::CreateResource { .. }
            | HueTransportCall::UpdateResource { .. }
            | HueTransportCall::DeleteResource { .. }
            | HueTransportCall::PutV1 { .. }
    )));

    finalize_released_control(&temp.storage, "bridge-1").unwrap();
    let retained = load_controller_ownership(&temp.storage, "bridge-1")
        .unwrap()
        .unwrap();
    assert_eq!(retained.phase, HueOwnershipPhase::SnapshotRetained);
    assert_eq!(
        retained.baseline().v2_resource("smart_scene").unwrap()["data"][0]["id"],
        "smart-scene-1"
    );
}

#[test]
fn recreated_authority_room_keeps_inputs_discoverable_as_room_members() {
    let temp = TempStorage::new("input-membership");
    let bridge = Arc::new(SpyHueTransport::new());
    for (resource_type, data) in [
        ("bridge", serde_json::json!([{"id": "bridge-1"}])),
        (
            "device",
            serde_json::json!([
                {
                    "id": "light-parent",
                    "metadata": {"name": "Mud light"},
                    "product_data": {"manufacturer_name": "Signify", "model_id": "LCA009"},
                    "services": [{"rid": "light-service", "rtype": "light"}]
                },
                {
                    "id": "button-parent",
                    "metadata": {"name": "Mud dimmer"},
                    "product_data": {"manufacturer_name": "Signify", "model_id": "RWL022"},
                    "services": [{"rid": "button-service", "rtype": "button"}]
                },
                {
                    "id": "motion-parent",
                    "metadata": {"name": "Mud Motion"},
                    "product_data": {"manufacturer_name": "Signify", "model_id": "SML003"},
                    "services": [{"rid": "motion-service", "rtype": "motion"}]
                }
            ]),
        ),
        ("light", serde_json::json!([])),
        ("button", serde_json::json!([])),
        ("motion", serde_json::json!([])),
        ("behavior_instance", serde_json::json!([])),
        (
            "room",
            serde_json::json!([{
                "id": "native-mud-room",
                "metadata": {"name": "Mud Room", "archetype": "living_room"},
                "children": [
                    {"rid": "light-parent", "rtype": "device"},
                    {"rid": "button-parent", "rtype": "device"},
                    {"rid": "motion-parent", "rtype": "device"}
                ],
                "services": [{"rid": "native-group", "rtype": "grouped_light"}]
            }]),
        ),
        ("zone", serde_json::json!([])),
        ("scene", serde_json::json!([])),
        ("smart_scene", serde_json::json!([])),
        (
            "zigbee_connectivity",
            serde_json::json!([
                {"owner": {"rid": "light-parent"}, "mac_address": "00:17:88:00:00:00:00:01"},
                {"owner": {"rid": "button-parent"}, "mac_address": "00:17:88:00:00:00:00:02"},
                {"owner": {"rid": "motion-parent"}, "mac_address": "00:17:88:00:00:00:00:03"}
            ]),
        ),
    ] {
        bridge.set_resource_response(
            resource_type,
            serde_json::json!({"data": data, "errors": []}),
        );
    }
    bridge.set_v1_response("rules", serde_json::json!({}));
    bridge.set_v1_response("schedules", serde_json::json!({}));

    let mut ownership =
        acquire_authoritative_control(&temp.storage, &key(), bridge.as_ref(), "user").unwrap();
    let controlled_device_ids = BTreeSet::from([
        "button-parent".to_string(),
        "light-parent".to_string(),
        "motion-parent".to_string(),
    ]);
    reconcile_managed_rooms(
        &temp.storage,
        &key(),
        &mut ownership,
        bridge.as_ref(),
        "user",
        &[DesiredHueRoom::new(
            "rhythm-mud-room",
            "Rhythm · Mud Room",
            controlled_device_ids.iter().cloned().collect(),
        )],
        &controlled_device_ids,
    )
    .unwrap();

    let managed_hue_room_id = ownership.managed_rooms()["rhythm-mud-room"]
        .hue_room_id
        .clone();
    let discovery = HueDiscovery::new_authoritative(
        bridge,
        "user".to_string(),
        BTreeSet::from([managed_hue_room_id.clone()]),
    );
    let rooms = discovery.discover_rooms().unwrap();
    assert_eq!(rooms.len(), 1);
    assert_eq!(
        rooms[0].device_ids,
        vec![
            "button-parent".to_string(),
            "light-parent".to_string(),
            "motion-parent".to_string(),
        ]
    );

    let identities = discovery.discover_identities().unwrap();
    let button = identities
        .iter()
        .find(|identity| identity.native_id == "button-parent")
        .unwrap();
    let motion = identities
        .iter()
        .find(|identity| identity.native_id == "motion-service")
        .unwrap();
    assert_eq!(
        button.room_id.as_deref(),
        Some(managed_hue_room_id.as_str())
    );
    assert_eq!(
        motion.room_id.as_deref(),
        Some(managed_hue_room_id.as_str())
    );
}
