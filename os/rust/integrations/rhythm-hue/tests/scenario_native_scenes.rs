//! Scenario coverage for Hue-authored scenes inside Rhythm-managed rooms.

use std::collections::BTreeSet;
use std::sync::Arc;

use rhythm_hue::discovery::HueDiscovery;
use rhythm_hue::managed_rooms::{reconcile_managed_rooms, DesiredHueRoom};
use rhythm_hue::ownership::{
    acquire_authoritative_control, persist_controller_ownership, reconcile_authoritative_control,
    HueManagedScene,
};
use rhythm_hue::test_support::{HueTransportCall, SpyHueTransport};
use rhythm_hue::transport::HueTransport;
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::discovery::HubDiscovery;
use rhythm_os::hub::HubType;
use rhythm_os::storage::FileStorage;

struct TempStorage {
    path: std::path::PathBuf,
    storage: Arc<FileStorage>,
}

impl TempStorage {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rhythm-hue-native-scenes-scenario-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        let storage = Arc::new(FileStorage::new(path.to_str().unwrap()).unwrap());
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

fn seed_empty_bridge(bridge: &SpyHueTransport) {
    for (resource_type, data) in [
        ("bridge", serde_json::json!([{"id": "bridge-scenes"}])),
        ("device", serde_json::json!([{"id": "bulb"}])),
        ("light", serde_json::json!([])),
        ("behavior_instance", serde_json::json!([])),
        ("room", serde_json::json!([])),
        ("zone", serde_json::json!([])),
        ("scene", serde_json::json!([])),
        ("smart_scene", serde_json::json!([])),
    ] {
        bridge.set_resource_response(
            resource_type,
            serde_json::json!({"data": data, "errors": []}),
        );
    }
    bridge.set_v1_response("rules", serde_json::json!({}));
    bridge.set_v1_response("schedules", serde_json::json!({}));
}

#[test]
fn hue_palette_and_basic_scenes_survive_sync_and_recall_under_authority() {
    let temp = TempStorage::new();
    let bridge = Arc::new(SpyHueTransport::new());
    seed_empty_bridge(&bridge);
    let mut active =
        acquire_authoritative_control(temp.storage.as_ref(), &key(), bridge.as_ref(), "user")
            .unwrap();
    reconcile_managed_rooms(
        temp.storage.as_ref(),
        &key(),
        &mut active,
        bridge.as_ref(),
        "user",
        &[DesiredHueRoom::new(
            "rhythm-room",
            "Rhythm room",
            vec!["bulb".to_string()],
        )],
        &BTreeSet::from(["bulb".to_string()]),
    )
    .unwrap();
    let managed_room_id = active.managed_rooms()["rhythm-room"].hue_room_id.clone();
    active
        .record_managed_scene(HueManagedScene {
            rhythm_room_id: "rhythm-room".to_string(),
            rhythm_scene_id: "rhythm-projection".to_string(),
            hue_room_id: managed_room_id.clone(),
            hue_scene_id: "managed-projection".to_string(),
            fingerprint: "fingerprint".to_string(),
            ephemeral: false,
        })
        .unwrap();
    persist_controller_ownership(temp.storage.as_ref(), &active).unwrap();
    bridge.set_resource_response(
        "scene",
        serde_json::json!({"data": [
            {
                "id": "hue-palette",
                "group": {"rtype": "room", "rid": managed_room_id},
                "metadata": {"name": "Arctic aurora"},
                "palette": {
                    "color": [{
                        "color": {"xy": {"x": 0.21, "y": 0.24}},
                        "dimming": {"brightness": 63}
                    }]
                },
                "actions": []
            },
            {
                "id": "hue-basic",
                "group": {"rtype": "room", "rid": managed_room_id},
                "metadata": {"name": "Warm basic"},
                "actions": [{
                    "action": {
                        "on": {"on": true},
                        "dimming": {"brightness": 55},
                        "color_temperature": {"mirek": 300}
                    }
                }]
            },
            {
                "id": "managed-projection",
                "group": {"rtype": "room", "rid": managed_room_id},
                "metadata": {"name": "Rhythm projection"},
                "actions": [{
                    "action": {
                        "on": {"on": true},
                        "color_temperature": {"mirek": 250}
                    }
                }]
            },
            {
                "id": "outside-scene",
                "group": {"rtype": "room", "rid": "outside-room"},
                "metadata": {"name": "Outside"},
                "actions": [{
                    "action": {
                        "on": {"on": true},
                        "color_temperature": {"mirek": 250}
                    }
                }]
            }
        ], "errors": []}),
    );
    bridge.set_resource_response(
        "smart_scene",
        serde_json::json!({"data": [{
            "id": "smart-scene",
            "group": {"rtype": "room", "rid": managed_room_id}
        }], "errors": []}),
    );

    let active = reconcile_authoritative_control(
        temp.storage.as_ref(),
        &key(),
        bridge.as_ref(),
        "user",
        active,
    )
    .unwrap();
    assert!(active
        .managed_scenes()
        .values()
        .any(|scene| { scene.hue_scene_id == "managed-projection" }));
    let remaining_scenes = bridge.get_resources("user", "scene").unwrap();
    let remaining_scene_ids = remaining_scenes["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|scene| scene.get("id").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        remaining_scene_ids,
        BTreeSet::from([
            "hue-basic".to_string(),
            "hue-palette".to_string(),
            "managed-projection".to_string(),
        ])
    );
    assert!(bridge.get_resources("user", "smart_scene").unwrap()["data"]
        .as_array()
        .unwrap()
        .is_empty());

    let discovery = HueDiscovery::new_with_ownership_storage(
        bridge.clone(),
        "user".to_string(),
        temp.storage.clone(),
    )
    .unwrap();
    let scenes = discovery.discover_scenes(&managed_room_id).unwrap();
    assert_eq!(scenes.len(), 2);
    assert!(scenes.iter().any(|scene| {
        scene.id == "native-hue-hue-palette"
            && scene.extensions.get("hue_palette_scene") == Some(&serde_json::Value::Bool(true))
    }));
    assert!(scenes
        .iter()
        .any(|scene| scene.id == "native-hue-hue-basic"));
    assert!(!scenes
        .iter()
        .any(|scene| scene.id == "native-hue-managed-projection"));

    bridge.reset();
    discovery.recall_scene("hue-palette", Some(700)).unwrap();
    assert_eq!(
        bridge.calls(),
        vec![HueTransportCall::RecallScene {
            scene_id: "hue-palette".to_string(),
            transition_ms: Some(700),
        }]
    );
}
