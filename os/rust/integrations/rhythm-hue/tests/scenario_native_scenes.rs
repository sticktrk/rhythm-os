//! Scenario coverage for Hue-authored topology and scenes under Rhythm authority.

use std::collections::BTreeSet;
use std::sync::Arc;

use rhythm_hue::discovery::HueDiscovery;
use rhythm_hue::ownership::{acquire_authoritative_control, reconcile_authoritative_control};
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

fn seed_bridge(bridge: &SpyHueTransport) {
    for (resource_type, data) in [
        ("bridge", serde_json::json!([{"id": "bridge-scenes"}])),
        (
            "device",
            serde_json::json!([{
                "id": "bulb",
                "metadata": {"name": "Bedside lamp"},
                "product_data": {"manufacturer_name": "Signify", "model_id": "LCA009"},
                "services": [{"rid": "bulb-light", "rtype": "light"}]
            }]),
        ),
        (
            "light",
            serde_json::json!([{
                "id": "bulb-light",
                "owner": {"rid": "bulb", "rtype": "device"}
            }]),
        ),
        (
            "behavior_instance",
            serde_json::json!([{
                "id": "motion-automation",
                "script_id": "automation-script-1",
                "enabled": true
            }]),
        ),
        (
            "behavior_script",
            serde_json::json!([{
                "id": "automation-script-1",
                "metadata": {"name": "Motion automation", "category": "automation"}
            }]),
        ),
        (
            "room",
            serde_json::json!([
                {
                    "id": "bedroom",
                    "metadata": {"name": "Bedroom", "archetype": "bedroom"},
                    "children": [{"rid": "bulb", "rtype": "device"}],
                    "services": [{"rid": "bedroom-group", "rtype": "grouped_light"}]
                },
                {
                    "id": "outside-room",
                    "metadata": {"name": "Outside", "archetype": "garden"},
                    "children": [],
                    "services": [{"rid": "outside-group", "rtype": "grouped_light"}]
                }
            ]),
        ),
        (
            "zone",
            serde_json::json!([{
                "id": "upstairs",
                "metadata": {"name": "Upstairs"},
                "children": [{"rid": "bulb", "rtype": "device"}]
            }]),
        ),
        (
            "scene",
            serde_json::json!([
                {
                    "id": "hue-palette",
                    "group": {"rtype": "room", "rid": "bedroom"},
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
                    "group": {"rtype": "room", "rid": "bedroom"},
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
            ]),
        ),
        (
            "smart_scene",
            serde_json::json!([{
                "id": "smart-scene",
                "metadata": {"name": "Natural light"},
                "group": {"rtype": "room", "rid": "bedroom"}
            }]),
        ),
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
fn hue_rooms_zones_and_scenes_survive_authority_reconcile_and_recall() {
    let temp = TempStorage::new();
    let bridge = Arc::new(SpyHueTransport::new());
    seed_bridge(&bridge);
    let original_rooms = bridge.get_resources("user", "room").unwrap();
    let original_zones = bridge.get_resources("user", "zone").unwrap();
    let original_scenes = bridge.get_resources("user", "scene").unwrap();
    let original_smart_scenes = bridge.get_resources("user", "smart_scene").unwrap();
    bridge.reset();

    let active =
        acquire_authoritative_control(temp.storage.as_ref(), &key(), bridge.as_ref(), "user")
            .unwrap();
    bridge.reset();
    reconcile_authoritative_control(
        temp.storage.as_ref(),
        &key(),
        bridge.as_ref(),
        "user",
        active,
    )
    .unwrap();

    assert_eq!(
        bridge.get_resources("user", "room").unwrap(),
        original_rooms
    );
    assert_eq!(
        bridge.get_resources("user", "zone").unwrap(),
        original_zones
    );
    assert_eq!(
        bridge.get_resources("user", "scene").unwrap(),
        original_scenes
    );
    assert_eq!(
        bridge.get_resources("user", "smart_scene").unwrap(),
        original_smart_scenes
    );
    let scene_ids = bridge.get_resources("user", "scene").unwrap()["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|scene| scene.get("id").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        scene_ids,
        BTreeSet::from([
            "hue-basic".to_string(),
            "hue-palette".to_string(),
            "outside-scene".to_string(),
        ])
    );
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

    let discovery = HueDiscovery::new(bridge.clone(), "user".to_string());
    let rooms = discovery.discover_rooms().unwrap();
    assert_eq!(rooms.len(), 2);
    assert!(rooms.iter().any(|room| room.id == "bedroom"));
    assert!(rooms.iter().any(|room| room.id == "outside-room"));
    let scenes = discovery.discover_scenes("bedroom").unwrap();
    assert_eq!(scenes.len(), 2);
    assert!(scenes.iter().any(|scene| {
        scene.id == "native-hue-hue-palette"
            && scene.extensions.get("hue_palette_scene") == Some(&serde_json::Value::Bool(true))
    }));
    assert!(scenes
        .iter()
        .any(|scene| scene.id == "native-hue-hue-basic"));

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
