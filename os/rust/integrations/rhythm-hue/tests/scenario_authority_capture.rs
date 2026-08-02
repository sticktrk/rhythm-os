//! Scenario coverage for capture-first Hue authority without automatic restore.

use rhythm_hue::ownership::{
    acquire_authoritative_control, finalize_released_control, load_controller_ownership,
    release_authoritative_control, HueBaselineCaptureScope, HueOwnershipPhase,
};
use rhythm_hue::test_support::{HueTransportCall, SpyHueTransport};
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::HubType;
use rhythm_os::storage::FileStorage;

struct TempStorage {
    path: std::path::PathBuf,
    storage: FileStorage,
}

impl TempStorage {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rhythm-hue-capture-scenario-{}",
            std::process::id()
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
    let temp = TempStorage::new();
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
