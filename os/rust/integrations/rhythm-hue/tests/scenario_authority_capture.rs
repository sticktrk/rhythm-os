//! Scenario coverage for consented Hue authority with verified restoration.

use rhythm_hue::ownership::{
    acquire_authoritative_control, finalize_released_control, load_controller_ownership,
    release_authoritative_control, release_authoritative_control_with_intent,
    HueBaselineCaptureScope, HueOwnershipPhase, HueOwnershipReleaseIntent,
};
use rhythm_hue::test_support::{HueTransportCall, SpyHueTransport};
use rhythm_hue::transport::HueTransport;
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::HubType;
use rhythm_os::storage::FileStorage;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

struct TempStorage {
    path: std::path::PathBuf,
    storage: FileStorage,
}

impl TempStorage {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rhythm-hue-capture-scenario-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
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

fn seed_single_automation_bridge(bridge: &SpyHueTransport) {
    bridge.set_resource_response(
        "bridge",
        serde_json::json!({"data": [{"id": "bridge-1"}], "errors": []}),
    );
    bridge.set_resource_response(
        "device",
        serde_json::json!({"data": [{"id": "device-1"}], "errors": []}),
    );
    bridge.set_resource_response(
        "light",
        serde_json::json!({"data": [{"id": "light-1"}], "errors": []}),
    );
    bridge.set_resource_response(
        "behavior_instance",
        serde_json::json!({"data": [{
            "id": "behavior-1",
            "script_id": "automation-script-1",
            "enabled": true
        }], "errors": []}),
    );
    bridge.set_resource_response(
        "behavior_script",
        serde_json::json!({"data": [{
            "id": "automation-script-1",
            "metadata": {"name": "Automation", "category": "automation"}
        }], "errors": []}),
    );
    for resource_type in ["room", "zone", "scene", "smart_scene"] {
        bridge.set_resource_response(resource_type, serde_json::json!({"data": [], "errors": []}));
    }
    bridge.set_v1_response(
        "rules",
        serde_json::json!({"1": {"name": "Rule", "status": "enabled"}}),
    );
    bridge.set_v1_response(
        "schedules",
        serde_json::json!({"2": {"name": "Schedule", "status": "enabled"}}),
    );
}

#[test]
fn takeover_only_disables_automatic_lighting_programs_and_preserves_hue_topology() {
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
        serde_json::json!({"data": [
            {
                "id": "behavior-1",
                "script_id": "automation-script-1",
                "enabled": true,
                "configuration": {
                    "room": {"rid": "room-1", "rtype": "room"},
                    "scene": {"rid": "scene-1", "rtype": "scene"}
                }
            },
            {
                "id": "accessory-behavior-1",
                "script_id": "accessory-script-1",
                "enabled": true,
                "configuration": {
                    "device": {"rid": "button-1", "rtype": "device"}
                },
                "dependees": [{
                    "target": {"rid": "button-1", "rtype": "device"}
                }]
            },
            {
                "id": "accessory-target-behavior-1",
                "script_id": "accessory-script-1",
                "enabled": true,
                "configuration": {
                    "device": {"rid": "button-1", "rtype": "device"},
                    "buttons": {
                        "button-1": {
                            "where": [{
                                "group": {"rid": "room-1", "rtype": "room"}
                            }]
                        }
                    }
                },
                "dependees": [
                    {"target": {"rid": "button-1", "rtype": "device"}},
                    {"target": {"rid": "room-1", "rtype": "room"}}
                ]
            }
        ], "errors": []}),
    );
    bridge.set_resource_response(
        "behavior_script",
        serde_json::json!({"data": [
            {
                "id": "automation-script-1",
                "metadata": {"name": "Motion automation", "category": "automation"}
            },
            {
                "id": "accessory-script-1",
                "metadata": {"name": "Dimmer accessory", "category": "accessory"}
            }
        ], "errors": []}),
    );
    bridge.set_resource_response(
        "room",
        serde_json::json!({"data": [{
            "id": "room-1",
            "metadata": {"name": "Bedroom"},
            "children": [{"rid": "light-device-1", "rtype": "device"}]
        }], "errors": []}),
    );
    bridge.set_resource_response(
        "zone",
        serde_json::json!({"data": [{
            "id": "zone-1",
            "metadata": {"name": "Upstairs"},
            "children": [{"rid": "light-device-1", "rtype": "device"}]
        }], "errors": []}),
    );
    bridge.set_resource_response(
        "scene",
        serde_json::json!({"data": [{
            "id": "scene-1",
            "metadata": {"name": "Relax"},
            "group": {"rid": "room-1", "rtype": "room"},
            "actions": []
        }], "errors": []}),
    );
    bridge.set_resource_response(
        "smart_scene",
        serde_json::json!({"data": [{
            "id": "smart-scene-1",
            "metadata": {"name": "Natural light"},
            "group": {"rid": "room-1", "rtype": "room"}
        }], "errors": []}),
    );
    bridge.set_v1_response(
        "rules",
        serde_json::json!({"1": {"name": "Motion rule", "status": "enabled"}}),
    );
    bridge.set_v1_response(
        "schedules",
        serde_json::json!({"2": {"name": "Wake schedule", "status": "enabled"}}),
    );

    let original_room = bridge.get_resources("user", "room").unwrap();
    let original_zone = bridge.get_resources("user", "zone").unwrap();
    let original_scene = bridge.get_resources("user", "scene").unwrap();
    let original_smart_scene = bridge.get_resources("user", "smart_scene").unwrap();
    bridge.reset();

    let active = acquire_authoritative_control(&temp.storage, &key(), &bridge, "user").unwrap();

    assert_eq!(active.phase, HueOwnershipPhase::Active);
    assert_eq!(
        active.baseline().capture_scope(),
        HueBaselineCaptureScope::FullV2Inventory
    );
    for resource_type in ["button", "room", "zone", "scene", "smart_scene"] {
        assert_eq!(
            active.baseline().v2_resource(resource_type).unwrap()["data"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
    assert_eq!(
        active.baseline().v2_resource("behavior_instance").unwrap()["data"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        active.baseline().v2_resource("behavior_script").unwrap()["data"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(bridge.get_resources("user", "room").unwrap(), original_room);
    assert_eq!(bridge.get_resources("user", "zone").unwrap(), original_zone);
    assert_eq!(
        bridge.get_resources("user", "scene").unwrap(),
        original_scene
    );
    assert_eq!(
        bridge.get_resources("user", "smart_scene").unwrap(),
        original_smart_scene
    );
    let behaviors = bridge.get_resources("user", "behavior_instance").unwrap();
    let behavior_enabled = |id: &str| {
        behaviors["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|behavior| behavior["id"] == id)
            .unwrap()["enabled"]
            .as_bool()
            .unwrap()
    };
    assert!(!behavior_enabled("behavior-1"));
    assert!(behavior_enabled("accessory-behavior-1"));
    assert!(!behavior_enabled("accessory-target-behavior-1"));
    assert_eq!(
        bridge.get_v1("user", "rules").unwrap()["1"]["status"],
        "disabled"
    );
    assert_eq!(
        bridge.get_v1("user", "schedules").unwrap()["2"]["status"],
        "disabled"
    );

    let writes = bridge
        .calls()
        .into_iter()
        .filter(|call| {
            matches!(
                call,
                HueTransportCall::CreateResource { .. }
                    | HueTransportCall::UpdateResource { .. }
                    | HueTransportCall::DeleteResource { .. }
                    | HueTransportCall::PostV1 { .. }
                    | HueTransportCall::PutV1 { .. }
                    | HueTransportCall::DeleteV1 { .. }
                    | HueTransportCall::UpdateRoomChildren { .. }
                    | HueTransportCall::CreateRoom { .. }
                    | HueTransportCall::UpdateRoom { .. }
                    | HueTransportCall::RenameRoom { .. }
                    | HueTransportCall::DeleteRoom { .. }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(writes.len(), 4);
    assert!(writes.iter().any(|call| matches!(
        call,
        HueTransportCall::UpdateResource {
            resource_type,
            resource_id,
            body,
        } if resource_type == "behavior_instance"
            && resource_id == "behavior-1"
            && body == &serde_json::json!({"enabled": false})
    )));
    assert!(!writes.iter().any(|call| matches!(
        call,
        HueTransportCall::UpdateResource {
            resource_type,
            resource_id,
            ..
        } if resource_type == "behavior_instance"
            && resource_id == "accessory-behavior-1"
    )));
    assert!(writes.iter().any(|call| matches!(
        call,
        HueTransportCall::UpdateResource {
            resource_type,
            resource_id,
            body,
        } if resource_type == "behavior_instance"
            && resource_id == "accessory-target-behavior-1"
            && body == &serde_json::json!({"enabled": false})
    )));
    for (path, id) in [("rules/1", "1"), ("schedules/2", "2")] {
        assert!(
            writes.iter().any(|call| matches!(
                call,
                HueTransportCall::PutV1 { path: written_path, body }
                    if written_path == path
                        && body == &serde_json::json!({"status": "disabled"})
            )),
            "missing suppression write for {id}"
        );
    }

    bridge.reset();
    let stable = rhythm_hue::ownership::reconcile_authoritative_control(
        &temp.storage,
        &key(),
        &bridge,
        "user",
        active,
    )
    .unwrap();
    assert_eq!(stable.phase, HueOwnershipPhase::Active);
    assert!(!bridge.calls().iter().any(|call| matches!(
        call,
        HueTransportCall::CreateResource { .. }
            | HueTransportCall::UpdateResource { .. }
            | HueTransportCall::DeleteResource { .. }
            | HueTransportCall::PostV1 { .. }
            | HueTransportCall::PutV1 { .. }
            | HueTransportCall::DeleteV1 { .. }
            | HueTransportCall::UpdateRoomChildren { .. }
            | HueTransportCall::CreateRoom { .. }
            | HueTransportCall::UpdateRoom { .. }
            | HueTransportCall::RenameRoom { .. }
            | HueTransportCall::DeleteRoom { .. }
    )));

    bridge.reset();
    let restored = release_authoritative_control(&temp.storage, &key(), &bridge, "user")
        .unwrap()
        .unwrap();
    assert_eq!(restored.phase, HueOwnershipPhase::Restored);
    let restored_behaviors = bridge.get_resources("user", "behavior_instance").unwrap();
    let restored_enabled = |id: &str| {
        restored_behaviors["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|behavior| behavior["id"] == id)
            .unwrap()["enabled"]
            .as_bool()
            .unwrap()
    };
    assert!(restored_enabled("behavior-1"));
    assert!(restored_enabled("accessory-behavior-1"));
    assert!(restored_enabled("accessory-target-behavior-1"));
    assert_eq!(
        bridge.get_v1("user", "rules").unwrap()["1"]["status"],
        "enabled"
    );
    assert_eq!(
        bridge.get_v1("user", "schedules").unwrap()["2"]["status"],
        "enabled"
    );
    assert!(!bridge.calls().iter().any(|call| matches!(
        call,
        HueTransportCall::CreateResource { .. }
            | HueTransportCall::DeleteResource { .. }
            | HueTransportCall::DeleteV1 { .. }
            | HueTransportCall::UpdateRoomChildren { .. }
            | HueTransportCall::CreateRoom { .. }
            | HueTransportCall::UpdateRoom { .. }
            | HueTransportCall::RenameRoom { .. }
            | HueTransportCall::DeleteRoom { .. }
    )));

    finalize_released_control(&temp.storage, "bridge-1").unwrap();
    assert!(load_controller_ownership(&temp.storage, "bridge-1")
        .unwrap()
        .is_none());
}

#[test]
fn rejected_takeover_mutation_fails_closed_before_later_suppression() {
    let temp = TempStorage::new();
    let bridge = SpyHueTransport::new();
    seed_single_automation_bridge(&bridge);
    bridge.set_fail_resource_update("behavior_instance", "behavior-1");

    acquire_authoritative_control(&temp.storage, &key(), &bridge, "user")
        .expect_err("a rejected Hue mutation must not grant controller authority");

    let incomplete = load_controller_ownership(&temp.storage, "bridge-1")
        .unwrap()
        .unwrap();
    assert_eq!(incomplete.phase, HueOwnershipPhase::ClearIncomplete);
    assert!(incomplete.receipts().any(|receipt| {
        receipt.resource_type == "behavior_instance"
            && receipt.status == rhythm_hue::ownership::HueOwnershipReceiptStatus::Failed
    }));
    assert_eq!(
        bridge.get_v1("user", "rules").unwrap()["1"]["status"],
        "enabled"
    );
    assert_eq!(
        bridge.get_v1("user", "schedules").unwrap()["2"]["status"],
        "enabled"
    );
}

#[test]
fn interrupted_room_policy_release_resumes_after_restart() {
    let temp = TempStorage::new();
    let bridge = SpyHueTransport::new();
    seed_single_automation_bridge(&bridge);

    let active = acquire_authoritative_control(&temp.storage, &key(), &bridge, "user").unwrap();
    assert_eq!(active.phase, HueOwnershipPhase::Active);
    assert_eq!(
        bridge.get_resources("user", "behavior_instance").unwrap()["data"][0]["enabled"],
        false
    );

    bridge.set_fail_resource_update("behavior_instance", "behavior-1");
    release_authoritative_control_with_intent(
        &temp.storage,
        &key(),
        &bridge,
        "user",
        HueOwnershipReleaseIntent::RoomAuthorityChanged,
    )
    .expect_err("an interrupted room-policy release must remain durable and retryable");

    let interrupted = load_controller_ownership(&temp.storage, "bridge-1")
        .unwrap()
        .unwrap();
    assert_eq!(interrupted.phase, HueOwnershipPhase::RestoreIncomplete);
    assert_eq!(
        interrupted.release_intent(),
        Some(HueOwnershipReleaseIntent::RoomAuthorityChanged)
    );

    let restarted_storage = FileStorage::new(temp.path.to_str().unwrap()).unwrap();
    bridge.set_fail_resource_update("behavior_instance", "not-a-real-resource");
    let restored = release_authoritative_control_with_intent(
        &restarted_storage,
        &key(),
        &bridge,
        "user",
        HueOwnershipReleaseIntent::RoomAuthorityChanged,
    )
    .unwrap()
    .unwrap();

    assert_eq!(restored.phase, HueOwnershipPhase::Restored);
    assert_eq!(
        restored.release_intent(),
        Some(HueOwnershipReleaseIntent::RoomAuthorityChanged)
    );
    assert_eq!(
        bridge.get_resources("user", "behavior_instance").unwrap()["data"][0]["enabled"],
        true
    );
    finalize_released_control(&restarted_storage, "bridge-1").unwrap();
    assert!(load_controller_ownership(&restarted_storage, "bridge-1")
        .unwrap()
        .is_none());
}
